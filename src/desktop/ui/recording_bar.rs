//! The small bar shown while recording: a red dot, the time, pause and stop.

use tiny_skia::Pixmap;

use super::canvas;
use super::geometry::Rect;
use super::text::Fonts;
use super::theme;

const BAR_WIDTH: f32 = 250.0;
const BAR_HEIGHT: f32 = 40.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarAction {
    Pause,
    Stop,
}

pub struct BarModel {
    pub elapsed_milliseconds: u64,
    pub paused: bool,
}

pub struct BarLayout {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub pause: Rect,
    pub stop: Rect,
}

/// "01:07" for 67 seconds.
pub fn format_time(milliseconds: u64) -> String {
    let seconds = milliseconds / 1000;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

impl BarLayout {
    pub fn new(fonts: &Fonts, scale: f32) -> BarLayout {
        let size = theme::LABEL_SIZE * scale;
        let height = BAR_HEIGHT * scale;
        let button_height = height - 12.0 * scale;
        let pause_width = fonts.width("resume", size) + 20.0 * scale;
        let stop_width = fonts.width("stop", size) + 20.0 * scale;
        let stop = Rect::new(BAR_WIDTH * scale - 6.0 * scale - stop_width, 6.0 * scale, stop_width, button_height);
        let pause = Rect::new(stop.x - 4.0 * scale - pause_width, 6.0 * scale, pause_width, button_height);
        BarLayout { width: (BAR_WIDTH * scale).round() as u32, height: height.round() as u32, scale, pause, stop }
    }

    pub fn action_at(&self, x: f32, y: f32) -> Option<BarAction> {
        if self.pause.contains(x, y) {
            Some(BarAction::Pause)
        } else if self.stop.contains(x, y) {
            Some(BarAction::Stop)
        } else {
            None
        }
    }
}

pub fn render(model: &BarModel, layout: &BarLayout, hovered: Option<BarAction>, fonts: &Fonts, pixmap: &mut Pixmap) {
    let scale = layout.scale;
    let size = theme::LABEL_SIZE * scale;
    let whole = Rect::new(0.0, 0.0, pixmap.width() as f32, pixmap.height() as f32);
    pixmap.fill(theme::ink());
    canvas::fill_rounded(pixmap, whole, theme::PANEL_RADIUS * scale, theme::ink_panel());
    canvas::stroke_rounded(pixmap, whole, theme::PANEL_RADIUS * scale, theme::rule(), 1.0);

    let middle = whole.height / 2.0;
    let dot_color = if model.paused { theme::text_dim() } else { theme::failure() };
    canvas::fill_circle(pixmap, (16.0 * scale, middle), 5.0 * scale, dot_color);
    let time_top = middle - fonts.line_height(size) / 2.0;
    fonts.draw(pixmap, &format_time(model.elapsed_milliseconds), 30.0 * scale, time_top, size, theme::text());

    let buttons = [
        (BarAction::Pause, layout.pause, if model.paused { "resume" } else { "pause" }),
        (BarAction::Stop, layout.stop, "stop"),
    ];
    for (action, rect, label) in buttons {
        if hovered == Some(action) {
            canvas::fill_rounded(pixmap, rect, theme::BUTTON_RADIUS * scale, theme::hover());
        } else {
            canvas::stroke_rounded(pixmap, rect, theme::BUTTON_RADIUS * scale, theme::rule(), 1.0);
        }
        let label_width = fonts.width(label, size);
        let color = if action == BarAction::Stop { theme::accent() } else { theme::text() };
        fonts.draw(pixmap, label, rect.x + (rect.width - label_width) / 2.0, middle - fonts.line_height(size) / 2.0, size, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::ui::testing;

    #[test]
    fn time_is_minutes_and_seconds() {
        assert_eq!(format_time(67_400), "01:07");
        assert_eq!(format_time(0), "00:00");
    }

    #[test]
    fn pause_and_stop_are_hit() {
        let fonts = Fonts::load();
        let layout = BarLayout::new(&fonts, 1.0);
        assert_eq!(layout.action_at(layout.stop.x + 2.0, layout.stop.y + 2.0), Some(BarAction::Stop));
        assert_eq!(layout.action_at(layout.pause.x + 2.0, layout.pause.y + 2.0), Some(BarAction::Pause));
        assert_eq!(layout.action_at(2.0, 2.0), None);
        assert!(layout.pause.x > 90.0, "the time has room");
    }

    #[test]
    fn renders_the_bar_to_png() {
        let fonts = Fonts::load();
        let layout = BarLayout::new(&fonts, 1.0);
        let mut pixmap = Pixmap::new(layout.width, layout.height).unwrap();
        render(&BarModel { elapsed_milliseconds: 83_000, paused: false }, &layout, None, &fonts, &mut pixmap);
        testing::write_preview("recording-bar", &pixmap);
    }
}
