//! The trim window: a frame of the recording, a timeline with two handles, keep or cancel.
//! Dragging a handle shows the frame under it.

use std::path::Path;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT, ID2D1Bitmap,
    ID2D1HwndRenderTarget, ID2D1RenderTarget,
};
use windows::Win32::Graphics::Dwm::{
    DWM_WINDOW_CORNER_PREFERENCE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ValidateRect};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetCursorPos, GetMessageW,
    GetWindowLongPtrW, IDC_ARROW, IDC_HAND, IDC_SIZEWE, LoadCursorW, MSG, RegisterClassExW, SW_SHOW, SetCursor,
    SetForegroundWindow, SetWindowLongPtrW, ShowWindow, TranslateMessage, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_PAINT, WM_SETCURSOR, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{Interface, Result, w};

use crate::output::Image;
use crate::overlay::Graphics;
use crate::style;
use crate::trim::FramePreview;

const WIDTH: f32 = 720.0;
const PADDING: f32 = 16.0;
const PREVIEW_HEIGHT: f32 = 360.0;
const TIMELINE_HEIGHT: f32 = 28.0;
const HANDLE_WIDTH: f32 = 10.0;
/// How near the pointer must be to take a handle.
const HANDLE_REACH: f32 = 14.0;
const BUTTON_HEIGHT: f32 = 32.0;
const HEIGHT: f32 = 40.0 + PREVIEW_HEIGHT + PADDING + TIMELINE_HEIGHT + 30.0 + BUTTON_HEIGHT + PADDING;
const SHORTEST_CUT: Duration = Duration::from_millis(200);
const KEY_STEP: Duration = Duration::from_millis(100);
/// Decoding a frame takes a moment, so a drag asks for one at most this often.
const PREVIEW_INTERVAL: Duration = Duration::from_millis(90);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Handle {
    Start,
    End,
}

/// The part being kept. It knows nothing about windows, so it can be tested without a screen.
#[derive(Debug)]
struct Range {
    length: Duration,
    start: Duration,
    end: Duration,
}

impl Range {
    fn new(length: Duration) -> Self {
        Self {
            length,
            start: Duration::ZERO,
            end: length,
        }
    }

    /// Moves a handle, keeping it inside the recording and clear of the other handle.
    fn set(&mut self, handle: Handle, time: Duration) {
        match handle {
            Handle::Start => self.start = time.min(self.end.saturating_sub(SHORTEST_CUT)),
            Handle::End => self.end = time.clamp((self.start + SHORTEST_CUT).min(self.length), self.length),
        }
    }

    fn time_of(&self, handle: Handle) -> Duration {
        match handle {
            Handle::Start => self.start,
            Handle::End => self.end,
        }
    }

    fn kept(&self) -> Duration {
        self.end.saturating_sub(self.start)
    }

    fn is_whole(&self) -> bool {
        self.start.is_zero() && self.end == self.length
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Button {
    Keep,
    Cancel,
}

struct TrimWindow {
    graphics: *const Graphics,
    target: ID2D1RenderTarget,
    preview: FramePreview,
    frame: Option<ID2D1Bitmap>,
    frame_size: (u32, u32),
    range: Range,
    active: Handle,
    dragging: bool,
    scale: f32,
    pointer: (f32, f32),
    last_preview: Option<Instant>,
    buttons: Vec<(Button, D2D_RECT_F)>,
    answer: Option<bool>,
}

fn track_rect() -> D2D_RECT_F {
    let top = 40.0 + PREVIEW_HEIGHT + PADDING;
    D2D_RECT_F {
        left: PADDING + HANDLE_WIDTH,
        top,
        right: WIDTH - PADDING - HANDLE_WIDTH,
        bottom: top + TIMELINE_HEIGHT,
    }
}

/// Where on the track a time sits, from left to right.
fn x_of(time: Duration, length: Duration) -> f32 {
    let track = track_rect();
    let share = time.as_secs_f32() / length.as_secs_f32().max(0.001);
    track.left + (track.right - track.left) * share.clamp(0.0, 1.0)
}

fn time_at(x: f32, length: Duration) -> Duration {
    let track = track_rect();
    let share = ((x - track.left) / (track.right - track.left)).clamp(0.0, 1.0);
    Duration::from_secs_f32(length.as_secs_f32() * share)
}

/// Asks which part of the recording to keep. Blocks until the user decides. Returns the part,
/// or nothing when the user cancelled or kept all of it.
pub fn ask(graphics: &Graphics, recording: &Path) -> std::result::Result<Option<(Duration, Duration)>, Box<dyn std::error::Error>> {
    let mut preview = FramePreview::open(recording)?;
    let first = preview.frame_at(Duration::ZERO)?;
    let length = preview.duration;

    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(trim_proc),
        hInstance: unsafe { GetModuleHandleW(None) }.unwrap_or_default().into(),
        lpszClassName: w!("oh-snap.trim"),
        ..Default::default()
    };
    unsafe { RegisterClassExW(&class) };
    let mut cursor = POINT::default();
    let mut monitor_info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let mut dpi_x = 96u32;
    let mut dpi_y = 96u32;
    unsafe {
        let _ = GetCursorPos(&mut cursor);
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let _ = GetMonitorInfoW(monitor, &mut monitor_info);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
    }
    let work: RECT = monitor_info.rcWork;
    let room = ((work.right - work.left) as f32 / WIDTH).min((work.bottom - work.top) as f32 / HEIGHT);
    let scale = (dpi_x as f32 / 96.0).min(room);
    let pixel_width = (WIDTH * scale).round() as i32;
    let pixel_height = (HEIGHT * scale).round() as i32;
    let instance = unsafe { GetModuleHandleW(None)? };
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("oh-snap.trim"),
            w!("oh-snap trim"),
            WS_POPUP,
            work.left + (work.right - work.left - pixel_width) / 2,
            work.top + (work.bottom - work.top - pixel_height) / 2,
            pixel_width,
            pixel_height,
            None,
            None,
            Some(instance.into()),
            None,
        )?
    };
    let corners: DWM_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corners as *const DWM_WINDOW_CORNER_PREFERENCE as *const _,
            size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        );
    }
    let target = create_target(graphics, hwnd, pixel_width, pixel_height, scale * 96.0)?;
    let frame = create_frame_bitmap(&target, &first).ok();
    let mut window = TrimWindow {
        graphics,
        target,
        frame_size: preview.size(),
        preview,
        frame,
        range: Range::new(length),
        active: Handle::Start,
        dragging: false,
        scale,
        pointer: (-1.0, -1.0),
        last_preview: None,
        buttons: Vec::new(),
        answer: None,
    };
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut window as *mut TrimWindow as isize);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
    }
    window.paint();

    let mut message = MSG::default();
    while window.answer.is_none() {
        if unsafe { GetMessageW(&mut message, None, 0, 0) }.0 <= 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        let _ = DestroyWindow(hwnd);
    }
    let keep = window.answer == Some(true) && !window.range.is_whole();
    Ok(keep.then_some((window.range.start, window.range.end)))
}

fn create_target(graphics: &Graphics, hwnd: HWND, width: i32, height: i32, dpi: f32) -> Result<ID2D1RenderTarget> {
    let properties = D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: dpi,
        dpiY: dpi,
        usage: D2D1_RENDER_TARGET_USAGE_NONE,
        minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
    };
    let hwnd_properties = D2D1_HWND_RENDER_TARGET_PROPERTIES {
        hwnd,
        pixelSize: D2D_SIZE_U {
            width: width as u32,
            height: height as u32,
        },
        presentOptions: D2D1_PRESENT_OPTIONS_NONE,
    };
    let target: ID2D1HwndRenderTarget = unsafe { graphics.d2d.CreateHwndRenderTarget(&properties, &hwnd_properties)? };
    target.cast()
}

fn create_frame_bitmap(target: &ID2D1RenderTarget, image: &Image) -> Result<ID2D1Bitmap> {
    let properties = D2D1_BITMAP_PROPERTIES {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_IGNORE,
        },
        dpiX: 96.0,
        dpiY: 96.0,
    };
    let size = D2D_SIZE_U {
        width: image.width,
        height: image.height,
    };
    unsafe { target.CreateBitmap(size, Some(image.bgra.as_ptr() as *const _), image.width * 4, &properties) }
}

unsafe extern "system" fn trim_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let window_pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut TrimWindow;
    if window_pointer.is_null() {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    let window = unsafe { &mut *window_pointer };
    match message {
        WM_PAINT => {
            window.paint();
            unsafe {
                let _ = ValidateRect(Some(hwnd), None);
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let shape = if window.dragging || window.handle_near(window.pointer).is_some() {
                IDC_SIZEWE
            } else if window.button_at(window.pointer).is_some() {
                IDC_HAND
            } else {
                IDC_ARROW
            };
            if let Ok(cursor) = unsafe { LoadCursorW(None, shape) } {
                unsafe { SetCursor(Some(cursor)) };
            }
            LRESULT(1)
        }
        WM_MOUSEMOVE => {
            window.pointer = window.to_dips(lparam);
            if window.dragging {
                let time = time_at(window.pointer.0, window.range.length);
                window.range.set(window.active, time);
                window.show_handle_frame(false);
            }
            window.paint();
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            window.pointer = window.to_dips(lparam);
            if let Some(handle) = window.handle_near(window.pointer) {
                window.active = handle;
                window.dragging = true;
                unsafe { SetCapture(hwnd) };
            }
            window.paint();
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            window.pointer = window.to_dips(lparam);
            if window.dragging {
                window.dragging = false;
                unsafe {
                    let _ = ReleaseCapture();
                }
                // The last frame of a drag is the one that counts, however fast it came.
                window.show_handle_frame(true);
            } else if let Some(button) = window.button_at(window.pointer) {
                window.answer = Some(button == Button::Keep);
            }
            window.paint();
            LRESULT(0)
        }
        WM_KEYDOWN => {
            window.on_key(wparam.0 as u16);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

impl TrimWindow {
    fn graphics(&self) -> &Graphics {
        unsafe { &*self.graphics }
    }

    fn to_dips(&self, lparam: LPARAM) -> (f32, f32) {
        let x = (lparam.0 & 0xFFFF) as u16 as i16 as f32;
        let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
        (x / self.scale, y / self.scale)
    }

    /// The handle under the pointer, the nearer one when both are close.
    fn handle_near(&self, point: (f32, f32)) -> Option<Handle> {
        let track = track_rect();
        if point.1 < track.top - 4.0 || point.1 > track.bottom + 4.0 {
            return None;
        }
        let to_start = (point.0 - x_of(self.range.start, self.range.length)).abs();
        let to_end = (point.0 - x_of(self.range.end, self.range.length)).abs();
        if to_start.min(to_end) > HANDLE_REACH {
            return None;
        }
        Some(if to_start <= to_end { Handle::Start } else { Handle::End })
    }

    fn button_at(&self, point: (f32, f32)) -> Option<Button> {
        self.buttons
            .iter()
            .find(|(_, rect)| point.0 >= rect.left && point.0 < rect.right && point.1 >= rect.top && point.1 < rect.bottom)
            .map(|(button, _)| *button)
    }

    fn on_key(&mut self, key: u16) {
        match key {
            key if key == VK_RETURN.0 => self.answer = Some(true),
            key if key == VK_ESCAPE.0 => self.answer = Some(false),
            key if key == u16::from(b'I') => self.active = Handle::Start,
            key if key == u16::from(b'O') => self.active = Handle::End,
            key if key == VK_LEFT.0 => {
                let time = self.range.time_of(self.active).saturating_sub(KEY_STEP);
                self.range.set(self.active, time);
                self.show_handle_frame(true);
            }
            key if key == VK_RIGHT.0 => {
                let time = self.range.time_of(self.active) + KEY_STEP;
                self.range.set(self.active, time);
                self.show_handle_frame(true);
            }
            _ => {}
        }
        self.paint();
    }

    fn show_handle_frame(&mut self, always: bool) {
        let is_due = self.last_preview.is_none_or(|last| last.elapsed() >= PREVIEW_INTERVAL);
        if !always && !is_due {
            return;
        }
        self.last_preview = Some(Instant::now());
        let time = self.range.time_of(self.active);
        match self.preview.frame_at(time) {
            Ok(image) => {
                if let Ok(bitmap) = create_frame_bitmap(&self.target, &image) {
                    self.frame = Some(bitmap);
                }
            }
            Err(error) => crate::log::write(&format!("trim preview failed: {error}")),
        }
    }

    fn paint(&mut self) {
        let target = self.target.clone();
        let brush = |color| unsafe { target.CreateSolidColorBrush(&color, None) };
        let (Ok(text), Ok(dim), Ok(accent), Ok(soft), Ok(rule), Ok(hover)) = (
            brush(style::TEXT),
            brush(style::TEXT_DIM),
            brush(style::ACCENT),
            brush(style::ACCENT_SOFT),
            brush(style::RULE),
            brush(style::HOVER),
        ) else {
            return;
        };
        let graphics = self.graphics();
        let rounded = |rect, radius| D2D1_ROUNDED_RECT { rect, radiusX: radius, radiusY: radius };
        unsafe {
            target.BeginDraw();
            target.Clear(Some(&style::INK));
            target.DrawRoundedRectangle(
                &rounded(D2D_RECT_F { left: 0.5, top: 0.5, right: WIDTH - 0.5, bottom: HEIGHT - 0.5 }, 8.0),
                &rule,
                1.0,
                None,
            );
        }
        let spark = D2D_RECT_F { left: PADDING - 6.0, top: 8.0, right: PADDING + 14.0, bottom: 32.0 };
        graphics.draw_spark(&target, spark, &accent);
        crate::overlay::draw_label(graphics, &target, "trim", PADDING + 18.0, 11.0, &text);

        if let Some(frame) = &self.frame {
            let box_width = WIDTH - PADDING * 2.0;
            let fit = (box_width / self.frame_size.0.max(1) as f32).min(PREVIEW_HEIGHT / self.frame_size.1.max(1) as f32);
            let shown_width = self.frame_size.0 as f32 * fit;
            let shown_height = self.frame_size.1 as f32 * fit;
            let left = (WIDTH - shown_width) / 2.0;
            let top = 40.0 + (PREVIEW_HEIGHT - shown_height) / 2.0;
            let area = D2D_RECT_F { left, top, right: left + shown_width, bottom: top + shown_height };
            unsafe { target.DrawBitmap(frame, Some(&area), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None) };
        }

        let track = track_rect();
        let length = self.range.length;
        let start_x = x_of(self.range.start, length);
        let end_x = x_of(self.range.end, length);
        let kept = D2D_RECT_F { left: start_x, top: track.top, right: end_x, bottom: track.bottom };
        unsafe {
            target.FillRoundedRectangle(&rounded(track, 4.0), &hover);
            target.FillRectangle(&kept, &soft);
        }
        for (handle, x) in [(Handle::Start, start_x), (Handle::End, end_x)] {
            let rect = D2D_RECT_F { left: x - HANDLE_WIDTH / 2.0, top: track.top - 3.0, right: x + HANDLE_WIDTH / 2.0, bottom: track.bottom + 3.0 };
            let color = if handle == self.active { &accent } else { &text };
            unsafe { target.FillRoundedRectangle(&rounded(rect, 3.0), color) };
        }

        let times_top = track.bottom + 8.0;
        let start_text = crate::recorder::clock_text_precise(self.range.start.as_millis() as u64);
        let end_text = crate::recorder::clock_text_precise(self.range.end.as_millis() as u64);
        crate::overlay::draw_label(graphics, &target, &start_text, track.left - HANDLE_WIDTH, times_top, &dim);
        let (end_width, _) = graphics.measure(&end_text);
        crate::overlay::draw_label(graphics, &target, &end_text, track.right + HANDLE_WIDTH - end_width, times_top, &dim);
        let kept_text = format!("keeps {}", crate::recorder::clock_text_precise(self.range.kept().as_millis() as u64));
        let (kept_width, _) = graphics.measure(&kept_text);
        crate::overlay::draw_label(graphics, &target, &kept_text, (WIDTH - kept_width) / 2.0, times_top, &text);

        let buttons_top = HEIGHT - PADDING - BUTTON_HEIGHT;
        let mut right = WIDTH - PADDING;
        let mut buttons = Vec::new();
        for (button, icon, key, name) in [
            (Button::Cancel, crate::icons::CANCEL, "[esc]", " cancel"),
            (Button::Keep, crate::icons::SAVE, "[enter]", " save a copy"),
        ] {
            let (key_width, _) = graphics.measure(key);
            let (name_width, _) = graphics.measure(name);
            let width = key_width + name_width + 42.0;
            let rect = D2D_RECT_F { left: right - width, top: buttons_top, right, bottom: buttons_top + BUTTON_HEIGHT };
            let is_hovered = self.pointer.0 >= rect.left && self.pointer.0 < rect.right && self.pointer.1 >= rect.top && self.pointer.1 < rect.bottom;
            if button == Button::Keep {
                unsafe { target.FillRoundedRectangle(&rounded(rect, style::BUTTON_RADIUS), &soft) };
            } else if is_hovered {
                unsafe { target.FillRoundedRectangle(&rounded(rect, style::BUTTON_RADIUS), &hover) };
            }
            let primary = button == Button::Keep;
            let icon_area = D2D_RECT_F { left: rect.left + 7.0, top: rect.top, right: rect.left + 27.0, bottom: rect.bottom };
            graphics.draw_icon(&target, icon, icon_area, if primary { &accent } else { &text });
            let label_top = rect.top + 7.0;
            crate::overlay::draw_label(graphics, &target, key, rect.left + 32.0, label_top, if primary { &accent } else { &dim });
            crate::overlay::draw_label(graphics, &target, name, rect.left + 32.0 + key_width, label_top, if primary { &accent } else { &text });
            buttons.push((button, rect));
            right = rect.left - 4.0;
        }
        unsafe {
            let _ = target.EndDraw(None, None);
        }
        self.buttons = buttons;
    }
}

/// Draws the window into memory for `oh-snap preview`. Returns BGRA pixels and the size.
pub fn render_offscreen(graphics: &Graphics, recording: &Path) -> std::result::Result<(Vec<u8>, u32, u32), Box<dyn std::error::Error>> {
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapCacheOnLoad,
    };
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    let mut preview = FramePreview::open(recording)?;
    let length = preview.duration;
    let mut range = Range::new(length);
    range.set(Handle::Start, length / 4);
    range.set(Handle::End, length * 3 / 4);
    let image = preview.frame_at(range.start)?;
    let width = WIDTH as u32;
    let height = HEIGHT as u32;
    let imaging: IWICImagingFactory = unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? };
    let bitmap = unsafe { imaging.CreateBitmap(width, height, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad)? };
    let properties = D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0,
        dpiY: 96.0,
        usage: D2D1_RENDER_TARGET_USAGE_NONE,
        minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
    };
    let target: ID2D1RenderTarget = unsafe { graphics.d2d.CreateWicBitmapRenderTarget(&bitmap, &properties)? }.cast()?;
    let frame = create_frame_bitmap(&target, &image).ok();
    let mut window = TrimWindow {
        graphics,
        target,
        frame_size: preview.size(),
        preview,
        frame,
        range,
        active: Handle::Start,
        dragging: false,
        scale: 1.0,
        pointer: (-1.0, -1.0),
        last_preview: None,
        buttons: Vec::new(),
        answer: None,
    };
    window.paint();
    let stride = width * 4;
    let mut pixels = vec![0u8; (stride * height) as usize];
    unsafe { bitmap.CopyPixels(std::ptr::null(), stride, &mut pixels)? };
    Ok((pixels, width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seconds(value: u64) -> Duration {
        Duration::from_secs(value)
    }

    #[test]
    fn a_new_range_keeps_everything() {
        let range = Range::new(seconds(10));
        assert!(range.is_whole());
        assert_eq!(range.kept(), seconds(10));
    }

    #[test]
    fn handles_cannot_cross_or_leave_the_recording() {
        let mut range = Range::new(seconds(10));
        range.set(Handle::End, seconds(4));
        range.set(Handle::Start, seconds(9));
        assert_eq!(range.start, seconds(4) - SHORTEST_CUT);
        range.set(Handle::End, seconds(1));
        assert_eq!(range.end, seconds(4));
        range.set(Handle::End, seconds(99));
        assert_eq!(range.end, seconds(10));
    }

    #[test]
    fn time_and_position_on_the_track_agree() {
        let length = seconds(100);
        let track = track_rect();
        assert_eq!(x_of(Duration::ZERO, length), track.left);
        assert_eq!(x_of(length, length), track.right);
        let back = time_at(x_of(seconds(25), length), length);
        assert!((back.as_secs_f32() - 25.0).abs() < 0.01);
        assert_eq!(time_at(-500.0, length), Duration::ZERO);
        assert_eq!(time_at(5000.0, length), length);
    }
}
