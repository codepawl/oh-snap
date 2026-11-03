//! The trim window: a timeline with two handles over a recording's length, and the buttons to
//! save a copy of the part between them. No video frames, since the platforms offer none;
//! the timeline and the times are enough to cut by.

use std::time::Duration;

use tiny_skia::Pixmap;

use super::canvas;
use super::geometry::Rect;
use super::overlay::Key;
use super::text::Fonts;
use super::theme;

const WIDTH: f32 = 560.0;
const PADDING: f32 = 16.0;
const TITLE_HEIGHT: f32 = 40.0;
const TIMELINE_HEIGHT: f32 = 28.0;
const HANDLE_WIDTH: f32 = 10.0;
/// How near the pointer must be to take a handle.
const HANDLE_REACH: f32 = 14.0;
const BUTTON_HEIGHT: f32 = 32.0;
const SHORTEST_CUT: Duration = Duration::from_millis(200);
const KEY_STEP: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    Start,
    End,
}

/// The part being kept. It knows nothing about windows, so it can be tested without a screen.
#[derive(Debug)]
pub struct Range {
    length: Duration,
    start: Duration,
    end: Duration,
}

impl Range {
    pub fn new(length: Duration) -> Range {
        Range { length, start: Duration::ZERO, end: length }
    }

    /// Moves a handle, keeping it inside the recording and clear of the other handle.
    pub fn set(&mut self, handle: Handle, time: Duration) {
        match handle {
            Handle::Start => self.start = time.min(self.end.saturating_sub(SHORTEST_CUT)),
            Handle::End => self.end = time.clamp((self.start + SHORTEST_CUT).min(self.length), self.length),
        }
    }

    pub fn time_of(&self, handle: Handle) -> Duration {
        match handle {
            Handle::Start => self.start,
            Handle::End => self.end,
        }
    }

    pub fn kept(&self) -> Duration {
        self.end.saturating_sub(self.start)
    }

    pub fn is_whole(&self) -> bool {
        self.start.is_zero() && self.end == self.length
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrimButton {
    Keep,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrimOutcome {
    Keep,
    Cancelled,
}

/// "01:07.3": minutes, seconds and tenths.
pub fn format_time(time: Duration) -> String {
    let tenths = time.as_millis() / 100;
    format!("{:02}:{:02}.{}", tenths / 600, tenths / 10 % 60, tenths % 10)
}

pub struct TrimLayout {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub track: Rect,
    pub buttons: Vec<(TrimButton, Rect)>,
}

impl TrimLayout {
    pub fn new(fonts: &Fonts, scale: f32) -> TrimLayout {
        let size = theme::LABEL_SIZE * scale;
        let padding = PADDING * scale;
        let width = WIDTH * scale;
        let track = Rect::new(padding + HANDLE_WIDTH * scale, TITLE_HEIGHT * scale + padding, width - padding * 2.0 - HANDLE_WIDTH * scale * 2.0, TIMELINE_HEIGHT * scale);
        let buttons_top = track.bottom() + 40.0 * scale;

        let mut right = width - padding;
        let mut buttons = Vec::new();
        for (button, key, name) in [(TrimButton::Keep, "[enter]", " save a copy"), (TrimButton::Cancel, "[esc]", " cancel")] {
            let button_width = fonts.width(key, size) + fonts.width(name, size) + 20.0 * scale;
            buttons.push((button, Rect::new(right - button_width, buttons_top, button_width, BUTTON_HEIGHT * scale)));
            right -= button_width + 4.0 * scale;
        }
        TrimLayout { width: width.round() as u32, height: (buttons_top + BUTTON_HEIGHT * scale + padding).round() as u32, scale, track, buttons }
    }

    pub fn button_at(&self, x: f32, y: f32) -> Option<TrimButton> {
        self.buttons.iter().find(|(_, rect)| rect.contains(x, y)).map(|(button, _)| *button)
    }

    fn x_of(&self, time: Duration, length: Duration) -> f32 {
        let share = time.as_secs_f32() / length.as_secs_f32().max(0.001);
        self.track.x + self.track.width * share.clamp(0.0, 1.0)
    }

    fn time_at(&self, x: f32, length: Duration) -> Duration {
        let share = ((x - self.track.x) / self.track.width).clamp(0.0, 1.0);
        Duration::from_secs_f32(length.as_secs_f32() * share)
    }
}

pub struct TrimState {
    range: Range,
    layout: TrimLayout,
    name: String,
    active: Handle,
    dragging: bool,
    pointer: (f32, f32),
    outcome: Option<TrimOutcome>,
}

impl TrimState {
    pub fn new(fonts: &Fonts, scale: f32, length: Duration, name: String) -> TrimState {
        TrimState { range: Range::new(length), layout: TrimLayout::new(fonts, scale), name, active: Handle::Start, dragging: false, pointer: (-1.0, -1.0), outcome: None }
    }

    pub fn layout(&self) -> &TrimLayout {
        &self.layout
    }

    pub fn range(&self) -> &Range {
        &self.range
    }

    pub fn outcome(&self) -> Option<TrimOutcome> {
        self.outcome
    }

    /// The kept part as seconds from the start, for `Platform::trim`.
    pub fn seconds(&self) -> (f64, f64) {
        (self.range.start.as_secs_f64(), self.range.end.as_secs_f64())
    }

    /// The handle under the pointer, the nearer one when both are close.
    fn handle_near(&self, x: f32, y: f32) -> Option<Handle> {
        let track = self.layout.track;
        if y < track.y - 4.0 * self.layout.scale || y > track.bottom() + 4.0 * self.layout.scale {
            return None;
        }
        let to_start = (x - self.layout.x_of(self.range.start, self.range.length)).abs();
        let to_end = (x - self.layout.x_of(self.range.end, self.range.length)).abs();
        if to_start.min(to_end) > HANDLE_REACH * self.layout.scale {
            return None;
        }
        Some(if to_start <= to_end { Handle::Start } else { Handle::End })
    }

    pub fn mouse_moved(&mut self, x: f32, y: f32) {
        self.pointer = (x, y);
        if self.dragging {
            let time = self.layout.time_at(x, self.range.length);
            self.range.set(self.active, time);
        }
    }

    pub fn mouse_pressed(&mut self) {
        let (x, y) = self.pointer;
        match self.layout.button_at(x, y) {
            Some(TrimButton::Keep) => self.outcome = Some(TrimOutcome::Keep),
            Some(TrimButton::Cancel) => self.outcome = Some(TrimOutcome::Cancelled),
            None => {
                if let Some(handle) = self.handle_near(x, y) {
                    self.active = handle;
                    self.dragging = true;
                    self.mouse_moved(x, y);
                }
            }
        }
    }

    pub fn mouse_released(&mut self) {
        self.dragging = false;
    }

    pub fn key_pressed(&mut self, key: Key) {
        match key {
            Key::Enter => self.outcome = Some(TrimOutcome::Keep),
            Key::Escape => self.outcome = Some(TrimOutcome::Cancelled),
            Key::Char(character) if character.eq_ignore_ascii_case(&'i') => self.active = Handle::Start,
            Key::Char(character) if character.eq_ignore_ascii_case(&'o') => self.active = Handle::End,
            Key::Left => {
                let time = self.range.time_of(self.active).saturating_sub(KEY_STEP);
                self.range.set(self.active, time);
            }
            Key::Right => {
                let time = self.range.time_of(self.active) + KEY_STEP;
                self.range.set(self.active, time);
            }
            _ => {}
        }
    }

    pub fn render(&self, fonts: &Fonts, pixmap: &mut Pixmap) {
        let scale = self.layout.scale;
        let size = theme::LABEL_SIZE * scale;
        let padding = PADDING * scale;
        let whole = Rect::new(0.0, 0.0, pixmap.width() as f32, pixmap.height() as f32);
        canvas::fill_rect(pixmap, whole, theme::ink());
        canvas::stroke_rounded(pixmap, whole, 0.0, theme::rule(), 1.0);

        canvas::spark(pixmap, (padding + 8.0 * scale, TITLE_HEIGHT * scale / 2.0), 8.0 * scale, theme::accent());
        fonts.draw(pixmap, "trim", padding + 26.0 * scale, 11.0 * scale, size, theme::text());
        let name_width = fonts.width(&self.name, size);
        fonts.draw(pixmap, &self.name, whole.width - padding - name_width, 11.0 * scale, size, theme::text_dim());

        let track = self.layout.track;
        let length = self.range.length;
        let start_x = self.layout.x_of(self.range.start, length);
        let end_x = self.layout.x_of(self.range.end, length);
        canvas::fill_rounded(pixmap, track, 4.0 * scale, theme::hover());
        canvas::fill_rect(pixmap, Rect::new(start_x, track.y, end_x - start_x, track.height), theme::accent_soft());
        for (handle, x) in [(Handle::Start, start_x), (Handle::End, end_x)] {
            let rect = Rect::new(x - HANDLE_WIDTH * scale / 2.0, track.y - 3.0 * scale, HANDLE_WIDTH * scale, track.height + 6.0 * scale);
            let color = if handle == self.active { theme::accent() } else { theme::text() };
            canvas::fill_rounded(pixmap, rect, 3.0 * scale, color);
        }

        let times_top = track.bottom() + 10.0 * scale;
        fonts.draw(pixmap, &format_time(self.range.start), track.x - HANDLE_WIDTH * scale, times_top, size, theme::text_dim());
        let end_text = format_time(self.range.end);
        fonts.draw(pixmap, &end_text, track.right() + HANDLE_WIDTH * scale - fonts.width(&end_text, size), times_top, size, theme::text_dim());
        let kept_text = format!("keeps {}", format_time(self.range.kept()));
        fonts.draw(pixmap, &kept_text, (whole.width - fonts.width(&kept_text, size)) / 2.0, times_top, size, theme::text());

        let hovered = self.layout.button_at(self.pointer.0, self.pointer.1);
        for &(button, rect) in &self.layout.buttons {
            let primary = button == TrimButton::Keep;
            if primary {
                canvas::fill_rounded(pixmap, rect, theme::BUTTON_RADIUS * scale, theme::accent_soft());
            } else if hovered == Some(button) {
                canvas::fill_rounded(pixmap, rect, theme::BUTTON_RADIUS * scale, theme::hover());
            }
            let (key, name) = if primary { ("[enter]", " save a copy") } else { ("[esc]", " cancel") };
            let (key_color, name_color) = if primary { (theme::accent(), theme::accent()) } else { (theme::text_dim(), theme::text()) };
            let text_top = rect.y + (rect.height - fonts.line_height(size)) / 2.0;
            fonts.draw(pixmap, key, rect.x + 10.0 * scale, text_top, size, key_color);
            fonts.draw(pixmap, name, rect.x + 10.0 * scale + fonts.width(key, size), text_top, size, name_color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::ui::testing;

    fn seconds(value: u64) -> Duration {
        Duration::from_secs(value)
    }

    fn state(length: Duration) -> (TrimState, Fonts) {
        let fonts = Fonts::load();
        (TrimState::new(&fonts, 1.0, length, "recording-2026.mp4".into()), fonts)
    }

    #[test]
    fn a_new_range_keeps_everything() {
        let range = Range::new(seconds(10));
        assert!(range.is_whole());
        assert_eq!(range.kept(), seconds(10));
    }

    #[test]
    fn handles_cannot_cross_or_leave_the_recording() {
        let mut range = Range::new(seconds(10));
        range.set(Handle::Start, seconds(99));
        assert_eq!(range.time_of(Handle::Start), seconds(10) - SHORTEST_CUT);
        range.set(Handle::End, Duration::ZERO);
        assert_eq!(range.time_of(Handle::End), seconds(10));
        range.set(Handle::Start, seconds(4));
        range.set(Handle::End, seconds(2));
        assert_eq!(range.time_of(Handle::End), seconds(4) + SHORTEST_CUT);
        assert!(!range.is_whole());
    }

    #[test]
    fn time_and_position_on_the_track_agree() {
        let (state, _) = state(seconds(20));
        let layout = state.layout();
        let x = layout.x_of(seconds(5), seconds(20));
        assert!((layout.time_at(x, seconds(20)).as_secs_f32() - 5.0).abs() < 0.01);
        assert_eq!(layout.time_at(-100.0, seconds(20)), Duration::ZERO);
    }

    #[test]
    fn keys_move_the_active_handle_by_a_tenth_and_enter_keeps() {
        let (mut state, _) = state(seconds(10));
        state.key_pressed(Key::Char('i'));
        state.key_pressed(Key::Right);
        state.key_pressed(Key::Right);
        assert_eq!(state.range().time_of(Handle::Start), Duration::from_millis(200));
        state.key_pressed(Key::Char('O'));
        state.key_pressed(Key::Left);
        assert_eq!(state.range().time_of(Handle::End), Duration::from_millis(9900));
        state.key_pressed(Key::Enter);
        assert_eq!(state.outcome(), Some(TrimOutcome::Keep));
        assert_eq!(state.seconds(), (0.2, 9.9));
    }

    #[test]
    fn escape_cancels() {
        let (mut state, _) = state(seconds(10));
        state.key_pressed(Key::Escape);
        assert_eq!(state.outcome(), Some(TrimOutcome::Cancelled));
    }

    #[test]
    fn dragging_a_handle_follows_the_pointer_and_the_nearer_handle_wins() {
        let (mut state, _) = state(seconds(10));
        let track = state.layout().track;
        let middle = track.y + track.height / 2.0;
        state.mouse_moved(track.right() - 2.0, middle);
        state.mouse_pressed();
        state.mouse_moved(track.x + track.width * 0.6, middle);
        state.mouse_released();
        assert!((state.range().time_of(Handle::End).as_secs_f32() - 6.0).abs() < 0.05);
        assert_eq!(state.range().time_of(Handle::Start), Duration::ZERO);
        state.mouse_moved(track.x + track.width * 0.3, middle + 100.0);
        state.mouse_pressed();
        state.mouse_moved(track.x + track.width * 0.4, middle);
        assert_eq!(state.range().time_of(Handle::Start), Duration::ZERO, "too far from the track to take a handle");
    }

    #[test]
    fn the_buttons_answer() {
        let (mut state, _) = state(seconds(10));
        let keep = state.layout().buttons.iter().find(|(button, _)| *button == TrimButton::Keep).unwrap().1;
        state.mouse_moved(keep.x + 5.0, keep.y + 5.0);
        state.mouse_pressed();
        assert_eq!(state.outcome(), Some(TrimOutcome::Keep));
    }

    #[test]
    fn times_read_as_minutes_seconds_and_tenths() {
        assert_eq!(format_time(Duration::from_millis(67_300)), "01:07.3");
        assert_eq!(format_time(Duration::ZERO), "00:00.0");
    }

    #[test]
    fn renders_the_trim_window_to_png() {
        let (mut state, fonts) = state(seconds(42));
        state.key_pressed(Key::Char('i'));
        for _ in 0..80 {
            state.key_pressed(Key::Right);
        }
        state.key_pressed(Key::Char('o'));
        for _ in 0..120 {
            state.key_pressed(Key::Left);
        }
        let layout = state.layout();
        let mut pixmap = Pixmap::new(layout.width, layout.height).unwrap();
        state.render(&fonts, &mut pixmap);
        testing::write_preview("trim-window", &pixmap);
    }
}
