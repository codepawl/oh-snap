//! The few switches a user can flip from the tray menu, kept in
//! %APPDATA%\oh-snap\settings.toml as `key = true` lines.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// Show the snip and ask save, edit or retake before anything is saved.
    pub confirm_before_save: bool,
    /// Pixel and ASCII bits that fly off the pointer while dragging.
    pub particles: bool,
    pub shutter_sound: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            confirm_before_save: true,
            particles: true,
            shutter_sound: true,
        }
    }
}

fn settings_path() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("oh-snap").join("settings.toml")
}

/// Missing file or unknown lines fall back to the defaults.
pub fn load() -> Settings {
    let mut settings = Settings::default();
    let Ok(text) = std::fs::read_to_string(settings_path()) else {
        return settings;
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let enabled = match value.trim() {
            "true" => true,
            "false" => false,
            _ => continue,
        };
        match key.trim() {
            "confirm_before_save" => settings.confirm_before_save = enabled,
            "particles" => settings.particles = enabled,
            "shutter_sound" => settings.shutter_sound = enabled,
            _ => {}
        }
    }
    settings
}

pub fn save(settings: &Settings) {
    let path = settings_path();
    if let Some(folder) = path.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    let text = format!(
        "confirm_before_save = {}\nparticles = {}\nshutter_sound = {}\n",
        settings.confirm_before_save, settings.particles, settings.shutter_sound
    );
    if let Err(error) = std::fs::write(&path, text) {
        crate::log::write(&format!("could not save settings: {error}"));
    }
}
