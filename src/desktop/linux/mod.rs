//! The Linux implementation of `Platform`, for X11 and Wayland. The pure parts (parsing,
//! command lines, file texts) are in `logic`; the OS-facing parts are one file each:
//! `xorg`, `wayland`, `tools` (ffmpeg, tesseract), `daemon` (socket, tray, hotkeys), `setup`.

mod daemon;
mod logic;
mod setup;
mod tools;
mod wayland;
mod xorg;

use std::fmt::Display;
use std::path::{Path, PathBuf};

use crate::desktop::platform::{Platform, RecordOptions, Recording, Screen, Trigger, WindowInfo};
use crate::output::Image;
use logic::session::{self, Session};
use logic::x11::Payload;

/// The logo, for the app menu entry and the tray.
pub(super) const ICON_PNG: &[u8] = include_bytes!("../../../packaging/linux/oh-snap.png");

pub(super) fn display_error<E: Display>(error: E) -> String {
    error.to_string()
}

pub struct LinuxPlatform;

impl LinuxPlatform {
    fn session(&self) -> Result<Session, String> {
        let variable = |name: &str| std::env::var(name).ok();
        session::detect(
            variable("WAYLAND_DISPLAY").as_deref(),
            variable("XDG_SESSION_TYPE").as_deref(),
            variable("DISPLAY").as_deref(),
        )
        .ok_or_else(|| "no graphical session: neither WAYLAND_DISPLAY nor DISPLAY is set".to_string())
    }

    /// On Wayland, falls back to the X clipboard through XWayland (GNOME syncs it to Wayland apps).
    fn copy(&self, payload: Payload) -> Result<(), String> {
        let x11_available = std::env::var_os("DISPLAY").is_some_and(|display| !display.is_empty());
        match self.session()? {
            Session::X11 => xorg::copy_to_clipboard(payload),
            Session::Wayland => {
                let outcome = match &payload {
                    Payload::Text(text) => wayland::copy_text(text),
                    Payload::Png(png) => wayland::copy_png(png),
                };
                match outcome {
                    Err(_) if x11_available => xorg::copy_to_clipboard(payload),
                    other => other,
                }
            }
        }
    }
}

impl Platform for LinuxPlatform {
    fn capture_screens(&self) -> Result<Vec<Screen>, String> {
        match self.session()? {
            Session::X11 => xorg::capture_screens(),
            Session::Wayland => wayland::capture_screens(),
        }
    }

    /// Wayland does not let a program see other windows, so the list is empty there.
    fn windows_on_screen(&self) -> Vec<WindowInfo> {
        match self.session() {
            Ok(Session::X11) => xorg::windows_on_screen(),
            _ => Vec::new(),
        }
    }

    fn copy_image(&self, _image: &Image, png: &[u8]) -> Result<(), String> {
        self.copy(Payload::Png(png.to_vec()))
    }

    fn copy_text(&self, text: &str) -> Result<(), String> {
        self.copy(Payload::Text(text.to_string()))
    }

    fn read_text(&self, image: &Image) -> Result<String, String> {
        tools::read_text(image)
    }

    fn start_recording(
        &self,
        area: (i32, i32, u32, u32),
        options: RecordOptions,
        output: &Path,
    ) -> Result<Box<dyn Recording>, String> {
        tools::start_recording(self.session()?, area, options, output)
    }

    /// Returns after starting its threads, unlike the Windows loop; the winit loop keeps the process alive.
    fn listen(&self, send: Box<dyn Fn(Trigger) + Send + Sync>) -> Result<(), String> {
        daemon::start(self.session()?, send)
    }

    fn install(&self) -> Result<String, String> {
        setup::install()
    }

    fn uninstall(&self) -> Result<String, String> {
        setup::uninstall()
    }

    fn make_gif(&self, recording: &Path) -> Result<PathBuf, String> {
        tools::make_gif(recording)
    }

    fn trim(&self, recording: &Path, from: f64, to: f64) -> Result<PathBuf, String> {
        tools::trim(recording, from, to)
    }

    fn install_update(&self, archive: &[u8]) -> Result<(), String> {
        setup::install_update(archive)
    }

    fn release_asset(&self) -> &'static str {
        setup::release_asset()
    }

    fn open(&self, path: &Path) {
        setup::open(path);
    }

    fn reveal(&self, path: &Path) {
        setup::reveal(path);
    }
}
