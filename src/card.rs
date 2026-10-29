//! The card that confirms a snip: a thumbnail, what happened and where the file went.
//! It never takes focus, hides after a few seconds and stays while the pointer is on it.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F,
    D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES, D2D1_DRAW_TEXT_OPTIONS_CLIP,
    D2D1_FEATURE_LEVEL_DEFAULT, D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE,
    D2D1_ROUNDED_RECT, ID2D1Bitmap, ID2D1HwndRenderTarget, ID2D1RenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::DWRITE_MEASURING_MODE_NATURAL;
use windows::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DWM_WINDOW_CORNER_PREFERENCE, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ValidateRect};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW, IDC_HAND, KillTimer,
    LoadCursorW, MA_NOACTIVATE, RegisterClassExW, SW_SHOWNOACTIVATE, SW_SHOWNORMAL, SetCursor, SetTimer,
    SetWindowLongPtrW, ShowWindow, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_NCDESTROY,
    WM_PAINT, WM_SETCURSOR, WM_TIMER, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{HSTRING, Interface, Result, w};

use crate::capture::PixelRect;
use crate::output::Image;
use crate::overlay::Graphics;
use crate::style;

const CARD_WIDTH: f32 = 320.0;
const PADDING: f32 = 12.0;
const THUMBNAIL_MAX_HEIGHT: f32 = 168.0;
const LINE_HEIGHT: f32 = 18.0;
const SCREEN_MARGIN: f32 = 16.0;
const HIDE_AFTER_MS: u32 = 5000;
const HIDE_TIMER: usize = 1;
// Not in the windows crate's WindowsAndMessaging module.
const WM_MOUSELEAVE: u32 = 0x02A3;

pub enum CardMessage {
    Saved { copied: bool, path: Option<PathBuf> },
    Failed(String),
}

struct CardState {
    graphics: *const Graphics,
    target: ID2D1RenderTarget,
    thumbnail: Option<ID2D1Bitmap>,
    image_size: (f32, f32),
    message: CardMessage,
    size: (f32, f32),
    hovered_action: Option<Action>,
    pointer_inside: bool,
    buttons: Vec<(Action, D2D_RECT_F)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    OpenFile,
    OpenFolder,
}

static mut CURRENT_CARD: HWND = HWND(std::ptr::null_mut());

/// Replaces any card on screen with a new one near the bottom right of `near`'s monitor.
pub fn show(graphics: &Graphics, image: Option<&Image>, message: CardMessage, near: PixelRect) -> Result<()> {
    close_current();
    register_class();

    let center = POINT {
        x: near.left + near.width / 2,
        y: near.top + near.height / 2,
    };
    let monitor = unsafe { MonitorFromPoint(center, MONITOR_DEFAULTTONEAREST) };
    let mut monitor_info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        let _ = GetMonitorInfoW(monitor, &mut monitor_info);
    }
    let mut dpi_x = 96u32;
    let mut dpi_y = 96u32;
    unsafe {
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
    }
    let scale = dpi_x as f32 / 96.0;

    let image_size = image.map(|image| (image.width as f32, image.height as f32)).unwrap_or((0.0, 0.0));
    let thumbnail_height = thumbnail_rect(image_size).map(|rect| rect.bottom - rect.top + PADDING).unwrap_or(0.0);
    let card_height = PADDING + thumbnail_height + LINE_HEIGHT * 2.0 + PADDING;
    let pixel_width = (CARD_WIDTH * scale).round() as i32;
    let pixel_height = (card_height * scale).round() as i32;
    let work: RECT = monitor_info.rcWork;
    let margin = (SCREEN_MARGIN * scale).round() as i32;

    let instance = unsafe { GetModuleHandleW(None)? };
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("oh-snap.card"),
            w!("oh-snap"),
            WS_POPUP,
            work.right - margin - pixel_width,
            work.bottom - margin - pixel_height,
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

    let target = create_target(graphics, hwnd, pixel_width, pixel_height, dpi_x as f32)?;
    let thumbnail = match image {
        Some(image) => Some(create_thumbnail(&target, image)?),
        None => None,
    };
    let state = Box::new(CardState {
        graphics,
        target,
        thumbnail,
        image_size,
        message,
        size: (CARD_WIDTH, card_height),
        hovered_action: None,
        pointer_inside: false,
        buttons: Vec::new(),
    });
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(Some(hwnd), HIDE_TIMER, HIDE_AFTER_MS, None);
        CURRENT_CARD = hwnd;
    }
    Ok(())
}

pub fn close_current() {
    unsafe {
        let current = CURRENT_CARD;
        if !current.is_invalid() {
            CURRENT_CARD = HWND(std::ptr::null_mut());
            let _ = DestroyWindow(current);
        }
    }
}

fn register_class() {
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(card_proc),
        hInstance: unsafe { GetModuleHandleW(None) }.unwrap_or_default().into(),
        lpszClassName: w!("oh-snap.card"),
        ..Default::default()
    };
    unsafe { RegisterClassExW(&class) };
}

fn create_target(graphics: &Graphics, hwnd: HWND, width: i32, height: i32, dpi: f32) -> Result<ID2D1RenderTarget> {
    let properties = D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        // The monitor's DPI, so the card is drawn in DIPs and looks the same at any scale.
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

fn create_thumbnail(target: &ID2D1RenderTarget, image: &Image) -> Result<ID2D1Bitmap> {
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

/// Where the thumbnail sits, fitted inside the card width and the height limit.
fn thumbnail_rect(image_size: (f32, f32)) -> Option<D2D_RECT_F> {
    let (image_width, image_height) = image_size;
    if image_width <= 0.0 || image_height <= 0.0 {
        return None;
    }
    let box_width = CARD_WIDTH - PADDING * 2.0;
    let fit = (box_width / image_width).min(THUMBNAIL_MAX_HEIGHT / image_height).min(1.0);
    let width = (image_width * fit).max(1.0);
    let height = (image_height * fit).max(1.0);
    let left = PADDING + (box_width - width) / 2.0;
    Some(D2D_RECT_F {
        left,
        top: PADDING,
        right: left + width,
        bottom: PADDING + height,
    })
}

unsafe extern "system" fn card_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let state_pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut CardState;
    if state_pointer.is_null() {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    if message == WM_NCDESTROY {
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(state_pointer));
            if CURRENT_CARD == hwnd {
                CURRENT_CARD = HWND(std::ptr::null_mut());
            }
        }
        return LRESULT(0);
    }
    let state = unsafe { &mut *state_pointer };
    match message {
        WM_PAINT => {
            state.paint();
            unsafe {
                let _ = ValidateRect(Some(hwnd), None);
            }
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_SETCURSOR => {
            if let Ok(hand) = unsafe { LoadCursorW(None, IDC_HAND) } {
                unsafe { SetCursor(Some(hand)) };
            }
            LRESULT(1)
        }
        WM_MOUSEMOVE => {
            if !state.pointer_inside {
                state.pointer_inside = true;
                let mut tracking = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                unsafe {
                    let _ = TrackMouseEvent(&mut tracking);
                    let _ = KillTimer(Some(hwnd), HIDE_TIMER);
                }
            }
            let hovered = state.action_at(state.to_dips(hwnd, lparam));
            if hovered != state.hovered_action {
                state.hovered_action = hovered;
                state.paint();
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            state.pointer_inside = false;
            state.hovered_action = None;
            state.paint();
            unsafe { SetTimer(Some(hwnd), HIDE_TIMER, HIDE_AFTER_MS, None) };
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let action = state.action_at(state.to_dips(hwnd, lparam)).unwrap_or(Action::OpenFile);
            state.run(action);
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_TIMER => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

impl CardState {
    fn graphics(&self) -> &Graphics {
        unsafe { &*self.graphics }
    }

    fn saved_path(&self) -> Option<&Path> {
        match &self.message {
            CardMessage::Saved { path, .. } => path.as_deref(),
            CardMessage::Failed(_) => None,
        }
    }

    fn to_dips(&self, hwnd: HWND, lparam: LPARAM) -> (f32, f32) {
        let pixel_x = (lparam.0 & 0xFFFF) as u16 as i16 as f32;
        let pixel_y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
        let mut client = RECT::default();
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut client);
        }
        let scale = client.right.max(1) as f32 / self.size.0;
        (pixel_x / scale, pixel_y / scale)
    }

    fn action_at(&self, point: (f32, f32)) -> Option<Action> {
        self.buttons
            .iter()
            .find(|(_, rect)| point.0 >= rect.left && point.0 < rect.right && point.1 >= rect.top && point.1 < rect.bottom)
            .map(|(action, _)| *action)
    }

    fn run(&self, action: Action) {
        let Some(path) = self.saved_path() else {
            return;
        };
        unsafe {
            match action {
                Action::OpenFile => {
                    ShellExecuteW(None, w!("open"), &HSTRING::from(path.as_os_str()), None, None, SW_SHOWNORMAL);
                }
                Action::OpenFolder => {
                    let select = format!("/select,\"{}\"", path.display());
                    ShellExecuteW(None, w!("open"), w!("explorer.exe"), &HSTRING::from(select), None, SW_SHOWNORMAL);
                }
            }
        }
    }

    fn paint(&mut self) {
        let target = self.target.clone();
        let brush = |color: D2D1_COLOR_F| unsafe { target.CreateSolidColorBrush(&color, None) };
        let (Ok(rule), Ok(text), Ok(dim), Ok(accent), Ok(hover)) = (
            brush(style::RULE),
            brush(style::TEXT),
            brush(style::TEXT_DIM),
            brush(style::ACCENT),
            brush(style::HOVER),
        ) else {
            return;
        };
        let failure = brush(style::FAILURE).ok();
        let (width, height) = self.size;
        unsafe {
            target.BeginDraw();
            target.Clear(Some(&style::INK));
            let edge = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F { left: 0.5, top: 0.5, right: width - 0.5, bottom: height - 0.5 },
                radiusX: 8.0,
                radiusY: 8.0,
            };
            target.DrawRoundedRectangle(&edge, &rule, 1.0, None);
        }

        let mut line_top = PADDING;
        if let (Some(thumbnail), Some(frame)) = (&self.thumbnail, thumbnail_rect(self.image_size)) {
            unsafe {
                target.DrawBitmap(thumbnail, Some(&frame), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                let outline = D2D_RECT_F {
                    left: frame.left - 0.5,
                    top: frame.top - 0.5,
                    right: frame.right + 0.5,
                    bottom: frame.bottom + 0.5,
                };
                target.DrawRectangle(&outline, &rule, 1.0, None);
            }
            line_top = frame.bottom + PADDING;
        }

        let mut buttons = Vec::new();
        match &self.message {
            CardMessage::Saved { copied, path } => {
                let headline = match (copied, path.is_some()) {
                    (true, true) => "copied · saved",
                    (true, false) => "copied to clipboard",
                    (false, true) => "saved",
                    (false, false) => "snipped",
                };
                draw_mark(&target, Mark::Check, PADDING, line_top, &accent);
                self.text(&target, headline, PADDING + 16.0, line_top, &text);
                if let Some(path) = path {
                    let file_name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
                    let folder_label = "folder";
                    let (label_width, _) = self.graphics().measure(folder_label);
                    let button = D2D_RECT_F {
                        left: width - PADDING - label_width - 12.0,
                        top: line_top - 3.0,
                        right: width - PADDING,
                        bottom: line_top + LINE_HEIGHT + 1.0,
                    };
                    if self.hovered_action == Some(Action::OpenFolder) {
                        let shape = D2D1_ROUNDED_RECT { rect: button, radiusX: 4.0, radiusY: 4.0 };
                        unsafe { target.FillRoundedRectangle(&shape, &hover) };
                    }
                    self.text(&target, folder_label, button.left + 6.0, line_top, &accent);
                    buttons.push((Action::OpenFolder, button));
                    self.text(&target, &file_name, PADDING + 16.0, line_top + LINE_HEIGHT, &dim);
                }
            }
            CardMessage::Failed(reason) => {
                let mark_brush = failure.as_ref().unwrap_or(&accent);
                draw_mark(&target, Mark::Cross, PADDING, line_top, mark_brush);
                self.text(&target, "snip failed", PADDING + 16.0, line_top, &text);
                self.text(&target, reason, PADDING + 16.0, line_top + LINE_HEIGHT, &dim);
            }
        }
        // Anywhere else on the card opens the file.
        buttons.push((
            Action::OpenFile,
            D2D_RECT_F { left: 0.0, top: 0.0, right: width, bottom: height },
        ));
        self.buttons = buttons;
        unsafe {
            let _ = target.EndDraw(None, None);
        }
    }

    fn text(&self, target: &ID2D1RenderTarget, text: &str, left: f32, top: f32, brush: &ID2D1SolidColorBrush) {
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let layout_rect = D2D_RECT_F {
            left,
            top,
            right: self.size.0 - PADDING,
            bottom: top + LINE_HEIGHT,
        };
        unsafe {
            target.DrawText(
                &utf16,
                &self.graphics().label_format,
                &layout_rect,
                brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }
}

/// Draws a card into memory for `oh-snap preview`. Returns BGRA pixels and the size.
pub fn render_offscreen(graphics: &Graphics, image: Option<&Image>, message: CardMessage) -> Result<(Vec<u8>, u32, u32)> {
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapCacheOnLoad,
    };
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};

    let image_size = image.map(|image| (image.width as f32, image.height as f32)).unwrap_or((0.0, 0.0));
    let thumbnail_height = thumbnail_rect(image_size).map(|rect| rect.bottom - rect.top + PADDING).unwrap_or(0.0);
    let card_height = PADDING + thumbnail_height + LINE_HEIGHT * 2.0 + PADDING;
    let width = CARD_WIDTH as u32;
    let height = card_height.round() as u32;
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
    let target: ID2D1RenderTarget = unsafe { graphics.d2d.CreateWicBitmapRenderTarget(&bitmap, &properties)? };
    let thumbnail = match image {
        Some(image) => Some(create_thumbnail(&target, image)?),
        None => None,
    };
    let mut state = CardState {
        graphics,
        target,
        thumbnail,
        image_size,
        message,
        size: (CARD_WIDTH, card_height),
        hovered_action: Some(Action::OpenFolder),
        pointer_inside: true,
        buttons: Vec::new(),
    };
    state.paint();
    let stride = width * 4;
    let mut pixels = vec![0u8; (stride * height) as usize];
    unsafe { bitmap.CopyPixels(std::ptr::null(), stride, &mut pixels)? };
    Ok((pixels, width, height))
}

enum Mark {
    Check,
    Cross,
}

/// The outcome mark, drawn as strokes so its shape does not depend on font fallback.
fn draw_mark(target: &ID2D1RenderTarget, mark: Mark, left: f32, line_top: f32, brush: &ID2D1SolidColorBrush) {
    use windows_numerics::Vector2;
    let middle = line_top + LINE_HEIGHT / 2.0;
    let segments: &[((f32, f32), (f32, f32))] = match mark {
        Mark::Check => &[((0.5, 0.0), (4.0, 3.5)), ((4.0, 3.5), (11.0, -4.5))],
        Mark::Cross => &[((1.0, -4.5), (10.0, 4.5)), ((10.0, -4.5), (1.0, 4.5))],
    };
    for (from, to) in segments {
        unsafe {
            target.DrawLine(
                Vector2 { X: left + from.0, Y: middle + from.1 },
                Vector2 { X: left + to.0, Y: middle + to.1 },
                brush,
                1.8,
                None,
            );
        }
    }
}
