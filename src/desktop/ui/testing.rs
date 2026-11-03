//! Fixtures for tests: screens with something to look at, a fake `Platform`, and a place to
//! write offscreen PNGs (`target/ui-previews`).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tiny_skia::Pixmap;

use crate::desktop::platform::{Platform, RecordOptions, Recording, Screen, Trigger, WindowInfo};
use crate::output::Image;

fn pixel(red: u8, green: u8, blue: u8) -> [u8; 4] {
    [blue, green, red, 255]
}

/// A warm gradient with two window-like panels, so a shade and a loupe have content.
pub fn sample_image(width: u32, height: u32) -> Image {
    let mut bgra = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let mut color = pixel(40 + (x * 60 / width) as u8, 60 + (y * 80 / height) as u8, 90);
            let in_window = |left: u32, top: u32, w: u32, h: u32| x >= left && x < left + w && y >= top && y < top + h;
            if in_window(60, 80, 360, 260) {
                color = if y < 110 { pixel(0x2B, 0x2A, 0x28) } else { pixel(0xF0, 0xEE, 0xE6) };
            }
            if in_window(300, 200, 320, 220) {
                color = if y < 230 { pixel(0xD9, 0x77, 0x57) } else { pixel(0xE9, 0xE6, 0xDC) };
            }
            bgra.extend_from_slice(&color);
        }
    }
    Image { width, height, bgra }
}

pub fn two_screens() -> Vec<Screen> {
    vec![
        Screen { x: 0, y: 0, scale: 1.0, image: sample_image(800, 500) },
        Screen { x: 800, y: 0, scale: 1.0, image: sample_image(800, 500) },
    ]
}

pub fn sample_windows() -> Vec<WindowInfo> {
    vec![
        WindowInfo { x: 300, y: 200, width: 320, height: 220, title: "front".into() },
        WindowInfo { x: 60, y: 80, width: 360, height: 260, title: "back".into() },
    ]
}

pub fn sample_thumbnail() -> Pixmap {
    super::canvas::pixmap_from_image(&sample_image(800, 500)).expect("pixmap")
}

pub fn preview_folder() -> PathBuf {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("ui-previews");
    std::fs::create_dir_all(&folder).expect("preview folder");
    folder
}

pub fn write_preview(name: &str, pixmap: &Pixmap) {
    pixmap.save_png(preview_folder().join(format!("{name}.png"))).expect("write preview");
}

/// Remembers what the app asked of the OS, and does nothing real.
#[derive(Default)]
pub struct FakePlatform {
    pub copied_images: Mutex<Vec<(u32, u32)>>,
    pub copied_texts: Mutex<Vec<String>>,
}

impl Platform for FakePlatform {
    fn capture_screens(&self) -> Result<Vec<Screen>, String> {
        Ok(two_screens())
    }
    fn windows_on_screen(&self) -> Vec<WindowInfo> {
        sample_windows()
    }
    fn copy_image(&self, image: &Image, _png: &[u8]) -> Result<(), String> {
        self.copied_images.lock().expect("lock").push((image.width, image.height));
        Ok(())
    }
    fn copy_text(&self, text: &str) -> Result<(), String> {
        self.copied_texts.lock().expect("lock").push(text.to_string());
        Ok(())
    }
    fn read_text(&self, _image: &Image) -> Result<String, String> {
        Ok("fake text".into())
    }
    fn start_recording(&self, _area: (i32, i32, u32, u32), _options: RecordOptions, _output: &Path) -> Result<Box<dyn Recording>, String> {
        Err("the fake platform does not record".into())
    }
    fn listen(&self, _send: Box<dyn Fn(Trigger) + Send + Sync>) -> Result<(), String> {
        Ok(())
    }
    fn install(&self) -> Result<String, String> {
        Ok("installed".into())
    }
    fn uninstall(&self) -> Result<String, String> {
        Ok("uninstalled".into())
    }
    fn make_gif(&self, recording: &Path) -> Result<PathBuf, String> {
        Ok(recording.with_extension("gif"))
    }
    fn trim(&self, recording: &Path, _from: f64, _to: f64) -> Result<PathBuf, String> {
        Ok(recording.with_file_name("trimmed.mp4"))
    }
    fn install_update(&self, _archive: &[u8]) -> Result<(), String> {
        Ok(())
    }
    fn release_asset(&self) -> &'static str {
        "oh-snap-fake.tar.gz"
    }
    fn open(&self, _path: &Path) {}
    fn reveal(&self, _path: &Path) {}
}
