//! The resident process: tray icon, keyboard hook for the snip shortcuts, and the
//! window other `oh-snap` invocations send commands to.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use windows::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::Threading::CreateMutexW;
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
    LLKHF_INJECTED, MF_STRING, MSG, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, SW_SHOWNORMAL, SendMessageW, SetForegroundWindow, SetWindowsHookExW,
    TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, UnhookWindowsHookEx,
    WH_KEYBOARD_LL, WM_APP, WM_COPYDATA, WM_KEYDOWN, WM_LBUTTONUP, WM_RBUTTONUP, WM_SYSKEYDOWN, WNDCLASSEXW,
};
use windows::core::{HSTRING, PCWSTR, Result, w};

use crate::capture::Capturer;
use crate::overlay::{Graphics, Mode};
use crate::card::CardMessage;
use crate::snip::{self, SnipRequest};
use crate::capture::PixelRect;

const DAEMON_CLASS: PCWSTR = w!("oh-snap.daemon");
const MESSAGE_TRIGGER: u32 = WM_APP + 1;
const MESSAGE_TRAY: u32 = WM_APP + 2;
const COPYDATA_COMMAND: usize = 0x6F68_736E;

const MENU_REGION: usize = 1;
const MENU_WINDOW: usize = 2;
const MENU_SCREEN: usize = 3;
const MENU_FOLDER: usize = 4;
const MENU_QUIT: usize = 5;

static DAEMON_WINDOW: AtomicIsize = AtomicIsize::new(0);
static SWALLOWING_S: AtomicBool = AtomicBool::new(false);
static SWALLOWING_PRINT_SCREEN: AtomicBool = AtomicBool::new(false);

struct Daemon {
    capturer: Option<Capturer>,
    graphics: Graphics,
    busy: bool,
    taskbar_created: u32,
    icon: HICON,
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

pub fn run() -> Result<()> {
    let _single_instance = unsafe { CreateMutexW(None, true, w!("Local\\oh-snap.daemon"))? };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return Ok(());
    }

    unsafe {
        // WIC, used to draw the logo, needs COM.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
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
        taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
        icon,
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
    match message {
        MESSAGE_TRIGGER => {
            let mode = mode_from_number(wparam.0);
            daemon.snip(hwnd, mode);
            LRESULT(0)
        }
        MESSAGE_TRAY => {
            let mouse_event = (lparam.0 & 0xFFFF) as u32;
            if mouse_event == WM_LBUTTONUP {
                daemon.snip(hwnd, Mode::Region);
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
                let mode = words.next().and_then(Mode::parse).unwrap_or(Mode::Region);
                // Answer the sender first; the snip runs from our own queue.
                unsafe {
                    let _ = PostMessageW(Some(hwnd), MESSAGE_TRIGGER, WPARAM(mode_number(mode)), LPARAM(0));
                }
            }
            Some("quit") => unsafe { PostQuitMessage(0) },
            _ => {}
        }
    }

    fn snip(&mut self, hwnd: HWND, mode: Mode) {
        if self.busy {
            return;
        }
        self.busy = true;
        // An old card would land in the new screenshot.
        crate::card::close_current();
        if self.capturer.is_none() {
            self.capturer = Capturer::new().ok();
        }
        if let Some(capturer) = self.capturer.as_mut() {
            let request = SnipRequest {
                mode,
                clipboard: true,
                save_to_folder: true,
                output_path: None,
                shutter_sound: true,
            };
            match snip::run(capturer, &self.graphics, Some(hwnd), &request) {
                Ok(Some(outcome)) => {
                    let message = CardMessage::Saved {
                        copied: outcome.copied,
                        path: outcome.saved_to,
                    };
                    let shown = crate::card::show(&self.graphics, Some(&outcome.image), message, outcome.area);
                    if let Err(error) = shown {
                        crate::log::write(&format!("card failed: {error}"));
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    crate::log::write(&format!("snip failed: {error}"));
                    let cursor_area = cursor_area();
                    let message = CardMessage::Failed(error.to_string());
                    let _ = crate::card::show(&self.graphics, None, message, cursor_area);
                    // A broken duplication is rebuilt on the next snip.
                    self.capturer = None;
                }
            }
        }
        if let Some(capturer) = self.capturer.as_mut() {
            capturer.release_cpu_memory();
        }
        self.busy = false;
    }

    fn show_menu(&mut self, hwnd: HWND) {
        let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
            return;
        };
        unsafe {
            let _ = AppendMenuW(menu, MF_STRING, MENU_REGION, w!("snip region\tPrtSc"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_WINDOW, w!("snip window"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_SCREEN, w!("snip screen"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_FOLDER, w!("open screenshots"));
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
                MENU_REGION => self.snip(hwnd, Mode::Region),
                MENU_WINDOW => self.snip(hwnd, Mode::Window),
                MENU_SCREEN => self.snip(hwnd, Mode::Screen),
                MENU_FOLDER => open_folder(&crate::output::screenshots_folder()),
                MENU_QUIT => PostQuitMessage(0),
                _ => {}
            }
        }
    }
}

fn open_folder(folder: &std::path::Path) {
    let _ = std::fs::create_dir_all(folder);
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(folder.as_os_str()), None, None, SW_SHOWNORMAL);
    }
}

fn mode_number(mode: Mode) -> usize {
    match mode {
        Mode::Region => 0,
        Mode::Window => 1,
        Mode::Screen => 2,
    }
}

fn mode_from_number(number: usize) -> Mode {
    match number {
        1 => Mode::Window,
        2 => Mode::Screen,
        _ => Mode::Region,
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
        if is_down && windows_key && shift && !control && !alt {
            // Key repeat sends more key-downs; only the first one snips.
            if !SWALLOWING_S.swap(true, Ordering::SeqCst) {
                trigger_from_hook(Mode::Region);
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
        if is_down && !windows_key && !alt {
            if !SWALLOWING_PRINT_SCREEN.swap(true, Ordering::SeqCst) {
                trigger_from_hook(Mode::Region);
            }
            return true;
        }
        if !is_down && SWALLOWING_PRINT_SCREEN.swap(false, Ordering::SeqCst) {
            return true;
        }
    }
    false
}

fn trigger_from_hook(mode: Mode) {
    // A harmless injected key: stops the Start menu from opening when Win is released,
    // and makes us the last input source so the overlay may take the foreground.
    send_dummy_key();
    let hwnd = HWND(DAEMON_WINDOW.load(Ordering::SeqCst) as *mut _);
    unsafe {
        let _ = PostMessageW(Some(hwnd), MESSAGE_TRIGGER, WPARAM(mode_number(mode)), LPARAM(0));
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
