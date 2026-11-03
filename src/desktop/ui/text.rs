//! Monospace text with the bundled JetBrains Mono (SIL OFL, `assets/fonts`).

use fontdue::{Font, FontSettings};
use tiny_skia::{Color, Pixmap};

const FONT_BYTES: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");

pub struct Fonts {
    font: Font,
}

impl Fonts {
    pub fn load() -> Fonts {
        let font = Font::from_bytes(FONT_BYTES, FontSettings::default()).expect("the bundled font is valid");
        Fonts { font }
    }

    pub fn width(&self, text: &str, size: f32) -> f32 {
        text.chars().map(|character| self.font.metrics(character, size).advance_width).sum()
    }

    pub fn line_height(&self, size: f32) -> f32 {
        size * 1.35
    }

    /// Draws `text` with its line box starting at `top`.
    pub fn draw(&self, pixmap: &mut Pixmap, text: &str, x: f32, top: f32, size: f32, color: Color) {
        let ascent = self.font.horizontal_line_metrics(size).map_or(size, |lines| lines.ascent);
        let baseline = top + (self.line_height(size) - size * 1.32).max(0.0) / 2.0 + ascent;
        let mut pen = x;
        for character in text.chars() {
            let (metrics, coverage) = self.font.rasterize(character, size);
            let glyph_left = (pen + metrics.xmin as f32).round() as i32;
            let glyph_top = (baseline - metrics.height as f32 - metrics.ymin as f32).round() as i32;
            blend_coverage(pixmap, &coverage, metrics.width, glyph_left, glyph_top, color);
            pen += metrics.advance_width;
        }
    }
}

fn blend_coverage(pixmap: &mut Pixmap, coverage: &[u8], glyph_width: usize, left: i32, top: i32, color: Color) {
    let width = pixmap.width() as i32;
    let height = pixmap.height() as i32;
    let data = pixmap.data_mut();
    for (index, &value) in coverage.iter().enumerate() {
        if value == 0 {
            continue;
        }
        let x = left + (index % glyph_width) as i32;
        let y = top + (index / glyph_width) as i32;
        if x < 0 || y < 0 || x >= width || y >= height {
            continue;
        }
        let source_alpha = color.alpha() * f32::from(value) / 255.0;
        let offset = ((y * width + x) * 4) as usize;
        let channels = [color.red(), color.green(), color.blue()];
        for (channel, &source) in channels.iter().enumerate() {
            let destination = f32::from(data[offset + channel]);
            data[offset + channel] = (source * 255.0 * source_alpha + destination * (1.0 - source_alpha)).round() as u8;
        }
        let destination_alpha = f32::from(data[offset + 3]);
        data[offset + 3] = (255.0 * source_alpha + destination_alpha * (1.0 - source_alpha)).round() as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monospace_width_is_per_character() {
        let fonts = Fonts::load();
        assert!((fonts.width("abcd", 13.0) - fonts.width("WWWW", 13.0)).abs() < 0.01);
    }

    #[test]
    fn drawing_text_changes_pixels() {
        let fonts = Fonts::load();
        let mut pixmap = Pixmap::new(80, 24).unwrap();
        fonts.draw(&mut pixmap, "[r] region", 2.0, 2.0, 13.0, tiny_skia::Color::WHITE);
        assert!(pixmap.data().iter().any(|&byte| byte != 0));
    }
}
