//! What happens to a finished snip, shared by the overlay flow and the command line:
//! clipboard first, then the file, then the recent list.

use std::path::{Path, PathBuf};

use super::platform::{Platform, Screen};
use super::ui::canvas;
use crate::output::{self, Image};
use crate::settings::{FileFormat, Settings};

/// What the caller wants on top of (or instead of) the settings.
#[derive(Clone, Debug, Default)]
pub struct Overrides {
    pub out: Option<PathBuf>,
    pub no_clipboard: bool,
    pub no_save: bool,
}

pub struct Finished {
    pub copied: bool,
    pub copy_error: Option<String>,
    pub saved_path: Option<PathBuf>,
    pub save_error: Option<String>,
}

pub fn finish_snip(platform: &dyn Platform, image: &Image, settings: &Settings, overrides: &Overrides) -> Result<Finished, String> {
    let png = output::encode_png(image).map_err(|error| error.to_string())?;
    let mut finished = Finished { copied: false, copy_error: None, saved_path: None, save_error: None };

    if settings.copy_to_clipboard && !overrides.no_clipboard {
        match platform.copy_image(image, &png) {
            Ok(()) => finished.copied = true,
            Err(error) => finished.copy_error = Some(error),
        }
    }
    if overrides.out.is_some() || (settings.save_to_folder && !overrides.no_save) {
        match save(image, &png, settings, overrides) {
            Ok(path) => finished.saved_path = Some(path),
            Err(error) => finished.save_error = Some(error),
        }
    }
    if settings.keep_recent_snips {
        output_history(&png);
    }
    Ok(finished)
}

fn output_history(png: &[u8]) {
    let _ = crate::history::record(png);
}

fn save(image: &Image, png: &[u8], settings: &Settings, overrides: &Overrides) -> Result<PathBuf, String> {
    if let Some(path) = &overrides.out {
        output::write_png_to(png, path).map_err(|error| error.to_string())?;
        return Ok(path.clone());
    }
    let folder = settings.save_folder();
    match settings.file_format {
        FileFormat::Png => output::save_png(png, &folder).map_err(|error| error.to_string()),
        FileFormat::Jpeg => {
            let bytes = output::encode_jpeg(image).map_err(|error| error.to_string())?;
            output::save_file(&bytes, &folder, "jpg").map_err(|error| error.to_string())
        }
    }
}

/// "100,200,640,480" as x, y, width, height.
pub fn parse_rect(text: &str) -> Result<(i32, i32, u32, u32), String> {
    let numbers: Vec<i64> = text.split(',').map(|part| part.trim().parse::<i64>().map_err(|_| format!("`{text}` is not X,Y,W,H"))).collect::<Result<_, _>>()?;
    match numbers[..] {
        [x, y, width, height] if width > 0 && height > 0 => Ok((x as i32, y as i32, width as u32, height as u32)),
        _ => Err(format!("`{text}` is not X,Y,W,H with a size above zero")),
    }
}

fn last_region_file() -> PathBuf {
    crate::folders::data_folder().join("last-region.txt")
}

pub fn remember_region(area: (i32, i32, u32, u32)) {
    let _ = std::fs::create_dir_all(crate::folders::data_folder());
    let _ = std::fs::write(last_region_file(), format!("{},{},{},{}", area.0, area.1, area.2, area.3));
}

pub fn last_region() -> Option<(i32, i32, u32, u32)> {
    parse_rect(std::fs::read_to_string(last_region_file()).ok()?.trim()).ok()
}

fn screen_containing(screens: &[Screen], x: i32, y: i32) -> Option<&Screen> {
    screens
        .iter()
        .find(|screen| x >= screen.x && y >= screen.y && x < screen.x + screen.image.width as i32 && y < screen.y + screen.image.height as i32)
        .or(screens.first())
}

/// The pixels of a desktop area, taken from the screen under its centre and clipped to it.
pub fn capture_area(platform: &dyn Platform, area: (i32, i32, u32, u32)) -> Result<Image, String> {
    let screens = platform.capture_screens()?;
    crop_from_screens(&screens, area)
}

pub fn crop_from_screens(screens: &[Screen], area: (i32, i32, u32, u32)) -> Result<Image, String> {
    let screen = screen_containing(screens, area.0 + area.2 as i32 / 2, area.1 + area.3 as i32 / 2).ok_or("no screen to capture")?;
    let left = (area.0 - screen.x).clamp(0, screen.image.width as i32);
    let top = (area.1 - screen.y).clamp(0, screen.image.height as i32);
    let right = (area.0 + area.2 as i32 - screen.x).clamp(0, screen.image.width as i32);
    let bottom = (area.1 + area.3 as i32 - screen.y).clamp(0, screen.image.height as i32);
    if right <= left || bottom <= top {
        return Err("that area is outside every screen".into());
    }
    Ok(canvas::crop_image(&screen.image, (left, top, (right - left) as u32, (bottom - top) as u32)))
}

pub fn recording_path(settings: &Settings) -> PathBuf {
    settings.save_folder().join(format!("recording-{}.mp4", output::screenshot_file_stem()))
}

pub fn file_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::ui::testing;

    #[test]
    fn parses_rects() {
        assert_eq!(parse_rect("10, 20,300,200"), Ok((10, 20, 300, 200)));
        assert!(parse_rect("1,2,3").is_err());
        assert!(parse_rect("1,2,0,5").is_err());
        assert!(parse_rect("a,b,c,d").is_err());
    }

    #[test]
    fn crops_from_the_screen_under_the_centre_and_clips() {
        let screens = testing::two_screens();
        let inside = crop_from_screens(&screens, (900, 100, 200, 100)).unwrap();
        assert_eq!((inside.width, inside.height), (200, 100));
        let clipped = crop_from_screens(&screens, (700, 100, 200, 100)).unwrap();
        assert_eq!(clipped.width, 100, "the part on that screen only");
        assert!(crop_from_screens(&screens, (5000, 5000, 10, 10)).is_err());
    }
}
