#![cfg_attr(windows, windows_subsystem = "windows")]

// Shared by every OS. Until the desktop port uses them, some items are unused off Windows.
#[cfg_attr(not(windows), allow(dead_code))]
mod clock;
#[cfg_attr(not(windows), allow(dead_code))]
mod crash;
#[cfg_attr(not(windows), allow(dead_code))]
mod folders;
#[cfg_attr(not(windows), allow(dead_code))]
mod history;
#[cfg_attr(not(windows), allow(dead_code))]
mod icons;
#[cfg_attr(not(windows), allow(dead_code))]
mod image_file;
#[cfg_attr(not(windows), allow(dead_code))]
mod log;
#[cfg_attr(not(windows), allow(dead_code))]
mod mode;
#[cfg_attr(not(windows), allow(dead_code))]
mod output;
#[cfg_attr(not(windows), allow(dead_code))]
mod palette;
#[cfg_attr(not(windows), allow(dead_code))]
mod qr;
#[cfg_attr(not(windows), allow(dead_code))]
mod settings;

// Windows only.
#[cfg(windows)]
mod addons;
#[cfg(windows)]
mod annotate;
#[cfg(windows)]
mod bench;
#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod card;
#[cfg(windows)]
mod clipboard;
#[cfg(windows)]
mod compare;
#[cfg(windows)]
mod confirm;
#[cfg(windows)]
mod daemon;
#[cfg(windows)]
mod editor;
#[cfg(windows)]
mod gif_export;
#[cfg(windows)]
mod image_menu;
#[cfg(windows)]
mod install;
#[cfg(windows)]
mod logo;
#[cfg(windows)]
mod markup;
#[cfg(windows)]
mod ocr;
#[cfg(windows)]
mod overlay;
#[cfg(windows)]
mod particles;
#[cfg(windows)]
mod pin;
#[cfg(windows)]
mod preview;
#[cfg(windows)]
mod recorder;
#[cfg(windows)]
mod recording_bar;
#[cfg(windows)]
mod settings_window;
#[cfg(windows)]
mod snip;
#[cfg(windows)]
mod sound;
#[cfg(windows)]
mod style;
#[cfg(windows)]
mod trim;
#[cfg(windows)]
mod trim_window;
#[cfg(windows)]
mod updater;
#[cfg(windows)]
mod windows_main;

// macOS and Linux.
// Built for real off Windows; on Windows only in tests, so the shared window drawing runs there.
#[cfg(any(test, not(windows)))]
#[cfg_attr(windows, allow(dead_code))]
mod desktop;

fn main() {
    #[cfg(windows)]
    windows_main::main();
    #[cfg(not(windows))]
    desktop::main();
}
