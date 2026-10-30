//! The resident process: tray icon, keyboard hook for the snip shortcuts, and the
//! window other `oh-snap` invocations send commands to.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::Foundation::WAIT_TIMEOUT;
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, SetProcessWorkingSetSize, WaitForSingleObject};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
    VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_SNAPSHOT,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, ShellExecuteW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CallNextHookEx, CreateIcon, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
    DispatchMessageW, FindWindowExW, GetCursorPos, GetMessageW, GetSystemMetrics, SM_CXSMICON, HHOOK, HICON, HWND_MESSAGE, KBDLLHOOKSTRUCT,
    LLKHF_INJECTED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MSG, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, SW_SHOWNORMAL, SendMessageW, SetForegroundWindow, SetWindowsHookExW,
    TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, UnhookWindowsHookEx,
    WH_KEYBOARD_LL, WM_APP, WM_COPYDATA, WM_KEYDOWN, WM_LBUTTONUP, WM_RBUTTONUP, WM_SYSKEYDOWN, WNDCLASSEXW,
    KillTimer, SetTimer, WM_TIMER,
};
use windows::core::{HSTRING, PCWSTR, Result, w};

use crate::capture::Capturer;
use crate::overlay::{Graphics, Mode};
use crate::card::CardMessage;
use crate::confirm::Choice;
use crate::settings::{Settings, UpdateMode};
use crate::updater::Check;
use crate::snip::{self, SnipRequest};
use crate::capture::PixelRect;

const DAEMON_CLASS: PCWSTR = w!("oh-snap.daemon");
const MESSAGE_TRIGGER: u32 = WM_APP + 1;
const MESSAGE_TRAY: u32 = WM_APP + 2;
const MESSAGE_UPDATE_CHECKED: u32 = WM_APP + 3;
const MESSAGE_UPDATE_DOWNLOADED: u32 = WM_APP + 4;
const COPYDATA_COMMAND: usize = 0x6F68_736E;
/// The process hands its memory back to Windows this long after the last snip, setting or
/// update. Back-to-back snips stay at full speed; the first one after a quiet spell pays
/// about 20 ms to page the capture device back in, from 57 MB to 0.2 MB in Task Manager.
const TRIM_TIMER: usize = 1;
const TRIM_AFTER_MILLISECONDS: u32 = 30_000;
/// A tray process can run for days, so it looks for a release again this often, not only
/// at start.
const UPDATE_TIMER: usize = 2;
const UPDATE_EVERY_MILLISECONDS: u32 = 6 * 60 * 60 * 1000;

const MENU_REGION: usize = 1;
const MENU_WINDOW: usize = 2;
const MENU_SCREEN: usize = 3;
const MENU_FOLDER: usize = 4;
const MENU_QUIT: usize = 5;
const MENU_SETTINGS: usize = 6;
const MENU_UPDATE: usize = 7;
const MENU_LAST_REGION: usize = 8;
const MENU_RECENT_FIRST: usize = 100;

static DAEMON_WINDOW: AtomicIsize = AtomicIsize::new(0);
static SWALLOWING_S: AtomicBool = AtomicBool::new(false);
static SWALLOWING_PRINT_SCREEN: AtomicBool = AtomicBool::new(false);
static USE_PRINT_SCREEN: AtomicBool = AtomicBool::new(true);
static USE_WIN_SHIFT_S: AtomicBool = AtomicBool::new(true);

struct Daemon {
    capturer: Option<Capturer>,
    graphics: Graphics,
    busy: bool,
    /// A verified new exe that waits for the current snip to finish.
    pending_update: Option<(String, Vec<u8>)>,
    settings: Settings,
    /// False until the first apply_settings at start.
    settings_applied: bool,
    taskbar_created: u32,
    icon: HICON,
    /// The area of the last snip, for Shift+PrtSc and "snip last region".
    last_area: Option<PixelRect>,
}

/// Returns the running daemon's window, if there is one.
pub fn find_running() -> Option<HWND> {
    let hwnd = unsafe { FindWindowExW(Some(HWND_MESSAGE), None, DAEMON_CLASS, None) }.ok()?;
    if hwnd.is_invalid() { None } else { Some(hwnd) }
}

/// Sends a text command ("capture region", "quit") to the running daemon.
pub fn send_command(daemon: HWND, command: &str) {
    let bytes = command.as_bytes();
    let data = COPYDATASTRUCT {
        dwData: COPYDATA_COMMAND,
        cbData: bytes.len() as u32,
        lpData: bytes.as_ptr() as *mut _,
    };
    unsafe {
        SendMessageW(
            daemon,
            WM_COPYDATA,
            Some(WPARAM(0)),
            Some(LPARAM(&data as *const COPYDATASTRUCT as isize)),
        );
    }
}

pub fn run(after_update: bool) -> Result<()> {
    let single_instance = unsafe { CreateMutexW(None, true, w!("Local\\oh-snap.daemon"))? };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        if !after_update {
            return Ok(());
        }
        // Started by an update: wait for the old version to let go, then take over.
        let waited = unsafe { WaitForSingleObject(single_instance, 15_000) };
        if waited == WAIT_TIMEOUT {
            return Ok(());
        }
    }
    crate::updater::clean_up_old_exe();

    unsafe {
        // WIC, used to draw the logo, needs COM; dragging a snip out of the card needs OLE.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let _ = windows::Win32::System::Ole::OleInitialize(None);
    }
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(daemon_proc),
        hInstance: instance,
        lpszClassName: DAEMON_CLASS,
        ..Default::default()
    };
    unsafe { RegisterClassExW(&class) };

    let graphics = Graphics::new()?;
    let icon = create_tray_icon(instance, &graphics)?;
    let mut daemon = Daemon {
        capturer: Capturer::new().ok(),
        graphics,
        busy: false,
        pending_update: None,
        settings: crate::settings::load(),
        settings_applied: false,
        taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
        icon,
        last_area: None,
    };
    let hwnd = unsafe {
        CreateWindowExW(
            Default::default(),
            DAEMON_CLASS,
            w!("oh-snap"),
            Default::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance),
            Some(&mut daemon as *mut Daemon as *const _),
        )?
    };
    DAEMON_WINDOW.store(hwnd.0 as isize, Ordering::SeqCst);
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
            &mut daemon as *mut Daemon as isize,
        );
    }
    add_tray_icon(hwnd, daemon.icon);
    crate::overlay::warm_up(&daemon.graphics);
    let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(instance), 0)? };
    let settings = daemon.settings.clone();
    daemon.apply_settings(settings);
    if after_update {
        let message = CardMessage::Notice {
            title: format!("updated to {}", crate::updater::CURRENT_VERSION),
            detail: "oh-snap restarted on the new version".into(),
        };
        let _ = crate::card::show(&daemon.graphics, None, message, cursor_area());
    } else if daemon.settings.updates != UpdateMode::Off {
        start_update_check(hwnd, false, std::time::Duration::from_secs(10));
    }

    unsafe {
        SetTimer(Some(hwnd), UPDATE_TIMER, UPDATE_EVERY_MILLISECONDS, None);
    }
    schedule_trim(hwnd);

    let mut message = MSG::default();
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    unsafe {
        let _ = UnhookWindowsHookEx(hook);
    }
    remove_tray_icon(hwnd);
    Ok(())
}

unsafe extern "system" fn daemon_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{GWLP_USERDATA, GetWindowLongPtrW};
    let daemon_pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Daemon;
    if daemon_pointer.is_null() {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    let daemon = unsafe { &mut *daemon_pointer };
    if matches!(message, MESSAGE_TRIGGER | MESSAGE_TRAY | MESSAGE_UPDATE_CHECKED | MESSAGE_UPDATE_DOWNLOADED | WM_COPYDATA) {
        schedule_trim(hwnd);
    }
    match message {
        WM_TIMER if wparam.0 == UPDATE_TIMER => {
            if daemon.settings.updates != UpdateMode::Off && daemon.pending_update.is_none() {
                start_update_check(hwnd, false, std::time::Duration::ZERO);
            }
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == TRIM_TIMER => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TRIM_TIMER);
            }
            if !daemon.busy {
                trim_memory();
            }
            LRESULT(0)
        }
        MESSAGE_TRIGGER => {
            if wparam.0 == LAST_REGION {
                daemon.snip_last_region(hwnd);
            } else {
                let mode = mode_from_number(wparam.0).unwrap_or(daemon.settings.default_mode);
                daemon.snip(hwnd, mode, None);
            }
            LRESULT(0)
        }
        MESSAGE_UPDATE_CHECKED => {
            let result = unsafe { Box::from_raw(lparam.0 as *mut (bool, std::result::Result<Check, String>)) };
            daemon.on_update_checked(hwnd, result.0, result.1);
            LRESULT(0)
        }
        MESSAGE_UPDATE_DOWNLOADED => {
            let result = unsafe { Box::from_raw(lparam.0 as *mut (String, std::result::Result<Vec<u8>, String>)) };
            daemon.on_update_downloaded(result.0, result.1);
            LRESULT(0)
        }
        MESSAGE_TRAY => {
            let mouse_event = (lparam.0 & 0xFFFF) as u32;
            if mouse_event == WM_LBUTTONUP {
                let mode = daemon.settings.default_mode;
                daemon.snip(hwnd, mode, None);
            } else if mouse_event == WM_RBUTTONUP {
                daemon.show_menu(hwnd);
            }
            LRESULT(0)
        }
        WM_COPYDATA => {
            let data = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
            if data.dwData != COPYDATA_COMMAND {
                return LRESULT(0);
            }
            let bytes = unsafe { std::slice::from_raw_parts(data.lpData as *const u8, data.cbData as usize) };
            let command = String::from_utf8_lossy(bytes).to_string();
            daemon.run_command(hwnd, &command);
            LRESULT(1)
        }
        _ if message == daemon.taskbar_created && message != 0 => {
            add_tray_icon(hwnd, daemon.icon);
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

impl Daemon {
    fn run_command(&mut self, hwnd: HWND, command: &str) {
        if let Some(path) = command.strip_prefix("edit ") {
            open_editor(&self.graphics, std::path::PathBuf::from(path));
            return;
        }
        let mut words = command.split_whitespace();
        match words.next() {
            Some("capture") => {
                let word = words.next();
                let number = match word {
                    Some("last") => LAST_REGION,
                    _ => word.and_then(Mode::parse).map(mode_number).unwrap_or(DEFAULT_MODE),
                };
                // Answer the sender first; the snip runs from our own queue.
                unsafe {
                    let _ = PostMessageW(Some(hwnd), MESSAGE_TRIGGER, WPARAM(number), LPARAM(0));
                }
            }
            Some("reload-settings") => self.apply_settings(crate::settings::load()),
            Some("settings") => self.open_settings(),
            Some("check-update") => start_update_check(hwnd, true, std::time::Duration::ZERO),
            Some("quit") => unsafe { PostQuitMessage(0) },
            _ => {}
        }
    }

    /// Takes new settings: shortcut switches, card time and the sign-in entry follow at once.
    fn apply_settings(&mut self, settings: Settings) {
        USE_PRINT_SCREEN.store(settings.use_print_screen, Ordering::SeqCst);
        USE_WIN_SHIFT_S.store(settings.use_win_shift_s, Ordering::SeqCst);
        crate::card::set_hide_after(settings.card_seconds);
        // Only a change by the user touches the sign-in entry; `install` writes the first one.
        if self.settings_applied && settings.start_at_sign_in != self.settings.start_at_sign_in {
            crate::install::set_start_at_sign_in(settings.start_at_sign_in);
        }
        self.settings = settings;
        self.settings_applied = true;
    }

    fn open_settings(&mut self) {
        if let Err(error) = crate::settings_window::open(&self.graphics, false) {
            crate::log::write(&format!("settings window failed: {error}"));
        }
    }

    /// Snips the last area again without the overlay, or picks a new one when there is none.
    fn snip_last_region(&mut self, hwnd: HWND) {
        let mode = self.settings.default_mode;
        let area = self.last_area;
        self.snip(hwnd, mode, area);
    }

    /// `repeat_area` skips the overlay and the dialog and snips that area of a fresh capture.
    fn snip(&mut self, hwnd: HWND, mode: Mode, repeat_area: Option<PixelRect>) {
        if self.busy {
            return;
        }
        self.busy = true;
        // An old card would land in the new screenshot.
        crate::card::close_current();
        self.count_down();
        if self.capturer.is_none() {
            self.capturer = Capturer::new().ok();
        }
        if let Err(error) = self.snip_until_decided(hwnd, mode, repeat_area) {
            crate::log::write(&format!("snip failed: {error}"));
            let message = CardMessage::Failed(error.to_string());
            let _ = crate::card::show(&self.graphics, None, message, cursor_area());
            // A broken duplication is rebuilt on the next snip.
            self.capturer = None;
        }
        if let Some(capturer) = self.capturer.as_mut() {
            capturer.release_cpu_memory();
        }
        self.busy = false;
        if let Some((version, exe)) = self.pending_update.take() {
            self.apply_update(version, exe);
        }
    }

    /// The delay setting: a card counts down, then leaves the screen before the capture.
    fn count_down(&mut self) {
        let seconds = self.settings.delay_seconds;
        if seconds == 0 {
            return;
        }
        for remaining in (1..=seconds).rev() {
            let _ = crate::card::show(&self.graphics, None, CardMessage::Countdown(remaining), cursor_area());
            pump_messages_for(std::time::Duration::from_secs(1));
        }
        crate::card::close_current();
        // Give DWM a frame or two to take the card off the screen.
        pump_messages_for(std::time::Duration::from_millis(80));
    }

    /// Overlay, then (unless switched off) "keep this snip?", until the user saves,
    /// edits or cancels. Retake goes round again with a fresh capture.
    fn snip_until_decided(
        &mut self,
        hwnd: HWND,
        mode: Mode,
        mut repeat_area: Option<PixelRect>,
    ) -> std::result::Result<(), Box<dyn std::error::Error>> {
        let Some(capturer) = self.capturer.as_mut() else {
            return Err("no screen capture device".into());
        };
        loop {
            let request = SnipRequest {
                mode,
                output_path: None,
                settings: self.settings.clone(),
            };
            let repeating = repeat_area.is_some();
            let area = match repeat_area.take() {
                Some(area) => {
                    capturer.capture()?;
                    area
                }
                None => match snip::pick(capturer, &self.graphics, &request)? {
                    Some(area) => area,
                    None => return Ok(()),
                },
            };
            self.last_area = Some(area);
            let mut request = request;
            if self.settings.confirm_before_save && !repeating {
                let image = snip::crop(capturer, area)?;
                // Like Snipping Tool, the snip is on the clipboard once the area is picked;
                // the dialog only decides about the file.
                if self.settings.copy_to_clipboard {
                    crate::image_menu::copy(hwnd, &image);
                    request.settings.copy_to_clipboard = false;
                }
                let answer = crate::confirm::ask(&self.graphics, &image, area, &self.settings.save_folder())?;
                if answer.stop_asking {
                    self.settings.confirm_before_save = false;
                    crate::settings::save(&self.settings);
                }
                match answer.choice {
                    Choice::Save => {}
                    Choice::Edit => {
                        crate::editor::open(&self.graphics, image, None, false)?;
                        return Ok(());
                    }
                    Choice::Retake => {
                        // Give DWM a frame to take the dialog off the screen.
                        std::thread::sleep(std::time::Duration::from_millis(60));
                        continue;
                    }
                    Choice::Cancel => return Ok(()),
                    Choice::Done => {
                        if self.settings.show_card {
                            let message = CardMessage::Saved {
                                copied: self.settings.copy_to_clipboard,
                                path: answer.saved_as,
                                history: None,
                            };
                            let _ = crate::card::show(&self.graphics, Some(&image), message, area);
                        }
                        return Ok(());
                    }
                }
            }
            let mut outcome = snip::deliver(capturer, Some(hwnd), &request, area)?;
            outcome.copied = self.settings.copy_to_clipboard;
            if self.settings.open_editor {
                crate::editor::open(&self.graphics, outcome.image, outcome.saved_to, false)?;
            } else if self.settings.show_card {
                let message = CardMessage::Saved {
                    copied: outcome.copied,
                    path: outcome.saved_to,
                    history: outcome.history,
                };
                if let Err(error) = crate::card::show(&self.graphics, Some(&outcome.image), message, outcome.area) {
                    crate::log::write(&format!("card failed: {error}"));
                }
            }
            return Ok(());
        }
    }

    /// Puts a recent snip back on the clipboard and shows its card, where it can be pinned,
    /// dragged out, edited or saved somewhere.
    fn open_recent(&mut self, path: &std::path::Path) {
        let image = match crate::image_file::load_png(path) {
            Ok(image) => image,
            Err(error) => {
                crate::log::write(&format!("recent snip {} failed: {error}", path.display()));
                return;
            }
        };
        let hwnd = HWND(DAEMON_WINDOW.load(Ordering::SeqCst) as *mut _);
        crate::image_menu::copy(hwnd, &image);
        crate::card::close_current();
        let message = CardMessage::Recent { path: path.to_path_buf() };
        if let Err(error) = crate::card::show(&self.graphics, Some(&image), message, cursor_area()) {
            crate::log::write(&format!("card failed: {error}"));
        }
    }

    fn show_menu(&mut self, hwnd: HWND) {
        let recent: Vec<std::path::PathBuf> = crate::history::list().into_iter().take(crate::history::LISTED).collect();
        let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
            return;
        };
        unsafe {
            let _ = AppendMenuW(menu, MF_STRING, MENU_REGION, w!("snip region"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_WINDOW, w!("snip window"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_SCREEN, w!("snip screen"));
            let last_region_flags = if self.last_area.is_some() { MF_STRING } else { MF_STRING | MF_GRAYED };
            let _ = AppendMenuW(menu, last_region_flags, MENU_LAST_REGION, w!("snip last region	Shift+PrtSc"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            if let Ok(recent_menu) = CreatePopupMenu() {
                for (index, path) in recent.iter().enumerate() {
                    let _ = AppendMenuW(recent_menu, MF_STRING, MENU_RECENT_FIRST + index, &HSTRING::from(crate::history::label(path)));
                }
                let recent_flags = if recent.is_empty() { MF_POPUP | MF_GRAYED } else { MF_POPUP };
                let _ = AppendMenuW(menu, recent_flags, recent_menu.0 as usize, w!("recent snips"));
            }
            let _ = AppendMenuW(menu, MF_STRING, MENU_FOLDER, w!("open screenshots"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_SETTINGS, w!("settings…"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_UPDATE, w!("check for updates"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("quit"));
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            // The menu only closes on an outside click when its owner is foreground.
            let _ = SetForegroundWindow(hwnd);
            let chosen = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
                cursor.x,
                cursor.y,
                None,
                hwnd,
                None,
            );
            let _ = DestroyMenu(menu);
            match chosen.0 as usize {
                MENU_REGION => self.snip(hwnd, Mode::Region, None),
                MENU_WINDOW => self.snip(hwnd, Mode::Window, None),
                MENU_SCREEN => self.snip(hwnd, Mode::Screen, None),
                MENU_LAST_REGION => self.snip_last_region(hwnd),
                chosen if (MENU_RECENT_FIRST..MENU_RECENT_FIRST + recent.len()).contains(&chosen) => {
                    self.open_recent(&recent[chosen - MENU_RECENT_FIRST]);
                }
                MENU_FOLDER => open_folder(&self.settings.save_folder()),
                MENU_SETTINGS => self.open_settings(),
                MENU_UPDATE => start_update_check(hwnd, true, std::time::Duration::ZERO),
                MENU_QUIT => PostQuitMessage(0),
                _ => {}
            }
        }
    }
}

/// Keeps windows painting while the daemon waits, unlike a plain sleep.
fn pump_messages_for(duration: std::time::Duration) {
    use windows::Win32::UI::WindowsAndMessaging::{PM_REMOVE, PeekMessageW};
    let deadline = std::time::Instant::now() + duration;
    let mut message = MSG::default();
    while std::time::Instant::now() < deadline {
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn open_folder(folder: &std::path::Path) {
    let _ = std::fs::create_dir_all(folder);
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(folder.as_os_str()), None, None, SW_SHOWNORMAL);
    }
}

/// Posted as the trigger's mode number when the default mode from the settings applies.
const DEFAULT_MODE: usize = 99;
/// Posted as the trigger's mode number for Shift+PrtSc: the last area again.
const LAST_REGION: usize = 98;

fn mode_number(mode: Mode) -> usize {
    match mode {
        Mode::Region => 0,
        Mode::Window => 1,
        Mode::Screen => 2,
    }
}

fn mode_from_number(number: usize) -> Option<Mode> {
    match number {
        0 => Some(Mode::Region),
        1 => Some(Mode::Window),
        2 => Some(Mode::Screen),
        _ => None,
    }
}

fn key_is_down(key: VIRTUAL_KEY) -> bool {
    let state = unsafe { GetAsyncKeyState(key.0 as i32) };
    state < 0
}

/// Takes Win+Shift+S and PrintScreen before Explorer hands them to Snipping Tool.
/// The hook must return fast, so it only posts a message to the daemon window.
unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let injected = event.flags.0 & LLKHF_INJECTED.0 != 0;
        if !injected && should_swallow(event.vkCode, wparam.0 as u32) {
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(Some(HHOOK::default()), code, wparam, lparam) }
}

fn should_swallow(virtual_key: u32, key_message: u32) -> bool {
    let is_down = key_message == WM_KEYDOWN || key_message == WM_SYSKEYDOWN;
    let windows_key = key_is_down(VK_LWIN) || key_is_down(VK_RWIN);
    let shift = key_is_down(VK_SHIFT);
    let control = key_is_down(VK_CONTROL);
    let alt = key_is_down(VK_MENU);

    if virtual_key == u32::from(b'S') {
        let enabled = USE_WIN_SHIFT_S.load(Ordering::SeqCst);
        if enabled && is_down && windows_key && shift && !control && !alt {
            // Key repeat sends more key-downs; only the first one snips.
            if !SWALLOWING_S.swap(true, Ordering::SeqCst) {
                trigger_from_hook(DEFAULT_MODE);
            }
            return true;
        }
        if !is_down && SWALLOWING_S.swap(false, Ordering::SeqCst) {
            return true;
        }
        return SWALLOWING_S.load(Ordering::SeqCst);
    }

    if virtual_key == u32::from(VK_SNAPSHOT.0) {
        // Win+PrtSc and Alt+PrtSc stay with Windows.
        let enabled = USE_PRINT_SCREEN.load(Ordering::SeqCst);
        if enabled && is_down && !windows_key && !alt {
            if !SWALLOWING_PRINT_SCREEN.swap(true, Ordering::SeqCst) {
                // Shift+PrtSc snips the last area again; Windows gives it no job of its own.
                trigger_from_hook(if shift { LAST_REGION } else { DEFAULT_MODE });
            }
            return true;
        }
        if !is_down && SWALLOWING_PRINT_SCREEN.swap(false, Ordering::SeqCst) {
            return true;
        }
    }
    false
}

fn trigger_from_hook(mode_number: usize) {
    // A harmless injected key: stops the Start menu from opening when Win is released,
    // and makes us the last input source so the overlay may take the foreground.
    send_dummy_key();
    let hwnd = HWND(DAEMON_WINDOW.load(Ordering::SeqCst) as *mut _);
    unsafe {
        let _ = PostMessageW(Some(hwnd), MESSAGE_TRIGGER, WPARAM(mode_number), LPARAM(0));
    }
}

fn send_dummy_key() {
    const UNASSIGNED_KEY: VIRTUAL_KEY = VIRTUAL_KEY(0xFF);
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: UNASSIGNED_KEY,
                dwFlags: flags,
                ..Default::default()
            },
        },
    };
    let inputs = [key(Default::default()), key(KEYEVENTF_KEYUP)];
    unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
}

fn add_tray_icon(hwnd: HWND, icon: HICON) {
    let mut data = tray_data(hwnd);
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    data.uCallbackMessage = MESSAGE_TRAY;
    data.hIcon = icon;
    let tip: Vec<u16> = "oh-snap".encode_utf16().collect();
    data.szTip[..tip.len()].copy_from_slice(&tip);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &data);
    }
}

fn remove_tray_icon(hwnd: HWND) {
    let data = tray_data(hwnd);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    }
}

/// The logo at the size Windows uses for small icons on this display.
fn create_tray_icon(instance: HINSTANCE, graphics: &Graphics) -> Result<HICON> {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16) as u32;
    let color = crate::logo::render(graphics, size)?;
    let mask = vec![0u8; (size * size / 8) as usize];
    unsafe { CreateIcon(Some(instance), size as i32, size as i32, 1, 32, mask.as_ptr(), color.as_ptr()) }
}

/// A small area around the pointer, for placing a card when there is no snip area.
fn cursor_area() -> PixelRect {
    let mut cursor = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut cursor);
    }
    PixelRect {
        left: cursor.x,
        top: cursor.y,
        width: 1,
        height: 1,
    }
}

/// Opens a saved snip in the editor, inside this process; errors go to the log.
pub fn open_editor(graphics: &Graphics, path: std::path::PathBuf) {
    let opened = crate::image_file::load_png(&path)
        .map_err(|error| error.to_string())
        .and_then(|image| crate::editor::open(graphics, image, Some(path.clone()), false).map_err(|error| error.to_string()));
    if let Err(error) = opened {
        crate::log::write(&format!("could not edit {}: {error}", path.display()));
    }
}

impl Daemon {
    fn on_update_checked(&mut self, hwnd: HWND, manual: bool, result: std::result::Result<Check, String>) {
        match result {
            Ok(Check::UpToDate) if manual => {
                let message = CardMessage::Notice {
                    title: "oh-snap is up to date".into(),
                    detail: format!("version {}", crate::updater::CURRENT_VERSION),
                };
                let _ = crate::card::show(&self.graphics, None, message, cursor_area());
            }
            Ok(Check::UpToDate) => {}
            Ok(Check::Available(release)) => {
                if manual || self.settings.updates == UpdateMode::Auto {
                    if manual {
                        let message = CardMessage::Notice {
                            title: format!("downloading {}", release.version),
                            detail: "oh-snap restarts by itself when it is ready".into(),
                        };
                        let _ = crate::card::show(&self.graphics, None, message, cursor_area());
                    }
                    start_update_download(hwnd, release);
                } else {
                    let message = CardMessage::Notice {
                        title: format!("oh-snap {} is out", release.version),
                        detail: "tray menu > check for updates".into(),
                    };
                    let _ = crate::card::show(&self.graphics, None, message, cursor_area());
                }
            }
            Err(error) => {
                crate::log::write(&format!("update check failed: {error}"));
                if manual {
                    let _ = crate::card::show(&self.graphics, None, CardMessage::Failed(format!("update check: {error}")), cursor_area());
                }
            }
        }
    }

    fn on_update_downloaded(&mut self, version: String, result: std::result::Result<Vec<u8>, String>) {
        match result {
            Ok(exe) if self.busy => self.pending_update = Some((version, exe)),
            Ok(exe) => self.apply_update(version, exe),
            Err(error) => {
                crate::log::write(&format!("update download failed: {error}"));
                let _ = crate::card::show(&self.graphics, None, CardMessage::Failed(format!("update: {error}")), cursor_area());
            }
        }
    }

    /// Swaps the exe and hands over to the new version, which waits for this one to quit.
    fn apply_update(&mut self, version: String, exe: Vec<u8>) {
        match crate::updater::install(&exe) {
            Ok(path) => {
                crate::log::write(&format!("updating to {version}"));
                if std::process::Command::new(&path).args(["daemon", "--after-update"]).spawn().is_ok() {
                    unsafe { PostQuitMessage(0) };
                }
            }
            Err(error) => {
                crate::log::write(&format!("update install failed: {error}"));
                let _ = crate::card::show(&self.graphics, None, CardMessage::Failed(format!("update: {error}")), cursor_area());
            }
        }
    }
}

/// Checks GitHub on a background thread and posts the answer to the daemon window.
fn start_update_check(hwnd: HWND, manual: bool, delay: std::time::Duration) {
    let window = hwnd.0 as isize;
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        let result = Box::new((manual, crate::updater::check()));
        unsafe {
            let _ = PostMessageW(Some(HWND(window as *mut _)), MESSAGE_UPDATE_CHECKED, WPARAM(0), LPARAM(Box::into_raw(result) as isize));
        }
    });
}

fn start_update_download(hwnd: HWND, release: crate::updater::Release) {
    let window = hwnd.0 as isize;
    std::thread::spawn(move || {
        let result = Box::new((release.version.clone(), crate::updater::download(&release)));
        unsafe {
            let _ = PostMessageW(Some(HWND(window as *mut _)), MESSAGE_UPDATE_DOWNLOADED, WPARAM(0), LPARAM(Box::into_raw(result) as isize));
        }
    });
}

/// The exe path of the process that owns a window.
pub fn process_path_of(hwnd: HWND) -> Option<std::path::PathBuf> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    let mut process_id = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process_id)) };
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }.ok()?;
    let mut buffer = [0u16; 1024];
    let mut length = buffer.len() as u32;
    let named = unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, windows::core::PWSTR(buffer.as_mut_ptr()), &mut length) };
    unsafe {
        let _ = CloseHandle(process);
    }
    named.ok()?;
    Some(std::path::PathBuf::from(String::from_utf16_lossy(&buffer[..length as usize])))
}

/// Starts or restarts the countdown to `trim_memory`.
fn schedule_trim(hwnd: HWND) {
    unsafe {
        SetTimer(Some(hwnd), TRIM_TIMER, TRIM_AFTER_MILLISECONDS, None);
    }
}

/// Moves this process's pages to the standby list. They stay in RAM and come back on
/// the next snip, but Windows can give them to other apps while oh-snap sits idle.
fn trim_memory() {
    unsafe {
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}
