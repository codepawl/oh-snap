//! The look: `palette`'s colours as tiny-skia colours, and the sizes that go with them.

use tiny_skia::Color;

use crate::palette::{self, Rgba};

pub const LABEL_SIZE: f32 = 13.0;
pub const PANEL_RADIUS: f32 = 6.0;
pub const BUTTON_RADIUS: f32 = 4.0;
pub const LOUPE_CELLS: i32 = 11;
pub const LOUPE_CELL_SIZE: f32 = 10.0;

pub fn color(value: Rgba) -> Color {
    Color::from_rgba(value.red, value.green, value.blue, value.alpha).unwrap_or(Color::BLACK)
}

pub fn ink() -> Color {
    color(palette::INK)
}
pub fn ink_panel() -> Color {
    color(palette::INK_PANEL)
}
pub fn text() -> Color {
    color(palette::TEXT)
}
pub fn text_dim() -> Color {
    color(palette::TEXT_DIM)
}
pub fn rule() -> Color {
    color(palette::RULE)
}
pub fn hover() -> Color {
    color(palette::HOVER)
}
pub fn accent() -> Color {
    color(palette::ACCENT)
}
pub fn accent_soft() -> Color {
    color(palette::ACCENT_SOFT)
}
pub fn shade() -> Color {
    color(palette::SHADE)
}
pub fn crosshair() -> Color {
    color(palette::CROSSHAIR)
}
pub fn failure() -> Color {
    color(palette::FAILURE)
}
