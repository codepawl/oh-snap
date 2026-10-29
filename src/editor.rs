//! The editor window: draw on a snip, run add-ons on it, then copy or save it.

use std::path::PathBuf;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_ELLIPSE, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT, ID2D1HwndRenderTarget,
    ID2D1RenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::DWRITE_MEASURING_MODE_NATURAL;
use windows::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ValidateRect};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, VK_BACK, VK_CONTROL, VK_ESCAPE, VK_RETURN, VK_SHIFT,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetClientRect,
    GetCursorPos, GetWindowLongPtrW, ICON_BIG, ICON_SMALL, IDC_ARROW, IDC_CROSS, IDC_HAND, IDC_IBEAM, IDNO, IDYES,
    LoadCursorW, MB_ICONQUESTION, MB_YESNOCANCEL, MINMAXINFO, MessageBoxW, PostMessageW, PostQuitMessage,
    RegisterClassExW, SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetCursor, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, WM_APP, WM_CHAR, WM_CLOSE, WM_DPICHANGED,
    WM_GETMINMAXINFO, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCDESTROY, WM_PAINT, WM_SETCURSOR,
    WM_SETICON, WM_SIZE, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
};
use windows::core::{BOOL, HSTRING, Interface, Result, w};
use windows_numerics::{Matrix3x2, Vector2};

use crate::addons::{self, Addon, AddonOutput};
use crate::annotate::{self, Annotation, COLORS, Canvas, Point, TOOLS, Tool, WIDTH_STEPS};
use crate::output::{self, Image};
use crate::overlay::Graphics;
use crate::style;

const TOOLBAR_HEIGHT: f32 = 48.0;
const STATUS_HEIGHT: f32 = 30.0;
const CANVAS_PADDING: f32 = 24.0;
const MIN_WIDTH: f32 = 1200.0;
const ICON_SLOT: f32 = 20.0;
const MIN_HEIGHT: f32 = 560.0;
const MESSAGE_ADDON_DONE: u32 = WM_APP + 20;
const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWA_CAPTION_COLOR: u32 = 35;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Control {
    Tool(Tool),
    Color(usize),
    Width(usize),
    AddonsMenu,
    Copy,
    Save,
    RunAddon(usize),
    OpenAddonsFolder,
}

enum Status {
    Done(String),
    Failed(String),
    Working(String),
}

struct Drag {
    start: Point,
    points: Vec<Point>,
}

struct TextEntry {
    at: Point,
    text: String,
}

struct Editor {
    graphics: *const Graphics,
    hwnd: HWND,
    target: ID2D1RenderTarget,
    /// Set for a real window; the offscreen preview has none.
    window_target: Option<ID2D1HwndRenderTarget>,
    canvas: Canvas,
    image: Image,
    hidden_pixels: Vec<u8>,
    path: Option<PathBuf>,
    annotations: Vec<Annotation>,
    undone: Vec<Annotation>,
    dirty: bool,
    tool: Tool,
    color: usize,
    width_step: usize,
    unit: f32,
    drag: Option<Drag>,
    text_entry: Option<TextEntry>,
    pointer: (f32, f32),
    controls: Vec<(Control, D2D_RECT_F)>,
    addons: Vec<Addon>,
    menu_open: bool,
    status: Option<Status>,
    standalone: bool,
}

/// Opens an editor window. A standalone editor (from the CLI) ends the message loop
/// when it closes; inside the tray process it just goes away.
pub fn open(graphics: &Graphics, image: Image, path: Option<PathBuf>, standalone: bool) -> Result<()> {
    register_class();
    let instance = unsafe { GetModuleHandleW(None)? };

    let mut cursor = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut cursor);
    }
    let monitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) };
    let mut monitor_info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        let _ = GetMonitorInfoW(monitor, &mut monitor_info);
    }
    let work = monitor_info.rcWork;
    let work_width = work.right - work.left;
    let work_height = work.bottom - work.top;
    // Room for the image at 1:1 plus the bars, kept within 90% of the work area.
    let wanted_width = (image.width as i32 + 2 * CANVAS_PADDING as i32).max(MIN_WIDTH as i32);
    let wanted_height = (image.height as i32 + (TOOLBAR_HEIGHT + STATUS_HEIGHT + 2.0 * CANVAS_PADDING) as i32 + 40)
        .max(MIN_HEIGHT as i32);
    let width = wanted_width.min(work_width * 9 / 10);
    let height = wanted_height.min(work_height * 9 / 10);

    let hwnd = unsafe {
        CreateWindowExW(
            Default::default(),
            w!("oh-snap.editor"),
            w!("oh-snap"),
            WS_OVERLAPPEDWINDOW,
            work.left + (work_width - width) / 2,
            work.top + (work_height - height) / 2,
            width,
            height,
            None,
            None,
            Some(instance.into()),
            None,
        )?
    };
    style_window_frame(hwnd, graphics);

    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32;
    let target = create_target(graphics, hwnd, dpi)?;
    let window_target = target;
    let target: ID2D1RenderTarget = window_target.cast()?;
    let hidden_pixels = annotate::pixelate(&image);
    let canvas = Canvas::new(graphics, &target, &image, &hidden_pixels)?;
    let unit = annotate::unit_for(&image);
    let editor = Box::new(Editor {
        graphics,
        hwnd,
        target,
        window_target: Some(window_target),
        canvas,
        image,
        hidden_pixels,
        path,
        annotations: Vec::new(),
        undone: Vec::new(),
        dirty: false,
        tool: Tool::Arrow,
        color: 0,
        width_step: 1,
        unit,
        drag: None,
        text_entry: None,
        pointer: (0.0, 0.0),
        controls: Vec::new(),
        addons: addons::discover(),
        menu_open: false,
        status: None,
        standalone,
    });
    let editor_pointer = Box::into_raw(editor);
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, editor_pointer as isize);
        (*editor_pointer).update_title();
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = SetForegroundWindow(hwnd);
    }
    Ok(())
}

fn register_class() {
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(editor_proc),
        hInstance: unsafe { GetModuleHandleW(None) }.unwrap_or_default().into(),
        lpszClassName: w!("oh-snap.editor"),
        ..Default::default()
    };
    unsafe { RegisterClassExW(&class) };
}

/// A dark title bar in the app's ink colour, and the logo as the window icon.
fn style_window_frame(hwnd: HWND, graphics: &Graphics) {
    let dark = BOOL(1);
    let caption: u32 = 0x001D_1E1F;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(DWMWA_USE_IMMERSIVE_DARK_MODE as i32),
            &dark as *const BOOL as *const _,
            size_of::<BOOL>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(DWMWA_CAPTION_COLOR as i32),
            &caption as *const u32 as *const _,
            size_of::<u32>() as u32,
        );
    }
    for (size, kind) in [(32, ICON_BIG), (16, ICON_SMALL)] {
        if let Some(icon) = logo_icon(graphics, size) {
            unsafe {
                SendMessageW(hwnd, WM_SETICON, Some(WPARAM(kind as usize)), Some(LPARAM(icon.0 as isize)));
            }
        }
    }
}

fn logo_icon(graphics: &Graphics, size: u32) -> Option<windows::Win32::UI::WindowsAndMessaging::HICON> {
    let color = crate::logo::render(graphics, size).ok()?;
    let mask = vec![0u8; (size * size / 8) as usize];
    let instance = unsafe { GetModuleHandleW(None) }.ok()?;
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::CreateIcon(
            Some(instance.into()),
            size as i32,
            size as i32,
            1,
            32,
            mask.as_ptr(),
            color.as_ptr(),
        )
    }
    .ok()
}

fn create_target(graphics: &Graphics, hwnd: HWND, dpi: f32) -> Result<ID2D1HwndRenderTarget> {
    let mut client = RECT::default();
    unsafe { GetClientRect(hwnd, &mut client)? };
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
            width: client.right.max(1) as u32,
            height: client.bottom.max(1) as u32,
        },
        presentOptions: D2D1_PRESENT_OPTIONS_NONE,
    };
    unsafe { graphics.d2d.CreateHwndRenderTarget(&properties, &hwnd_properties) }
}

unsafe extern "system" fn editor_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let editor_pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Editor;
    if editor_pointer.is_null() {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    if message == WM_NCDESTROY {
        let editor = unsafe { Box::from_raw(editor_pointer) };
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
        if editor.standalone {
            unsafe { PostQuitMessage(0) };
        }
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    let editor = unsafe { &mut *editor_pointer };
    match message {
        WM_PAINT => {
            editor.paint();
            unsafe {
                let _ = ValidateRect(Some(hwnd), None);
            }
            LRESULT(0)
        }
        WM_SIZE => {
            let width = (lparam.0 & 0xFFFF) as u32;
            let height = ((lparam.0 >> 16) & 0xFFFF) as u32;
            unsafe {
                if let Some(window_target) = &editor.window_target {
                    let _ = window_target.Resize(&D2D_SIZE_U { width, height });
                }
            }
            editor.paint();
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let dpi = (wparam.0 & 0xFFFF) as f32;
            unsafe { editor.target.SetDpi(dpi, dpi) };
            let suggested = unsafe { &*(lparam.0 as *const RECT) };
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let limits = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            let scale = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32 / 96.0;
            limits.ptMinTrackSize.x = (MIN_WIDTH * scale) as i32;
            limits.ptMinTrackSize.y = (MIN_HEIGHT * scale) as i32;
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let cursor_id = editor.cursor_shape();
            if let Ok(cursor) = unsafe { LoadCursorW(None, cursor_id) } {
                unsafe { SetCursor(Some(cursor)) };
                return LRESULT(1);
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_MOUSEMOVE => {
            editor.pointer = editor.to_dips(lparam);
            editor.on_pointer_move();
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            editor.pointer = editor.to_dips(lparam);
            editor.on_press();
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            editor.pointer = editor.to_dips(lparam);
            editor.on_release();
            LRESULT(0)
        }
        WM_KEYDOWN => {
            editor.on_key(wparam.0 as u16);
            LRESULT(0)
        }
        WM_CHAR => {
            editor.on_character(wparam.0 as u32);
            LRESULT(0)
        }
        MESSAGE_ADDON_DONE => {
            let result = unsafe { Box::from_raw(lparam.0 as *mut (String, std::result::Result<AddonOutput, String>)) };
            editor.on_addon_done(result.0, result.1);
            LRESULT(0)
        }
        WM_CLOSE => {
            if editor.confirm_close() {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

impl Editor {
    fn graphics(&self) -> &Graphics {
        unsafe { &*self.graphics }
    }

    fn target(&self) -> ID2D1RenderTarget {
        self.target.clone()
    }

    fn client_size(&self) -> (f32, f32) {
        let size = unsafe { self.target.GetSize() };
        (size.width, size.height)
    }

    fn to_dips(&self, lparam: LPARAM) -> (f32, f32) {
        let pixel_x = (lparam.0 & 0xFFFF) as u16 as i16 as f32;
        let pixel_y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
        let mut dpi_x = 96.0;
        let mut dpi_y = 96.0;
        unsafe { self.target.GetDpi(&mut dpi_x, &mut dpi_y) };
        (pixel_x * 96.0 / dpi_x, pixel_y * 96.0 / dpi_y)
    }

    /// Image pixels to DIPs: the kept area, centred and shown at 1:1 or smaller.
    fn image_transform(&self) -> Matrix3x2 {
        let view = annotate::view_rect(&self.image, &self.annotations);
        let (width, height) = self.client_size();
        let mut dpi_x = 96.0;
        let mut dpi_y = 96.0;
        unsafe { self.target.GetDpi(&mut dpi_x, &mut dpi_y) };
        let pixels_per_dip = dpi_x / 96.0;
        let available_width = width - 2.0 * CANVAS_PADDING;
        let available_height = height - TOOLBAR_HEIGHT - STATUS_HEIGHT - 2.0 * CANVAS_PADDING;
        let natural_width = view.width as f32 / pixels_per_dip;
        let natural_height = view.height as f32 / pixels_per_dip;
        let fit = 1.0_f32
            .min(available_width / natural_width.max(1.0))
            .min(available_height / natural_height.max(1.0))
            .max(0.01);
        let scale = fit / pixels_per_dip;
        let shown_width = view.width as f32 * scale;
        let shown_height = view.height as f32 * scale;
        let origin_x = ((width - shown_width) / 2.0).round();
        let origin_y = (TOOLBAR_HEIGHT + CANVAS_PADDING + (available_height - shown_height) / 2.0).round();
        Matrix3x2 {
            M11: scale,
            M12: 0.0,
            M21: 0.0,
            M22: scale,
            M31: origin_x - view.left as f32 * scale,
            M32: origin_y - view.top as f32 * scale,
        }
    }

    fn to_image(&self, point: (f32, f32)) -> Point {
        let transform = self.image_transform();
        ((point.0 - transform.M31) / transform.M11, (point.1 - transform.M32) / transform.M22)
    }

    fn zoom_percent(&self) -> u32 {
        let mut dpi_x = 96.0;
        let mut dpi_y = 96.0;
        unsafe { self.target.GetDpi(&mut dpi_x, &mut dpi_y) };
        (self.image_transform().M11 * dpi_x / 96.0 * 100.0).round() as u32
    }

    fn stroke_width(&self) -> f32 {
        self.unit * WIDTH_STEPS[self.width_step] * 2.0
    }

    fn control_at(&self, point: (f32, f32)) -> Option<Control> {
        // Menu items sit on top of everything else, so they are checked first.
        self.controls
            .iter()
            .rev()
            .find(|(_, rect)| point.0 >= rect.left && point.0 < rect.right && point.1 >= rect.top && point.1 < rect.bottom)
            .map(|(control, _)| *control)
    }

    fn over_canvas(&self, point: (f32, f32)) -> bool {
        let (_, height) = self.client_size();
        !self.menu_open && point.1 > TOOLBAR_HEIGHT && point.1 < height - STATUS_HEIGHT
    }

    fn cursor_shape(&self) -> windows::core::PCWSTR {
        if self.control_at(self.pointer).is_some() {
            return IDC_HAND;
        }
        if !self.over_canvas(self.pointer) {
            return IDC_ARROW;
        }
        if self.tool == Tool::Text { IDC_IBEAM } else { IDC_CROSS }
    }

    fn update_title(&self) {
        let name = self
            .path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "new snip".into());
        let marker = if self.dirty { "• " } else { "" };
        unsafe {
            let _ = SetWindowTextW(self.hwnd, &HSTRING::from(format!("{marker}{name} · oh-snap")));
        }
    }

    fn set_dirty(&mut self) {
        if !self.dirty {
            self.dirty = true;
            self.update_title();
        }
    }

    fn add(&mut self, annotation: Annotation) {
        self.annotations.push(annotation);
        self.undone.clear();
        self.set_dirty();
    }

    fn on_pointer_move(&mut self) {
        let image_point = self.to_image(self.pointer);
        if let Some(drag) = self.drag.as_mut() {
            drag.points.push(image_point);
        }
        self.paint();
    }

    fn on_press(&mut self) {
        if let Some(control) = self.control_at(self.pointer) {
            self.activate(control);
            return;
        }
        if self.menu_open {
            self.menu_open = false;
            self.paint();
            return;
        }
        if !self.over_canvas(self.pointer) {
            return;
        }
        self.commit_text();
        let image_point = self.to_image(self.pointer);
        if self.tool == Tool::Text {
            self.text_entry = Some(TextEntry {
                at: image_point,
                text: String::new(),
            });
            self.paint();
            return;
        }
        self.drag = Some(Drag {
            start: image_point,
            points: vec![image_point],
        });
        unsafe { SetCapture(self.hwnd) };
    }

    fn on_release(&mut self) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        unsafe {
            let _ = ReleaseCapture();
        }
        if let Some(annotation) = self.annotation_from(&drag) {
            self.add(annotation);
        }
        self.paint();
    }

    /// The annotation a finished drag makes, or None when the drag was too small.
    fn annotation_from(&self, drag: &Drag) -> Option<Annotation> {
        let end = *drag.points.last()?;
        let rect = annotate::normalized(drag.start, end);
        let big_enough = rect.right - rect.left >= 3.0 && rect.bottom - rect.top >= 3.0;
        let moved = ((end.0 - drag.start.0).powi(2) + (end.1 - drag.start.1).powi(2)).sqrt() >= 3.0;
        let width = self.stroke_width();
        match self.tool {
            Tool::Arrow if moved => Some(Annotation::Arrow { from: drag.start, to: end, color: self.color, width }),
            Tool::Box if big_enough => Some(Annotation::Box { rect, color: self.color, width }),
            Tool::Pen | Tool::Marker => Some(Annotation::Stroke {
                points: drag.points.clone(),
                color: self.color,
                width,
                marker: self.tool == Tool::Marker,
            }),
            Tool::Hide if big_enough => Some(Annotation::Hide { rect }),
            Tool::Crop if big_enough => Some(Annotation::Crop { rect }),
            _ => None,
        }
    }

    fn activate(&mut self, control: Control) {
        match control {
            Control::Tool(tool) => {
                self.commit_text();
                self.tool = tool;
            }
            Control::Color(index) => self.color = index,
            Control::Width(index) => self.width_step = index,
            Control::AddonsMenu => {
                self.addons = addons::discover();
                self.menu_open = !self.menu_open;
            }
            Control::Copy => self.copy(),
            Control::Save => self.save(),
            Control::RunAddon(index) => {
                self.menu_open = false;
                self.run_addon(index);
            }
            Control::OpenAddonsFolder => {
                self.menu_open = false;
                let folder = addons::addons_folder();
                let _ = std::fs::create_dir_all(&folder);
                unsafe {
                    ShellExecuteW(None, w!("open"), &HSTRING::from(folder.as_os_str()), None, None, SW_SHOWNORMAL);
                }
            }
        }
        self.paint();
    }

    fn on_key(&mut self, key: u16) {
        let control_down = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        let shift_down = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        if self.text_entry.is_some() {
            if key == VK_ESCAPE.0 {
                self.text_entry = None;
                self.paint();
            }
            // Other keys arrive as WM_CHAR while typing.
            return;
        }
        if control_down {
            match key {
                key if key == u16::from(b'Z') && shift_down => self.redo(),
                key if key == u16::from(b'Z') => self.undo(),
                key if key == u16::from(b'Y') => self.redo(),
                key if key == u16::from(b'C') => self.copy(),
                key if key == u16::from(b'S') => self.save(),
                _ => {}
            }
            self.paint();
            return;
        }
        if key == VK_ESCAPE.0 {
            if self.menu_open {
                self.menu_open = false;
                self.paint();
            } else {
                unsafe {
                    let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
            return;
        }
        if let Some((tool, _, _)) = TOOLS.iter().find(|(_, shortcut, _)| u16::from(*shortcut) == key) {
            self.tool = *tool;
        } else if (u16::from(b'1')..=u16::from(b'5')).contains(&key) {
            self.color = (key - u16::from(b'1')) as usize;
        }
        self.paint();
    }

    fn on_character(&mut self, character: u32) {
        let Some(entry) = self.text_entry.as_mut() else {
            return;
        };
        let shift_down = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        if character == VK_RETURN.0 as u32 {
            if shift_down {
                entry.text.push('\n');
            } else {
                self.commit_text();
            }
        } else if character == VK_BACK.0 as u32 {
            entry.text.pop();
        } else if let Some(typed) = char::from_u32(character) {
            if !typed.is_control() {
                entry.text.push(typed);
            }
        }
        self.paint();
    }

    fn commit_text(&mut self) {
        let Some(entry) = self.text_entry.take() else {
            return;
        };
        if entry.text.trim().is_empty() {
            return;
        }
        let size = annotate::text_size(self.stroke_width());
        self.add(Annotation::Text {
            at: entry.at,
            text: entry.text,
            color: self.color,
            size,
        });
    }

    fn undo(&mut self) {
        if let Some(annotation) = self.annotations.pop() {
            self.undone.push(annotation);
            self.set_dirty();
        }
    }

    fn redo(&mut self) {
        if let Some(annotation) = self.undone.pop() {
            self.annotations.push(annotation);
            self.set_dirty();
        }
    }

    fn flattened(&mut self) -> std::result::Result<Image, String> {
        self.commit_text();
        annotate::compose(self.graphics(), &self.image, &self.hidden_pixels, &self.annotations).map_err(|error| error.to_string())
    }

    fn copy(&mut self) {
        let outcome = self.flattened().and_then(|image| {
            let png_bytes = output::encode_png(&image).map_err(|error| error.to_string())?;
            output::copy_to_clipboard(Some(self.hwnd), &image, &png_bytes).map_err(|error| error.to_string())
        });
        self.status = Some(match outcome {
            Ok(()) => Status::Done("copied".into()),
            Err(error) => Status::Failed(format!("copy failed: {error}")),
        });
    }

    fn save(&mut self) {
        let outcome = self.flattened().and_then(|image| {
            let png_bytes = output::encode_png(&image).map_err(|error| error.to_string())?;
            match &self.path {
                Some(path) => output::write_png_to(&png_bytes, path).map(|()| path.clone()),
                None => output::save_png(&png_bytes, &output::screenshots_folder()),
            }
            .map_err(|error| error.to_string())
        });
        match outcome {
            Ok(path) => {
                let name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
                self.status = Some(Status::Done(format!("saved {name}")));
                self.path = Some(path);
                self.dirty = false;
                self.update_title();
            }
            Err(error) => self.status = Some(Status::Failed(format!("save failed: {error}"))),
        }
    }

    /// Asks before closing with unsaved edits. Returns true when the window may close.
    fn confirm_close(&mut self) -> bool {
        self.commit_text();
        if !self.dirty {
            return true;
        }
        let answer = unsafe {
            MessageBoxW(Some(self.hwnd), w!("Save your changes to this snip?"), w!("oh-snap"), MB_YESNOCANCEL | MB_ICONQUESTION)
        };
        if answer == IDYES {
            self.save();
            return !self.dirty;
        }
        answer == IDNO
    }

    fn run_addon(&mut self, index: usize) {
        let Some(addon) = self.addons.get(index).cloned() else {
            return;
        };
        let image = match self.flattened() {
            Ok(image) => image,
            Err(error) => {
                self.status = Some(Status::Failed(error));
                return;
            }
        };
        let input_path = std::env::temp_dir().join(format!("oh-snap-input-{}.png", std::process::id()));
        let written = output::encode_png(&image).and_then(|png_bytes| output::write_png_to(&png_bytes, &input_path));
        if let Err(error) = written {
            self.status = Some(Status::Failed(error.to_string()));
            return;
        }
        self.status = Some(Status::Working(format!("{}…", addon.name)));
        let window = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            let result = addons::run(&addon, &input_path);
            let _ = std::fs::remove_file(&input_path);
            let message = Box::new((addon.name.clone(), result));
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(window as *mut _)),
                    MESSAGE_ADDON_DONE,
                    WPARAM(0),
                    LPARAM(Box::into_raw(message) as isize),
                );
            }
        });
    }

    fn on_addon_done(&mut self, name: String, result: std::result::Result<AddonOutput, String>) {
        self.status = Some(match result {
            Ok(AddonOutput::Text(text)) => {
                if text.is_empty() {
                    Status::Failed(format!("{name}: no text found"))
                } else {
                    match output::copy_text_to_clipboard(Some(self.hwnd), &text) {
                        Ok(()) => Status::Done(format!("{name}: copied {} lines", text.lines().count())),
                        Err(error) => Status::Failed(format!("{name}: {error}")),
                    }
                }
            }
            Ok(AddonOutput::Image(path)) => match crate::image_file::load_png(&path) {
                Ok(image) => {
                    let _ = std::fs::remove_file(&path);
                    self.replace_image(image);
                    Status::Done(format!("{name}: image updated"))
                }
                Err(error) => Status::Failed(format!("{name}: {error}")),
            },
            Ok(AddonOutput::Url(url)) => {
                unsafe {
                    ShellExecuteW(None, w!("open"), &HSTRING::from(url.as_str()), None, None, SW_SHOWNORMAL);
                }
                Status::Done(format!("{name}: opened {url}"))
            }
            Ok(AddonOutput::Message(text)) => Status::Done(format!("{name}: {text}")),
            Ok(AddonOutput::Nothing) => Status::Done(format!("{name}: done")),
            Err(error) => Status::Failed(format!("{name}: {error}")),
        });
        self.paint();
    }

    /// An image add-on's result becomes the new snip; it already contains the edits.
    fn replace_image(&mut self, image: Image) {
        let hidden_pixels = annotate::pixelate(&image);
        let target = self.target();
        if let Ok(canvas) = Canvas::new(self.graphics(), &target, &image, &hidden_pixels) {
            self.canvas = canvas;
            self.unit = annotate::unit_for(&image);
            self.image = image;
            self.hidden_pixels = hidden_pixels;
            self.annotations.clear();
            self.undone.clear();
            self.dirty = true;
            self.update_title();
        }
    }

    fn paint(&mut self) {
        let target = self.target();
        let brushes = match Brushes::new(&target) {
            Ok(brushes) => brushes,
            Err(_) => return,
        };
        unsafe {
            target.BeginDraw();
            target.Clear(Some(&style::INK));
        }
        self.draw_canvas(&target, &brushes);
        let mut controls = Vec::new();
        self.draw_toolbar(&target, &brushes, &mut controls);
        self.draw_status(&target, &brushes);
        if self.menu_open {
            self.draw_addons_menu(&target, &brushes, &mut controls);
        }
        self.controls = controls;
        unsafe {
            let _ = target.EndDraw(None, None);
        }
    }

    fn draw_canvas(&self, target: &ID2D1RenderTarget, brushes: &Brushes) {
        let transform = self.image_transform();
        let view = annotate::view_rect(&self.image, &self.annotations);
        let kept = D2D_RECT_F {
            left: view.left as f32,
            top: view.top as f32,
            right: view.right() as f32,
            bottom: view.bottom() as f32,
        };
        unsafe {
            target.SetTransform(&transform);
            target.PushAxisAlignedClip(&kept, D2D1_ANTIALIAS_MODE_ALIASED);
        }
        annotate::draw_scene(self.graphics(), target, &self.canvas, (self.image.width, self.image.height), &self.annotations);

        if let Some(drag) = &self.drag {
            if let Some(preview) = self.annotation_from(drag) {
                annotate::draw_annotation(self.graphics(), target, &self.canvas, &preview);
            }
        }
        unsafe { target.PopAxisAlignedClip() };

        // The crop being dragged shows as an outline over the whole image.
        if let (Some(drag), Tool::Crop) = (&self.drag, self.tool) {
            if let Some(end) = drag.points.last() {
                let rect = annotate::normalized(drag.start, *end);
                unsafe { target.DrawRectangle(&rect, &brushes.accent, 2.0 / transform.M11, None) };
            }
        }
        if let Some(entry) = &self.text_entry {
            let size = annotate::text_size(self.stroke_width());
            let shown = format!("{}▏", entry.text);
            if let Ok(brush) = unsafe { target.CreateSolidColorBrush(&COLORS[self.color], None) } {
                annotate::draw_text(self.graphics(), target, &shown, entry.at, size, &brush);
            }
        }
        unsafe { target.SetTransform(&Matrix3x2::identity()) };
    }

    fn draw_toolbar(&self, target: &ID2D1RenderTarget, brushes: &Brushes, controls: &mut Vec<(Control, D2D_RECT_F)>) {
        let (width, _) = self.client_size();
        let bar = D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: width,
            bottom: TOOLBAR_HEIGHT,
        };
        unsafe { target.FillRectangle(&bar, &brushes.panel) };
        let hovered = self.control_at(self.pointer);
        let spark_area = D2D_RECT_F {
            left: 8.0,
            top: 0.0,
            right: 40.0,
            bottom: TOOLBAR_HEIGHT,
        };
        self.graphics().draw_spark(target, spark_area, &brushes.accent);

        let mut left = 44.0;
        for (tool, _, label) in TOOLS {
            let control = Control::Tool(tool);
            let rect = self.text_button(target, brushes, tool_icon(tool), label, left, self.tool == tool, hovered == Some(control));
            controls.push((control, rect));
            left = rect.right + 2.0;
        }

        left += 14.0;
        for (index, color) in COLORS.iter().enumerate() {
            let rect = D2D_RECT_F {
                left,
                top: 12.0,
                right: left + 24.0,
                bottom: 36.0,
            };
            let center = Vector2 { X: left + 12.0, Y: 24.0 };
            unsafe {
                if index == self.color {
                    let ring = D2D1_ELLIPSE { point: center, radiusX: 10.5, radiusY: 10.5 };
                    target.DrawEllipse(&ring, &brushes.text, 1.5, None);
                }
                if let Ok(fill) = target.CreateSolidColorBrush(color, None) {
                    let dot = D2D1_ELLIPSE { point: center, radiusX: 7.0, radiusY: 7.0 };
                    target.FillEllipse(&dot, &fill);
                    target.DrawEllipse(&dot, &brushes.rule, 1.0, None);
                }
            }
            controls.push((Control::Color(index), rect));
            left = rect.right + 2.0;
        }

        left += 12.0;
        for index in 0..WIDTH_STEPS.len() {
            let rect = D2D_RECT_F {
                left,
                top: 12.0,
                right: left + 24.0,
                bottom: 36.0,
            };
            let active = index == self.width_step;
            if active || hovered == Some(Control::Width(index)) {
                let fill = if active { &brushes.accent_soft } else { &brushes.hover };
                let shape = D2D1_ROUNDED_RECT { rect, radiusX: 4.0, radiusY: 4.0 };
                unsafe { target.FillRoundedRectangle(&shape, fill) };
            }
            let radius = 1.5 + index as f32 * 1.5;
            let dot = D2D1_ELLIPSE {
                point: Vector2 { X: left + 12.0, Y: 24.0 },
                radiusX: radius,
                radiusY: radius,
            };
            let dot_brush = if active { &brushes.accent } else { &brushes.text_dim };
            unsafe { target.FillEllipse(&dot, dot_brush) };
            controls.push((Control::Width(index), rect));
            left = rect.right + 2.0;
        }

        // Right side, laid out from the edge inwards.
        let mut right = width - 10.0;
        let actions = [
            (Control::Save, crate::icons::SAVE, "save"),
            (Control::Copy, crate::icons::COPY, "copy"),
            (Control::AddonsMenu, crate::icons::ADD_ONS, "add-ons"),
        ];
        for (control, icon, label) in actions {
            let (label_width, _) = self.graphics().measure(label);
            let button_left = right - label_width - 20.0 - ICON_SLOT;
            let active = control == Control::AddonsMenu && self.menu_open;
            let rect = self.text_button(target, brushes, icon, label, button_left, active, hovered == Some(control));
            controls.push((control, rect));
            right = button_left - 2.0;
        }
    }

    /// A quiet text button: accent text on a soft fill when active, a faint fill on hover.
    fn text_button(&self, target: &ID2D1RenderTarget, brushes: &Brushes, icon: char, label: &str, left: f32, active: bool, hovered: bool) -> D2D_RECT_F {
        let (label_width, label_height) = self.graphics().measure(label);
        let rect = D2D_RECT_F {
            left,
            top: 9.0,
            right: left + label_width + 20.0 + ICON_SLOT,
            bottom: TOOLBAR_HEIGHT - 9.0,
        };
        if active || hovered {
            let fill = if active { &brushes.accent_soft } else { &brushes.hover };
            let shape = D2D1_ROUNDED_RECT { rect, radiusX: 4.0, radiusY: 4.0 };
            unsafe { target.FillRoundedRectangle(&shape, fill) };
        }
        let brush = if active { &brushes.accent } else { &brushes.text };
        let top = (TOOLBAR_HEIGHT - label_height) / 2.0;
        let icon_area = D2D_RECT_F { left: left + 6.0, top: rect.top, right: left + 6.0 + ICON_SLOT - 4.0, bottom: rect.bottom };
        self.graphics().draw_icon(target, icon, icon_area, brush);
        self.label(target, label, left + 10.0 + ICON_SLOT, top, rect.right, brush);
        rect
    }

    fn draw_status(&self, target: &ID2D1RenderTarget, brushes: &Brushes) {
        let (width, height) = self.client_size();
        let top = height - STATUS_HEIGHT;
        let view = annotate::view_rect(&self.image, &self.annotations);
        let details = format!("{} × {}   {}%", view.width, view.height, self.zoom_percent());
        let (_, text_height) = self.graphics().measure(&details);
        let text_top = top + (STATUS_HEIGHT - text_height) / 2.0;
        self.label(target, &details, 14.0, text_top, width / 2.0, &brushes.text_dim);

        let Some(status) = &self.status else {
            return;
        };
        let (text, mark_brush, mark) = match status {
            Status::Done(text) => (text, &brushes.accent, Mark::Check),
            Status::Failed(text) => (text, &brushes.failure, Mark::Cross),
            Status::Working(text) => (text, &brushes.text_dim, Mark::Dots),
        };
        let (text_width, _) = self.graphics().measure(text);
        let text_left = (width - 14.0 - text_width).max(width / 2.0);
        draw_mark(target, mark, text_left - 18.0, top + STATUS_HEIGHT / 2.0, mark_brush);
        self.label(target, text, text_left, text_top, width - 8.0, &brushes.text);
    }

    fn draw_addons_menu(&self, target: &ID2D1RenderTarget, brushes: &Brushes, controls: &mut Vec<(Control, D2D_RECT_F)>) {
        let Some((_, anchor)) = controls.iter().find(|(control, _)| *control == Control::AddonsMenu).copied() else {
            return;
        };
        let row_height = 44.0;
        let menu_width = 320.0;
        // Each add-on plus "open add-ons folder", or the single empty-state row.
        let rows = if self.addons.is_empty() { 1 } else { self.addons.len() + 1 };
        let menu = D2D_RECT_F {
            left: (anchor.right - menu_width).max(8.0),
            top: anchor.bottom + 6.0,
            right: anchor.right,
            bottom: anchor.bottom + 6.0 + rows as f32 * row_height + 8.0,
        };
        let shape = D2D1_ROUNDED_RECT { rect: menu, radiusX: 6.0, radiusY: 6.0 };
        unsafe {
            target.FillRoundedRectangle(&shape, &brushes.panel);
            target.DrawRoundedRectangle(&shape, &brushes.rule, 1.0, None);
        }
        let hovered = self.control_at(self.pointer);
        let mut top = menu.top + 4.0;
        let mut add_row = |control: Control, title: &str, detail: &str, controls: &mut Vec<(Control, D2D_RECT_F)>| {
            let row = D2D_RECT_F {
                left: menu.left + 4.0,
                top,
                right: menu.right - 4.0,
                bottom: top + row_height,
            };
            if hovered == Some(control) {
                let row_shape = D2D1_ROUNDED_RECT { rect: row, radiusX: 4.0, radiusY: 4.0 };
                unsafe { target.FillRoundedRectangle(&row_shape, &brushes.hover) };
            }
            self.label(target, title, row.left + 10.0, row.top + 5.0, row.right - 8.0, &brushes.text);
            self.label(target, detail, row.left + 10.0, row.top + 23.0, row.right - 8.0, &brushes.text_dim);
            controls.push((control, row));
            top += row_height;
        };
        if self.addons.is_empty() {
            add_row(Control::OpenAddonsFolder, "no add-ons yet", "drop one in the add-ons folder", controls);
        } else {
            for (index, addon) in self.addons.iter().enumerate() {
                add_row(Control::RunAddon(index), &addon.name, &addon.description, controls);
            }
            add_row(Control::OpenAddonsFolder, "open add-ons folder", "add your own: addon.toml + a command", controls);
        }
    }

    fn label(&self, target: &ID2D1RenderTarget, text: &str, left: f32, top: f32, right: f32, brush: &ID2D1SolidColorBrush) {
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let layout_rect = D2D_RECT_F {
            left,
            top,
            right: right.max(left + 1.0),
            bottom: top + 40.0,
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

struct Brushes {
    panel: ID2D1SolidColorBrush,
    rule: ID2D1SolidColorBrush,
    text: ID2D1SolidColorBrush,
    text_dim: ID2D1SolidColorBrush,
    accent: ID2D1SolidColorBrush,
    accent_soft: ID2D1SolidColorBrush,
    hover: ID2D1SolidColorBrush,
    failure: ID2D1SolidColorBrush,
}

impl Brushes {
    fn new(target: &ID2D1RenderTarget) -> Result<Self> {
        let brush = |color: D2D1_COLOR_F| unsafe { target.CreateSolidColorBrush(&color, None) };
        Ok(Self {
            panel: brush(style::rgb(0x26, 0x26, 0x24, 1.0))?,
            rule: brush(style::RULE)?,
            text: brush(style::TEXT)?,
            text_dim: brush(style::TEXT_DIM)?,
            accent: brush(style::ACCENT)?,
            accent_soft: brush(style::ACCENT_SOFT)?,
            hover: brush(style::HOVER)?,
            failure: brush(style::FAILURE)?,
        })
    }
}

enum Mark {
    Check,
    Cross,
    Dots,
}

/// Outcome marks drawn as strokes: a different shape for each outcome.
fn draw_mark(target: &ID2D1RenderTarget, mark: Mark, left: f32, middle: f32, brush: &ID2D1SolidColorBrush) {
    let segments: &[((f32, f32), (f32, f32))] = match mark {
        Mark::Check => &[((0.5, 0.0), (4.0, 3.5)), ((4.0, 3.5), (11.0, -4.5))],
        Mark::Cross => &[((1.0, -4.5), (10.0, 4.5)), ((10.0, -4.5), (1.0, 4.5))],
        Mark::Dots => &[((1.0, 0.0), (2.0, 0.0)), ((5.0, 0.0), (6.0, 0.0)), ((9.0, 0.0), (10.0, 0.0))],
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

/// `oh-snap edit FILE` without the tray process: one window, its own message loop.
pub fn run_standalone(path: PathBuf) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, MSG, TranslateMessage};
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }
    let image = crate::image_file::load_png(&path)?;
    let graphics = Graphics::new()?;
    open(&graphics, image, Some(path), true)?;
    let mut message = MSG::default();
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    Ok(())
}

/// A frozen editor for `oh-snap preview`: some annotations, a tool, maybe the add-ons menu.
pub struct EditorScene {
    pub annotations: Vec<Annotation>,
    pub tool: Tool,
    pub menu_open: bool,
    pub status_text: Option<String>,
    pub pointer: (f32, f32),
}

/// Draws an editor into memory at 1280 x 800 and returns its BGRA pixels.
/// COM must be initialised on this thread.
pub fn render_offscreen(graphics: &Graphics, image: Image, scene: EditorScene) -> Result<(Vec<u8>, u32, u32)> {
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapCacheOnLoad,
    };
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};

    let (width, height) = (1280u32, 800u32);
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
    let hidden_pixels = annotate::pixelate(&image);
    let canvas = Canvas::new(graphics, &target, &image, &hidden_pixels)?;
    let unit = annotate::unit_for(&image);
    let mut editor = Editor {
        graphics,
        hwnd: HWND(std::ptr::null_mut()),
        target,
        window_target: None,
        canvas,
        image,
        hidden_pixels,
        path: None,
        annotations: scene.annotations,
        undone: Vec::new(),
        dirty: true,
        tool: scene.tool,
        color: 0,
        width_step: 1,
        unit,
        drag: None,
        text_entry: None,
        pointer: scene.pointer,
        controls: Vec::new(),
        addons: addons::discover(),
        menu_open: false,
        status: scene.status_text.map(Status::Done),
        standalone: false,
    };
    // The first paint lays out the toolbar; the menu needs that layout to anchor on.
    editor.paint();
    if scene.menu_open {
        editor.menu_open = true;
        editor.paint();
    }
    let stride = width * 4;
    let mut pixels = vec![0u8; (stride * height) as usize];
    unsafe { bitmap.CopyPixels(std::ptr::null(), stride, &mut pixels)? };
    Ok((pixels, width, height))
}

fn tool_icon(tool: Tool) -> char {
    match tool {
        Tool::Arrow => crate::icons::ARROW,
        Tool::Box => crate::icons::BOX,
        Tool::Pen => crate::icons::PEN,
        Tool::Marker => crate::icons::MARK,
        Tool::Text => crate::icons::TEXT,
        Tool::Hide => crate::icons::HIDE,
        Tool::Crop => crate::icons::CROP,
    }
}
