//! The settings window: every option in two columns of groups, each a row of quiet
//! text buttons. A click saves at once and tells the tray process to reload.

use std::path::PathBuf;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT};
use windows::Win32::Graphics::Direct2D::{
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_HWND_RENDER_TARGET_PROPERTIES,
    D2D1_PRESENT_OPTIONS_NONE, D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT,
    D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT, ID2D1HwndRenderTarget, ID2D1RenderTarget, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::DirectWrite::DWRITE_MEASURING_MODE_NATURAL;
use windows::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ValidateRect};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::Shell::{FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetCursorPos,
    GetWindowLongPtrW, IDC_ARROW, IDC_HAND, IsWindow, LoadCursorW, PostQuitMessage, RegisterClassExW, SW_SHOWNORMAL,
    SetCursor, SetForegroundWindow, SetWindowLongPtrW, ShowWindow, WM_KEYDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_NCDESTROY, WM_PAINT, WM_SETCURSOR, WNDCLASSEXW, WS_CAPTION, WS_EX_LEFT, WS_MINIMIZEBOX, WS_OVERLAPPED,
    WS_SYSMENU,
};
use windows::core::{BOOL, Interface, Result, w};

use crate::overlay::{Graphics, Mode};
use crate::settings::{self, CARD_SECONDS_CHOICES, DELAY_CHOICES, FileFormat, ParticleMode, RECORD_FRAME_RATES, Settings, UpdateMode};
use crate::icons;
use crate::style;

const WIDTH: f32 = 1160.0;
const PADDING: f32 = 32.0;
const TITLE_HEIGHT: f32 = 72.0;
const COLUMN_GAP: f32 = 48.0;
const SECTION_HEIGHT: f32 = 44.0;
const SECTION_GAP: f32 = 18.0;
const ROW_HEIGHT: f32 = 42.0;
const LABEL_WIDTH: f32 = 196.0;
const ICON_SLOT: f32 = 26.0;
const BUTTON_HEIGHT: f32 = 30.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    DefaultMode,
    Delay,
    ConfirmBeforeSave,
    SnapToWindows,
    Loupe,
    Crosshair,
    Particles,
    ShutterSound,
    PrintScreen,
    WinShiftS,
    WinShiftR,
    RecordFrameRate,
    RecordSound,
    CopyToClipboard,
    SaveToFolder,
    Folder,
    Format,
    Card,
    CardSeconds,
    OpenEditor,
    StartAtSignIn,
    Updates,
    CheckForUpdates,
}

struct Row {
    field: Field,
    icon: char,
    label: &'static str,
    options: Vec<String>,
    /// None for the folder row, whose buttons are actions rather than a choice.
    selected: Option<usize>,
}

struct Section {
    icon: char,
    title: &'static str,
    rows: Vec<Row>,
}

fn on_off(value: bool) -> (Vec<String>, Option<usize>) {
    (vec!["on".into(), "off".into()], Some(if value { 0 } else { 1 }))
}

fn row(field: Field, icon: char, label: &'static str, choice: (Vec<String>, Option<usize>)) -> Row {
    Row {
        field,
        icon,
        label,
        options: choice.0,
        selected: choice.1,
    }
}

/// The two columns, groups and rows in reading order, built from the current settings.
fn columns(settings: &Settings) -> [Vec<Section>; 2] {
    let modes = [Mode::Region, Mode::Window, Mode::Screen];
    let mode_choice = (
        modes.iter().map(|mode| settings::mode_name(*mode).to_string()).collect(),
        modes.iter().position(|mode| *mode == settings.default_mode),
    );
    let delay_choice = (
        DELAY_CHOICES.iter().map(|seconds| if *seconds == 0 { "off".to_string() } else { format!("{seconds} s") }).collect(),
        DELAY_CHOICES.iter().position(|seconds| *seconds == settings.delay_seconds),
    );
    let particle_modes = [ParticleMode::WhileDragging, ParticleMode::StartAndEnd, ParticleMode::Off];
    let particle_choice = (
        vec!["while dragging".into(), "start + end".into(), "off".into()],
        particle_modes.iter().position(|mode| *mode == settings.particles),
    );
    let format_choice = (
        vec!["PNG".into(), "JPG".into()],
        Some(if settings.file_format == FileFormat::Png { 0 } else { 1 }),
    );
    let card_choice = (
        CARD_SECONDS_CHOICES.iter().map(|seconds| format!("{seconds} s")).collect(),
        CARD_SECONDS_CHOICES.iter().position(|seconds| *seconds == settings.card_seconds),
    );
    let update_modes = [UpdateMode::Auto, UpdateMode::Notify, UpdateMode::Off];
    let update_choice = (
        vec!["auto".into(), "notify".into(), "off".into()],
        update_modes.iter().position(|mode| *mode == settings.updates),
    );
    let mut folder_actions = vec!["change".to_string()];
    if settings.save_folder.is_some() {
        folder_actions.push("reset".to_string());
    }

    let frame_rate_choice = (
        RECORD_FRAME_RATES.iter().map(|rate| format!("{rate} fps")).collect(),
        RECORD_FRAME_RATES.iter().position(|rate| *rate == settings.record_frames_per_second),
    );

    let left = vec![
        Section {
            icon: icons::CAMERA,
            title: "Capture",
            rows: vec![
                row(Field::DefaultMode, icons::REGION, "Default mode", mode_choice),
                row(Field::Delay, icons::TIMER, "Delay", delay_choice),
                row(Field::ConfirmBeforeSave, icons::QUESTION, "Ask before saving", on_off(settings.confirm_before_save)),
                row(Field::SnapToWindows, icons::WINDOW, "Snap to windows", on_off(settings.snap_to_windows)),
                row(Field::Loupe, icons::SEARCH, "Loupe", on_off(settings.show_loupe)),
                row(Field::Crosshair, icons::PLUS, "Crosshair", on_off(settings.show_crosshair)),
            ],
        },
        Section {
            icon: icons::PALETTE,
            title: "Effects",
            rows: vec![
                row(Field::Particles, icons::SPARKLE, "Drag particles", particle_choice),
                row(Field::ShutterSound, icons::SPEAKER, "Shutter sound", on_off(settings.shutter_sound)),
            ],
        },
        Section {
            icon: icons::KEYBOARD,
            title: "Shortcuts",
            rows: vec![
                row(Field::PrintScreen, icons::KEYBOARD, "PrtSc", on_off(settings.use_print_screen)),
                row(Field::WinShiftS, icons::KEYBOARD, "Win+Shift+S", on_off(settings.use_win_shift_s)),
                row(Field::WinShiftR, icons::KEYBOARD, "Win+Shift+R", on_off(settings.use_win_shift_r)),
            ],
        },
    ];
    let right = vec![
        Section {
            icon: icons::SAVE,
            title: "After capture",
            rows: vec![
                row(Field::CopyToClipboard, icons::CLIPBOARD, "Copy to clipboard", on_off(settings.copy_to_clipboard)),
                row(Field::SaveToFolder, icons::SAVE, "Save to folder", on_off(settings.save_to_folder)),
                row(Field::Folder, icons::FOLDER, "Folder", (folder_actions, None)),
                row(Field::Format, icons::IMAGE, "Format", format_choice),
                row(Field::Card, icons::CARD, "Result card", on_off(settings.show_card)),
                row(Field::CardSeconds, icons::TIMER, "Card stays", card_choice),
                row(Field::OpenEditor, icons::EDIT, "Open editor", on_off(settings.open_editor)),
            ],
        },
        Section {
            icon: icons::VIDEO,
            title: "Recording",
            rows: vec![
                row(Field::RecordFrameRate, icons::TIMER, "Frame rate", frame_rate_choice),
                row(Field::RecordSound, icons::SPEAKER, "System sound", on_off(settings.record_system_sound)),
            ],
        },
        Section {
            icon: icons::GEAR,
            title: "System",
            rows: vec![
                row(Field::StartAtSignIn, icons::POWER, "Start at sign-in", on_off(settings.start_at_sign_in)),
                row(Field::Updates, icons::RETAKE, "Updates", update_choice),
                row(Field::CheckForUpdates, icons::SEARCH, "Version", (vec!["check now".into()], None)),
            ],
        },
    ];
    [left, right]
}

fn column_height(sections: &[Section]) -> f32 {
    sections
        .iter()
        .map(|section| SECTION_HEIGHT + section.rows.len() as f32 * ROW_HEIGHT)
        .sum::<f32>()
        + SECTION_GAP * (sections.len().saturating_sub(1)) as f32
}

fn window_height(settings: &Settings) -> f32 {
    let [left, right] = columns(settings);
    TITLE_HEIGHT + column_height(&left).max(column_height(&right)) + PADDING
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Hit {
    field: Field,
    option: usize,
}

struct SettingsWindow {
    graphics: *const Graphics,
    hwnd: HWND,
    target: ID2D1RenderTarget,
    settings: Settings,
    pointer: (f32, f32),
    hits: Vec<(Hit, D2D_RECT_F)>,
    standalone: bool,
}

static mut OPEN_WINDOW: HWND = HWND(std::ptr::null_mut());

/// Opens the window, or brings the open one forward.
pub fn open(graphics: &Graphics, standalone: bool) -> Result<()> {
    unsafe {
        let existing = OPEN_WINDOW;
        if !existing.is_invalid() && IsWindow(Some(existing)).as_bool() {
            let _ = SetForegroundWindow(existing);
            return Ok(());
        }
    }
    register_class();
    let settings = settings::load();
    let instance = unsafe { GetModuleHandleW(None)? };
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LEFT,
            w!("oh-snap.settings"),
            w!("oh-snap settings"),
            style,
            0,
            0,
            100,
            100,
            None,
            None,
            Some(instance.into()),
            None,
        )?
    };
    // Size and centre the window for the DPI of the monitor it opens on.
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let scale = dpi as f32 / 96.0;
    let mut frame = RECT {
        left: 0,
        top: 0,
        right: (WIDTH * scale).round() as i32,
        bottom: (window_height(&settings) * scale).round() as i32,
    };
    unsafe {
        let _ = AdjustWindowRectExForDpi(&mut frame, style, false, WS_EX_LEFT, dpi);
    }
    let mut cursor = Default::default();
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
    let width = frame.right - frame.left;
    let height = frame.bottom - frame.top;
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{SWP_NOZORDER, SetWindowPos};
        let _ = SetWindowPos(
            hwnd,
            None,
            work.left + (work.right - work.left - width) / 2,
            work.top + (work.bottom - work.top - height) / 2,
            width,
            height,
            SWP_NOZORDER,
        );
    }
    dark_frame(hwnd, graphics);
    let target = create_target(graphics, hwnd, dpi as f32)?;
    let window = Box::new(SettingsWindow {
        graphics,
        hwnd,
        target,
        settings,
        pointer: (-1.0, -1.0),
        hits: Vec::new(),
        standalone,
    });
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(window) as isize);
        OPEN_WINDOW = hwnd;
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = SetForegroundWindow(hwnd);
    }
    Ok(())
}

/// `oh-snap settings` with no tray process running: one window and its own loop.
pub fn run_standalone() -> std::result::Result<(), Box<dyn std::error::Error>> {
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, MSG, TranslateMessage};
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }
    let graphics = Graphics::new()?;
    open(&graphics, true)?;
    let mut message = MSG::default();
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    Ok(())
}

fn register_class() {
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(window_proc),
        hInstance: unsafe { GetModuleHandleW(None) }.unwrap_or_default().into(),
        lpszClassName: w!("oh-snap.settings"),
        ..Default::default()
    };
    unsafe { RegisterClassExW(&class) };
}

fn dark_frame(hwnd: HWND, graphics: &Graphics) {
    let dark = BOOL(1);
    let caption: u32 = 0x001D_1E1F;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(20),
            &dark as *const BOOL as *const _,
            size_of::<BOOL>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(35),
            &caption as *const u32 as *const _,
            size_of::<u32>() as u32,
        );
    }
    let _ = graphics;
}

fn create_target(graphics: &Graphics, hwnd: HWND, dpi: f32) -> Result<ID2D1RenderTarget> {
    let mut client = RECT::default();
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut client)? };
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
    let target: ID2D1HwndRenderTarget = unsafe { graphics.d2d.CreateHwndRenderTarget(&properties, &hwnd_properties)? };
    target.cast()
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let window_pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut SettingsWindow;
    if window_pointer.is_null() {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    if message == WM_NCDESTROY {
        let window = unsafe { Box::from_raw(window_pointer) };
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            OPEN_WINDOW = HWND(std::ptr::null_mut());
        }
        if window.standalone {
            unsafe { PostQuitMessage(0) };
        }
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
            let shape = if window.hit_at(window.pointer).is_some() { IDC_HAND } else { IDC_ARROW };
            if let Ok(cursor) = unsafe { LoadCursorW(None, shape) } {
                unsafe { SetCursor(Some(cursor)) };
                return LRESULT(1);
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_MOUSEMOVE => {
            window.pointer = window.to_dips(lparam);
            window.paint();
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            window.pointer = window.to_dips(lparam);
            if let Some(hit) = window.hit_at(window.pointer) {
                window.choose(hit);
            }
            window.paint();
            LRESULT(0)
        }
        WM_KEYDOWN if wparam.0 as u16 == VK_ESCAPE.0 => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

impl SettingsWindow {
    fn graphics(&self) -> &Graphics {
        unsafe { &*self.graphics }
    }

    fn to_dips(&self, lparam: LPARAM) -> (f32, f32) {
        let pixel_x = (lparam.0 & 0xFFFF) as u16 as i16 as f32;
        let pixel_y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
        let mut dpi_x = 96.0;
        let mut dpi_y = 96.0;
        unsafe { self.target.GetDpi(&mut dpi_x, &mut dpi_y) };
        (pixel_x * 96.0 / dpi_x, pixel_y * 96.0 / dpi_y)
    }

    fn hit_at(&self, point: (f32, f32)) -> Option<Hit> {
        self.hits
            .iter()
            .find(|(_, rect)| point.0 >= rect.left && point.0 < rect.right && point.1 >= rect.top && point.1 < rect.bottom)
            .map(|(hit, _)| *hit)
    }

    /// Applies one click, saves, and tells the tray process to reload.
    fn choose(&mut self, hit: Hit) {
        let settings = &mut self.settings;
        let on = hit.option == 0;
        match hit.field {
            Field::DefaultMode => settings.default_mode = [Mode::Region, Mode::Window, Mode::Screen][hit.option],
            Field::Delay => settings.delay_seconds = DELAY_CHOICES[hit.option],
            Field::ConfirmBeforeSave => settings.confirm_before_save = on,
            Field::SnapToWindows => settings.snap_to_windows = on,
            Field::Loupe => settings.show_loupe = on,
            Field::Crosshair => settings.show_crosshair = on,
            Field::Particles => {
                settings.particles = [ParticleMode::WhileDragging, ParticleMode::StartAndEnd, ParticleMode::Off][hit.option];
            }
            Field::ShutterSound => settings.shutter_sound = on,
            Field::PrintScreen => settings.use_print_screen = on,
            Field::WinShiftS => settings.use_win_shift_s = on,
            Field::WinShiftR => settings.use_win_shift_r = on,
            Field::RecordFrameRate => settings.record_frames_per_second = RECORD_FRAME_RATES[hit.option],
            Field::RecordSound => settings.record_system_sound = on,
            Field::CopyToClipboard => settings.copy_to_clipboard = on,
            Field::SaveToFolder => settings.save_to_folder = on,
            Field::Folder if hit.option == 0 => {
                if let Some(folder) = pick_folder(self.hwnd) {
                    settings.save_folder = Some(folder);
                }
            }
            Field::Folder => settings.save_folder = None,
            Field::Format => settings.file_format = if on { FileFormat::Png } else { FileFormat::Jpeg },
            Field::Card => settings.show_card = on,
            Field::CardSeconds => settings.card_seconds = CARD_SECONDS_CHOICES[hit.option],
            Field::OpenEditor => settings.open_editor = on,
            Field::StartAtSignIn => settings.start_at_sign_in = on,
            Field::Updates => settings.updates = [UpdateMode::Auto, UpdateMode::Notify, UpdateMode::Off][hit.option],
            Field::CheckForUpdates => {
                check_for_updates();
                return;
            }
        }
        settings::save(&self.settings);
        if let Some(daemon) = crate::daemon::find_running() {
            crate::daemon::send_command(daemon, "reload-settings");
        } else if hit.field == Field::StartAtSignIn {
            // No tray process to apply it, so apply it here.
            crate::install::set_start_at_sign_in(self.settings.start_at_sign_in);
        }
    }

    fn paint(&mut self) {
        let target = self.target.clone();
        let brush = |color: D2D1_COLOR_F| unsafe { target.CreateSolidColorBrush(&color, None) };
        let (Ok(text), Ok(dim), Ok(accent), Ok(accent_soft), Ok(hover)) = (
            brush(style::TEXT),
            brush(style::TEXT_DIM),
            brush(style::ACCENT),
            brush(style::ACCENT_SOFT),
            brush(style::HOVER),
        ) else {
            return;
        };
        let brushes = RowBrushes { text: &text, dim: &dim, accent: &accent, accent_soft: &accent_soft, hover: &hover };
        unsafe {
            target.BeginDraw();
            target.Clear(Some(&style::INK));
        }
        let spark = D2D_RECT_F { left: PADDING - 8.0, top: 18.0, right: PADDING + 22.0, bottom: 54.0 };
        self.graphics().draw_spark(&target, spark, &accent);
        self.label(&target, "settings", PADDING + 28.0, 27.0, 400.0, &text);

        let hovered = self.hit_at(self.pointer);
        let column_width = (WIDTH - 2.0 * PADDING - COLUMN_GAP) / 2.0;
        let mut hits = Vec::new();
        for (column_index, sections) in columns(&self.settings).iter().enumerate() {
            let left = PADDING + column_index as f32 * (column_width + COLUMN_GAP);
            let mut top = TITLE_HEIGHT;
            for section in sections {
                let icon_area = D2D_RECT_F { left: left - 2.0, top: top + 10.0, right: left + 20.0, bottom: top + 34.0 };
                self.graphics().draw_icon(&target, section.icon, icon_area, &accent);
                self.label(&target, section.title, left + ICON_SLOT, top + 14.0, left + column_width, &accent);
                top += SECTION_HEIGHT;
                for row in &section.rows {
                    self.draw_row(&target, &brushes, row, left, top, column_width, hovered, &mut hits);
                    top += ROW_HEIGHT;
                }
                top += SECTION_GAP;
            }
        }
        self.hits = hits;
        unsafe {
            let _ = target.EndDraw(None, None);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_row(
        &self,
        target: &ID2D1RenderTarget,
        brushes: &RowBrushes,
        row: &Row,
        left: f32,
        top: f32,
        width: f32,
        hovered: Option<Hit>,
        hits: &mut Vec<(Hit, D2D_RECT_F)>,
    ) {
        let (_, text_height) = self.graphics().measure(row.label);
        let text_top = top + (ROW_HEIGHT - text_height) / 2.0;
        let icon_area = D2D_RECT_F { left: left - 2.0, top, right: left + 20.0, bottom: top + ROW_HEIGHT };
        self.graphics().draw_icon(target, row.icon, icon_area, brushes.dim);
        self.label(target, row.label, left + ICON_SLOT, text_top, left + LABEL_WIDTH, brushes.text);

        // Buttons are laid out from the right edge so every row lines up on the right.
        let mut right = left + width;
        let mut placed = Vec::new();
        for (index, option) in row.options.iter().enumerate().rev() {
            let (option_width, _) = self.graphics().measure(option);
            let button = D2D_RECT_F {
                left: right - option_width - 20.0,
                top: top + (ROW_HEIGHT - BUTTON_HEIGHT) / 2.0,
                right,
                bottom: top + (ROW_HEIGHT + BUTTON_HEIGHT) / 2.0,
            };
            placed.push((index, option, button));
            right = button.left - 4.0;
        }
        for (index, option, button) in placed.into_iter().rev() {
            let hit = Hit { field: row.field, option: index };
            let selected = row.selected == Some(index);
            if selected || hovered == Some(hit) {
                let fill = if selected { brushes.accent_soft } else { brushes.hover };
                let shape = D2D1_ROUNDED_RECT { rect: button, radiusX: 4.0, radiusY: 4.0 };
                unsafe { target.FillRoundedRectangle(&shape, fill) };
            }
            let brush = if selected { brushes.accent } else { brushes.text };
            self.label(target, option, button.left + 10.0, text_top, button.right, brush);
            hits.push((hit, button));
        }

        if row.field == Field::CheckForUpdates {
            let version = format!("v{}", crate::updater::CURRENT_VERSION);
            self.label(target, &version, left + LABEL_WIDTH, text_top, right - 8.0, brushes.dim);
        }
        if row.field == Field::Folder {
            let folder = self.settings.save_folder().display().to_string();
            self.label(target, &folder, left + LABEL_WIDTH, text_top, right - 8.0, brushes.dim);
        }
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

struct RowBrushes<'a> {
    text: &'a ID2D1SolidColorBrush,
    dim: &'a ID2D1SolidColorBrush,
    accent: &'a ID2D1SolidColorBrush,
    accent_soft: &'a ID2D1SolidColorBrush,
    hover: &'a ID2D1SolidColorBrush,
}

/// The standard Windows folder picker.
fn pick_folder(owner: HWND) -> Option<PathBuf> {
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let options = dialog.GetOptions().ok()?;
        dialog.SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
        dialog.Show(Some(owner)).ok()?;
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        path.map(PathBuf::from)
    }
}

/// Draws the window into memory for `oh-snap preview`. Returns BGRA pixels and the size.
pub fn render_offscreen(graphics: &Graphics, settings: Settings, pointer: (f32, f32)) -> Result<(Vec<u8>, u32, u32)> {
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapCacheOnLoad,
    };
    let width = WIDTH as u32;
    let height = window_height(&settings).round() as u32;
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
    let mut window = SettingsWindow {
        graphics,
        hwnd: HWND(std::ptr::null_mut()),
        target,
        settings,
        pointer: (-1.0, -1.0),
        hits: Vec::new(),
        standalone: false,
    };
    window.paint();
    window.pointer = pointer;
    window.paint();
    let stride = width * 4;
    let mut pixels = vec![0u8; (stride * height) as usize];
    unsafe { bitmap.CopyPixels(std::ptr::null(), stride, &mut pixels)? };
    Ok((pixels, width, height))
}

/// The tray process checks and shows the answer on a card; with no tray process the
/// command line updater runs instead.
fn check_for_updates() {
    if let Some(daemon) = crate::daemon::find_running() {
        crate::daemon::send_command(daemon, "check-update");
        return;
    }
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg("update").spawn();
    }
}
