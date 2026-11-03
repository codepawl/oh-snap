//! The seam between the shared app and one OS. Each OS implements `Platform` once; the
//! overlay, card, editor and pin windows talk to this trait and never to OS APIs.

use std::path::{Path, PathBuf};

use crate::mode::Mode;
use crate::output::Image;

/// One monitor, captured. Positions and sizes are physical pixels in the shared desktop space.
pub struct Screen {
    pub x: i32,
    pub y: i32,
    /// Physical pixels per logical point (2.0 on Retina).
    pub scale: f32,
    /// BGRA, opaque, like every other `Image`.
    pub image: Image,
}

/// A visible window, for snapping the window mode to it.
pub struct WindowInfo {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub title: String,
}

/// What the hotkeys, the tray menu and the local socket ask the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    Snip(Mode),
    LastRegion,
    Record,
    Quit,
    OpenSettings,
    OpenFolder,
    Recent(PathBuf),
    CheckUpdate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordOptions {
    pub frames_per_second: u32,
    pub system_sound: bool,
    pub microphone: bool,
}

/// A recording in progress.
pub trait Recording: Send {
    fn pause(&mut self, paused: bool);
    /// Ends the recording and returns the finished file.
    fn stop(self: Box<Self>) -> Result<PathBuf, String>;
}

pub trait Platform: Send + Sync {
    /// Every monitor, left to right as the OS lists them.
    fn capture_screens(&self) -> Result<Vec<Screen>, String>;
    /// Visible windows, front to back.
    fn windows_on_screen(&self) -> Vec<WindowInfo>;
    /// Puts the image on the clipboard; `png` is the same image already encoded.
    fn copy_image(&self, image: &Image, png: &[u8]) -> Result<(), String>;
    fn copy_text(&self, text: &str) -> Result<(), String>;
    /// OCR: the text in the image, one line per text line. Empty when there is none.
    fn read_text(&self, image: &Image) -> Result<String, String>;
    /// `area` is x, y, width, height in physical pixels.
    fn start_recording(
        &self,
        area: (i32, i32, u32, u32),
        options: RecordOptions,
        output: &Path,
    ) -> Result<Box<dyn Recording>, String>;
    /// Registers the hotkeys and the tray icon and returns at once; called once from the main
    /// thread inside the running event loop, which keeps the process alive. `send` gets each
    /// trigger. The command socket lives in `desktop::socket`, not here.
    fn listen(&self, send: Box<dyn Fn(Trigger) + Send + Sync>) -> Result<(), String>;
    /// Sets up start at sign-in and the shortcuts; returns what it did, as text to print.
    fn install(&self) -> Result<String, String>;
    fn uninstall(&self) -> Result<String, String>;
    /// Makes a GIF next to a recording and returns its path.
    fn make_gif(&self, recording: &Path) -> Result<PathBuf, String>;
    /// Saves the part of a recording from `from` to `to` seconds as a new file next to it.
    fn trim(&self, recording: &Path, from: f64, to: f64) -> Result<PathBuf, String>;
    /// Replaces the installed program with the release asset in `archive`, already
    /// checked against SHA256SUMS. The running process keeps going until it restarts.
    fn install_update(&self, archive: &[u8]) -> Result<(), String>;
    /// The release asset name for this OS and CPU, as in SHA256SUMS.txt.
    fn release_asset(&self) -> &'static str;
    /// Opens the file in the default app.
    fn open(&self, path: &Path);
    /// Shows the file in the file manager.
    fn reveal(&self, path: &Path);
}
