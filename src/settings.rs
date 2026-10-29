//! Everything a user can change, kept in %APPDATA%\oh-snap\settings.toml as
//! `key = value` lines. Unknown or broken lines fall back to the defaults.

use std::path::PathBuf;

use crate::overlay::Mode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParticleMode {
    /// Bits break off the pointer for the whole drag.
    WhileDragging,
    /// One burst where the drag starts and one where it ends.
    StartAndEnd,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileFormat {
    Png,
    Jpeg,
}

#[derive(Clone, Debug)]
pub struct Settings {
    // Capture
    pub default_mode: Mode,
    pub delay_seconds: u32,
    pub confirm_before_save: bool,
    pub snap_to_windows: bool,
    pub show_loupe: bool,
    pub show_crosshair: bool,
    // Effects
    pub particles: ParticleMode,
    pub shutter_sound: bool,
    // After capture
    pub copy_to_clipboard: bool,
    pub save_to_folder: bool,
    /// None means Pictures\Screenshots.
    pub save_folder: Option<PathBuf>,
    pub file_format: FileFormat,
    pub show_card: bool,
    pub card_seconds: u32,
    pub open_editor: bool,
    // Shortcuts
    pub use_print_screen: bool,
    pub use_win_shift_s: bool,
    // System
    pub start_at_sign_in: bool,
}

pub const DELAY_CHOICES: [u32; 4] = [0, 3, 5, 10];
pub const CARD_SECONDS_CHOICES: [u32; 3] = [3, 5, 10];

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_mode: Mode::Region,
            delay_seconds: 0,
            confirm_before_save: true,
            snap_to_windows: true,
            show_loupe: true,
            show_crosshair: true,
            particles: ParticleMode::WhileDragging,
            shutter_sound: true,
            copy_to_clipboard: true,
            save_to_folder: true,
            save_folder: None,
            file_format: FileFormat::Png,
            show_card: true,
            card_seconds: 5,
            open_editor: false,
            use_print_screen: true,
            use_win_shift_s: true,
            start_at_sign_in: true,
        }
    }
}

impl Settings {
    pub fn save_folder(&self) -> PathBuf {
        self.save_folder.clone().unwrap_or_else(crate::output::screenshots_folder)
    }
}

fn settings_path() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("oh-snap").join("settings.toml")
}

pub fn load() -> Settings {
    let mut settings = Settings::default();
    let Ok(text) = std::fs::read_to_string(settings_path()) else {
        return settings;
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        apply(&mut settings, key.trim(), value.trim());
    }
    settings
}

fn apply(settings: &mut Settings, key: &str, value: &str) {
    let flag = match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    };
    let text = value.strip_prefix('"').and_then(|rest| rest.strip_suffix('"'));
    let number = value.parse::<u32>().ok();
    match key {
        "default_mode" => {
            if let Some(mode) = text.and_then(Mode::parse) {
                settings.default_mode = mode;
            }
        }
        "delay_seconds" => {
            if let Some(seconds) = number.filter(|seconds| DELAY_CHOICES.contains(seconds)) {
                settings.delay_seconds = seconds;
            }
        }
        "particles" => match (text, flag) {
            (Some("while_dragging"), _) | (_, Some(true)) => settings.particles = ParticleMode::WhileDragging,
            (Some("start_and_end"), _) => settings.particles = ParticleMode::StartAndEnd,
            (Some("off"), _) | (_, Some(false)) => settings.particles = ParticleMode::Off,
            _ => {}
        },
        "save_folder" => settings.save_folder = text.filter(|path| !path.is_empty()).map(PathBuf::from),
        "file_format" => match text {
            Some("png") => settings.file_format = FileFormat::Png,
            Some("jpeg") => settings.file_format = FileFormat::Jpeg,
            _ => {}
        },
        "card_seconds" => {
            if let Some(seconds) = number.filter(|seconds| CARD_SECONDS_CHOICES.contains(seconds)) {
                settings.card_seconds = seconds;
            }
        }
        _ => {
            let Some(enabled) = flag else {
                return;
            };
            match key {
                "confirm_before_save" => settings.confirm_before_save = enabled,
                "snap_to_windows" => settings.snap_to_windows = enabled,
                "show_loupe" => settings.show_loupe = enabled,
                "show_crosshair" => settings.show_crosshair = enabled,
                "shutter_sound" => settings.shutter_sound = enabled,
                "copy_to_clipboard" => settings.copy_to_clipboard = enabled,
                "save_to_folder" => settings.save_to_folder = enabled,
                "show_card" => settings.show_card = enabled,
                "open_editor" => settings.open_editor = enabled,
                "use_print_screen" => settings.use_print_screen = enabled,
                "use_win_shift_s" => settings.use_win_shift_s = enabled,
                "start_at_sign_in" => settings.start_at_sign_in = enabled,
                _ => {}
            }
        }
    }
}

pub fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Region => "region",
        Mode::Window => "window",
        Mode::Screen => "screen",
    }
}

pub fn save(settings: &Settings) {
    let path = settings_path();
    if let Some(folder) = path.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    let particles = match settings.particles {
        ParticleMode::WhileDragging => "while_dragging",
        ParticleMode::StartAndEnd => "start_and_end",
        ParticleMode::Off => "off",
    };
    let format = match settings.file_format {
        FileFormat::Png => "png",
        FileFormat::Jpeg => "jpeg",
    };
    let folder = settings
        .save_folder
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    let lines = [
        "# Capture".to_string(),
        format!("default_mode = \"{}\"", mode_name(settings.default_mode)),
        format!("delay_seconds = {}", settings.delay_seconds),
        format!("confirm_before_save = {}", settings.confirm_before_save),
        format!("snap_to_windows = {}", settings.snap_to_windows),
        format!("show_loupe = {}", settings.show_loupe),
        format!("show_crosshair = {}", settings.show_crosshair),
        "# Effects".to_string(),
        format!("particles = \"{particles}\""),
        format!("shutter_sound = {}", settings.shutter_sound),
        "# After capture".to_string(),
        format!("copy_to_clipboard = {}", settings.copy_to_clipboard),
        format!("save_to_folder = {}", settings.save_to_folder),
        format!("save_folder = \"{folder}\""),
        format!("file_format = \"{format}\""),
        format!("show_card = {}", settings.show_card),
        format!("card_seconds = {}", settings.card_seconds),
        format!("open_editor = {}", settings.open_editor),
        "# Shortcuts".to_string(),
        format!("use_print_screen = {}", settings.use_print_screen),
        format!("use_win_shift_s = {}", settings.use_win_shift_s),
        "# System".to_string(),
        format!("start_at_sign_in = {}", settings.start_at_sign_in),
    ];
    if let Err(error) = std::fs::write(&path, lines.join("\n") + "\n") {
        crate::log::write(&format!("could not save settings: {error}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_survives_save_and_load() {
        let folder = std::env::temp_dir().join(format!("oh-snap-settings-test-{}", std::process::id()));
        // SAFETY: the only test that touches APPDATA, before any thread reads it.
        unsafe { std::env::set_var("APPDATA", &folder) };
        let changed = Settings {
            default_mode: Mode::Window,
            delay_seconds: 5,
            confirm_before_save: false,
            snap_to_windows: false,
            show_loupe: false,
            show_crosshair: false,
            particles: ParticleMode::StartAndEnd,
            shutter_sound: false,
            copy_to_clipboard: false,
            save_to_folder: false,
            save_folder: Some(PathBuf::from(r"D:\shots\a b")),
            file_format: FileFormat::Jpeg,
            show_card: false,
            card_seconds: 10,
            open_editor: true,
            use_print_screen: false,
            use_win_shift_s: false,
            start_at_sign_in: false,
        };
        save(&changed);
        let loaded = load();
        let _ = std::fs::remove_dir_all(&folder);
        assert_eq!(format!("{changed:?}"), format!("{loaded:?}"));
    }

    #[test]
    fn old_true_false_particle_lines_still_load() {
        let mut settings = Settings::default();
        apply(&mut settings, "particles", "false");
        assert_eq!(settings.particles, ParticleMode::Off);
        apply(&mut settings, "particles", "true");
        assert_eq!(settings.particles, ParticleMode::WhileDragging);
    }
}
