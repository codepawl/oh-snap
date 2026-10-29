//! The look: a terminal's mono type in Claude's warm palette, ivory on warm ink with
//! a clay-orange accent.

use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

pub const INK: D2D1_COLOR_F = rgb(0x1F, 0x1E, 0x1D, 1.0);
pub const INK_PANEL: D2D1_COLOR_F = rgb(0x26, 0x26, 0x24, 0.96);
pub const TEXT: D2D1_COLOR_F = rgb(0xF0, 0xEE, 0xE6, 1.0);
pub const TEXT_DIM: D2D1_COLOR_F = rgb(0xA3, 0x9E, 0x93, 1.0);
pub const RULE: D2D1_COLOR_F = rgb(0x3E, 0x3C, 0x38, 1.0);
pub const HOVER: D2D1_COLOR_F = rgb(0xF0, 0xEE, 0xE6, 0.07);
pub const ACCENT: D2D1_COLOR_F = rgb(0xD9, 0x77, 0x57, 1.0);
pub const ACCENT_SOFT: D2D1_COLOR_F = rgb(0xD9, 0x77, 0x57, 0.16);
pub const SHADE: D2D1_COLOR_F = rgb(0x14, 0x14, 0x13, 0.58);
pub const CROSSHAIR: D2D1_COLOR_F = rgb(0xF0, 0xEE, 0xE6, 0.32);
pub const FAILURE: D2D1_COLOR_F = rgb(0xE5, 0x53, 0x4B, 1.0);

/// Tried in order; Consolas ships with every Windows install.
pub const MONO_FONTS: [&str; 3] = ["Cascadia Mono", "Cascadia Code", "Consolas"];
pub const LABEL_SIZE: f32 = 13.0;
/// Claude Code's spinner glyph, the toolbar's only mark.
pub const SPARK: &str = "✻";
pub const SPARK_SIZE: f32 = 19.0;

pub const PANEL_RADIUS: f32 = 6.0;
pub const BUTTON_RADIUS: f32 = 4.0;

pub const LOUPE_CELLS: i32 = 11;
pub const LOUPE_CELL_SIZE: f32 = 10.0;

pub const fn rgb(red: u8, green: u8, blue: u8, alpha: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: red as f32 / 255.0,
        g: green as f32 / 255.0,
        b: blue as f32 / 255.0,
        a: alpha,
    }
}
