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
    SetWindowLongPtrW, ShowWindow, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_NCDESTROY, WM_RBUTTONUP,
    WM_PAINT, WM_SETCURSOR, WM_TIMER, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{HSTRING, Interface, Result, w};

use crate::capture::PixelRect;
use crate::image_menu::{self, ImageAction};
use crate::output::Image;
use crate::overlay::Graphics;
use crate::style;

const CARD_WIDTH: f32 = 320.0;
const PADDING: f32 = 12.0;
const THUMBNAIL_MAX_HEIGHT: f32 = 168.0;
const LINE_HEIGHT: f32 = 18.0;
const SCREEN_MARGIN: f32 = 16.0;
/// How long a card stays, from the "card_seconds" setting.
static HIDE_AFTER_MS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(5000);

pub fn set_hide_after(seconds: u32) {
    HIDE_AFTER_MS.store(seconds * 1000, std::sync::atomic::Ordering::Relaxed);
}

fn hide_after_ms() -> u32 {
    HIDE_AFTER_MS.load(std::sync::atomic::Ordering::Relaxed)
}
const HIDE_TIMER: usize = 1;
/// Redraws the countdown ring while the card waits to hide.
const TICK_TIMER: usize = 2;
const TICK_MILLISECONDS: u32 = 33;
const RING_RADIUS: f32 = 7.0;
/// How far, in DIPs, the pointer moves with the button down before a click becomes a drag.
const DRAG_DISTANCE: f32 = 4.0;
// Not in the windows crate's WindowsAndMessaging module.
const WM_MOUSELEAVE: u32 = 0x02A3;

pub enum CardMessage {
    /// `history` is the kept copy, dragged out when the snip was not saved to a folder.
    Saved { copied: bool, path: Option<PathBuf>, history: Option<PathBuf> },
    /// A snip from the tray's recent list, put back on the clipboard.
    Recent { path: PathBuf },
    /// A colour from the overlay's colour tool, with its hex value on the clipboard.
    Color([u8; 3]),
    Failed(String),
    /// The delay timer: seconds left before the capture.
    Countdown(u32),
    /// A plain note with the spark: updates and other news from the tray process.
    Notice { title: String, detail: String },
}

struct CardState {
    graphics: *const Graphics,
    target: ID2D1RenderTarget,
    thumbnail: Option<ID2D1Bitmap>,
    /// A copy of the snip for the right-click menu.
    image: Option<Image>,
    /// Where the snip came from on screen; a pin opens there.
    area: PixelRect,
    image_size: (f32, f32),
    message: CardMessage,
    size: (f32, f32),
    hovered_action: Option<Action>,
    pointer_inside: bool,
    buttons: Vec<(Action, D2D_RECT_F)>,
    /// When the hide countdown started; none while the pointer holds the card open.
    hide_started: Option<std::time::Instant>,
    hide_after: std::time::Duration,
    /// Where the left button went down on the thumbnail; moving away from it starts a drag.
    press: Option<(f32, f32)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    OpenFile,
    OpenFolder,
    Edit,
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
    let mut state = Box::new(CardState {
        graphics,
        target,
        thumbnail,
        image: image.map(|image| Image {
            width: image.width,
            height: image.height,
            bgra: image.bgra.clone(),
        }),
        area: near,
        image_size,
        message,
        size: (CARD_WIDTH, card_height),
        hovered_action: None,
        pointer_inside: false,
        buttons: Vec::new(),
        hide_started: None,
        hide_after: std::time::Duration::from_millis(u64::from(hide_after_ms())),
        press: None,
    });
    state.start_countdown(hwnd);
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
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
                }
                state.stop_countdown(hwnd);
                state.paint();
            }
            let left_button_down = wparam.0 & 0x0001 != 0;
            if let (Some(press), true) = (state.press, left_button_down) {
                let point = state.to_dips(hwnd, lparam);
                if (point.0 - press.0).abs() > DRAG_DISTANCE || (point.1 - press.1).abs() > DRAG_DISTANCE {
                    state.press = None;
                    if let Some(file) = state.drag_file() {
                        state.stop_countdown(hwnd);
                        drag_out(hwnd, &file);
                        unsafe {
                            let _ = DestroyWindow(hwnd);
                        }
                        return LRESULT(0);
                    }
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
            state.start_countdown(hwnd);
            state.paint();
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let point = state.to_dips(hwnd, lparam);
            state.press = state.over_thumbnail(point).then_some(point);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            state.press = None;
            let action = state.action_at(state.to_dips(hwnd, lparam)).unwrap_or(Action::OpenFile);
            state.run(action);
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            let point = state.to_dips(hwnd, lparam);
            if state.over_thumbnail(point) && state.open_image_menu(hwnd) {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TICK_TIMER => {
            state.paint();
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
    /// Starts the time to hide from the full "card_seconds".
    fn start_countdown(&mut self, hwnd: HWND) {
        self.hide_started = Some(std::time::Instant::now());
        let milliseconds = self.hide_after.as_millis() as u32;
        unsafe {
            SetTimer(Some(hwnd), HIDE_TIMER, milliseconds, None);
            SetTimer(Some(hwnd), TICK_TIMER, TICK_MILLISECONDS, None);
        }
    }

    /// Holds the card open, for as long as the pointer is on it or a menu is up.
    fn stop_countdown(&mut self, hwnd: HWND) {
        self.hide_started = None;
        unsafe {
            let _ = KillTimer(Some(hwnd), HIDE_TIMER);
            let _ = KillTimer(Some(hwnd), TICK_TIMER);
        }
    }

    /// The share of the countdown still left, from 1 down to 0; 1 while held open.
    fn time_left(&self) -> f32 {
        match self.hide_started {
            Some(started) => {
                let fraction = started.elapsed().as_secs_f32() / self.hide_after.as_secs_f32().max(0.001);
                (1.0 - fraction).clamp(0.0, 1.0)
            }
            None => 1.0,
        }
    }

    fn graphics(&self) -> &Graphics {
        unsafe { &*self.graphics }
    }

    fn saved_path(&self) -> Option<&Path> {
        match &self.message {
            CardMessage::Saved { path, .. } => path.as_deref(),
            CardMessage::Recent { path } => Some(path),
            CardMessage::Failed(_) | CardMessage::Countdown(_) | CardMessage::Notice { .. } | CardMessage::Color(_) => None,
        }
    }

    fn over_thumbnail(&self, point: (f32, f32)) -> bool {
        thumbnail_rect(self.image_size).is_some_and(|frame| {
            point.0 >= frame.left && point.0 < frame.right && point.1 >= frame.top && point.1 < frame.bottom
        })
    }

    /// The file a drag hands over: the saved snip, or the kept copy when saving is off.
    fn drag_file(&self) -> Option<PathBuf> {
        if let Some(path) = self.saved_path() {
            return Some(path.to_path_buf());
        }
        match &self.message {
            CardMessage::Saved { history, .. } => history.clone(),
            _ => None,
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

    /// Returns true when the card has done its job and should close.
    fn open_image_menu(&mut self, hwnd: HWND) -> bool {
        if self.image.is_none() {
            return false;
        }
        self.stop_countdown(hwnd);
        let Some(image) = self.image.as_ref() else {
            return false;
        };
        let finished = match image_menu::show(hwnd, &image_menu::SNIP_ACTIONS) {
            Some(ImageAction::Copy) => {
                image_menu::copy(hwnd, image);
                false
            }
            Some(ImageAction::SaveAs) => {
                let folder = match self.saved_path().and_then(Path::parent) {
                    Some(folder) => folder.to_path_buf(),
                    None => crate::settings::load().save_folder(),
                };
                image_menu::save_as(hwnd, image, &folder).is_some()
            }
            Some(ImageAction::Pin) => {
                if let Err(error) = crate::pin::open(self.graphics(), crate::pin::copy_image(image), self.area) {
                    crate::log::write(&format!("pin failed: {error}"));
                }
                true
            }
            Some(ImageAction::Close) => true,
            Some(ImageAction::Edit) => {
                let copy = Image {
                    width: image.width,
                    height: image.height,
                    bgra: image.bgra.clone(),
                };
                let saved_path = self.saved_path().map(Path::to_path_buf);
                if let Err(error) = crate::editor::open(self.graphics(), copy, saved_path, false) {
                    crate::log::write(&format!("editor failed: {error}"));
                }
                true
            }
            None => false,
        };
        if !finished && !self.pointer_inside {
            self.start_countdown(hwnd);
        }
        finished
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
                Action::Edit => crate::daemon::open_editor(self.graphics(), path.to_path_buf()),
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
            CardMessage::Saved { .. } | CardMessage::Recent { .. } => {
                let (headline, path) = match &self.message {
                    CardMessage::Saved { copied, path, .. } => {
                        let headline = match (copied, path.is_some()) {
                            (true, true) => "copied · saved",
                            (true, false) => "copied to clipboard",
                            (false, true) => "saved",
                            (false, false) => "snipped",
                        };
                        (headline, path.as_ref())
                    }
                    CardMessage::Recent { path } => ("copied again", Some(path)),
                    _ => ("", None),
                };
                draw_mark(&target, Mark::Check, PADDING, line_top, &accent);
                self.text(&target, headline, PADDING + 16.0, line_top, &text);
                if let Some(path) = path {
                    let file_name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
                    let mut right = width - PADDING;
                    let actions = [(Action::OpenFolder, crate::icons::FOLDER, "folder"), (Action::Edit, crate::icons::EDIT, "edit")];
                    for (action, icon, label) in actions {
                        let (label_width, _) = self.graphics().measure(label);
                        let button = D2D_RECT_F {
                            left: right - label_width - 12.0 - 18.0,
                            top: line_top - 3.0,
                            right,
                            bottom: line_top + LINE_HEIGHT + 1.0,
                        };
                        if self.hovered_action == Some(action) {
                            let shape = D2D1_ROUNDED_RECT { rect: button, radiusX: 4.0, radiusY: 4.0 };
                            unsafe { target.FillRoundedRectangle(&shape, &hover) };
                        }
                        let icon_area = D2D_RECT_F { left: button.left + 3.0, top: button.top, right: button.left + 21.0, bottom: button.bottom };
                        self.graphics().draw_icon(&target, icon, icon_area, &accent);
                        self.text(&target, label, button.left + 6.0 + 18.0, line_top, &accent);
                        buttons.push((action, button));
                        right = button.left - 2.0;
                    }
                    self.text(&target, &file_name, PADDING + 16.0, line_top + LINE_HEIGHT, &dim);
                }
            }
            CardMessage::Failed(reason) => {
                let mark_brush = failure.as_ref().unwrap_or(&accent);
                draw_mark(&target, Mark::Cross, PADDING, line_top, mark_brush);
                self.text(&target, "snip failed", PADDING + 16.0, line_top, &text);
                self.text(&target, reason, PADDING + 16.0, line_top + LINE_HEIGHT, &dim);
            }
            CardMessage::Color(rgb) => {
                let swatch = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F { left: PADDING, top: line_top + 2.0, right: PADDING + 12.0, bottom: line_top + 14.0 },
                    radiusX: 3.0,
                    radiusY: 3.0,
                };
                if let Ok(fill) = brush(style::rgb(rgb[0], rgb[1], rgb[2], 1.0)) {
                    unsafe {
                        target.FillRoundedRectangle(&swatch, &fill);
                        target.DrawRoundedRectangle(&swatch, &rule, 1.0, None);
                    }
                }
                let title = format!("{} copied", crate::output::color_hex(*rgb));
                self.text(&target, &title, PADDING + 18.0, line_top, &text);
                self.text(&target, &crate::output::color_details(*rgb), PADDING + 18.0, line_top + LINE_HEIGHT, &dim);
            }
            CardMessage::Notice { title, detail } => {
                let spark = D2D_RECT_F { left: PADDING - 6.0, top: line_top - 4.0, right: PADDING + 14.0, bottom: line_top + LINE_HEIGHT + 2.0 };
                self.graphics().draw_spark(&target, spark, &accent);
                self.text(&target, title, PADDING + 16.0, line_top, &text);
                self.text(&target, detail, PADDING + 16.0, line_top + LINE_HEIGHT, &dim);
            }
            CardMessage::Countdown(seconds) => {
                let spark = D2D_RECT_F { left: PADDING - 6.0, top: line_top - 4.0, right: PADDING + 14.0, bottom: line_top + LINE_HEIGHT + 2.0 };
                self.graphics().draw_spark(&target, spark, &accent);
                self.text(&target, &format!("snipping in {seconds}"), PADDING + 16.0, line_top, &text);
            }
        }
        if !matches!(self.message, CardMessage::Countdown(_)) {
            let center = (width - PADDING - RING_RADIUS, PADDING + RING_RADIUS);
            let held_open = self.hide_started.is_none();
            let arc_brush = if held_open { &dim } else { &accent };
            draw_countdown_ring(self.graphics(), &target, center, self.time_left(), &rule, arc_brush);
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
        image: None,
        area: PixelRect { left: 0, top: 0, width: 0, height: 0 },
        image_size,
        message,
        size: (CARD_WIDTH, card_height),
        hovered_action: Some(Action::OpenFolder),
        pointer_inside: true,
        buttons: Vec::new(),
        // A third of the way through, so the preview shows the ring running.
        hide_started: std::time::Instant::now().checked_sub(std::time::Duration::from_millis(1700)),
        hide_after: std::time::Duration::from_millis(5000),
        press: None,
    };
    state.paint();
    let stride = width * 4;
    let mut pixels = vec![0u8; (stride * height) as usize];
    unsafe { bitmap.CopyPixels(std::ptr::null(), stride, &mut pixels)? };
    Ok((pixels, width, height))
}

/// Drags the snip file out of the card, into a chat, a browser or Explorer. The shell's own
/// data object for the file carries it, so every app that takes dropped files takes it.
fn drag_out(hwnd: HWND, file: &Path) {
    use windows::Win32::System::Com::IDataObject;
    use windows::Win32::System::Ole::DROPEFFECT_COPY;
    use windows::Win32::UI::Shell::{BHID_DataObject, IShellItem, SHCreateItemFromParsingName, SHDoDragDrop};
    let dragged = unsafe {
        SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(file.as_os_str()), None)
            .and_then(|item| item.BindToHandler::<_, IDataObject>(None, &BHID_DataObject))
            .and_then(|data| SHDoDragDrop(Some(hwnd), &data, None, DROPEFFECT_COPY))
    };
    if let Err(error) = dragged {
        crate::log::write(&format!("drag of {} failed: {error}", file.display()));
    }
}

/// A ring in the corner that empties clockwise until the card hides. A dark disc keeps it
/// readable over the thumbnail.
fn draw_countdown_ring(
    graphics: &Graphics,
    target: &ID2D1RenderTarget,
    center: (f32, f32),
    time_left: f32,
    track: &ID2D1SolidColorBrush,
    arc: &ID2D1SolidColorBrush,
) {
    use windows::Win32::Graphics::Direct2D::Common::{D2D_SIZE_F, D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_OPEN};
    use windows::Win32::Graphics::Direct2D::{
        D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_LARGE, D2D1_ARC_SIZE_SMALL, D2D1_ELLIPSE, D2D1_SWEEP_DIRECTION_CLOCKWISE,
    };
    use windows_numerics::Vector2;
    const STROKE: f32 = 2.0;
    let middle = Vector2 { X: center.0, Y: center.1 };
    let ring = D2D1_ELLIPSE { point: middle, radiusX: RING_RADIUS, radiusY: RING_RADIUS };
    let backing = D2D1_ELLIPSE { point: middle, radiusX: RING_RADIUS + 4.0, radiusY: RING_RADIUS + 4.0 };
    let Ok(ink) = (unsafe { target.CreateSolidColorBrush(&style::INK, None) }) else {
        return;
    };
    unsafe {
        target.FillEllipse(&backing, &ink);
        target.DrawEllipse(&ring, track, STROKE, None);
    }
    if time_left >= 0.999 {
        unsafe { target.DrawEllipse(&ring, arc, STROKE, None) };
        return;
    }
    if time_left <= 0.001 {
        return;
    }
    let point_at = |turn: f32| {
        let angle = turn * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        Vector2 { X: center.0 + RING_RADIUS * angle.cos(), Y: center.1 + RING_RADIUS * angle.sin() }
    };
    let Ok(geometry) = (unsafe { graphics.d2d.CreatePathGeometry() }) else {
        return;
    };
    let Ok(sink) = (unsafe { geometry.Open() }) else {
        return;
    };
    let segment = D2D1_ARC_SEGMENT {
        point: point_at(time_left),
        size: D2D_SIZE_F { width: RING_RADIUS, height: RING_RADIUS },
        rotationAngle: 0.0,
        sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
        arcSize: if time_left > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
    };
    unsafe {
        sink.BeginFigure(point_at(0.0), D2D1_FIGURE_BEGIN_HOLLOW);
        sink.AddArc(&segment);
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        if sink.Close().is_ok() {
            target.DrawGeometry(&geometry, arc, STROKE, None);
        }
    }
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
