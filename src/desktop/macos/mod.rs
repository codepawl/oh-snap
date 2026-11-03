//! The macOS implementation of `Platform`. Every method answers "not yet" until it is built.

use std::path::{Path, PathBuf};

use crate::desktop::platform::{Platform, RecordOptions, Recording, Screen, Trigger, WindowInfo};
use crate::output::Image;

const NOT_YET: &str = "not yet";

pub struct MacPlatform;

impl Platform for MacPlatform {
    fn capture_screens(&self) -> Result<Vec<Screen>, String> {
        Err(NOT_YET.into())
    }

    fn windows_on_screen(&self) -> Vec<WindowInfo> {
        Vec::new()
    }

    fn copy_image(&self, _image: &Image, _png: &[u8]) -> Result<(), String> {
        Err(NOT_YET.into())
    }

    fn copy_text(&self, _text: &str) -> Result<(), String> {
        Err(NOT_YET.into())
    }

    fn read_text(&self, _image: &Image) -> Result<String, String> {
        Err(NOT_YET.into())
    }

    fn start_recording(
        &self,
        _area: (i32, i32, u32, u32),
        _options: RecordOptions,
        _output: &Path,
    ) -> Result<Box<dyn Recording>, String> {
        Err(NOT_YET.into())
    }

    fn listen(&self, _send: Box<dyn Fn(Trigger) + Send + Sync>) -> Result<(), String> {
        Err(NOT_YET.into())
    }

    fn install(&self) -> Result<String, String> {
        Err(NOT_YET.into())
    }

    fn uninstall(&self) -> Result<String, String> {
        Err(NOT_YET.into())
    }

    fn make_gif(&self, _recording: &Path) -> Result<PathBuf, String> {
        Err(NOT_YET.into())
    }

    fn trim(&self, _recording: &Path, _from: f64, _to: f64) -> Result<PathBuf, String> {
        Err(NOT_YET.into())
    }

    fn install_update(&self, _archive: &[u8]) -> Result<(), String> {
        Err(NOT_YET.into())
    }

    fn release_asset(&self) -> &'static str {
        "not-yet"
    }

    fn open(&self, _path: &Path) {}

    fn reveal(&self, _path: &Path) {}
}
