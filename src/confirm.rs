//! "Keep this snip?": the picked area at a glance, before anything is saved, with save,
//! edit, retake and a switch to stop asking for people who want speed.

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
    DWM_WINDOW_CORNER_PREFERENCE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ValidateRect};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_ESCAPE, VK_RETURN};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetMessageW, GetWindowLongPtrW,
    IDC_ARROW, IDC_HAND, LoadCursorW, MSG, RegisterClassExW, SW_SHOW, SetCursor, SetForegroundWindow,
    SetWindowLongPtrW, ShowWindow, TranslateMessage, WM_KEYDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT,
    WM_RBUTTONUP, WM_SETCURSOR, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{Interface, Result, w};

use std::path::{Path, PathBuf};

use crate::capture::PixelRect;
use crate::image_menu::{self, ImageAction};
use crate::output::Image;
use crate::overlay::Graphics;
use crate::style;

const WIDTH: f32 = 680.0;
const ICON_SLOT: f32 = 20.0;
const PADDING: f32 = 16.0;
const HEADER_HEIGHT: f32 = 40.0;
const BUTTON_HEIGHT: f32 = 32.0;
const PREVIEW_MAX_HEIGHT: f32 = 340.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Choice {
    Save,
    Edit,
    Retake,
    Cancel,
    /// Saved from the right-click menu; nothing more to do.
    Done,
}

pub struct Answer {
    pub choice: Choice,
    /// The user ticked "don't ask again".
    pub stop_asking: bool,
    /// Where "save as…" in the right-click menu wrote the snip.
    pub saved_as: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Control {
    Choose(Choice),
    StopAsking,
}

const BUTTONS: [(Choice, char, &str, &str); 4] = [
    (Choice::Save, crate::icons::SAVE, "[enter]", " save"),
    (Choice::Edit, crate::icons::EDIT, "[e]", " edit"),
    (Choice::Retake, crate::icons::RETAKE, "[r]", " retake"),
    (Choice::Cancel, crate::icons::CANCEL, "[esc]", " cancel"),
];

struct Dialog {
    graphics: *const Graphics,
    image: *const Image,
    save_folder: PathBuf,
    saved_as: Option<PathBuf>,
    target: ID2D1RenderTarget,
    preview: ID2D1Bitmap,
    image_size: (u32, u32),
    height: f32,
    pointer: (f32, f32),
    scale: f32,
    stop_asking: bool,
    controls: Vec<(Control, D2D_RECT_F)>,
    choice: Option<Choice>,
}

/// Shows the dialog centred on the snip's monitor and blocks until the user decides.
pub fn ask(graphics: &Graphics, image: &Image, near: PixelRect, save_folder: &Path) -> Result<Answer> {
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
    let height = dialog_height(image);
    let pixel_width = (WIDTH * scale).round() as i32;
    let pixel_height = (height * scale).round() as i32;
    let work: RECT = monitor_info.rcWork;

    let instance = unsafe { GetModuleHandleW(None)? };
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("oh-snap.confirm"),
            w!("oh-snap"),
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
    let target = create_target(graphics, hwnd, pixel_width, pixel_height, dpi_x as f32)?;
    let mut dialog = Dialog::new(graphics, target, image, height, scale)?;
    dialog.save_folder = save_folder.to_path_buf();
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut dialog as *mut Dialog as isize);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
    }
    dialog.paint();

    let mut message = MSG::default();
    while dialog.choice.is_none() {
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
    Ok(Answer {
        choice: dialog.choice.unwrap_or(Choice::Cancel),
        stop_asking: dialog.stop_asking,
        saved_as: dialog.saved_as,
    })
}

fn dialog_height(image: &Image) -> f32 {
    let preview = preview_rect(image.width, image.height, 0.0);
    HEADER_HEIGHT + (preview.bottom - preview.top) + PADDING + BUTTON_HEIGHT + PADDING
}

/// The snip fitted under the header, never enlarged past 1:1.
fn preview_rect(width: u32, height: u32, top: f32) -> D2D_RECT_F {
    let box_width = WIDTH - 2.0 * PADDING;
    let fit = (box_width / width.max(1) as f32).min(PREVIEW_MAX_HEIGHT / height.max(1) as f32).min(1.0);
    let shown_width = (width as f32 * fit).max(1.0);
    let shown_height = (height as f32 * fit).max(1.0);
    let left = ((WIDTH - shown_width) / 2.0).round();
    D2D_RECT_F {
        left,
        top: top + HEADER_HEIGHT,
        right: left + shown_width,
        bottom: top + HEADER_HEIGHT + shown_height,
    }
}

fn register_class() {
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(dialog_proc),
        hInstance: unsafe { GetModuleHandleW(None) }.unwrap_or_default().into(),
        lpszClassName: w!("oh-snap.confirm"),
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

unsafe extern "system" fn dialog_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let dialog_pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Dialog;
    if dialog_pointer.is_null() {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    let dialog = unsafe { &mut *dialog_pointer };
    match message {
        WM_PAINT => {
            dialog.paint();
            unsafe {
                let _ = ValidateRect(Some(hwnd), None);
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let shape = if dialog.control_at(dialog.pointer).is_some() { IDC_HAND } else { IDC_ARROW };
            if let Ok(cursor) = unsafe { LoadCursorW(None, shape) } {
                unsafe { SetCursor(Some(cursor)) };
            }
            LRESULT(1)
        }
        WM_MOUSEMOVE => {
            dialog.pointer = dialog.to_dips(lparam);
            dialog.paint();
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            dialog.pointer = dialog.to_dips(lparam);
            match dialog.control_at(dialog.pointer) {
                Some(Control::Choose(choice)) => dialog.choice = Some(choice),
                Some(Control::StopAsking) => dialog.stop_asking = !dialog.stop_asking,
                None => {}
            }
            dialog.paint();
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            dialog.pointer = dialog.to_dips(lparam);
            if dialog.over_preview(dialog.pointer) {
                dialog.open_image_menu(hwnd);
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let key = wparam.0 as u16;
            match key {
                key if key == VK_RETURN.0 => dialog.choice = Some(Choice::Save),
                key if key == VK_ESCAPE.0 => dialog.choice = Some(Choice::Cancel),
                key if key == u16::from(b'E') => dialog.choice = Some(Choice::Edit),
                key if key == u16::from(b'R') => dialog.choice = Some(Choice::Retake),
                key if key == u16::from(b'D') => dialog.stop_asking = !dialog.stop_asking,
                _ => {}
            }
            dialog.paint();
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

impl Dialog {
    fn new(graphics: &Graphics, target: ID2D1RenderTarget, image: &Image, height: f32, scale: f32) -> Result<Self> {
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
        let preview = unsafe { target.CreateBitmap(size, Some(image.bgra.as_ptr() as *const _), image.width * 4, &properties)? };
        Ok(Self {
            graphics,
            image,
            save_folder: PathBuf::new(),
            saved_as: None,
            target,
            preview,
            image_size: (image.width, image.height),
            height,
            pointer: (-1.0, -1.0),
            scale,
            stop_asking: false,
            controls: Vec::new(),
            choice: None,
        })
    }

    fn over_preview(&self, point: (f32, f32)) -> bool {
        let frame = preview_rect(self.image_size.0, self.image_size.1, 0.0);
        point.0 >= frame.left && point.0 < frame.right && point.1 >= frame.top && point.1 < frame.bottom
    }

    fn open_image_menu(&mut self, hwnd: HWND) {
        let image = unsafe { &*self.image };
        match image_menu::show(hwnd) {
            Some(ImageAction::Copy) => image_menu::copy(hwnd, image),
            Some(ImageAction::SaveAs) => {
                if let Some(path) = image_menu::save_as(hwnd, image, &self.save_folder) {
                    self.saved_as = Some(path);
                    self.choice = Some(Choice::Done);
                }
            }
            Some(ImageAction::Edit) => self.choice = Some(Choice::Edit),
            None => {}
        }
    }

    fn graphics(&self) -> &Graphics {
        unsafe { &*self.graphics }
    }

    fn to_dips(&self, lparam: LPARAM) -> (f32, f32) {
        let pixel_x = (lparam.0 & 0xFFFF) as u16 as i16 as f32;
        let pixel_y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
        (pixel_x / self.scale, pixel_y / self.scale)
    }

    fn control_at(&self, point: (f32, f32)) -> Option<Control> {
        self.controls
            .iter()
            .find(|(_, rect)| point.0 >= rect.left && point.0 < rect.right && point.1 >= rect.top && point.1 < rect.bottom)
            .map(|(control, _)| *control)
    }

    fn paint(&mut self) {
        let target = self.target.clone();
        let brush = |color: D2D1_COLOR_F| unsafe { target.CreateSolidColorBrush(&color, None) };
        let (Ok(rule), Ok(text), Ok(dim), Ok(accent), Ok(accent_soft), Ok(hover)) = (
            brush(style::RULE),
            brush(style::TEXT),
            brush(style::TEXT_DIM),
            brush(style::ACCENT),
            brush(style::ACCENT_SOFT),
            brush(style::HOVER),
        ) else {
            return;
        };
        let hovered = self.control_at(self.pointer);
        unsafe {
            target.BeginDraw();
            target.Clear(Some(&style::INK));
            let edge = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F { left: 0.5, top: 0.5, right: WIDTH - 0.5, bottom: self.height - 0.5 },
                radiusX: 8.0,
                radiusY: 8.0,
            };
            target.DrawRoundedRectangle(&edge, &rule, 1.0, None);
        }

        let spark = D2D_RECT_F { left: PADDING - 6.0, top: 4.0, right: PADDING + 22.0, bottom: HEADER_HEIGHT };
        self.graphics().draw_spark(&target, spark, &accent);
        self.label(&target, "keep this snip?", PADDING + 26.0, 13.0, WIDTH / 2.0 + 60.0, &text);
        let size_text = format!("{} × {}", self.image_size.0, self.image_size.1);
        let (size_width, _) = self.graphics().measure(&size_text);
        self.label(&target, &size_text, WIDTH - PADDING - size_width, 13.0, WIDTH - PADDING + 2.0, &dim);

        let frame = preview_rect(self.image_size.0, self.image_size.1, 0.0);
        unsafe {
            target.DrawBitmap(&self.preview, Some(&frame), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
            let outline = D2D_RECT_F {
                left: frame.left - 0.5,
                top: frame.top - 0.5,
                right: frame.right + 0.5,
                bottom: frame.bottom + 0.5,
            };
            target.DrawRectangle(&outline, &rule, 1.0, None);
        }

        let mut controls = Vec::new();
        let row_top = frame.bottom + PADDING;
        let toggle_label = if self.stop_asking { "[x] don't ask again" } else { "[ ] don't ask again" };
        let (toggle_width, text_height) = self.graphics().measure(toggle_label);
        let text_top = row_top + (BUTTON_HEIGHT - text_height) / 2.0;
        let toggle = D2D_RECT_F {
            left: PADDING - 8.0,
            top: row_top,
            right: PADDING + toggle_width + 8.0,
            bottom: row_top + BUTTON_HEIGHT,
        };
        if hovered == Some(Control::StopAsking) {
            self.fill(&target, toggle, &hover);
        }
        let toggle_brush = if self.stop_asking { &text } else { &dim };
        self.label(&target, toggle_label, PADDING, text_top, toggle.right, toggle_brush);
        controls.push((Control::StopAsking, toggle));

        // Buttons from the right edge inwards, so save sits nearest the snip's size.
        let mut right = WIDTH - PADDING + 8.0;
        for (choice, icon, key, name) in BUTTONS.iter().rev() {
            let (key_width, _) = self.graphics().measure(key);
            let (name_width, _) = self.graphics().measure(name);
            let button = D2D_RECT_F {
                left: right - key_width - name_width - 20.0 - ICON_SLOT,
                top: row_top,
                right,
                bottom: row_top + BUTTON_HEIGHT,
            };
            let control = Control::Choose(*choice);
            let primary = *choice == Choice::Save;
            if primary {
                self.fill(&target, button, &accent_soft);
            } else if hovered == Some(control) {
                self.fill(&target, button, &hover);
            }
            let (key_brush, name_brush) = if primary { (&accent, &accent) } else { (&dim, &text) };
            let icon_area = D2D_RECT_F { left: button.left + 6.0, top: button.top, right: button.left + 6.0 + ICON_SLOT, bottom: button.bottom };
            self.graphics().draw_icon(&target, *icon, icon_area, name_brush);
            self.label(&target, key, button.left + 10.0 + ICON_SLOT, text_top, button.right, key_brush);
            self.label(&target, name, button.left + 10.0 + ICON_SLOT + key_width, text_top, button.right, name_brush);
            controls.push((control, button));
            right = button.left - 4.0;
        }
        self.controls = controls;
        unsafe {
            let _ = target.EndDraw(None, None);
        }
    }

    fn fill(&self, target: &ID2D1RenderTarget, rect: D2D_RECT_F, brush: &ID2D1SolidColorBrush) {
        let shape = D2D1_ROUNDED_RECT { rect, radiusX: 4.0, radiusY: 4.0 };
        unsafe { target.FillRoundedRectangle(&shape, brush) };
    }

    fn label(&self, target: &ID2D1RenderTarget, text: &str, left: f32, top: f32, right: f32, brush: &ID2D1SolidColorBrush) {
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let layout_rect = D2D_RECT_F {
            left,
            top,
            right: right.max(left + 1.0),
            bottom: top + 30.0,
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

/// Draws the dialog into memory for `oh-snap preview`. Returns BGRA pixels and the size.
pub fn render_offscreen(graphics: &Graphics, image: &Image, hovered: Option<(f32, f32)>) -> Result<(Vec<u8>, u32, u32)> {
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapCacheOnLoad,
    };
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};

    let height = dialog_height(image);
    let width = WIDTH as u32;
    let pixel_height = height.round() as u32;
    let imaging: IWICImagingFactory = unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? };
    let bitmap = unsafe { imaging.CreateBitmap(width, pixel_height, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad)? };
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
    let mut dialog = Dialog::new(graphics, target, image, height, 1.0)?;
    dialog.paint();
    if let Some(pointer) = hovered {
        dialog.pointer = pointer;
        dialog.paint();
    }
    let stride = width * 4;
    let mut pixels = vec![0u8; (stride * pixel_height) as usize];
    unsafe { bitmap.CopyPixels(std::ptr::null(), stride, &mut pixels)? };
    Ok((pixels, width, pixel_height))
}
