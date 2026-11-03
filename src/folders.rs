//! Where oh-snap keeps its files, per OS.
//!
//! Windows: %APPDATA% for settings, %LOCALAPPDATA% for data, the Screenshots known folder.
//! macOS: ~/Library/Application Support/oh-snap for both, ~/Pictures/Screenshots.
//! Linux: $XDG_CONFIG_HOME and $XDG_DATA_HOME (defaults ~/.config and ~/.local/share),
//! the XDG pictures folder with Screenshots inside it (default ~/Pictures/Screenshots).

use std::path::PathBuf;

/// Settings live here.
pub fn config_folder() -> PathBuf {
    #[cfg(windows)]
    {
        windows_base("APPDATA").join("oh-snap")
    }
    #[cfg(target_os = "macos")]
    {
        home_folder().join("Library").join("Application Support").join("oh-snap")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        xdg_folder("XDG_CONFIG_HOME", ".config").join("oh-snap")
    }
}

/// The log and the recent snips live here.
pub fn data_folder() -> PathBuf {
    #[cfg(windows)]
    {
        windows_base("LOCALAPPDATA").join("oh-snap")
    }
    #[cfg(target_os = "macos")]
    {
        config_folder()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        xdg_folder("XDG_DATA_HOME", ".local/share").join("oh-snap")
    }
}

/// Where snips are saved unless the settings say otherwise.
pub fn screenshots_folder() -> PathBuf {
    #[cfg(windows)]
    {
        windows_screenshots_folder()
    }
    #[cfg(target_os = "macos")]
    {
        home_folder().join("Pictures").join("Screenshots")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        xdg_pictures_folder().join("Screenshots")
    }
}

#[cfg(windows)]
fn windows_base(variable: &str) -> PathBuf {
    PathBuf::from(std::env::var(variable).unwrap_or_else(|_| ".".into()))
}

/// The folder Windows itself uses for screenshots, so Photos and Explorer list our files too.
#[cfg(windows)]
fn windows_screenshots_folder() -> PathBuf {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Screenshots, KF_FLAG_CREATE, SHGetKnownFolderPath};

    let known = unsafe { SHGetKnownFolderPath(&FOLDERID_Screenshots, KF_FLAG_CREATE, None) };
    if let Ok(raw_path) = known {
        let path = unsafe { raw_path.to_string() }.unwrap_or_default();
        unsafe { CoTaskMemFree(Some(raw_path.0 as *const _)) };
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    let profile = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
    PathBuf::from(profile).join("Pictures").join("Screenshots")
}

#[cfg(unix)]
fn home_folder() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

/// An XDG base folder: the variable when it holds an absolute path, else `~/<fallback>`.
#[cfg(all(unix, not(target_os = "macos")))]
fn xdg_folder(variable: &str, fallback: &str) -> PathBuf {
    match std::env::var(variable) {
        Ok(value) if std::path::Path::new(&value).is_absolute() => PathBuf::from(value),
        _ => home_folder().join(fallback),
    }
}

/// XDG_PICTURES_DIR from ~/.config/user-dirs.dirs, else ~/Pictures.
#[cfg(all(unix, not(target_os = "macos")))]
fn xdg_pictures_folder() -> PathBuf {
    let home = home_folder();
    let user_dirs = xdg_folder("XDG_CONFIG_HOME", ".config").join("user-dirs.dirs");
    if let Ok(text) = std::fs::read_to_string(user_dirs) {
        if let Some(path) = pictures_from_user_dirs(&text, &home) {
            return path;
        }
    }
    home.join("Pictures")
}

/// Reads `XDG_PICTURES_DIR="$HOME/Pictures"` out of the text of user-dirs.dirs.
#[cfg(all(unix, not(target_os = "macos")))]
fn pictures_from_user_dirs(text: &str, home: &std::path::Path) -> Option<PathBuf> {
    let line = text.lines().find(|line| line.trim_start().starts_with("XDG_PICTURES_DIR"))?;
    let value = line.split_once('=')?.1.trim().trim_matches('"');
    let path = match value.strip_prefix("$HOME") {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None => PathBuf::from(value),
    };
    path.is_absolute().then_some(path)
}

#[cfg(all(test, unix, not(target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn user_dirs_file_names_the_pictures_folder() {
        let text = "XDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_PICTURES_DIR=\"$HOME/Bilder\"\n";
        let home = std::path::Path::new("/home/an");
        assert_eq!(pictures_from_user_dirs(text, home), Some(PathBuf::from("/home/an/Bilder")));
        assert_eq!(pictures_from_user_dirs("XDG_DESKTOP_DIR=\"$HOME/Desktop\"", home), None);
    }
}
