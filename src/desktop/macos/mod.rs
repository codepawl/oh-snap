//! The macOS implementation of `Platform`: ScreenCaptureKit for pictures and recording,
//! Vision for text, AppKit for the clipboard, and a menu-bar icon with global hotkeys.
//! Needs macOS 14; recording needs macOS 15.

use std::path::{Path, PathBuf};

use crate::desktop::platform::{Platform, RecordOptions, Recording, Screen, Trigger, WindowInfo};
use crate::output::Image;

mod capture;
mod cgimage;
mod clipboard;
mod install;
mod ocr;
mod pure;
mod record;
mod sck;
mod tray;
mod video;

pub struct MacPlatform;

impl Platform for MacPlatform {
    fn capture_screens(&self) -> Result<Vec<Screen>, String> {
        capture::capture_screens()
    }

    fn windows_on_screen(&self) -> Vec<WindowInfo> {
        capture::windows_on_screen()
    }

    fn copy_image(&self, image: &Image, png: &[u8]) -> Result<(), String> {
        clipboard::copy_image(image, png)
    }

    fn copy_text(&self, text: &str) -> Result<(), String> {
        clipboard::copy_text(text)
    }

    fn read_text(&self, image: &Image) -> Result<String, String> {
        ocr::read_text(image)
    }

    fn start_recording(
        &self,
        area: (i32, i32, u32, u32),
        options: RecordOptions,
        output: &Path,
    ) -> Result<Box<dyn Recording>, String> {
        record::start_recording(area, options, output)
    }

    /// Registers the hotkeys and the menu-bar icon, then returns: the winit loop that
    /// called it keeps them alive and delivers their events.
    fn listen(&self, send: Box<dyn Fn(Trigger) + Send + Sync>) -> Result<(), String> {
        tray::listen(send)
    }

    fn install(&self) -> Result<String, String> {
        install::install()
    }

    fn uninstall(&self) -> Result<String, String> {
        install::uninstall()
    }

    fn make_gif(&self, recording: &Path) -> Result<PathBuf, String> {
        video::make_gif(recording)
    }

    fn trim(&self, recording: &Path, from: f64, to: f64) -> Result<PathBuf, String> {
        video::trim(recording, from, to)
    }

    fn install_update(&self, archive: &[u8]) -> Result<(), String> {
        install::install_update(archive)
    }

    fn release_asset(&self) -> &'static str {
        pure::RELEASE_ASSET
    }

    fn open(&self, path: &Path) {
        install::open(path);
    }

    fn reveal(&self, path: &Path) {
        install::reveal(path);
    }
}
