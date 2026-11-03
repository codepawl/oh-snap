//! The look as plain colours: a terminal's mono type in a warm palette, ivory on warm ink
//! with a clay-orange accent. Windows turns these into Direct2D colours in `style`.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub red: f32,
    pub green: f32,
    pub blue: f32,
    pub alpha: f32,
}

pub const fn rgb(red: u8, green: u8, blue: u8, alpha: f32) -> Rgba {
    Rgba {
        red: red as f32 / 255.0,
        green: green as f32 / 255.0,
        blue: blue as f32 / 255.0,
        alpha,
    }
}

pub const INK: Rgba = rgb(0x1F, 0x1E, 0x1D, 1.0);
pub const INK_PANEL: Rgba = rgb(0x26, 0x26, 0x24, 0.96);
pub const TEXT: Rgba = rgb(0xF0, 0xEE, 0xE6, 1.0);
pub const TEXT_DIM: Rgba = rgb(0xA3, 0x9E, 0x93, 1.0);
pub const RULE: Rgba = rgb(0x3E, 0x3C, 0x38, 1.0);
pub const HOVER: Rgba = rgb(0xF0, 0xEE, 0xE6, 0.07);
pub const ACCENT: Rgba = rgb(0xD9, 0x77, 0x57, 1.0);
pub const ACCENT_SOFT: Rgba = rgb(0xD9, 0x77, 0x57, 0.16);
pub const SHADE: Rgba = rgb(0x14, 0x14, 0x13, 0.58);
pub const CROSSHAIR: Rgba = rgb(0xF0, 0xEE, 0xE6, 0.32);
pub const FAILURE: Rgba = rgb(0xE5, 0x53, 0x4B, 1.0);
