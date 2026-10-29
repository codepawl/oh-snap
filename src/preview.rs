//! `oh-snap preview DIR`: renders the overlay over a made-up desktop into PNG files,
//! so the look can be checked without capturing anyone's real screen.

use std::path::{Path, PathBuf};

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};

use crate::capture::{PixelRect, Screenshot};
use crate::card::{self, CardMessage};
use crate::output::{self, Image};
use crate::overlay::{self, Graphics, Mode, PreviewScene};

const WIDTH: i32 = 1600;
const HEIGHT: i32 = 900;

pub fn run(folder: &Path) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let windows = fake_windows();
    let screenshot = fake_desktop(&windows);
    let graphics = Graphics::new()?;
    let scenes = [
        ("hover-window", Mode::Region, (520, 380), None),
        ("drag-region", Mode::Region, (1130, 640), Some((700, 300))),
        ("window-mode", Mode::Window, (1250, 200), None),
        ("screen-mode", Mode::Screen, (1500, 820), None),
    ];
    std::fs::create_dir_all(folder)?;
    for (name, mode, cursor, drag_from) in scenes {
        let scene = PreviewScene {
            mode,
            cursor,
            drag_from,
            window_rects: windows.clone(),
        };
        let pixels = overlay::render_offscreen(&graphics, &screenshot, scene)?;
        let image = Image {
            width: WIDTH as u32,
            height: HEIGHT as u32,
            bgra: pixels,
        };
        let path = folder.join(format!("{name}.png"));
        output::write_png_to(&output::encode_png(&image)?, &path)?;
        println!("{}", path.display());
    }

    let snipped = output::crop(&screenshot, windows[1]).ok_or("fake window is off screen")?;
    let saved = CardMessage::Saved {
        copied: true,
        path: Some(PathBuf::from(r"C:\Users\you\Pictures\Screenshots\Screenshot 2026-10-29 201500.png")),
    };
    let failed = CardMessage::Failed("the selected area is off screen".into());
    for (name, image, message) in [("card-saved", Some(&snipped), saved), ("card-failed", None, failed)] {
        let (pixels, width, height) = card::render_offscreen(&graphics, image, message)?;
        let card_image = Image { width, height, bgra: pixels };
        let path = folder.join(format!("{name}.png"));
        output::write_png_to(&output::encode_png(&card_image)?, &path)?;
        println!("{}", path.display());
    }
    Ok(())
}

fn fake_windows() -> Vec<PixelRect> {
    vec![
        PixelRect { left: 980, top: 90, width: 520, height: 340 },
        PixelRect { left: 160, top: 140, width: 760, height: 520 },
        PixelRect { left: 0, top: 860, width: WIDTH, height: 40 },
    ]
}

/// A soft gradient wallpaper with flat app windows: title bar, sidebar, text lines.
fn fake_desktop(windows: &[PixelRect]) -> Screenshot {
    let mut pixels = vec![0u8; (WIDTH * HEIGHT * 4) as usize];
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let horizontal = x as f32 / WIDTH as f32;
            let vertical = y as f32 / HEIGHT as f32;
            let red = 40.0 + 60.0 * horizontal;
            let green = 70.0 + 50.0 * vertical;
            let blue = 120.0 + 70.0 * (1.0 - horizontal);
            put(&mut pixels, x, y, [red as u8, green as u8, blue as u8]);
        }
    }
    for window in windows.iter().rev() {
        paint_window(&mut pixels, *window);
    }
    Screenshot {
        bounds: PixelRect { left: 0, top: 0, width: WIDTH, height: HEIGHT },
        monitors: vec![PixelRect { left: 0, top: 0, width: WIDTH, height: HEIGHT }],
        pixels,
    }
}

fn paint_window(pixels: &mut [u8], window: PixelRect) {
    let is_taskbar = window.height < 60;
    for y in window.top..window.bottom() {
        for x in window.left..window.right() {
            let inside_x = x - window.left;
            let inside_y = y - window.top;
            let color = if is_taskbar {
                [32, 32, 36]
            } else if inside_y < 32 {
                [236, 236, 240]
            } else if inside_x < 160 {
                [243, 243, 246]
            } else if is_text_line(inside_x, inside_y, window.width) {
                [120, 124, 132]
            } else {
                [252, 252, 252]
            };
            put(pixels, x, y, color);
        }
    }
}

fn is_text_line(inside_x: i32, inside_y: i32, width: i32) -> bool {
    let line_top = (inside_y - 60) % 28;
    let line_index = (inside_y - 60) / 28;
    let line_length = width - 220 - (line_index * 53) % 180;
    inside_y >= 60 && (0..8).contains(&line_top) && inside_x >= 190 && inside_x < 190 + line_length
}

fn put(pixels: &mut [u8], x: i32, y: i32, rgb: [u8; 3]) {
    let offset = ((y * WIDTH + x) * 4) as usize;
    pixels[offset] = rgb[2];
    pixels[offset + 1] = rgb[1];
    pixels[offset + 2] = rgb[0];
    pixels[offset + 3] = 255;
}
