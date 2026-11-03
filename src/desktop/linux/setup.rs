//! Install, uninstall, self-update, and opening files in other programs.

use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::logic::install::{self as texts, SavedSetting};
use super::logic::{update, uri};
use super::{daemon, display_error};

struct Folders {
    binary: PathBuf,
    autostart_entry: PathBuf,
    menu_entry: PathBuf,
    icon: PathBuf,
    state: PathBuf,
}

fn folders() -> Result<Folders, String> {
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?);
    let from_env = |name: &str, fallback: &str| match std::env::var_os(name) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home.join(fallback),
    };
    let config = from_env("XDG_CONFIG_HOME", ".config");
    let data = from_env("XDG_DATA_HOME", ".local/share");
    Ok(Folders {
        binary: home.join(".local/bin/oh-snap"),
        autostart_entry: config.join("autostart/oh-snap.desktop"),
        menu_entry: data.join("applications/oh-snap.desktop"),
        icon: data.join("icons/hicolor/512x512/apps/oh-snap.png"),
        state: config.join("oh-snap/linux-install.state"),
    })
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    std::fs::write(path, bytes).map_err(|error| format!("{}: {error}", path.display()))
}

fn gsettings(arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("gsettings").args(arguments).output().map_err(display_error)?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn desktop_name() -> String {
    std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default()
}

/// Copies the running program to ~/.local/bin/oh-snap, through a temporary name so a running copy is never cut short.
fn install_binary(target: &Path) -> Result<String, String> {
    let current = std::env::current_exe().map_err(display_error)?;
    if current.canonicalize().ok() == target.canonicalize().ok() {
        return Ok(format!("already at {}", target.display()));
    }
    let bytes = std::fs::read(&current).map_err(display_error)?;
    let staged = target.with_extension("new");
    write_file(&staged, &bytes)?;
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).map_err(display_error)?;
    std::fs::rename(&staged, target).map_err(display_error)?;
    Ok(format!("copied to {}", target.display()))
}

fn load_saved(path: &Path) -> Vec<SavedSetting> {
    std::fs::read_to_string(path).map(|text| texts::parse_saved(&text)).unwrap_or_default()
}

fn remember(saved: &mut Vec<SavedSetting>, schema: &str, key: &str) -> Option<String> {
    let value = gsettings(&["get", schema, key]).ok()?;
    if !saved.iter().any(|setting| setting.schema == schema && setting.key == key) {
        saved.push(SavedSetting { schema: schema.into(), key: key.into(), value: value.clone() });
    }
    Some(value)
}

/// GNOME: Print and friends as custom keybindings that run `oh-snap capture ...`, and GNOME's own
/// screenshot keys emptied. The old values are written down for uninstall.
fn bind_gnome_keys(folders: &Folders, report: &mut Vec<String>) {
    let mut saved = load_saved(&folders.state);
    let Some(current) = remember(&mut saved, texts::MEDIA_KEYS_SCHEMA, "custom-keybindings") else {
        report.push("gsettings is not available; bind Print to `oh-snap capture region` yourself".into());
        return;
    };
    for key in texts::GNOME_SCREENSHOT_KEYS {
        remember(&mut saved, texts::SHELL_KEYBINDINGS_SCHEMA, key);
    }
    if let Err(error) = write_file(&folders.state, texts::serialize_saved(&saved).as_bytes()) {
        report.push(format!("could not save the old shortcuts, so shortcuts were left alone: {error}"));
        return;
    }
    let program = folders.binary.display().to_string();
    for binding in &texts::KEYBINDINGS {
        let schema = format!("{}:{}", texts::CUSTOM_KEYBINDING_SCHEMA, texts::keybinding_path(binding.id));
        let command = format!("{program} {}", binding.arguments);
        for (key, value) in [("name", binding.name), ("command", command.as_str()), ("binding", binding.binding)] {
            let _ = gsettings(&["set", &schema, key, value]);
        }
    }
    let list = texts::format_string_list(&texts::add_ours(&texts::parse_string_list(&current)));
    let _ = gsettings(&["set", texts::MEDIA_KEYS_SCHEMA, "custom-keybindings", &list]);
    for key in texts::GNOME_SCREENSHOT_KEYS {
        let _ = gsettings(&["set", texts::SHELL_KEYBINDINGS_SCHEMA, key, "@as []"]);
    }
    report.push("GNOME: Print = region, Shift+Print = last region, Ctrl+Print = screen; GNOME's own screenshot keys are off".into());
}

fn restore_gnome_keys(folders: &Folders, report: &mut Vec<String>) {
    let saved = load_saved(&folders.state);
    for setting in &saved {
        let _ = gsettings(&["set", &setting.schema, &setting.key, &setting.value]);
    }
    if !saved.iter().any(|setting| setting.key == "custom-keybindings") {
        if let Ok(current) = gsettings(&["get", texts::MEDIA_KEYS_SCHEMA, "custom-keybindings"]) {
            let list = texts::format_string_list(&texts::remove_ours(&texts::parse_string_list(&current)));
            let _ = gsettings(&["set", texts::MEDIA_KEYS_SCHEMA, "custom-keybindings", &list]);
        }
    }
    for binding in &texts::KEYBINDINGS {
        let schema = format!("{}:{}", texts::CUSTOM_KEYBINDING_SCHEMA, texts::keybinding_path(binding.id));
        for key in ["name", "command", "binding"] {
            let _ = gsettings(&["reset", &schema, key]);
        }
    }
    if !saved.is_empty() {
        report.push("GNOME shortcuts put back".into());
    }
    let _ = std::fs::remove_file(&folders.state);
}

pub fn install() -> Result<String, String> {
    let folders = folders()?;
    let mut report = vec![format!("program {}", install_binary(&folders.binary)?)];
    let program = folders.binary.display().to_string();
    write_file(&folders.icon, super::ICON_PNG)?;
    write_file(&folders.menu_entry, texts::desktop_entry(&program, false).as_bytes())?;
    write_file(&folders.autostart_entry, texts::desktop_entry(&program, true).as_bytes())?;
    report.push(format!("start at sign-in: {}", folders.autostart_entry.display()));
    report.push(format!("app menu entry: {}", folders.menu_entry.display()));

    let desktop = desktop_name();
    if texts::is_gnome(&desktop) {
        bind_gnome_keys(&folders, &mut report);
    } else if texts::is_kde(&desktop) {
        report.push(format!(
            "KDE: System Settings > Keyboard > Shortcuts > Add Command: `{program} capture region`, key Print"
        ));
    } else {
        report.push(format!("bind Print to `{program} capture region` in your desktop's keyboard settings if oh-snap does not get it"));
    }
    if !daemon::is_running() {
        let started = Command::new(&folders.binary)
            .arg("daemon")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn();
        report.push(if started.is_ok() { "oh-snap started in the tray".into() } else { "start it with `oh-snap daemon`".into() });
    }
    if !folders.binary.parent().map(|bin| path_has(bin)).unwrap_or(false) {
        report.push("add ~/.local/bin to your PATH to run `oh-snap` from a terminal".into());
    }
    Ok(report.join("\n"))
}

fn path_has(folder: &Path) -> bool {
    std::env::var_os("PATH").map(|path| std::env::split_paths(&path).any(|entry| entry == folder)).unwrap_or(false)
}

pub fn uninstall() -> Result<String, String> {
    let folders = folders()?;
    let mut report = Vec::new();
    let _ = daemon::send_command("quit");
    if texts::is_gnome(&desktop_name()) || folders.state.exists() {
        restore_gnome_keys(&folders, &mut report);
    }
    for path in [&folders.autostart_entry, &folders.menu_entry, &folders.icon, &folders.binary] {
        if std::fs::remove_file(path).is_ok() {
            report.push(format!("removed {}", path.display()));
        }
    }
    if report.is_empty() {
        report.push("nothing to undo".into());
    }
    Ok(report.join("\n"))
}

/// Replaces the program that is running. The rename is atomic and the old file stays mapped in
/// this process, so a failed step leaves the old program in place.
pub fn install_update(archive: &[u8]) -> Result<(), String> {
    let bytes = update::extract_binary(archive)?;
    if !bytes.starts_with(b"\x7fELF") {
        return Err("the update is not a Linux program".into());
    }
    let current = std::env::current_exe().map_err(display_error)?;
    let staged = current.with_extension("new");
    write_file(&staged, &bytes)?;
    let finished = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
        .and_then(|()| std::fs::rename(&staged, &current));
    if let Err(error) = finished {
        let _ = std::fs::remove_file(&staged);
        return Err(format!("could not replace {}: {error}", current.display()));
    }
    Ok(())
}

pub fn release_asset() -> &'static str {
    update::asset_for(std::env::consts::ARCH)
}

pub fn open(path: &Path) {
    let _ = Command::new("xdg-open")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
}

/// Selects the file in the file manager over D-Bus; without one, opens the folder.
pub fn reveal(path: &Path) {
    if show_in_file_manager(path).is_err() {
        open(path.parent().unwrap_or(path));
    }
}

fn show_in_file_manager(path: &Path) -> Result<(), String> {
    let file_uri = uri::path_to_file_uri(&path.to_string_lossy());
    futures_lite::future::block_on(async {
        let connection = ashpd::zbus::Connection::session().await?;
        connection
            .call_method(
                Some("org.freedesktop.FileManager1"),
                "/org/freedesktop/FileManager1",
                Some("org.freedesktop.FileManager1"),
                "ShowItems",
                &(vec![file_uri], ""),
            )
            .await?;
        Ok::<(), ashpd::zbus::Error>(())
    })
    .map_err(display_error)
}
