//! The selection overlay: the frozen screenshot under a shade, a terminal-style toolbar,
//! a loupe, and region, window or screen picking.

use std::time::Instant;

use windows_numerics::Vector2;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, TRUE, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F,
    D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, D2D1_BITMAP_PROPERTIES, D2D1_CAP_STYLE_FLAT,
    D2D1_DASH_STYLE_DASH, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_FEATURE_LEVEL_DEFAULT, D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_LINE_JOIN_MITER,
    D2D1_PRESENT_OPTIONS_IMMEDIATELY, D2D1_RENDER_TARGET_PROPERTIES, D2D1_ROUNDED_RECT, D2D1_RENDER_TARGET_TYPE_DEFAULT,
    D2D1_RENDER_TARGET_USAGE_NONE, D2D1_STROKE_STYLE_PROPERTIES, D2D1CreateFactory, ID2D1Bitmap,
    ID2D1Factory, ID2D1HwndRenderTarget, ID2D1RenderTarget, ID2D1SolidColorBrush, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapCacheOnLoad,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_METRICS, DWriteCreateFactory,
    IDWriteFactory, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DWMWA_TRANSITIONS_FORCEDISABLED, DwmGetWindowAttribute,
    DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::ValidateRect;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, SetFocus, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT,
    VK_UP, GetKeyState,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows,
    GWL_EXSTYLE, GWLP_USERDATA, GetCursorPos, GetMessageW, GetWindowLongPtrW, GetWindowLongW,
    GetWindowThreadProcessId, IDC_ARROW, IDC_CROSS, IsIconic, IsWindowVisible, LoadCursorW, MSG,
    SW_SHOW, SetCursor, SetCursorPos, SetForegroundWindow, SetWindowLongPtrW, ShowWindow,
    TranslateMessage, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT,
    WM_RBUTTONUP, WM_SETCURSOR, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    WS_POPUP, RegisterClassExW,
};
use windows::core::{BOOL, HSTRING, Interface, Result, w};

use crate::capture::{PixelRect, Screenshot};
use crate::style;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Region,
    Window,
    Screen,
}

impl Mode {
    pub fn parse(text: &str) -> Option<Mode> {
        match text.to_ascii_lowercase().as_str() {
            "region" | "rect" | "rectangle" => Some(Mode::Region),
            "window" => Some(Mode::Window),
            "screen" | "fullscreen" | "monitor" => Some(Mode::Screen),
            _ => None,
        }
    }
}

const TOOLBAR_ITEMS: [(Mode, &str, &str); 3] = [
    (Mode::Region, "r", "region"),
    (Mode::Window, "w", "window"),
    (Mode::Screen, "s", "screen"),
];

/// Device-independent factories, created once and reused by every overlay.
pub struct Graphics {
    pub(crate) d2d: ID2D1Factory,
    pub(crate) dwrite: IDWriteFactory,
    pub(crate) label_format: IDWriteTextFormat,
    /// Centred in its box; draws the ✻ mark.
    pub(crate) spark_format: IDWriteTextFormat,
}

impl Graphics {
    pub fn new() -> Result<Self> {
        let d2d: ID2D1Factory = unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let dwrite: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let family = pick_mono_font(&dwrite);
        let label_format = create_format(&dwrite, family, style::LABEL_SIZE)?;
        let spark_format = create_format(&dwrite, "Segoe UI Symbol", style::SPARK_SIZE)?;
        unsafe {
            spark_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            spark_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        }
        Ok(Self {
            d2d,
            dwrite,
            label_format,
            spark_format,
        })
    }

    pub(crate) fn draw_spark(&self, target: &ID2D1RenderTarget, area: D2D_RECT_F, brush: &ID2D1SolidColorBrush) {
        let utf16: Vec<u16> = style::SPARK.encode_utf16().collect();
        unsafe {
            target.DrawText(
                &utf16,
                &self.spark_format,
                &area,
                brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    pub(crate) fn measure(&self, text: &str) -> (f32, f32) {
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let layout = unsafe { self.dwrite.CreateTextLayout(&utf16, &self.label_format, 10_000.0, 1_000.0) };
        let Ok(layout) = layout else {
            return (text.len() as f32 * 8.0, style::LABEL_SIZE * 1.3);
        };
        let mut metrics = DWRITE_TEXT_METRICS::default();
        if unsafe { layout.GetMetrics(&mut metrics) }.is_err() {
            return (text.len() as f32 * 8.0, style::LABEL_SIZE * 1.3);
        }
        (metrics.widthIncludingTrailingWhitespace, metrics.height)
    }
}

fn create_format(dwrite: &IDWriteFactory, family: &str, size: f32) -> Result<IDWriteTextFormat> {
    unsafe {
        dwrite.CreateTextFormat(
            &HSTRING::from(family),
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!("en-us"),
        )
    }
}

fn pick_mono_font(dwrite: &IDWriteFactory) -> &'static str {
    let mut collection = None;
    if unsafe { dwrite.GetSystemFontCollection(&mut collection, false) }.is_ok() {
        if let Some(collection) = collection {
            for family in style::MONO_FONTS {
                let mut index = 0;
                let mut exists = BOOL(0);
                let found = unsafe { collection.FindFamilyName(&HSTRING::from(family), &mut index, &mut exists) };
                if found.is_ok() && exists.as_bool() {
                    return family;
                }
            }
        }
    }
    style::MONO_FONTS[style::MONO_FONTS.len() - 1]
}

pub struct OverlayOptions {
    pub start_mode: Mode,
    /// For the benchmark: close right after the first frame is on screen.
    pub close_after_first_frame: bool,
}

pub struct OverlayOutcome {
    pub selection: Option<PixelRect>,
    pub first_frame_at: Option<Instant>,
}

struct Brushes {
    ink_panel: ID2D1SolidColorBrush,
    text: ID2D1SolidColorBrush,
    text_dim: ID2D1SolidColorBrush,
    rule: ID2D1SolidColorBrush,
    hover: ID2D1SolidColorBrush,
    accent: ID2D1SolidColorBrush,
    accent_soft: ID2D1SolidColorBrush,
    shade: ID2D1SolidColorBrush,
    crosshair: ID2D1SolidColorBrush,
}

struct OverlayState {
    graphics: *const Graphics,
    screenshot: *const Screenshot,
    window_rects: Vec<PixelRect>,
    mode: Mode,
    cursor: (i32, i32),
    press_point: Option<(i32, i32)>,
    dragging: bool,
    toolbar_hits: Vec<(Mode, D2D_RECT_F)>,
    target: ID2D1RenderTarget,
    bitmap: ID2D1Bitmap,
    brushes: Brushes,
    dash: ID2D1StrokeStyle,
    selection: Option<PixelRect>,
    done: bool,
    close_after_first_frame: bool,
    first_frame_at: Option<Instant>,
}

/// Shows the overlay and blocks until the user picks an area or cancels.
pub fn select_area(graphics: &Graphics, screenshot: &Screenshot, options: OverlayOptions) -> Result<OverlayOutcome> {
    let bounds = screenshot.bounds;
    let window_rects = visible_window_rects(bounds);
    register_class();
    let instance = unsafe { GetModuleHandleW(None)? };
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("oh-snap.overlay"),
            w!("oh-snap"),
            WS_POPUP,
            bounds.left,
            bounds.top,
            bounds.width,
            bounds.height,
            None,
            None,
            Some(instance.into()),
            None,
        )?
    };
    let disable_transitions = TRUE;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            &disable_transitions as *const BOOL as *const _,
            size_of::<BOOL>() as u32,
        );
    }

    let target: ID2D1RenderTarget = create_target(graphics, hwnd, bounds)?.cast()?;
    let mut cursor_point = POINT::default();
    unsafe { GetCursorPos(&mut cursor_point)? };
    let cursor = (cursor_point.x, cursor_point.y);
    let mut state = OverlayState::new(graphics, screenshot, target, window_rects, options.start_mode, cursor)?;
    state.close_after_first_frame = options.close_after_first_frame;
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut state as *mut OverlayState as isize) };

    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
    }
    state.paint();
    state.first_frame_at = Some(Instant::now());
    if state.close_after_first_frame {
        state.done = true;
    }

    let mut message = MSG::default();
    while !state.done {
        let got_message = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if got_message.0 <= 0 {
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
    Ok(OverlayOutcome {
        selection: state.selection,
        first_frame_at: state.first_frame_at,
    })
}

/// Loads the GPU driver and D2D device code once, so the first real snip is as fast as
/// the rest. The window never shows.
pub fn warm_up(graphics: &Graphics) {
    register_class();
    let Ok(instance) = (unsafe { GetModuleHandleW(None) }) else {
        return;
    };
    let created = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("oh-snap.overlay"),
            w!("oh-snap warm-up"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance.into()),
            None,
        )
    };
    let Ok(hwnd) = created else {
        return;
    };
    let one_pixel = PixelRect {
        left: 0,
        top: 0,
        width: 1,
        height: 1,
    };
    if let Ok(target) = create_target(graphics, hwnd, one_pixel) {
        unsafe {
            target.BeginDraw();
            target.Clear(Some(&style::INK));
            let _ = target.EndDraw(None, None);
        }
    }
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

/// A frozen moment of the overlay for checking the look without a real screen.
pub struct PreviewScene {
    pub mode: Mode,
    pub cursor: (i32, i32),
    pub drag_from: Option<(i32, i32)>,
    pub window_rects: Vec<PixelRect>,
}

/// Draws the overlay into memory and returns BGRA pixels the size of the screenshot.
/// COM must be initialised on this thread.
pub fn render_offscreen(graphics: &Graphics, screenshot: &Screenshot, scene: PreviewScene) -> Result<Vec<u8>> {
    let width = screenshot.bounds.width as u32;
    let height = screenshot.bounds.height as u32;
    let imaging: IWICImagingFactory = unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? };
    let bitmap =
        unsafe { imaging.CreateBitmap(width, height, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad)? };
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
    let mut state = OverlayState::new(graphics, screenshot, target, scene.window_rects, scene.mode, scene.cursor)?;
    state.press_point = scene.drag_from;
    state.dragging = scene.drag_from.is_some();
    state.paint();

    let stride = width * 4;
    let mut pixels = vec![0u8; (stride * height) as usize];
    unsafe { bitmap.CopyPixels(std::ptr::null(), stride, &mut pixels)? };
    Ok(pixels)
}

fn register_class() {
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(overlay_proc),
        hInstance: unsafe { GetModuleHandleW(None) }.unwrap_or_default().into(),
        lpszClassName: w!("oh-snap.overlay"),
        ..Default::default()
    };
    // Registering twice fails harmlessly; the first registration stays.
    unsafe { RegisterClassExW(&class) };
}

fn create_target(graphics: &Graphics, hwnd: HWND, bounds: PixelRect) -> Result<ID2D1HwndRenderTarget> {
    let properties = D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        // 96 DPI makes one D2D unit one physical pixel, matching the capture.
        dpiX: 96.0,
        dpiY: 96.0,
        usage: D2D1_RENDER_TARGET_USAGE_NONE,
        minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
    };
    let hwnd_properties = D2D1_HWND_RENDER_TARGET_PROPERTIES {
        hwnd,
        pixelSize: D2D_SIZE_U {
            width: bounds.width as u32,
            height: bounds.height as u32,
        },
        presentOptions: D2D1_PRESENT_OPTIONS_IMMEDIATELY,
    };
    unsafe { graphics.d2d.CreateHwndRenderTarget(&properties, &hwnd_properties) }
}

fn create_screen_bitmap(target: &ID2D1RenderTarget, screenshot: &Screenshot) -> Result<ID2D1Bitmap> {
    let properties = D2D1_BITMAP_PROPERTIES {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_IGNORE,
        },
        dpiX: 96.0,
        dpiY: 96.0,
    };
    let size = D2D_SIZE_U {
        width: screenshot.bounds.width as u32,
        height: screenshot.bounds.height as u32,
    };
    unsafe {
        target.CreateBitmap(
            size,
            Some(screenshot.pixels.as_ptr() as *const _),
            screenshot.stride() as u32,
            &properties,
        )
    }
}

fn create_brushes(target: &ID2D1RenderTarget) -> Result<Brushes> {
    let brush = |color: D2D1_COLOR_F| unsafe { target.CreateSolidColorBrush(&color, None) };
    Ok(Brushes {
        ink_panel: brush(style::INK_PANEL)?,
        text: brush(style::TEXT)?,
        text_dim: brush(style::TEXT_DIM)?,
        rule: brush(style::RULE)?,
        hover: brush(style::HOVER)?,
        accent: brush(style::ACCENT)?,
        accent_soft: brush(style::ACCENT_SOFT)?,
        shade: brush(style::SHADE)?,
        crosshair: brush(style::CROSSHAIR)?,
    })
}

fn create_dash(graphics: &Graphics) -> Result<ID2D1StrokeStyle> {
    let properties = D2D1_STROKE_STYLE_PROPERTIES {
        startCap: D2D1_CAP_STYLE_FLAT,
        endCap: D2D1_CAP_STYLE_FLAT,
        dashCap: D2D1_CAP_STYLE_FLAT,
        lineJoin: D2D1_LINE_JOIN_MITER,
        miterLimit: 10.0,
        dashStyle: D2D1_DASH_STYLE_DASH,
        dashOffset: 0.0,
    };
    unsafe { graphics.d2d.CreateStrokeStyle(&properties, None) }
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let state_pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut OverlayState;
    if state_pointer.is_null() {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
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
        WM_ERASEBKGND => LRESULT(1),
        WM_SETCURSOR => {
            let cursor_id = if state.toolbar_item_at(state.cursor).is_some() { IDC_ARROW } else { IDC_CROSS };
            if let Ok(cursor) = unsafe { LoadCursorW(None, cursor_id) } {
                unsafe { SetCursor(Some(cursor)) };
            }
            LRESULT(1)
        }
        WM_MOUSEMOVE => {
            state.cursor = state.to_screen(lparam);
            if let Some(press_point) = state.press_point {
                let moved = (state.cursor.0 - press_point.0).abs() + (state.cursor.1 - press_point.1).abs();
                if state.mode == Mode::Region && moved > 3 {
                    state.dragging = true;
                }
            }
            state.paint();
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            state.cursor = state.to_screen(lparam);
            if let Some(mode) = state.toolbar_item_at(state.cursor) {
                state.mode = mode;
                state.paint();
                return LRESULT(0);
            }
            state.press_point = Some(state.cursor);
            unsafe { SetCapture(hwnd) };
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            state.cursor = state.to_screen(lparam);
            if state.press_point.is_some() {
                unsafe {
                    let _ = ReleaseCapture();
                }
                state.finish_press();
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            state.done = true;
            LRESULT(0)
        }
        WM_KEYDOWN => {
            state.on_key(wparam.0 as u16);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

impl OverlayState {
    fn new(
        graphics: &Graphics,
        screenshot: &Screenshot,
        target: ID2D1RenderTarget,
        window_rects: Vec<PixelRect>,
        mode: Mode,
        cursor: (i32, i32),
    ) -> Result<Self> {
        let bitmap = create_screen_bitmap(&target, screenshot)?;
        let brushes = create_brushes(&target)?;
        let dash = create_dash(graphics)?;
        Ok(Self {
            graphics,
            screenshot,
            window_rects,
            mode,
            cursor,
            press_point: None,
            dragging: false,
            toolbar_hits: Vec::new(),
            target,
            bitmap,
            brushes,
            dash,
            selection: None,
            done: false,
            close_after_first_frame: false,
            first_frame_at: None,
        })
    }

    fn graphics(&self) -> &Graphics {
        unsafe { &*self.graphics }
    }

    fn screenshot(&self) -> &Screenshot {
        unsafe { &*self.screenshot }
    }

    fn to_screen(&self, lparam: LPARAM) -> (i32, i32) {
        let client_x = (lparam.0 & 0xFFFF) as u16 as i16 as i32;
        let client_y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
        let bounds = self.screenshot().bounds;
        (client_x + bounds.left, client_y + bounds.top)
    }

    fn to_client(&self, x: i32, y: i32) -> (f32, f32) {
        let bounds = self.screenshot().bounds;
        ((x - bounds.left) as f32, (y - bounds.top) as f32)
    }

    fn client_rect(&self, rect: PixelRect) -> D2D_RECT_F {
        let (left, top) = self.to_client(rect.left, rect.top);
        D2D_RECT_F {
            left,
            top,
            right: left + rect.width as f32,
            bottom: top + rect.height as f32,
        }
    }

    fn current_monitor(&self) -> PixelRect {
        let screenshot = self.screenshot();
        screenshot
            .monitor_at(self.cursor.0, self.cursor.1)
            .unwrap_or(screenshot.monitors[0])
    }

    /// What a click would pick right now, or the live drag rectangle.
    fn highlighted(&self) -> Option<PixelRect> {
        if self.dragging {
            let start = self.press_point?;
            let rect = PixelRect::from_edges(start.0, start.1, self.cursor.0, self.cursor.1);
            return rect.intersect(&self.screenshot().bounds);
        }
        match self.mode {
            Mode::Screen => Some(self.current_monitor()),
            Mode::Region | Mode::Window => self
                .window_rects
                .iter()
                .find(|rect| rect.contains(self.cursor.0, self.cursor.1))
                .and_then(|rect| rect.intersect(&self.screenshot().bounds)),
        }
    }

    fn finish_press(&mut self) {
        let picked = self.highlighted();
        self.press_point = None;
        self.dragging = false;
        if let Some(rect) = picked {
            if rect.width >= 2 && rect.height >= 2 {
                self.selection = Some(rect);
                self.done = true;
                return;
            }
        }
        self.paint();
    }

    fn on_key(&mut self, key: u16) {
        let shift_state = unsafe { GetKeyState(VK_SHIFT.0 as i32) };
        let fast = shift_state < 0;
        let step = if fast { 10 } else { 1 };
        match key {
            key if key == VK_ESCAPE.0 => self.done = true,
            key if key == VK_RETURN.0 => {
                self.selection = self.highlighted().or(Some(self.current_monitor()));
                self.done = true;
            }
            key if key == u16::from(b'R') => self.set_mode(Mode::Region),
            key if key == u16::from(b'W') => self.set_mode(Mode::Window),
            key if key == u16::from(b'S') => self.set_mode(Mode::Screen),
            key if key == VK_LEFT.0 => self.nudge(-step, 0),
            key if key == VK_RIGHT.0 => self.nudge(step, 0),
            key if key == VK_UP.0 => self.nudge(0, -step),
            key if key == VK_DOWN.0 => self.nudge(0, step),
            _ => {}
        }
    }

    fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.paint();
    }

    fn nudge(&mut self, delta_x: i32, delta_y: i32) {
        unsafe {
            let _ = SetCursorPos(self.cursor.0 + delta_x, self.cursor.1 + delta_y);
        }
    }

    fn toolbar_item_at(&self, point: (i32, i32)) -> Option<Mode> {
        if self.dragging {
            return None;
        }
        let (x, y) = self.to_client(point.0, point.1);
        self.toolbar_hits
            .iter()
            .find(|(_, rect)| x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom)
            .map(|(mode, _)| *mode)
    }

    fn paint(&mut self) {
        let target = self.target.clone();
        unsafe {
            target.BeginDraw();
            let full = self.client_rect(self.screenshot().bounds);
            target.DrawBitmap(
                &self.bitmap,
                Some(&full),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                None,
            );
        }
        let highlighted = self.highlighted();
        self.draw_shade(highlighted);
        if let Some(rect) = highlighted {
            self.draw_frame(rect);
            self.draw_size_label(rect);
        }
        if self.mode == Mode::Region && !self.dragging {
            self.draw_crosshair();
        }
        if self.dragging {
            self.toolbar_hits.clear();
        } else {
            self.draw_toolbar();
        }
        self.draw_loupe();
        unsafe {
            let _ = target.EndDraw(None, None);
        }
    }

    fn draw_shade(&self, hole: Option<PixelRect>) {
        let full = self.client_rect(self.screenshot().bounds);
        let shade = &self.brushes.shade;
        let Some(hole) = hole else {
            unsafe { self.target.FillRectangle(&full, shade) };
            return;
        };
        let hole = self.client_rect(hole);
        let bands = [
            D2D_RECT_F { left: full.left, top: full.top, right: full.right, bottom: hole.top },
            D2D_RECT_F { left: full.left, top: hole.bottom, right: full.right, bottom: full.bottom },
            D2D_RECT_F { left: full.left, top: hole.top, right: hole.left, bottom: hole.bottom },
            D2D_RECT_F { left: hole.right, top: hole.top, right: full.right, bottom: hole.bottom },
        ];
        for band in bands {
            if band.right > band.left && band.bottom > band.top {
                unsafe { self.target.FillRectangle(&band, shade) };
            }
        }
    }

    /// A thin outline with heavier corner brackets, like ┌ ┐ └ ┘ around the pick.
    fn draw_frame(&self, rect: PixelRect) {
        let frame = self.client_rect(rect);
        let outline = D2D_RECT_F {
            left: frame.left + 0.5,
            top: frame.top + 0.5,
            right: frame.right - 0.5,
            bottom: frame.bottom - 0.5,
        };
        unsafe { self.target.DrawRectangle(&outline, &self.brushes.text_dim, 1.0, None) };

        let arm = 14.0_f32.min((frame.right - frame.left) / 2.0).min((frame.bottom - frame.top) / 2.0);
        let corners = [
            (frame.left, frame.top, 1.0, 1.0),
            (frame.right, frame.top, -1.0, 1.0),
            (frame.left, frame.bottom, 1.0, -1.0),
            (frame.right, frame.bottom, -1.0, -1.0),
        ];
        for (corner_x, corner_y, direction_x, direction_y) in corners {
            let inset_x = corner_x + direction_x;
            let inset_y = corner_y + direction_y;
            self.line(inset_x, inset_y, inset_x + direction_x * arm, inset_y, &self.brushes.accent, 2.0);
            self.line(inset_x, inset_y, inset_x, inset_y + direction_y * arm, &self.brushes.accent, 2.0);
        }
    }

    fn draw_size_label(&self, rect: PixelRect) {
        let text = format!("[ {} × {} ]", rect.width, rect.height);
        let (text_width, text_height) = self.graphics().measure(&text);
        let monitor = self.current_monitor();
        let frame = self.client_rect(rect);
        let monitor_frame = self.client_rect(monitor);
        let chip_width = text_width + 16.0;
        let chip_height = text_height + 8.0;
        let mut chip_left = frame.left;
        let mut chip_top = frame.bottom + 8.0;
        if chip_top + chip_height > monitor_frame.bottom {
            chip_top = frame.top + 8.0;
            chip_left = frame.left + 8.0;
        }
        chip_left = chip_left.clamp(monitor_frame.left, (monitor_frame.right - chip_width).max(monitor_frame.left));
        let chip = D2D_RECT_F {
            left: chip_left,
            top: chip_top,
            right: chip_left + chip_width,
            bottom: chip_top + chip_height,
        };
        self.rounded(chip, style::BUTTON_RADIUS, &self.brushes.ink_panel, Some(&self.brushes.rule));
        self.text(&text, chip.left + 8.0, chip.top + 4.0, &self.brushes.text);
    }

    fn draw_crosshair(&self) {
        let monitor = self.client_rect(self.current_monitor());
        let (x, y) = self.to_client(self.cursor.0, self.cursor.1);
        let x = x + 0.5;
        let y = y + 0.5;
        unsafe {
            self.target.DrawLine(
                Vector2 { X: monitor.left, Y: y },
                Vector2 { X: monitor.right, Y: y },
                &self.brushes.crosshair,
                1.0,
                &self.dash,
            );
            self.target.DrawLine(
                Vector2 { X: x, Y: monitor.top },
                Vector2 { X: x, Y: monitor.bottom },
                &self.brushes.crosshair,
                1.0,
                &self.dash,
            );
        }
    }

    /// `✻  [r] region  [w] window  [s] screen`, top center of the monitor under the cursor.
    fn draw_toolbar(&mut self) {
        let monitor = self.client_rect(self.current_monitor());
        let padding_x = 10.0;
        let padding_y = 6.0;
        let gap = 2.0;
        let inset = 4.0;
        let labels: Vec<(Mode, String, String)> = TOOLBAR_ITEMS
            .iter()
            .map(|(mode, key, name)| (*mode, format!("[{key}]"), format!(" {name}")))
            .collect();
        let sizes: Vec<(f32, f32, f32)> = labels
            .iter()
            .map(|(_, key, name)| {
                let (key_width, height) = self.graphics().measure(key);
                let (name_width, _) = self.graphics().measure(name);
                (key_width, name_width, height)
            })
            .collect();
        let spark_slot = 34.0;
        let button_height = sizes.iter().map(|size| size.2).fold(0.0, f32::max) + padding_y * 2.0;
        let buttons_width: f32 = sizes.iter().map(|size| size.0 + size.1 + padding_x * 2.0).sum::<f32>()
            + gap * (labels.len() as f32 - 1.0);
        let total_width = spark_slot + buttons_width + inset * 2.0;
        let bar_left = ((monitor.left + monitor.right - total_width) / 2.0).round();
        let bar_top = monitor.top + 16.0;
        let bar = D2D_RECT_F {
            left: bar_left,
            top: bar_top,
            right: bar_left + total_width,
            bottom: bar_top + button_height + inset * 2.0,
        };
        self.rounded(bar, style::PANEL_RADIUS, &self.brushes.ink_panel, Some(&self.brushes.rule));
        let spark_area = D2D_RECT_F {
            left: bar.left + inset,
            top: bar.top,
            right: bar.left + inset + spark_slot,
            bottom: bar.bottom - 1.0,
        };
        self.graphics().draw_spark(&self.target, spark_area, &self.brushes.accent);

        let hovered = self.toolbar_item_at(self.cursor);
        let mut hits = Vec::new();
        let mut button_left = bar.left + inset + spark_slot;
        for ((mode, key, name), (key_width, name_width, _)) in labels.iter().zip(sizes.iter()) {
            let button = D2D_RECT_F {
                left: button_left,
                top: bar.top + inset,
                right: button_left + key_width + name_width + padding_x * 2.0,
                bottom: bar.top + inset + button_height,
            };
            let active = *mode == self.mode;
            if active {
                self.rounded(button, style::BUTTON_RADIUS, &self.brushes.accent_soft, None);
            } else if hovered == Some(*mode) {
                self.rounded(button, style::BUTTON_RADIUS, &self.brushes.hover, None);
            }
            let (key_brush, name_brush) = if active {
                (&self.brushes.accent, &self.brushes.accent)
            } else {
                (&self.brushes.text_dim, &self.brushes.text)
            };
            self.text(key, button.left + padding_x, button.top + padding_y, key_brush);
            self.text(name, button.left + padding_x + key_width, button.top + padding_y, name_brush);
            hits.push((*mode, button));
            button_left = button.right + gap;
        }
        self.toolbar_hits = hits;
    }

    /// A pixel-grid magnifier with a colour swatch, the hex value and the position.
    fn draw_loupe(&self) {
        let cells = style::LOUPE_CELLS;
        let padding = 6.0;
        let color = self.screenshot().pixel_at(self.cursor.0, self.cursor.1).unwrap_or([0, 0, 0]);
        let info = format!("#{:02X}{:02X}{:02X}  {},{}", color[0], color[1], color[2], self.cursor.0, self.cursor.1);
        let (info_width, info_height) = self.graphics().measure(&info);
        let swatch_size = info_height - 4.0;
        // The zoom grows with the caption so the two always share one width.
        let caption_width = swatch_size + 6.0 + info_width;
        let cell_size = style::LOUPE_CELL_SIZE.max((caption_width / cells as f32).ceil());
        let zoom_size = cells as f32 * cell_size;
        let panel_width = zoom_size + padding * 2.0;
        let panel_height = zoom_size + info_height + padding * 3.0;

        let monitor = self.client_rect(self.current_monitor());
        let (cursor_x, cursor_y) = self.to_client(self.cursor.0, self.cursor.1);
        let mut left = cursor_x + 24.0;
        let mut top = cursor_y + 24.0;
        if left + panel_width > monitor.right {
            left = cursor_x - 24.0 - panel_width;
        }
        if top + panel_height > monitor.bottom {
            top = cursor_y - 24.0 - panel_height;
        }
        let panel = D2D_RECT_F {
            left,
            top,
            right: left + panel_width,
            bottom: top + panel_height,
        };
        self.rounded(panel, style::PANEL_RADIUS, &self.brushes.ink_panel, Some(&self.brushes.rule));

        let half = cells / 2;
        let source_left = cursor_x - half as f32;
        let source_top = cursor_y - half as f32;
        let source = D2D_RECT_F {
            left: source_left,
            top: source_top,
            right: source_left + cells as f32,
            bottom: source_top + cells as f32,
        };
        let zoom = D2D_RECT_F {
            left: panel.left + padding,
            top: panel.top + padding,
            right: panel.left + padding + zoom_size,
            bottom: panel.top + padding + zoom_size,
        };
        unsafe {
            self.target.DrawBitmap(
                &self.bitmap,
                Some(&zoom),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                Some(&source),
            );
        }
        let center = D2D_RECT_F {
            left: zoom.left + half as f32 * cell_size + 0.5,
            top: zoom.top + half as f32 * cell_size + 0.5,
            right: zoom.left + (half + 1) as f32 * cell_size - 0.5,
            bottom: zoom.top + (half + 1) as f32 * cell_size - 0.5,
        };
        unsafe { self.target.DrawRectangle(&center, &self.brushes.accent, 1.0, None) };

        let caption_top = zoom.bottom + padding;
        let swatch = D2D_RECT_F {
            left: zoom.left,
            top: caption_top + 2.0,
            right: zoom.left + swatch_size,
            bottom: caption_top + 2.0 + swatch_size,
        };
        let swatch_color = style::rgb(color[0], color[1], color[2], 1.0);
        if let Ok(swatch_brush) = unsafe { self.target.CreateSolidColorBrush(&swatch_color, None) } {
            self.rounded(swatch, 2.0, &swatch_brush, Some(&self.brushes.rule));
        }
        self.text(&info, swatch.right + 6.0, caption_top, &self.brushes.text);
    }

    fn rounded(&self, rect: D2D_RECT_F, radius: f32, fill: &ID2D1SolidColorBrush, border: Option<&ID2D1SolidColorBrush>) {
        let shape = D2D1_ROUNDED_RECT {
            rect,
            radiusX: radius,
            radiusY: radius,
        };
        unsafe { self.target.FillRoundedRectangle(&shape, fill) };
        let Some(border) = border else {
            return;
        };
        // Half-pixel inset keeps a 1 px border crisp.
        let outline = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: rect.left + 0.5,
                top: rect.top + 0.5,
                right: rect.right - 0.5,
                bottom: rect.bottom - 0.5,
            },
            radiusX: radius,
            radiusY: radius,
        };
        unsafe { self.target.DrawRoundedRectangle(&outline, border, 1.0, None) };
    }

    fn line(&self, from_x: f32, from_y: f32, to_x: f32, to_y: f32, brush: &ID2D1SolidColorBrush, width: f32) {
        unsafe {
            self.target.DrawLine(
                Vector2 { X: from_x, Y: from_y },
                Vector2 { X: to_x, Y: to_y },
                brush,
                width,
                None,
            );
        }
    }

    fn text(&self, text: &str, left: f32, top: f32, brush: &ID2D1SolidColorBrush) {
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let layout_rect = D2D_RECT_F {
            left,
            top,
            right: left + 2000.0,
            bottom: top + 200.0,
        };
        unsafe {
            self.target.DrawText(
                &utf16,
                &self.graphics().label_format,
                &layout_rect,
                brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }
}

/// Top-level windows a click can pick, topmost first, in DWM's visible bounds (no shadow).
fn visible_window_rects(bounds: PixelRect) -> Vec<PixelRect> {
    let mut rects: Vec<PixelRect> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect_window), LPARAM(&mut rects as *mut Vec<PixelRect> as isize));
    }
    rects.retain(|rect| rect.intersect(&bounds).is_some());
    rects
}

unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let rects = unsafe { &mut *(lparam.0 as *mut Vec<PixelRect>) };
    if let Some(rect) = pickable_window_rect(hwnd) {
        rects.push(rect);
    }
    TRUE
}

fn pickable_window_rect(hwnd: HWND) -> Option<PixelRect> {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return None;
        }
        let extended_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if extended_style & WS_EX_TRANSPARENT.0 != 0 {
            return None;
        }
        let mut process_id = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut process_id));
        if process_id == GetCurrentProcessId() {
            return None;
        }
        let mut cloaked = 0u32;
        let cloak_read = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut _,
            size_of::<u32>() as u32,
        );
        if cloak_read.is_ok() && cloaked != 0 {
            return None;
        }
        let mut frame = RECT::default();
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut frame as *mut RECT as *mut _,
            size_of::<RECT>() as u32,
        )
        .ok()?;
        let rect = PixelRect::from_edges(frame.left, frame.top, frame.right, frame.bottom);
        if rect.is_empty() {
            return None;
        }
        Some(rect)
    }
}
