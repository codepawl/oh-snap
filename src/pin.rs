//! A snip pinned on top of everything, to read from while you work in another window.
//! Drag it to move, scroll to zoom, Ctrl+scroll for opacity, Esc or double-click to close.

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, ID2D1Bitmap, ID2D1HwndRenderTarget,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ValidateRect,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_C, VK_CONTROL, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DROPSHADOW, CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetCursorPos, GetWindowLongPtrW,
    GetWindowRect, HTCAPTION, IDC_SIZEALL, LWA_ALPHA, LoadCursorW, RegisterClassExW, SW_SHOW, SWP_NOACTIVATE,
    SWP_NOZORDER, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow, WM_KEYDOWN,
    WM_MOUSEWHEEL, WM_NCDESTROY, WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_NCRBUTTONUP, WM_PAINT, WM_SIZE, WNDCLASSEXW,
    WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{Result, w};

use crate::capture::PixelRect;
use crate::image_menu::{self, ImageAction};
use crate::output::Image;
use crate::overlay::Graphics;
use crate::style;

const MINIMUM_SIDE: i32 = 48;
const ZOOM_STEP: f32 = 1.1;
const OPACITY_STEP: f32 = 0.1;
const LOWEST_OPACITY: f32 = 0.2;
/// A pin starts at most this share of its monitor's work area.
const LARGEST_SHARE: f32 = 0.8;

struct Pin {
    graphics: *const Graphics,
    image: Image,
    target: ID2D1HwndRenderTarget,
    bitmap: ID2D1Bitmap,
    opacity: f32,
}

const MENU_ACTIONS: [ImageAction; 4] = [ImageAction::Copy, ImageAction::SaveAs, ImageAction::Edit, ImageAction::Close];

/// Opens a pin over the area the snip came from, shrunk to fit its monitor when larger.
/// The window lives on the calling thread's message loop and frees itself when closed.
pub fn open(graphics: &Graphics, image: Image, area: PixelRect) -> Result<()> {
    register_class();
    let (left, top, width, height) = placement(&image, area);
    let instance = unsafe { GetModuleHandleW(None)? };
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            w!("oh-snap.pin"),
            w!("oh-snap pin"),
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
    let target = create_target(graphics, hwnd, width, height)?;
    let bitmap = create_bitmap(&target, &image)?;
    let pin = Box::new(Pin {
        graphics,
        image,
        target,
        bitmap,
        opacity: 1.0,
    });
    unsafe {
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(pin) as isize);
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
    Ok(())
}

/// Where the pin goes: over the snipped area at 1:1, or fitted into the monitor.
fn placement(image: &Image, area: PixelRect) -> (i32, i32, i32, i32) {
    let center = POINT {
        x: area.left + area.width / 2,
        y: area.top + area.height / 2,
    };
    let monitor = unsafe { MonitorFromPoint(center, MONITOR_DEFAULTTONEAREST) };
    let mut monitor_info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        let _ = GetMonitorInfoW(monitor, &mut monitor_info);
    }
    let work = monitor_info.rcWork;
    let work_width = (work.right - work.left) as f32;
    let work_height = (work.bottom - work.top) as f32;
    let fit = (work_width * LARGEST_SHARE / image.width.max(1) as f32)
        .min(work_height * LARGEST_SHARE / image.height.max(1) as f32)
        .min(1.0);
    let width = ((image.width as f32 * fit).round() as i32).max(MINIMUM_SIDE);
    let height = ((image.height as f32 * fit).round() as i32).max(MINIMUM_SIDE);
    let left = (center.x - width / 2).clamp(work.left, (work.right - width).max(work.left));
    let top = (center.y - height / 2).clamp(work.top, (work.bottom - height).max(work.top));
    (left, top, width, height)
}

fn register_class() {
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: CS_DROPSHADOW,
        lpfnWndProc: Some(pin_proc),
        hInstance: unsafe { GetModuleHandleW(None) }.unwrap_or_default().into(),
        hCursor: unsafe { LoadCursorW(None, IDC_SIZEALL) }.unwrap_or_default(),
        lpszClassName: w!("oh-snap.pin"),
        ..Default::default()
    };
    unsafe { RegisterClassExW(&class) };
}

fn create_target(graphics: &Graphics, hwnd: HWND, width: i32, height: i32) -> Result<ID2D1HwndRenderTarget> {
    let properties = D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        // Physical pixels, so a pin at 1:1 shows the snip sharp.
        dpiX: 96.0,
        dpiY: 96.0,
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
    unsafe { graphics.d2d.CreateHwndRenderTarget(&properties, &hwnd_properties) }
}

fn create_bitmap(target: &ID2D1HwndRenderTarget, image: &Image) -> Result<ID2D1Bitmap> {
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

unsafe extern "system" fn pin_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let pin_pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Pin;
    if pin_pointer.is_null() {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    if message == WM_NCDESTROY {
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(pin_pointer));
        }
        return LRESULT(0);
    }
    let pin = unsafe { &mut *pin_pointer };
    match message {
        // The whole pin is a title bar: Windows moves it on drag.
        WM_NCHITTEST => LRESULT(HTCAPTION as isize),
        WM_PAINT => {
            pin.paint();
            unsafe {
                let _ = ValidateRect(Some(hwnd), None);
            }
            LRESULT(0)
        }
        WM_SIZE => {
            let size = D2D_SIZE_U {
                width: (lparam.0 & 0xFFFF) as u32,
                height: ((lparam.0 >> 16) & 0xFFFF) as u32,
            };
            unsafe {
                let _ = pin.target.Resize(&size);
            }
            pin.paint();
            LRESULT(0)
        }
        WM_NCLBUTTONDBLCLK => {
            close(hwnd);
            LRESULT(0)
        }
        WM_NCRBUTTONUP => {
            pin.open_menu(hwnd);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let notches = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32 / 120.0;
            let control_held = unsafe { GetKeyState(i32::from(VK_CONTROL.0)) } < 0;
            if control_held {
                pin.change_opacity(hwnd, notches * OPACITY_STEP);
            } else {
                pin.zoom(hwnd, ZOOM_STEP.powf(notches));
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let key = wparam.0 as u16;
            let control_held = unsafe { GetKeyState(i32::from(VK_CONTROL.0)) } < 0;
            if key == VK_ESCAPE.0 {
                close(hwnd);
            } else if key == VK_C.0 && control_held {
                image_menu::copy(hwnd, &pin.image);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn close(hwnd: HWND) {
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

impl Pin {
    fn graphics(&self) -> &Graphics {
        unsafe { &*self.graphics }
    }

    fn paint(&self) {
        let target = &self.target;
        let size = unsafe { target.GetSize() };
        let frame = D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: size.width,
            bottom: size.height,
        };
        let edge = D2D_RECT_F {
            left: 0.5,
            top: 0.5,
            right: size.width - 0.5,
            bottom: size.height - 0.5,
        };
        unsafe {
            target.BeginDraw();
            target.Clear(Some(&style::INK));
            target.DrawBitmap(&self.bitmap, Some(&frame), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
            // An accent edge tells a pin apart from the screen under it.
            if let Ok(accent) = target.CreateSolidColorBrush(&style::ACCENT, None) {
                target.DrawRectangle(&edge, &accent, 1.0, None);
            }
            let _ = target.EndDraw(None, None);
        }
    }

    /// Grows or shrinks the pin around the pointer, keeping the snip's proportions.
    fn zoom(&mut self, hwnd: HWND, factor: f32) {
        let mut window = RECT::default();
        let mut pointer = POINT::default();
        unsafe {
            let _ = GetWindowRect(hwnd, &mut window);
            let _ = GetCursorPos(&mut pointer);
        }
        let old_width = (window.right - window.left) as f32;
        let new_width = (old_width * factor).round().max(MINIMUM_SIDE as f32);
        let new_height = (new_width * self.image.height as f32 / self.image.width.max(1) as f32).round().max(MINIMUM_SIDE as f32);
        let applied = new_width / old_width;
        let left = pointer.x as f32 - (pointer.x - window.left) as f32 * applied;
        let top = pointer.y as f32 - (pointer.y - window.top) as f32 * applied;
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                None,
                left.round() as i32,
                top.round() as i32,
                new_width as i32,
                new_height as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    fn change_opacity(&mut self, hwnd: HWND, change: f32) {
        self.opacity = (self.opacity + change).clamp(LOWEST_OPACITY, 1.0);
        let alpha = (self.opacity * 255.0).round() as u8;
        unsafe {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA);
        }
    }

    fn open_menu(&mut self, hwnd: HWND) {
        match image_menu::show(hwnd, &MENU_ACTIONS) {
            Some(ImageAction::Copy) => image_menu::copy(hwnd, &self.image),
            Some(ImageAction::SaveAs) => {
                let folder = crate::settings::load().save_folder();
                let _ = image_menu::save_as(hwnd, &self.image, &folder);
            }
            Some(ImageAction::Edit) => {
                let copy = copy_image(&self.image);
                if let Err(error) = crate::editor::open(self.graphics(), copy, None, false) {
                    crate::log::write(&format!("editor failed: {error}"));
                }
            }
            Some(ImageAction::Close) => close(hwnd),
            Some(ImageAction::Pin) | None => {}
        }
    }
}

pub fn copy_image(image: &Image) -> Image {
    Image {
        width: image.width,
        height: image.height,
        bgra: image.bgra.clone(),
    }
}
