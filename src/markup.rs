//! Drawing on a snip right on the overlay, before it is kept: the desktop stays frozen, the
//! picked area stays bright, and a bar next to it holds the tools. The annotations are the
//! editor's, so they look the same and flatten with the same code.

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_COLOR_F};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, D2D1_ELLIPSE, D2D1_ROUNDED_RECT,
    ID2D1Bitmap, ID2D1RenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::Gdi::ValidateRect;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_BACK, VK_CONTROL, VK_ESCAPE, VK_RETURN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetMessageW, GetWindowLongPtrW,
    IDC_ARROW, IDC_CROSS, IDC_HAND, IDC_IBEAM, LoadCursorW, MSG, RegisterClassExW, SW_SHOW, SetCursor,
    SetForegroundWindow, SetWindowLongPtrW, ShowWindow, TranslateMessage, WM_CHAR, WM_ERASEBKGND, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_SETCURSOR, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP,
};
use windows::core::{Interface, Result, w};
use windows_numerics::{Matrix3x2, Vector2};

use crate::annotate::{self, Annotation, COLORS, Canvas, Point, TOOLS, Tool};
use crate::capture::{PixelRect, Screenshot};
use crate::output::Image;
use crate::overlay::Graphics;
use crate::style;

/// Strokes use the middle width of the editor's three.
const STROKE_UNITS: f32 = 2.0;
const SMALLEST_SHAPE: f32 = 3.0;
const BAR_HEIGHT: f32 = 44.0;
const BAR_INSET: f32 = 6.0;
const BUTTON_PADDING: f32 = 9.0;
const ICON_SLOT: f32 = 22.0;
const SWATCH_SLOT: f32 = 24.0;
const GROUP_GAP: f32 = 12.0;

struct Drag {
    start: Point,
    points: Vec<Point>,
}

struct TextEntry {
    at: Point,
    text: String,
}

/// What the user has drawn so far and what the pointer is doing, in image pixels. It knows
/// nothing about windows, so it can be tested without a screen.
struct Session {
    size: (f32, f32),
    stroke_width: f32,
    tool: Tool,
    color: usize,
    annotations: Vec<Annotation>,
    undone: Vec<Annotation>,
    drag: Option<Drag>,
    entry: Option<TextEntry>,
}

impl Session {
    fn new(image: &Image) -> Self {
        Self {
            size: (image.width as f32, image.height as f32),
            stroke_width: annotate::unit_for(image) * STROKE_UNITS,
            tool: Tool::Arrow,
            color: 0,
            annotations: Vec::new(),
            undone: Vec::new(),
            drag: None,
            entry: None,
        }
    }

    fn clamped(&self, point: Point) -> Point {
        (point.0.clamp(0.0, self.size.0), point.1.clamp(0.0, self.size.1))
    }

    fn contains(&self, point: Point) -> bool {
        point.0 >= 0.0 && point.1 >= 0.0 && point.0 < self.size.0 && point.1 < self.size.1
    }

    fn add(&mut self, annotation: Annotation) {
        self.annotations.push(annotation);
        self.undone.clear();
    }

    /// A press inside the snip starts a shape, or a piece of text with the text tool.
    fn press(&mut self, point: Point) {
        if !self.contains(point) {
            return;
        }
        self.commit_text();
        if self.tool == Tool::Text {
            self.entry = Some(TextEntry { at: point, text: String::new() });
            return;
        }
        self.drag = Some(Drag { start: point, points: vec![point] });
    }

    fn pointer_moved(&mut self, point: Point) {
        let point = self.clamped(point);
        if let Some(drag) = self.drag.as_mut() {
            drag.points.push(point);
        }
    }

    fn release(&mut self) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if let Some(annotation) = self.annotation_from(&drag) {
            self.add(annotation);
        }
    }

    /// What a drag makes, or nothing when it was too small. The live drag draws through this too.
    fn annotation_from(&self, drag: &Drag) -> Option<Annotation> {
        let end = *drag.points.last()?;
        let rect = annotate::normalized(drag.start, end);
        let big_enough = rect.right - rect.left >= SMALLEST_SHAPE && rect.bottom - rect.top >= SMALLEST_SHAPE;
        let moved = (end.0 - drag.start.0).hypot(end.1 - drag.start.1) >= SMALLEST_SHAPE;
        let width = self.stroke_width;
        let color = self.color;
        match self.tool {
            Tool::Arrow if moved => Some(Annotation::Arrow { from: drag.start, to: end, color, width }),
            Tool::Box if big_enough => Some(Annotation::Box { rect, color, width }),
            Tool::Pen | Tool::Marker => Some(Annotation::Stroke {
                points: drag.points.clone(),
                color,
                width,
                marker: self.tool == Tool::Marker,
            }),
            Tool::Hide if big_enough => Some(Annotation::Hide { rect }),
            _ => None,
        }
    }

    fn live_annotation(&self) -> Option<Annotation> {
        self.annotation_from(self.drag.as_ref()?)
    }

    fn select_tool(&mut self, tool: Tool) {
        self.commit_text();
        self.tool = tool;
    }

    fn undo(&mut self) {
        if let Some(annotation) = self.annotations.pop() {
            self.undone.push(annotation);
        }
    }

    fn redo(&mut self) {
        if let Some(annotation) = self.undone.pop() {
            self.annotations.push(annotation);
        }
    }

    fn is_typing(&self) -> bool {
        self.entry.is_some()
    }

    fn cancel_typing(&mut self) {
        self.entry = None;
    }

    fn type_character(&mut self, character: u32, shift_down: bool) {
        let Some(entry) = self.entry.as_mut() else {
            return;
        };
        if character == u32::from(VK_RETURN.0) {
            if shift_down {
                entry.text.push('\n');
            } else {
                self.commit_text();
            }
        } else if character == u32::from(VK_BACK.0) {
            entry.text.pop();
        } else if let Some(typed) = char::from_u32(character).filter(|typed| !typed.is_control()) {
            entry.text.push(typed);
        }
    }

    fn commit_text(&mut self) {
        let Some(entry) = self.entry.take() else {
            return;
        };
        if entry.text.trim().is_empty() {
            return;
        }
        self.add(Annotation::Text {
            at: entry.at,
            text: entry.text,
            color: self.color,
            size: annotate::text_size(self.stroke_width),
        });
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Control {
    Tool(Tool),
    Color(usize),
    Undo,
    Done,
    Cancel,
}

/// How a markup ended.
enum Ending {
    Open,
    Done,
    Cancelled,
}

struct MarkupWindow {
    graphics: *const Graphics,
    screenshot: *const Screenshot,
    target: ID2D1RenderTarget,
    desktop: ID2D1Bitmap,
    canvas: Canvas,
    image_size: (u32, u32),
    area: PixelRect,
    session: Session,
    pointer: (f32, f32),
    controls: Vec<(Control, D2D_RECT_F)>,
    ending: Ending,
}

/// Lets the user draw on `area` of the frozen desktop. `held_window` is the overlay, kept on
/// screen until this window has drawn its first frame so the live desktop never flashes.
/// Returns the flattened snip, or nothing when the user cancelled.
pub fn run(graphics: &Graphics, screenshot: &Screenshot, area: PixelRect, held_window: HWND) -> Result<Option<Image>> {
    let mut held_window = HeldWindow(Some(held_window));
    let result = run_window(graphics, screenshot, area, &mut held_window);
    held_window.close();
    result
}

/// The overlay until it is closed. A closed window's handle can come back as another window,
/// so it must close exactly once.
struct HeldWindow(Option<HWND>);

impl HeldWindow {
    fn close(&mut self) {
        if let Some(hwnd) = self.0.take() {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
    }
}

fn run_window(graphics: &Graphics, screenshot: &Screenshot, area: PixelRect, held_window: &mut HeldWindow) -> Result<Option<Image>> {
    let Some(image) = crate::output::crop(screenshot, area) else {
        return Ok(None);
    };
    let area = area.intersect(&screenshot.bounds).unwrap_or(area);
    let hidden_pixels = annotate::pixelate(&image);
    let bounds = screenshot.bounds;
    let instance = unsafe { GetModuleHandleW(None)? };
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(markup_proc),
        hInstance: instance.into(),
        lpszClassName: w!("oh-snap.markup"),
        ..Default::default()
    };
    unsafe { RegisterClassExW(&class) };
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("oh-snap.markup"),
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
    let opened = open_state(graphics, screenshot, hwnd, &image, &hidden_pixels, area);
    let mut window = match opened {
        Ok(window) => window,
        Err(error) => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return Err(error);
        }
    };
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut window as *mut MarkupWindow as isize);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
    }
    window.paint();
    held_window.close();

    let mut message = MSG::default();
    while matches!(window.ending, Ending::Open) {
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
    if !matches!(window.ending, Ending::Done) {
        return Ok(None);
    }
    if window.session.annotations.is_empty() {
        return Ok(Some(image));
    }
    annotate::compose(graphics, &image, &hidden_pixels, &window.session.annotations).map(Some)
}

fn open_state(
    graphics: &Graphics,
    screenshot: &Screenshot,
    hwnd: HWND,
    image: &Image,
    hidden_pixels: &[u8],
    area: PixelRect,
) -> Result<MarkupWindow> {
    let target: ID2D1RenderTarget = crate::overlay::create_target(graphics, hwnd, screenshot.bounds)?.cast()?;
    let desktop = crate::overlay::create_screen_bitmap(&target, screenshot)?;
    let canvas = Canvas::new(graphics, &target, image, hidden_pixels)?;
    Ok(MarkupWindow {
        graphics,
        screenshot,
        target,
        desktop,
        canvas,
        image_size: (image.width, image.height),
        area,
        session: Session::new(image),
        pointer: (-1.0, -1.0),
        controls: Vec::new(),
        ending: Ending::Open,
    })
}

unsafe extern "system" fn markup_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let window_pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut MarkupWindow;
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
        WM_ERASEBKGND => LRESULT(1),
        WM_SETCURSOR => {
            let shape = window.cursor_shape();
            if let Ok(cursor) = unsafe { LoadCursorW(None, shape) } {
                unsafe { SetCursor(Some(cursor)) };
            }
            LRESULT(1)
        }
        WM_MOUSEMOVE => {
            window.pointer = client_point(lparam);
            let image_point = window.to_image(window.pointer);
            window.session.pointer_moved(image_point);
            window.paint();
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            window.pointer = client_point(lparam);
            window.on_press(hwnd);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if window.session.drag.is_some() {
                unsafe {
                    let _ = ReleaseCapture();
                }
                window.session.release();
                window.paint();
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            window.on_key(wparam.0 as u16);
            LRESULT(0)
        }
        WM_CHAR => {
            let shift_down = unsafe { GetKeyState(i32::from(VK_SHIFT.0)) } < 0;
            window.session.type_character(wparam.0 as u32, shift_down);
            window.paint();
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn client_point(lparam: LPARAM) -> (f32, f32) {
    let x = (lparam.0 & 0xFFFF) as u16 as i16 as f32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
    (x, y)
}

fn hit(rect: &D2D_RECT_F, point: (f32, f32)) -> bool {
    point.0 >= rect.left && point.0 < rect.right && point.1 >= rect.top && point.1 < rect.bottom
}

impl MarkupWindow {
    fn screenshot(&self) -> &Screenshot {
        unsafe { &*self.screenshot }
    }

    fn graphics(&self) -> &Graphics {
        unsafe { &*self.graphics }
    }

    fn to_image(&self, client: (f32, f32)) -> Point {
        let origin = area_origin(self.screenshot(), self.area);
        (client.0 - origin.0, client.1 - origin.1)
    }

    fn control_at(&self, point: (f32, f32)) -> Option<Control> {
        self.controls.iter().find(|(_, rect)| hit(rect, point)).map(|(control, _)| *control)
    }

    fn cursor_shape(&self) -> windows::core::PCWSTR {
        if self.control_at(self.pointer).is_some() {
            return IDC_HAND;
        }
        if !self.session.contains(self.to_image(self.pointer)) {
            return IDC_ARROW;
        }
        if self.session.tool == Tool::Text { IDC_IBEAM } else { IDC_CROSS }
    }

    fn on_press(&mut self, hwnd: HWND) {
        if let Some(control) = self.control_at(self.pointer) {
            self.activate(control);
            return;
        }
        let image_point = self.to_image(self.pointer);
        self.session.press(image_point);
        if self.session.drag.is_some() {
            unsafe { SetCapture(hwnd) };
        }
        self.paint();
    }

    fn activate(&mut self, control: Control) {
        match control {
            Control::Tool(tool) => self.session.select_tool(tool),
            Control::Color(index) => self.session.color = index,
            Control::Undo => {
                self.session.commit_text();
                self.session.undo();
            }
            Control::Done => {
                self.session.commit_text();
                self.ending = Ending::Done;
            }
            Control::Cancel => self.ending = Ending::Cancelled,
        }
        self.paint();
    }

    fn on_key(&mut self, key: u16) {
        if self.session.is_typing() {
            if key == VK_ESCAPE.0 {
                self.session.cancel_typing();
                self.paint();
            }
            // Other keys arrive as WM_CHAR while typing.
            return;
        }
        let control_down = unsafe { GetKeyState(i32::from(VK_CONTROL.0)) } < 0;
        let shift_down = unsafe { GetKeyState(i32::from(VK_SHIFT.0)) } < 0;
        if control_down {
            match key {
                key if key == u16::from(b'Z') && shift_down => self.session.redo(),
                key if key == u16::from(b'Z') => self.session.undo(),
                key if key == u16::from(b'Y') => self.session.redo(),
                _ => {}
            }
        } else if key == VK_ESCAPE.0 {
            self.ending = Ending::Cancelled;
        } else if key == VK_RETURN.0 {
            self.ending = Ending::Done;
        } else if let Some((tool, _, _)) = markup_tools().find(|(_, shortcut, _)| u16::from(*shortcut) == key) {
            self.session.tool = *tool;
        } else if (u16::from(b'1')..=u16::from(b'5')).contains(&key) {
            self.session.color = (key - u16::from(b'1')) as usize;
        }
        self.paint();
    }

    fn paint(&mut self) {
        let controls = draw(self.graphics(), &self.target, &self.desktop, &self.canvas, self.image_size, self.screenshot(), self.area, &self.session, self.pointer);
        self.controls = controls;
    }
}

/// The editor's tools, without crop: the area was just picked.
fn markup_tools() -> impl Iterator<Item = &'static (Tool, u8, &'static str)> {
    TOOLS.iter().filter(|(tool, _, _)| *tool != Tool::Crop)
}

fn area_origin(screenshot: &Screenshot, area: PixelRect) -> (f32, f32) {
    ((area.left - screenshot.bounds.left) as f32, (area.top - screenshot.bounds.top) as f32)
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

struct Brushes {
    text: ID2D1SolidColorBrush,
    dim: ID2D1SolidColorBrush,
    accent: ID2D1SolidColorBrush,
    accent_soft: ID2D1SolidColorBrush,
    hover: ID2D1SolidColorBrush,
    panel: ID2D1SolidColorBrush,
    rule: ID2D1SolidColorBrush,
    shade: ID2D1SolidColorBrush,
}

fn create_brushes(target: &ID2D1RenderTarget) -> Result<Brushes> {
    let brush = |color: D2D1_COLOR_F| unsafe { target.CreateSolidColorBrush(&color, None) };
    Ok(Brushes {
        text: brush(style::TEXT)?,
        dim: brush(style::TEXT_DIM)?,
        accent: brush(style::ACCENT)?,
        accent_soft: brush(style::ACCENT_SOFT)?,
        hover: brush(style::HOVER)?,
        panel: brush(style::INK_PANEL)?,
        rule: brush(style::RULE)?,
        shade: brush(style::SHADE)?,
    })
}

/// Draws one frame and returns where the bar's controls ended up.
#[allow(clippy::too_many_arguments)]
fn draw(
    graphics: &Graphics,
    target: &ID2D1RenderTarget,
    desktop: &ID2D1Bitmap,
    canvas: &Canvas,
    image_size: (u32, u32),
    screenshot: &Screenshot,
    area: PixelRect,
    session: &Session,
    pointer: (f32, f32),
) -> Vec<(Control, D2D_RECT_F)> {
    let Ok(brushes) = create_brushes(target) else {
        return Vec::new();
    };
    let identity = Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 };
    let origin = area_origin(screenshot, area);
    let width = screenshot.bounds.width as f32;
    let height = screenshot.bounds.height as f32;
    let hole = D2D_RECT_F {
        left: origin.0,
        top: origin.1,
        right: origin.0 + area.width as f32,
        bottom: origin.1 + area.height as f32,
    };
    let full = D2D_RECT_F { left: 0.0, top: 0.0, right: width, bottom: height };
    let shift = Matrix3x2 { M31: origin.0, M32: origin.1, ..identity };
    unsafe {
        target.BeginDraw();
        target.SetTransform(&identity);
        target.DrawBitmap(desktop, Some(&full), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None);
        let shades = [
            D2D_RECT_F { left: 0.0, top: 0.0, right: width, bottom: hole.top },
            D2D_RECT_F { left: 0.0, top: hole.bottom, right: width, bottom: height },
            D2D_RECT_F { left: 0.0, top: hole.top, right: hole.left, bottom: hole.bottom },
            D2D_RECT_F { left: hole.right, top: hole.top, right: width, bottom: hole.bottom },
        ];
        for shade in &shades {
            target.FillRectangle(shade, &brushes.shade);
        }
        target.PushAxisAlignedClip(&hole, D2D1_ANTIALIAS_MODE_ALIASED);
        target.SetTransform(&shift);
    }
    annotate::draw_scene(graphics, target, canvas, image_size, &session.annotations);
    if let Some(live) = session.live_annotation() {
        annotate::draw_annotation(graphics, target, canvas, &live);
    }
    if let Some(entry) = &session.entry {
        let size = annotate::text_size(session.stroke_width);
        let shown = format!("{}|", entry.text);
        if let Ok(brush) = unsafe { target.CreateSolidColorBrush(&COLORS[session.color], None) } {
            annotate::draw_text(graphics, target, &shown, entry.at, size, &brush);
        }
    }
    unsafe {
        target.SetTransform(&identity);
        target.PopAxisAlignedClip();
        let edge = D2D_RECT_F { left: hole.left - 0.5, top: hole.top - 0.5, right: hole.right + 0.5, bottom: hole.bottom + 0.5 };
        target.DrawRectangle(&edge, &brushes.accent, 1.0, None);
    }
    let controls = draw_bar(graphics, target, &brushes, screenshot, area, session, pointer);
    unsafe {
        let _ = target.EndDraw(None, None);
    }
    controls
}

fn draw_bar(
    graphics: &Graphics,
    target: &ID2D1RenderTarget,
    brushes: &Brushes,
    screenshot: &Screenshot,
    area: PixelRect,
    session: &Session,
    pointer: (f32, f32),
) -> Vec<(Control, D2D_RECT_F)> {
    const UNDO: &str = "undo";
    const DONE_KEY: &str = "[enter]";
    const DONE_NAME: &str = " done";
    const CANCEL_KEY: &str = "[esc]";
    const CANCEL_NAME: &str = " cancel";
    let button_width = |text: &str| graphics.measure(text).0 + BUTTON_PADDING * 2.0 + ICON_SLOT;
    let tools_width: f32 = markup_tools().map(|(_, _, label)| button_width(label)).sum();
    let colors_width = SWATCH_SLOT * COLORS.len() as f32;
    let done_width = button_width(&format!("{DONE_KEY}{DONE_NAME}"));
    let cancel_width = button_width(&format!("{CANCEL_KEY}{CANCEL_NAME}"));
    let bar_width = BAR_INSET * 2.0 + tools_width + GROUP_GAP + colors_width + GROUP_GAP + button_width(UNDO) + GROUP_GAP + done_width + cancel_width;

    let center_x = area.left + area.width / 2;
    let center_y = area.top + area.height / 2;
    let monitor = screenshot.monitor_at(center_x, center_y).unwrap_or(screenshot.bounds);
    let monitor_rect = RECT { left: monitor.left, top: monitor.top, right: monitor.right(), bottom: monitor.bottom() };
    let (bar_left, bar_top) = crate::recording_bar::bar_position(area, monitor_rect, bar_width.ceil() as i32, BAR_HEIGHT as i32);
    let left = (bar_left - screenshot.bounds.left) as f32;
    let top = (bar_top - screenshot.bounds.top) as f32;
    let bar = D2D_RECT_F { left, top, right: left + bar_width, bottom: top + BAR_HEIGHT };
    let shape = D2D1_ROUNDED_RECT { rect: bar, radiusX: style::PANEL_RADIUS, radiusY: style::PANEL_RADIUS };
    unsafe {
        target.FillRoundedRectangle(&shape, &brushes.panel);
        target.DrawRoundedRectangle(&shape, &brushes.rule, 1.0, None);
    }

    let button_top = top + BAR_INSET;
    let button_bottom = top + BAR_HEIGHT - BAR_INSET;
    let text_top = top + (BAR_HEIGHT - graphics.measure("[").1) / 2.0;
    let mut controls = Vec::new();
    let mut cursor = left + BAR_INSET;
    let mut draw_button = |control: Control, icon: char, key: &str, name: &str, active: bool, cursor: &mut f32| {
        let width = button_width(&format!("{key}{name}"));
        let rect = D2D_RECT_F { left: *cursor, top: button_top, right: *cursor + width, bottom: button_bottom };
        let shape = D2D1_ROUNDED_RECT { rect, radiusX: style::BUTTON_RADIUS, radiusY: style::BUTTON_RADIUS };
        if active {
            unsafe { target.FillRoundedRectangle(&shape, &brushes.accent_soft) };
        } else if hit(&rect, pointer) {
            unsafe { target.FillRoundedRectangle(&shape, &brushes.hover) };
        }
        let (key_brush, name_brush) = if active { (&brushes.accent, &brushes.accent) } else { (&brushes.dim, &brushes.text) };
        let icon_area = D2D_RECT_F { left: rect.left + BUTTON_PADDING - 3.0, top: rect.top, right: rect.left + BUTTON_PADDING + ICON_SLOT - 6.0, bottom: rect.bottom };
        graphics.draw_icon(target, icon, icon_area, name_brush);
        let text_left = rect.left + BUTTON_PADDING + ICON_SLOT;
        crate::overlay::draw_label(graphics, target, key, text_left, text_top, key_brush);
        crate::overlay::draw_label(graphics, target, name, text_left + graphics.measure(key).0, text_top, name_brush);
        controls.push((control, rect));
        *cursor = rect.right;
    };

    for (tool, _, label) in markup_tools() {
        draw_button(Control::Tool(*tool), tool_icon(*tool), "", label, session.tool == *tool, &mut cursor);
    }
    cursor += GROUP_GAP;
    let swatches_left = cursor;
    cursor += colors_width + GROUP_GAP;
    draw_button(Control::Undo, crate::icons::UNDO, "", UNDO, false, &mut cursor);
    cursor += GROUP_GAP;
    draw_button(Control::Done, crate::icons::SAVE, DONE_KEY, DONE_NAME, true, &mut cursor);
    draw_button(Control::Cancel, crate::icons::CANCEL, CANCEL_KEY, CANCEL_NAME, false, &mut cursor);

    let middle = top + BAR_HEIGHT / 2.0;
    for (index, color) in COLORS.iter().enumerate() {
        let center = swatches_left + SWATCH_SLOT * (index as f32 + 0.5);
        let dot = D2D1_ELLIPSE { point: Vector2 { X: center, Y: middle }, radiusX: 7.0, radiusY: 7.0 };
        if let Ok(fill) = unsafe { target.CreateSolidColorBrush(color, None) } {
            unsafe {
                target.FillEllipse(&dot, &fill);
                target.DrawEllipse(&dot, &brushes.rule, 1.0, None);
            }
        }
        if session.color == index {
            let ring = D2D1_ELLIPSE { radiusX: 10.0, radiusY: 10.0, ..dot };
            unsafe { target.DrawEllipse(&ring, &brushes.text, 1.5, None) };
        }
        let rect = D2D_RECT_F {
            left: center - SWATCH_SLOT / 2.0,
            top: button_top,
            right: center + SWATCH_SLOT / 2.0,
            bottom: button_bottom,
        };
        controls.push((Control::Color(index), rect));
    }
    controls
}

/// Draws a sample markup and its flattened result for `oh-snap preview`. Returns the frame
/// as BGRA pixels the size of the screenshot, and the flattened image.
pub fn render_offscreen(graphics: &Graphics, screenshot: &Screenshot, area: PixelRect) -> Result<(Vec<u8>, Image)> {
    use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT};
    use windows::Win32::Graphics::Direct2D::{
        D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE,
    };
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapCacheOnLoad,
    };
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    let image = crate::output::crop(screenshot, area).ok_or_else(windows::core::Error::empty)?;
    let hidden_pixels = annotate::pixelate(&image);
    let mut session = Session::new(&image);
    let (image_width, image_height) = session.size;
    session.color = 1;
    session.press((image_width * 0.15, image_height * 0.75));
    session.pointer_moved((image_width * 0.45, image_height * 0.4));
    session.release();
    session.tool = Tool::Box;
    session.press((image_width * 0.5, image_height * 0.15));
    session.pointer_moved((image_width * 0.85, image_height * 0.4));
    session.release();
    session.tool = Tool::Hide;
    session.press((image_width * 0.55, image_height * 0.55));
    session.pointer_moved((image_width * 0.9, image_height * 0.7));
    session.release();
    session.tool = Tool::Text;
    session.color = 2;
    session.press((image_width * 0.1, image_height * 0.1));
    for character in "look here".chars() {
        session.type_character(character as u32, false);
    }
    session.commit_text();
    session.tool = Tool::Marker;
    session.color = 0;

    let width = screenshot.bounds.width as u32;
    let height = screenshot.bounds.height as u32;
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
    let desktop = crate::overlay::create_screen_bitmap(&target, screenshot)?;
    let canvas = Canvas::new(graphics, &target, &image, &hidden_pixels)?;
    draw(graphics, &target, &desktop, &canvas, (image.width, image.height), screenshot, area, &session, (-1.0, -1.0));
    let stride = width * 4;
    let mut pixels = vec![0u8; (stride * height) as usize];
    unsafe { bitmap.CopyPixels(std::ptr::null(), stride, &mut pixels)? };
    let flattened = annotate::compose(graphics, &image, &hidden_pixels, &session.annotations)?;
    Ok((pixels, flattened))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        let image = Image {
            width: 600,
            height: 400,
            bgra: vec![255; 600 * 400 * 4],
        };
        Session::new(&image)
    }

    fn drag(session: &mut Session, from: Point, to: Point) {
        session.press(from);
        session.pointer_moved(to);
        session.release();
    }

    #[test]
    fn a_drag_makes_the_chosen_shape() {
        let mut markup = session();
        drag(&mut markup, (10.0, 10.0), (100.0, 80.0));
        markup.tool = Tool::Box;
        drag(&mut markup, (10.0, 10.0), (100.0, 80.0));
        markup.tool = Tool::Hide;
        drag(&mut markup, (10.0, 10.0), (100.0, 80.0));
        assert!(matches!(markup.annotations[0], Annotation::Arrow { .. }));
        assert!(matches!(markup.annotations[1], Annotation::Box { .. }));
        assert!(matches!(markup.annotations[2], Annotation::Hide { .. }));
    }

    #[test]
    fn a_tiny_drag_makes_nothing() {
        let mut markup = session();
        drag(&mut markup, (10.0, 10.0), (11.0, 10.0));
        assert!(markup.annotations.is_empty());
    }

    #[test]
    fn a_press_outside_the_snip_does_nothing() {
        let mut markup = session();
        markup.press((-5.0, 10.0));
        markup.press((700.0, 10.0));
        assert!(markup.drag.is_none());
    }

    #[test]
    fn a_drag_past_the_edge_stops_at_the_edge() {
        let mut markup = session();
        drag(&mut markup, (10.0, 10.0), (5000.0, -300.0));
        let Annotation::Arrow { to, .. } = markup.annotations[0] else {
            panic!("expected an arrow");
        };
        assert_eq!(to, (600.0, 0.0));
    }

    #[test]
    fn undo_and_redo_walk_the_history_and_a_new_shape_clears_redo() {
        let mut markup = session();
        drag(&mut markup, (10.0, 10.0), (100.0, 80.0));
        markup.undo();
        assert!(markup.annotations.is_empty());
        markup.redo();
        assert_eq!(markup.annotations.len(), 1);
        markup.undo();
        drag(&mut markup, (20.0, 20.0), (100.0, 80.0));
        markup.redo();
        assert_eq!(markup.annotations.len(), 1);
    }

    #[test]
    fn text_is_typed_edited_and_kept_on_enter() {
        let mut markup = session();
        markup.tool = Tool::Text;
        markup.press((50.0, 50.0));
        assert!(markup.is_typing());
        for character in "hix".chars() {
            markup.type_character(character as u32, false);
        }
        markup.type_character(u32::from(VK_BACK.0), false);
        markup.type_character(u32::from(VK_RETURN.0), true);
        markup.type_character('!' as u32, false);
        markup.type_character(u32::from(VK_RETURN.0), false);
        assert!(!markup.is_typing());
        let Annotation::Text { text, .. } = &markup.annotations[0] else {
            panic!("expected text");
        };
        assert_eq!(text, "hi\n!");
    }

    #[test]
    fn empty_text_leaves_nothing_and_escape_drops_it() {
        let mut markup = session();
        markup.tool = Tool::Text;
        markup.press((50.0, 50.0));
        markup.commit_text();
        assert!(markup.annotations.is_empty());
        markup.press((50.0, 50.0));
        markup.type_character('a' as u32, false);
        markup.cancel_typing();
        assert!(markup.annotations.is_empty());
    }

    #[test]
    fn changing_tool_keeps_the_text_being_typed() {
        let mut markup = session();
        markup.tool = Tool::Text;
        markup.press((50.0, 50.0));
        markup.type_character('a' as u32, false);
        markup.select_tool(Tool::Arrow);
        assert_eq!(markup.annotations.len(), 1);
    }

    #[test]
    fn crop_is_not_offered() {
        assert!(markup_tools().all(|(tool, _, _)| *tool != Tool::Crop));
        assert_eq!(markup_tools().count(), 6);
    }
}
