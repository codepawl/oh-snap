//! `oh-snap install` and `oh-snap uninstall`: take over from Snipping Tool, and give it back.
//!
//! Everything lives under HKCU, so no admin rights are needed, and every change keeps
//! enough of the old state to be undone.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{ERROR_SUCCESS, LPARAM, WIN32_ERROR, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_DWORD, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE,
    RRF_NOEXPAND, RRF_RT_REG_DWORD, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW,
    RegSetValueExW,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, IPersistFile,
};
use windows::Win32::UI::Shell::{FOLDERID_Programs, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink};
use windows::core::{HSTRING, Interface, PCWSTR, w};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const KEYBOARD_KEY: &str = r"Control Panel\Keyboard";
const PRINT_SCREEN_VALUE: &str = "PrintScreenKeyForSnippingEnabled";
const STATE_KEY: &str = r"Software\oh-snap";
const PRINT_SCREEN_BACKUP: &str = "PrintScreenBackup";
const NO_PREVIOUS_VALUE: u32 = u32::MAX;
const PROTOCOLS: [&str; 2] = ["ms-screenclip", "oh-snap"];

type InstallResult = Result<(), Box<dyn std::error::Error>>;

pub fn install_folder() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("Programs").join("oh-snap")
}

pub fn install() -> InstallResult {
    stop_running_daemon();
    let installed_exe = copy_self_to(&install_folder())?;
    let exe = installed_exe.display().to_string();
    let icon = install_folder().join("oh-snap.ico");
    std::fs::write(&icon, ICON_BYTES)?;
    create_start_menu_shortcut(&installed_exe, &icon)?;

    set_string(RUN_KEY, "oh-snap", &format!("\"{exe}\" daemon"))?;
    back_up_print_screen_setting()?;
    set_dword(KEYBOARD_KEY, PRINT_SCREEN_VALUE, 0)?;
    for protocol in PROTOCOLS {
        let class_key = format!(r"Software\Classes\{protocol}");
        set_string(&class_key, "", &format!("URL:{protocol}"))?;
        set_string(&class_key, "URL Protocol", "")?;
        set_string(&format!(r"{class_key}\shell\open\command"), "", &format!("\"{exe}\" \"%1\""))?;
    }

    std::process::Command::new(&installed_exe).arg("daemon").spawn()?;
    add_to_user_path(&install_folder())?;
    println!("installed to {exe}");
    println!("PrtSc and Win+Shift+S now open oh-snap; `oh-snap uninstall` gives them back");
    Ok(())
}

pub fn uninstall() -> InstallResult {
    stop_running_daemon();
    delete_value(RUN_KEY, "oh-snap");
    remove_from_user_path(&install_folder())?;
    restore_print_screen_setting()?;
    for protocol in PROTOCOLS {
        let class_key = format!(r"Software\Classes\{protocol}");
        if protocol == "oh-snap" {
            delete_tree(&class_key);
        } else {
            // Leave the key itself; Windows may own it. Only our handler goes.
            delete_tree(&format!(r"{class_key}\shell"));
            delete_value(&class_key, "URL Protocol");
            delete_value(&class_key, "");
        }
    }
    delete_tree(STATE_KEY);
    let _ = std::fs::remove_file(start_menu_shortcut());
    println!("uninstalled; Snipping Tool has its shortcuts back");
    println!("the program files stay in {}", install_folder().display());
    Ok(())
}

fn stop_running_daemon() {
    if let Some(daemon) = crate::daemon::find_running() {
        crate::daemon::send_command(daemon, "quit");
        for _ in 0..50 {
            if crate::daemon::find_running().is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

fn copy_self_to(folder: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let current = std::env::current_exe()?;
    std::fs::create_dir_all(folder)?;
    let destination = folder.join("oh-snap.exe");
    if current != destination {
        std::fs::copy(&current, &destination)?;
    }
    Ok(destination)
}

fn back_up_print_screen_setting() -> InstallResult {
    if read_dword(STATE_KEY, PRINT_SCREEN_BACKUP).is_some() {
        return Ok(());
    }
    let previous = read_dword(KEYBOARD_KEY, PRINT_SCREEN_VALUE).unwrap_or(NO_PREVIOUS_VALUE);
    set_dword(STATE_KEY, PRINT_SCREEN_BACKUP, previous)
}

fn restore_print_screen_setting() -> InstallResult {
    match read_dword(STATE_KEY, PRINT_SCREEN_BACKUP) {
        Some(NO_PREVIOUS_VALUE) | None => delete_value(KEYBOARD_KEY, PRINT_SCREEN_VALUE),
        Some(previous) => set_dword(KEYBOARD_KEY, PRINT_SCREEN_VALUE, previous)?,
    }
    Ok(())
}

fn check(status: WIN32_ERROR, what: &str) -> InstallResult {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("{what}: {}", windows::core::Error::from(status.to_hresult()).message()).into())
    }
}

fn open_for_write(subkey: &str) -> Result<HKEY, Box<dyn std::error::Error>> {
    let mut key = HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(subkey),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut key,
            None,
        )
    };
    check(status, subkey)?;
    Ok(key)
}

fn set_value(subkey: &str, name: &str, value_type: REG_VALUE_TYPE, bytes: &[u8]) -> InstallResult {
    let key = open_for_write(subkey)?;
    let status = unsafe { RegSetValueExW(key, &HSTRING::from(name), None, value_type, Some(bytes)) };
    unsafe {
        let _ = RegCloseKey(key);
    }
    check(status, &format!(r"{subkey}\{name}"))
}

fn set_string(subkey: &str, name: &str, value: &str) -> InstallResult {
    let utf16: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = unsafe { std::slice::from_raw_parts(utf16.as_ptr() as *const u8, utf16.len() * 2) };
    set_value(subkey, name, REG_SZ, bytes)
}

fn set_dword(subkey: &str, name: &str, value: u32) -> InstallResult {
    set_value(subkey, name, REG_DWORD, &value.to_le_bytes())
}

fn read_dword(subkey: &str, name: &str) -> Option<u32> {
    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(subkey),
            &HSTRING::from(name),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(value)
}

fn delete_value(subkey: &str, name: &str) {
    unsafe {
        let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(subkey), &HSTRING::from(name));
    }
}

fn delete_tree(subkey: &str) {
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(subkey));
    }
}

const ICON_BYTES: &[u8] = include_bytes!("../assets/oh-snap.ico");

fn start_menu_shortcut() -> PathBuf {
    let programs = unsafe { SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT, None) };
    let folder = match programs {
        Ok(raw_path) => {
            let path = unsafe { raw_path.to_string() }.unwrap_or_default();
            unsafe { CoTaskMemFree(Some(raw_path.0 as *const _)) };
            PathBuf::from(path)
        }
        Err(_) => PathBuf::from(std::env::var("APPDATA").unwrap_or_default())
            .join(r"Microsoft\Windows\Start Menu\Programs"),
    };
    folder.join("oh-snap.lnk")
}

/// A Start menu entry with the logo; it starts the tray process, so searching
/// "oh-snap" in Start brings it back after a quit.
fn create_start_menu_shortcut(exe: &Path, icon: &Path) -> InstallResult {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(exe.as_os_str()))?;
        link.SetArguments(w!("daemon"))?;
        link.SetDescription(w!("Fast screen snips"))?;
        link.SetIconLocation(&HSTRING::from(icon.as_os_str()), 0)?;
        let file: IPersistFile = link.cast()?;
        file.Save(&HSTRING::from(start_menu_shortcut().as_os_str()), true)?;
    }
    Ok(())
}

const ENVIRONMENT_KEY: &str = "Environment";

/// Puts the install folder on the user's PATH, so `oh-snap` works in new terminals.
fn add_to_user_path(folder: &Path) -> InstallResult {
    let folder_text = folder.display().to_string();
    let current = read_user_path();
    let already_there = current.split(';').any(|entry| entry.eq_ignore_ascii_case(&folder_text));
    if already_there {
        return Ok(());
    }
    let updated = if current.is_empty() {
        folder_text
    } else {
        format!("{};{folder_text}", current.trim_end_matches(';'))
    };
    write_user_path(&updated)
}

fn remove_from_user_path(folder: &Path) -> InstallResult {
    let folder_text = folder.display().to_string();
    let current = read_user_path();
    let kept: Vec<&str> = current
        .split(';')
        .filter(|entry| !entry.is_empty() && !entry.eq_ignore_ascii_case(&folder_text))
        .collect();
    write_user_path(&kept.join(";"))
}

fn read_user_path() -> String {
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
    let mut size = 0u32;
    let subkey = HSTRING::from(ENVIRONMENT_KEY);
    let name = HSTRING::from("Path");
    let status = unsafe { RegGetValueW(HKEY_CURRENT_USER, &subkey, &name, flags, None, None, Some(&mut size)) };
    if status != ERROR_SUCCESS || size == 0 {
        return String::new();
    }
    let mut buffer = vec![0u16; size as usize / 2 + 1];
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &subkey,
            &name,
            flags,
            None,
            Some(buffer.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return String::new();
    }
    let length = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..length])
}

fn write_user_path(value: &str) -> InstallResult {
    let utf16: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = unsafe { std::slice::from_raw_parts(utf16.as_ptr() as *const u8, utf16.len() * 2) };
    set_value(ENVIRONMENT_KEY, "Path", REG_EXPAND_SZ, bytes)?;
    // Explorer re-reads the environment on this broadcast, so new terminals see the change.
    let mut result = 0usize;
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(w!("Environment").as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            2000,
            Some(&mut result),
        );
    }
    Ok(())
}
