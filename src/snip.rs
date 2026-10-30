//! One snip from start to finish: capture, pick an area, then clipboard and file.

use std::path::PathBuf;
use std::time::Instant;

use windows::Win32::Foundation::HWND;

use crate::capture::{Capturer, PixelRect};
use crate::output::{self, Image};
use crate::overlay::{self, Graphics, Mode, OverlayOptions, Tool};
use crate::settings::{FileFormat, ParticleMode, Settings};
use crate::sound;

pub struct SnipRequest {
    pub mode: Mode,
    /// Overrides the settings' folder: the CLI's `--out`.
    pub output_path: Option<PathBuf>,
    pub settings: Settings,
}

pub struct SnipOutcome {
    pub area: PixelRect,
    pub image: Image,
    pub copied: bool,
    pub saved_to: Option<PathBuf>,
    /// The copy kept for the tray's recent snips.
    pub history: Option<PathBuf>,
}

/// What the user picked on the overlay.
pub enum Picked {
    Area(PixelRect),
    /// Read the text in this area instead of keeping the image.
    Text(PixelRect),
    /// A pixel's red, green and blue.
    Color([u8; 3]),
}

pub type SnipResult = Result<Option<SnipOutcome>, Box<dyn std::error::Error>>;

/// Captures the screen and shows the overlay; returns the picked area, or None on cancel.
pub fn pick(
    capturer: &mut Capturer,
    graphics: &Graphics,
    request: &SnipRequest,
    extra_tools: bool,
) -> Result<Option<Picked>, Box<dyn std::error::Error>> {
    let screenshot = capturer.capture()?;
    let options = OverlayOptions {
        start_mode: request.mode,
        close_after_first_frame: false,
        particles: request.settings.particles,
        show_loupe: request.settings.show_loupe,
        show_crosshair: request.settings.show_crosshair,
        snap_to_windows: request.settings.snap_to_windows,
        extra_tools,
    };
    let outcome = overlay::select_area(graphics, screenshot, options)?;
    if let Some(color) = outcome.color {
        return Ok(Some(Picked::Color(color)));
    }
    Ok(outcome.selection.map(|area| match outcome.tool {
        Tool::Text => Picked::Text(area),
        Tool::Snip | Tool::Color => Picked::Area(area),
    }))
}

/// Pick and deliver in one go, for the CLI; the tray process adds the confirm step.
pub fn run(capturer: &mut Capturer, graphics: &Graphics, owner: Option<HWND>, request: &SnipRequest) -> SnipResult {
    match pick(capturer, graphics, request, false)? {
        Some(Picked::Area(area)) => deliver(capturer, owner, request, area).map(Some),
        // The toolbar here offers neither text nor colour.
        Some(Picked::Text(_) | Picked::Color(_)) | None => Ok(None),
    }
}

/// Captures a known area without any UI (the CLI's `--rect`), quietly.
pub fn run_area(capturer: &mut Capturer, owner: Option<HWND>, request: &SnipRequest, area: PixelRect) -> SnipResult {
    capturer.capture()?;
    let mut settings = request.settings.clone();
    settings.shutter_sound = false;
    settings.particles = ParticleMode::Off;
    let quiet = SnipRequest {
        mode: request.mode,
        output_path: request.output_path.clone(),
        settings,
    };
    deliver(capturer, owner, &quiet, area).map(Some)
}

/// The picked area as an image, from the frame the overlay showed.
pub fn crop(capturer: &Capturer, area: PixelRect) -> Result<Image, Box<dyn std::error::Error>> {
    Ok(output::crop(capturer.screenshot(), area).ok_or("the selected area is off screen")?)
}

/// Clipboard and file for an area of the last capture, as the settings say.
pub fn deliver(
    capturer: &Capturer,
    owner: Option<HWND>,
    request: &SnipRequest,
    area: PixelRect,
) -> Result<SnipOutcome, Box<dyn std::error::Error>> {
    let started = Instant::now();
    let settings = &request.settings;
    if settings.shutter_sound {
        sound::play_shutter();
    }
    let image = crop(capturer, area)?;
    let png_bytes = output::encode_png(&image)?;
    if settings.copy_to_clipboard {
        output::copy_to_clipboard(owner, &image, &png_bytes)?;
    }
    let history = crate::history::record(&png_bytes);
    let mut saved_to = None;
    if let Some(path) = &request.output_path {
        output::write_png_to(&png_bytes, path)?;
        saved_to = Some(path.clone());
    } else if settings.save_to_folder {
        let folder = settings.save_folder();
        saved_to = Some(match settings.file_format {
            FileFormat::Png => output::save_file(&png_bytes, &folder, "png")?,
            FileFormat::Jpeg => output::save_file(&output::encode_jpeg(&image)?, &folder, "jpg")?,
        });
    }
    crate::log::write(&format!(
        "snip {}x{} at {},{} delivered in {} ms",
        area.width,
        area.height,
        area.left,
        area.top,
        started.elapsed().as_millis()
    ));
    Ok(SnipOutcome {
        area,
        image,
        copied: settings.copy_to_clipboard,
        saved_to,
        history,
    })
}
