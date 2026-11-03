//! The look in Direct2D terms. The colours themselves are in `palette`.

use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

use crate::palette::{self, Rgba};

pub const INK: D2D1_COLOR_F = direct2d(palette::INK);
pub const INK_PANEL: D2D1_COLOR_F = direct2d(palette::INK_PANEL);
pub const TEXT: D2D1_COLOR_F = direct2d(palette::TEXT);
pub const TEXT_DIM: D2D1_COLOR_F = direct2d(palette::TEXT_DIM);
pub const RULE: D2D1_COLOR_F = direct2d(palette::RULE);
pub const HOVER: D2D1_COLOR_F = direct2d(palette::HOVER);
pub const ACCENT: D2D1_COLOR_F = direct2d(palette::ACCENT);
pub const ACCENT_SOFT: D2D1_COLOR_F = direct2d(palette::ACCENT_SOFT);
pub const SHADE: D2D1_COLOR_F = direct2d(palette::SHADE);
pub const CROSSHAIR: D2D1_COLOR_F = direct2d(palette::CROSSHAIR);
pub const FAILURE: D2D1_COLOR_F = direct2d(palette::FAILURE);

/// Tried in order; Consolas ships with every Windows install.
pub const MONO_FONTS: [&str; 3] = ["Cascadia Mono", "Cascadia Code", "Consolas"];
pub const LABEL_SIZE: f32 = 13.0;
/// Claude Code's spinner glyph, the toolbar's only mark.
pub const SPARK: &str = "✻";
pub const SPARK_SIZE: f32 = 19.0;
pub const ICON_SIZE: f32 = 16.0;

pub const PANEL_RADIUS: f32 = 6.0;
pub const BUTTON_RADIUS: f32 = 4.0;

pub const LOUPE_CELLS: i32 = 11;
pub const LOUPE_CELL_SIZE: f32 = 10.0;

pub const fn direct2d(color: Rgba) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: color.red,
        g: color.green,
        b: color.blue,
        a: color.alpha,
    }
}

pub const fn rgb(red: u8, green: u8, blue: u8, alpha: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: red as f32 / 255.0,
        g: green as f32 / 255.0,
        b: blue as f32 / 255.0,
        a: alpha,
    }
}
