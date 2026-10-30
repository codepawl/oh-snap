//! What the screen shows while recording: a small bar with the time, pause and stop, and a
//! thin frame around the recorded area. Windows keeps both out of the recording.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ELLIPSE, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
    ID2D1HwndRenderTarget, ID2D1RenderTarget,
};
use windows::Win32::Graphics::Dwm::{
    DWM_WINDOW_CORNER_PREFERENCE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    CombineRgn, CreateRectRgn, CreateSolidBrush, DeleteObject, GetMonitorInfoW, HBRUSH, MONITOR_DEFAULTTONEAREST,
    MONITORINFO, MonitorFromPoint, RGN_DIFF, SetWindowRgn, ValidateRect,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW, IDC_ARROW, IDC_HAND, LWA_ALPHA,
    LoadCursorW, MA_NOACTIVATE, RegisterClassExW, SW_SHOWNOACTIVATE, SetCursor, SetLayeredWindowAttributes, SetTimer,
    SetWindowDisplayAffinity, SetWindowLongPtrW, ShowWindow, WDA_EXCLUDEFROMCAPTURE, WM_LBUTTONUP, WM_MOUSEACTIVATE,
    WM_MOUSEMOVE, WM_NCDESTROY, WM_PAINT, WM_SETCURSOR, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{Interface, Result, w};
use windows_numerics::Vector2;

use crate::capture::PixelRect;
use crate::overlay::Graphics;
use crate::recorder::Controls;
use crate::style;

const BAR_WIDTH: f32 = 248.0;
const BAR_HEIGHT: f32 = 40.0;
const GAP_TO_AREA: i32 = 10;
const FRAME_THICKNESS: i32 = 2;
const REDRAW_TIMER: usize = 1;
const REDRAW_MILLISECONDS: u32 = 200;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Button {
    Pause,
    Stop,
}

/// The state one drawing of the bar needs.
struct Look {
    recorded_milliseconds: u64,
    paused: bool,
    saving: bool,
    hovered: Option<Button>,
}

struct Bar {
    graphics: *const Graphics,
    controls: Arc<Controls>,
    target: ID2D1RenderTarget,
    scale: f32,
    hovered: Option<Button>,
    buttons: Vec<(Button, D2D_RECT_F)>,
}

/// The two windows of a recording. They live on the calling thread's message loop.
pub struct RecordingWindows {
    bar: HWND,
    frame: HWND,
}

impl RecordingWindows {
    pub fn close(&self) {
        unsafe {
            let _ = DestroyWindow(self.bar);
            let _ = DestroyWindow(self.frame);
        }
    }
}

pub fn open(graphics: &Graphics, controls: Arc<Controls>, area: PixelRect) -> Result<RecordingWindows> {
    let frame = open_frame(area)?;
    match open_bar(graphics, controls, area) {
        Ok(bar) => Ok(RecordingWindows { bar, frame }),
        Err(error) => {
            unsafe {
                let _ = DestroyWindow(frame);
            }
            Err(error)
        }
    }
}

fn exclude_from_capture(hwnd: HWND) {
    unsafe {
        // Windows 10 before 2004 refuses this; the bar then shows if it is inside the area.
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
    }
}

/// A click-through ring just outside the area.
fn open_frame(area: PixelRect) -> Result<HWND> {
    let accent = style::ACCENT;
    let color = u32::from((accent.r * 255.0) as u8) | u32::from((accent.g * 255.0) as u8) << 8 | u32::from((accent.b * 255.0) as u8) << 16;
    let brush: HBRUSH = unsafe { CreateSolidBrush(COLORREF(color)) };
    let instance = unsafe { GetModuleHandleW(None)? };
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(frame_proc),
        hInstance: instance.into(),
        hbrBackground: brush,
        lpszClassName: w!("oh-snap.recording-frame"),
        ..Default::default()
    };
    unsafe { RegisterClassExW(&class) };
    let outer_width = area.width + FRAME_THICKNESS * 2;
    let outer_height = area.height + FRAME_THICKNESS * 2;
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT,
            w!("oh-snap.recording-frame"),
            w!("oh-snap recording"),
            WS_POPUP,
            area.left - FRAME_THICKNESS,
            area.top - FRAME_THICKNESS,
            outer_width,
            outer_height,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        let ring = CreateRectRgn(0, 0, outer_width, outer_height);
        let hole = CreateRectRgn(FRAME_THICKNESS, FRAME_THICKNESS, outer_width - FRAME_THICKNESS, outer_height - FRAME_THICKNESS);
        CombineRgn(Some(ring), Some(ring), Some(hole), RGN_DIFF);
        let _ = DeleteObject(hole.into());
        // The window owns the region from here on.
        SetWindowRgn(hwnd, Some(ring), false);
        exclude_from_capture(hwnd);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        Ok(hwnd)
    }
}

unsafe extern "system" fn frame_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

/// Where the bar goes: under the area, else over it, else inside its bottom edge.
pub(crate) fn bar_position(area: PixelRect, work: RECT, width: i32, height: i32) -> (i32, i32) {
    let centered = area.left + (area.width - width) / 2;
    let left = centered.clamp(work.left, (work.right - width).max(work.left));
    let below = area.bottom() + GAP_TO_AREA;
    let above = area.top - GAP_TO_AREA - height;
    let top = if below + height <= work.bottom {
        below
    } else if above >= work.top {
        above
    } else {
        area.bottom().min(work.bottom) - GAP_TO_AREA - height
    };
    (left, top)
}

fn open_bar(graphics: &Graphics, controls: Arc<Controls>, area: PixelRect) -> Result<HWND> {
    let center = POINT {
        x: area.left + area.width / 2,
        y: area.top + area.height / 2,
    };
    let monitor = unsafe { MonitorFromPoint(center, MONITOR_DEFAULTTONEAREST) };
    let mut monitor_info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let mut dpi_x = 96u32;
    let mut dpi_y = 96u32;
    unsafe {
        let _ = GetMonitorInfoW(monitor, &mut monitor_info);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
    }
    let scale = dpi_x as f32 / 96.0;
    let width = (BAR_WIDTH * scale).round() as i32;
    let height = (BAR_HEIGHT * scale).round() as i32;
    let (left, top) = bar_position(area, monitor_info.rcWork, width, height);

    let instance = unsafe { GetModuleHandleW(None)? };
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(bar_proc),
        hInstance: instance.into(),
        lpszClassName: w!("oh-snap.recording-bar"),
        ..Default::default()
    };
    unsafe { RegisterClassExW(&class) };
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("oh-snap.recording-bar"),
            w!("oh-snap recording"),
            WS_POPUP,
            left,
            top,
            width,
            height,
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
    let target = create_target(graphics, hwnd, width, height, dpi_x as f32)?;
    let bar = Box::new(Bar {
        graphics,
        controls,
        target,
        scale,
        hovered: None,
        buttons: Vec::new(),
    });
    exclude_from_capture(hwnd);
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(bar) as isize);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(Some(hwnd), REDRAW_TIMER, REDRAW_MILLISECONDS, None);
    }
    Ok(hwnd)
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

unsafe extern "system" fn bar_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let bar_pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Bar;
    if bar_pointer.is_null() {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    if message == WM_NCDESTROY {
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(bar_pointer));
        }
        return LRESULT(0);
    }
    let bar = unsafe { &mut *bar_pointer };
    match message {
        WM_PAINT => {
            bar.paint();
            unsafe {
                let _ = ValidateRect(Some(hwnd), None);
            }
            LRESULT(0)
        }
        WM_TIMER => {
            bar.paint();
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_SETCURSOR => {
            let shape = if bar.hovered.is_some() { IDC_HAND } else { IDC_ARROW };
            if let Ok(cursor) = unsafe { LoadCursorW(None, shape) } {
                unsafe { SetCursor(Some(cursor)) };
            }
            LRESULT(1)
        }
        WM_MOUSEMOVE => {
            let hovered = bar.button_at(lparam);
            if hovered != bar.hovered {
                bar.hovered = hovered;
                bar.paint();
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            match bar.button_at(lparam) {
                Some(Button::Pause) => {
                    let paused = bar.controls.paused.load(Ordering::SeqCst);
                    bar.controls.paused.store(!paused, Ordering::SeqCst);
                }
                Some(Button::Stop) => bar.controls.stop.store(true, Ordering::SeqCst),
                None => {}
            }
            bar.paint();
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

impl Bar {
    fn button_at(&self, lparam: LPARAM) -> Option<Button> {
        let x = (lparam.0 & 0xFFFF) as u16 as i16 as f32 / self.scale;
        let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32 / self.scale;
        self.buttons
            .iter()
            .find(|(_, rect)| x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom)
            .map(|(button, _)| *button)
    }

    fn paint(&mut self) {
        let look = Look {
            recorded_milliseconds: self.controls.recorded_milliseconds.load(Ordering::SeqCst),
            paused: self.controls.paused.load(Ordering::SeqCst),
            saving: self.controls.stop.load(Ordering::SeqCst),
            hovered: self.hovered,
        };
        let graphics = unsafe { &*self.graphics };
        self.buttons = draw(graphics, &self.target, &look);
    }
}

/// Draws the bar and returns where its buttons ended up.
fn draw(graphics: &Graphics, target: &ID2D1RenderTarget, look: &Look) -> Vec<(Button, D2D_RECT_F)> {
    let brush = |color| unsafe { target.CreateSolidColorBrush(&color, None) };
    let (Ok(text), Ok(dim), Ok(hover), Ok(failure), Ok(accent), Ok(rule)) = (
        brush(style::TEXT),
        brush(style::TEXT_DIM),
        brush(style::HOVER),
        brush(style::FAILURE),
        brush(style::ACCENT),
        brush(style::RULE),
    ) else {
        return Vec::new();
    };
    let middle = BAR_HEIGHT / 2.0;
    let text_top = middle - 9.0;
    let mut buttons = Vec::new();
    unsafe {
        target.BeginDraw();
        target.Clear(Some(&style::INK));
        let edge = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: 0.5, top: 0.5, right: BAR_WIDTH - 0.5, bottom: BAR_HEIGHT - 0.5 },
            radiusX: 8.0,
            radiusY: 8.0,
        };
        target.DrawRoundedRectangle(&edge, &rule, 1.0, None);
    }
    if look.saving {
        let spark = D2D_RECT_F { left: 6.0, top: 0.0, right: 26.0, bottom: BAR_HEIGHT - 1.0 };
        graphics.draw_spark(target, spark, &accent);
        crate::overlay::draw_label(graphics, target, "saving…", 30.0, text_top, &text);
    } else {
        // A red dot records; two accent bars are paused. The shape says it, not only the colour.
        if look.paused {
            let icon = D2D_RECT_F { left: 8.0, top: 0.0, right: 28.0, bottom: BAR_HEIGHT };
            graphics.draw_icon(target, crate::icons::PAUSE, icon, &accent);
        } else {
            let dot = D2D1_ELLIPSE { point: Vector2 { X: 18.0, Y: middle }, radiusX: 5.0, radiusY: 5.0 };
            unsafe { target.FillEllipse(&dot, &failure) };
        }
        let time = crate::recorder::clock_text(look.recorded_milliseconds);
        crate::overlay::draw_label(graphics, target, &time, 32.0, text_top, if look.paused { &dim } else { &text });
        let mut right = BAR_WIDTH - 6.0;
        let pause_item = if look.paused { (crate::icons::PLAY, "go on") } else { (crate::icons::PAUSE, "pause") };
        let items = [(Button::Stop, crate::icons::STOP, "stop"), (Button::Pause, pause_item.0, pause_item.1)];
        for (button, icon, label) in items {
            let (label_width, _) = graphics.measure(label);
            let rect = D2D_RECT_F {
                left: right - label_width - 34.0,
                top: 6.0,
                right,
                bottom: BAR_HEIGHT - 6.0,
            };
            if look.hovered == Some(button) {
                let shape = D2D1_ROUNDED_RECT { rect, radiusX: 4.0, radiusY: 4.0 };
                unsafe { target.FillRoundedRectangle(&shape, &hover) };
            }
            let icon_area = D2D_RECT_F { left: rect.left + 5.0, top: rect.top, right: rect.left + 25.0, bottom: rect.bottom };
            graphics.draw_icon(target, icon, icon_area, &accent);
            crate::overlay::draw_label(graphics, target, label, rect.left + 27.0, text_top, &text);
            buttons.push((button, rect));
            right = rect.left - 2.0;
        }
    }
    unsafe {
        let _ = target.EndDraw(None, None);
    }
    buttons
}

/// Draws the bar into memory for `oh-snap preview`. Returns BGRA pixels and the size.
pub fn render_offscreen(graphics: &Graphics, paused: bool, saving: bool) -> Result<(Vec<u8>, u32, u32)> {
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapCacheOnLoad,
    };
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    let width = BAR_WIDTH as u32;
    let height = BAR_HEIGHT as u32;
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
    let target = unsafe { graphics.d2d.CreateWicBitmapRenderTarget(&bitmap, &properties)? };
    let look = Look {
        recorded_milliseconds: 83_000,
        paused,
        saving,
        hovered: None,
    };
    draw(graphics, &target, &look);
    let stride = width * 4;
    let mut pixels = vec![0u8; (stride * height) as usize];
    unsafe { bitmap.CopyPixels(std::ptr::null(), stride, &mut pixels)? };
    Ok((pixels, width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: RECT = RECT { left: 0, top: 0, right: 1920, bottom: 1040 };

    #[test]
    fn the_bar_goes_under_then_over_then_inside() {
        let middle = PixelRect { left: 500, top: 200, width: 400, height: 300 };
        assert_eq!(bar_position(middle, WORK, 248, 40).1, 510);
        let low = PixelRect { left: 500, top: 700, width: 400, height: 320 };
        assert_eq!(bar_position(low, WORK, 248, 40).1, 650);
        let whole = PixelRect { left: 0, top: 0, width: 1920, height: 1080 };
        assert_eq!(bar_position(whole, WORK, 248, 40).1, 990);
    }

    #[test]
    fn the_bar_stays_on_the_monitor() {
        let far_left = PixelRect { left: 0, top: 200, width: 40, height: 40 };
        assert_eq!(bar_position(far_left, WORK, 248, 40).0, 0);
        let far_right = PixelRect { left: 1880, top: 200, width: 40, height: 40 };
        assert_eq!(bar_position(far_right, WORK, 248, 40).0, 1672);
    }
}
