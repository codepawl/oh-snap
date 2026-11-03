//! The snipping overlay: one frozen screenshot per screen with a shade, a selection, the
//! `[r] region ...` toolbar, a crosshair and the colour loupe. `OverlayState` takes events in
//! global desktop pixels and ends in one `Outcome`; `render` draws one screen of it.

use tiny_skia::Pixmap;

use super::canvas;
use super::geometry::Rect;
use super::text::Fonts;
use super::theme;
use crate::desktop::platform::{Screen, WindowInfo};
use crate::mode::{Mode, Tool};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarItem {
    Mode(Mode),
    Tool(Tool),
}

pub const TOOLBAR_ITEMS: [(ToolbarItem, &str, &str); 8] = [
    (ToolbarItem::Mode(Mode::Region), "r", "region"),
    (ToolbarItem::Mode(Mode::Window), "w", "window"),
    (ToolbarItem::Mode(Mode::Screen), "s", "screen"),
    (ToolbarItem::Mode(Mode::Freeform), "f", "free"),
    (ToolbarItem::Tool(Tool::Draw), "d", "draw"),
    (ToolbarItem::Tool(Tool::Record), "v", "video"),
    (ToolbarItem::Tool(Tool::Text), "t", "text"),
    (ToolbarItem::Tool(Tool::Color), "c", "color"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Escape,
    Enter,
    Left,
    Right,
    Up,
    Down,
    Backspace,
    Char(char),
}

#[derive(Clone, Copy, Debug)]
pub struct OverlayOptions {
    /// Draw, video, text and colour next to the four modes.
    pub extra_tools: bool,
    pub snap_to_windows: bool,
    pub show_loupe: bool,
    pub show_crosshair: bool,
}

/// What was picked, in the pixels of one screen's image.
#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub screen: usize,
    /// x, y, width, height inside that screen's image.
    pub area: (i32, i32, u32, u32),
    /// Freeform outline in the pixels of the cropped area, for `output::keep_inside`.
    pub outline: Option<Vec<(f32, f32)>>,
    pub tool: Tool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Cancelled,
    Color([u8; 3]),
    Picked(Pick),
}

type Area = (i32, i32, i32, i32);

fn intersect(first: Area, second: Area) -> Option<Area> {
    let left = first.0.max(second.0);
    let top = first.1.max(second.1);
    let right = (first.0 + first.2).min(second.0 + second.2);
    let bottom = (first.1 + first.3).min(second.1 + second.3);
    (right > left && bottom > top).then_some((left, top, right - left, bottom - top))
}

// ---------------------------------------------------------------- toolbar

pub struct ToolbarButton {
    pub item: ToolbarItem,
    pub rect: Rect,
    pub key: String,
    pub name: String,
    pub key_width: f32,
}

/// `✻  [r] region  [w] window ...  [esc] close`, top centre of a screen, in that screen's pixels.
pub struct Toolbar {
    pub bar: Rect,
    pub spark_center: (f32, f32),
    pub buttons: Vec<ToolbarButton>,
    pub close: Rect,
    pub close_key_width: f32,
    pub text_size: f32,
    pub padding_x: f32,
    pub padding_y: f32,
    pub scale: f32,
}

impl Toolbar {
    pub fn layout(fonts: &Fonts, extra_tools: bool, screen_width: f32, scale: f32) -> Toolbar {
        let text_size = theme::LABEL_SIZE * scale;
        let (padding_x, padding_y, gap, inset) = (10.0 * scale, 6.0 * scale, 2.0 * scale, 4.0 * scale);
        let spark_slot = 34.0 * scale;
        let close_gap = 10.0 * scale;
        let button_height = fonts.line_height(text_size) + padding_y * 2.0;

        let entries: Vec<(ToolbarItem, String, String)> = TOOLBAR_ITEMS
            .iter()
            .filter(|(item, _, _)| extra_tools || matches!(item, ToolbarItem::Mode(_)))
            .map(|(item, key, name)| (*item, format!("[{key}]"), format!(" {name}")))
            .collect();
        let button_width = |key: &str, name: &str| fonts.width(key, text_size) + fonts.width(name, text_size) + padding_x * 2.0;
        let close_width = button_width("[esc]", " close");
        let tools_gap = if extra_tools { close_gap - gap } else { 0.0 };
        let buttons_width: f32 = entries.iter().map(|(_, key, name)| button_width(key, name)).sum::<f32>() + gap * (entries.len() as f32 - 1.0);
        let total_width = spark_slot + buttons_width + tools_gap + close_gap + close_width + inset * 2.0;

        let bar = Rect::new(((screen_width - total_width) / 2.0).round(), 16.0 * scale, total_width, button_height + inset * 2.0);
        let mut left = bar.x + inset + spark_slot;
        let mut buttons = Vec::new();
        for (item, key, name) in entries {
            if item == ToolbarItem::Tool(Tool::Draw) {
                left += tools_gap;
            }
            let width = button_width(&key, &name);
            let key_width = fonts.width(&key, text_size);
            buttons.push(ToolbarButton { item, rect: Rect::new(left, bar.y + inset, width, button_height), key, name, key_width });
            left += width + gap;
        }
        let close = Rect::new(left - gap + close_gap, bar.y + inset, close_width, button_height);
        Toolbar {
            bar,
            spark_center: (bar.x + inset + spark_slot / 2.0, bar.y + bar.height / 2.0),
            buttons,
            close,
            close_key_width: fonts.width("[esc]", text_size),
            text_size,
            padding_x,
            padding_y,
            scale,
        }
    }

    pub fn item_at(&self, x: f32, y: f32) -> Option<ToolbarItem> {
        self.buttons.iter().find(|button| button.rect.contains(x, y)).map(|button| button.item)
    }

    pub fn over_close(&self, x: f32, y: f32) -> bool {
        self.close.contains(x, y)
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.bar.contains(x, y)
    }
}

// ---------------------------------------------------------------- state

struct Drag {
    screen: usize,
    start: (i32, i32),
}

pub struct OverlayState {
    screens: Vec<Screen>,
    frames: Vec<Pixmap>,
    windows: Vec<WindowInfo>,
    options: OverlayOptions,
    pub mode: Mode,
    pub tool: Tool,
    cursor: (i32, i32),
    drag: Option<Drag>,
    outline: Vec<(i32, i32)>,
    armed: bool,
    outcome: Option<Outcome>,
}

impl OverlayState {
    pub fn new(screens: Vec<Screen>, windows: Vec<WindowInfo>, options: OverlayOptions, mode: Mode, tool: Tool) -> OverlayState {
        let frames = screens
            .iter()
            .map(|screen| canvas::pixmap_from_image(&screen.image).unwrap_or_else(|| Pixmap::new(1, 1).expect("1x1 pixmap")))
            .collect();
        let first = screens.first().map_or((0, 0), |screen| (screen.x + screen.image.width as i32 / 2, screen.y + screen.image.height as i32 / 2));
        OverlayState { screens, frames, windows, options, mode, tool, cursor: first, drag: None, outline: Vec::new(), armed: false, outcome: None }
    }

    pub fn screens(&self) -> &[Screen] {
        &self.screens
    }

    pub fn into_screens(self) -> Vec<Screen> {
        self.screens
    }

    pub fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }

    pub fn cursor(&self) -> (i32, i32) {
        self.cursor
    }

    fn screen_area(&self, index: usize) -> Area {
        let screen = &self.screens[index];
        (screen.x, screen.y, screen.image.width as i32, screen.image.height as i32)
    }

    /// The screen under a point, or the nearest one when the point is in a gap.
    pub fn screen_at(&self, point: (i32, i32)) -> usize {
        let distance = |index: usize| {
            let (x, y, width, height) = self.screen_area(index);
            let dx = (x - point.0).max(0).max(point.0 - (x + width - 1));
            let dy = (y - point.1).max(0).max(point.1 - (y + height - 1));
            i64::from(dx) * i64::from(dx) + i64::from(dy) * i64::from(dy)
        };
        (0..self.screens.len()).min_by_key(|&index| distance(index)).unwrap_or(0)
    }

    fn to_local(&self, index: usize, point: (i32, i32)) -> (f32, f32) {
        ((point.0 - self.screens[index].x) as f32, (point.1 - self.screens[index].y) as f32)
    }

    fn toolbar(&self, fonts: &Fonts) -> (Toolbar, usize) {
        let index = self.screen_at(self.cursor);
        let screen = &self.screens[index];
        (Toolbar::layout(fonts, self.options.extra_tools, screen.image.width as f32, screen.scale), index)
    }

    /// The window or screen the click would take, in global pixels.
    pub fn highlight(&self) -> Option<Area> {
        match self.mode {
            Mode::Window if self.options.snap_to_windows => self
                .windows
                .iter()
                .find(|window| {
                    window.width > 0
                        && window.height > 0
                        && self.cursor.0 >= window.x
                        && self.cursor.0 < window.x + window.width as i32
                        && self.cursor.1 >= window.y
                        && self.cursor.1 < window.y + window.height as i32
                })
                .map(|window| (window.x, window.y, window.width as i32, window.height as i32)),
            Mode::Screen => Some(self.screen_area(self.screen_at(self.cursor))),
            _ => None,
        }
    }

    /// The rectangle being dragged, kept inside the screen where the drag began.
    pub fn drag_rect(&self) -> Option<Area> {
        let drag = self.drag.as_ref()?;
        if self.mode != Mode::Region {
            return None;
        }
        let corner = Rect::from_corners((drag.start.0 as f32, drag.start.1 as f32), (self.cursor.0 as f32, self.cursor.1 as f32));
        let rough = (corner.x as i32, corner.y as i32, corner.width as i32, corner.height as i32);
        intersect(rough, self.screen_area(drag.screen))
    }

    fn pick_area(&mut self, area: Area, outline: Option<&[(i32, i32)]>) {
        let screen = self.screen_at((area.0 + area.2 / 2, area.1 + area.3 / 2));
        let Some(clipped) = intersect(area, self.screen_area(screen)) else {
            return;
        };
        let local = (clipped.0 - self.screens[screen].x, clipped.1 - self.screens[screen].y, clipped.2 as u32, clipped.3 as u32);
        let outline = outline.map(|points| points.iter().map(|point| ((point.0 - clipped.0) as f32, (point.1 - clipped.1) as f32)).collect());
        self.outcome = Some(Outcome::Picked(Pick { screen, area: local, outline, tool: self.tool }));
    }

    pub fn pixel_at(&self, point: (i32, i32)) -> [u8; 3] {
        let index = self.screen_at(point);
        let image = &self.screens[index].image;
        let x = (point.0 - self.screens[index].x).clamp(0, image.width as i32 - 1) as usize;
        let y = (point.1 - self.screens[index].y).clamp(0, image.height as i32 - 1) as usize;
        let offset = (y * image.width as usize + x) * 4;
        [image.bgra[offset + 2], image.bgra[offset + 1], image.bgra[offset]]
    }

    fn pick_color(&mut self) {
        self.outcome = Some(Outcome::Color(self.pixel_at(self.cursor)));
    }

    fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        if self.tool == Tool::Color {
            self.tool = Tool::Snip;
        }
        self.drag = None;
        self.outline.clear();
    }

    fn toggle_tool(&mut self, tool: Tool) {
        self.tool = if self.tool == tool { Tool::Snip } else { tool };
    }

    fn choose(&mut self, item: ToolbarItem) {
        match item {
            ToolbarItem::Mode(mode) => self.set_mode(mode),
            ToolbarItem::Tool(tool) => self.toggle_tool(tool),
        }
    }

    pub fn mouse_moved(&mut self, x: i32, y: i32) {
        self.cursor = (x, y);
        if self.drag.is_some() && self.mode == Mode::Freeform {
            let far_enough = self.outline.last().is_none_or(|last| (last.0 - x).abs() + (last.1 - y).abs() >= 2);
            if far_enough {
                self.outline.push((x, y));
            }
        }
    }

    pub fn mouse_pressed(&mut self, fonts: &Fonts) {
        if self.outcome.is_some() {
            return;
        }
        let (toolbar, index) = self.toolbar(fonts);
        let (x, y) = self.to_local(index, self.cursor);
        if toolbar.over_close(x, y) {
            self.outcome = Some(Outcome::Cancelled);
        } else if let Some(item) = toolbar.item_at(x, y) {
            self.choose(item);
        } else if toolbar.contains(x, y) {
            // The bar's own margin does not start a selection.
        } else if self.tool == Tool::Color {
            self.pick_color();
        } else if matches!(self.mode, Mode::Region | Mode::Freeform) {
            self.drag = Some(Drag { screen: self.screen_at(self.cursor), start: self.cursor });
            self.outline = vec![self.cursor];
        } else {
            self.armed = true;
        }
    }

    pub fn mouse_released(&mut self) {
        if self.outcome.is_some() {
            return;
        }
        match self.mode {
            Mode::Region => {
                let rect = self.drag_rect();
                self.drag = None;
                // A click without a drag selects nothing.
                if let Some(area) = rect.filter(|area| area.2 >= 3 && area.3 >= 3) {
                    self.pick_area(area, None);
                }
            }
            Mode::Freeform => {
                let points = std::mem::take(&mut self.outline);
                if self.drag.take().is_some() && points.len() >= 3 {
                    let xs = points.iter().map(|point| point.0);
                    let ys = points.iter().map(|point| point.1);
                    let (left, right) = (xs.clone().min().unwrap_or(0), xs.max().unwrap_or(0));
                    let (top, bottom) = (ys.clone().min().unwrap_or(0), ys.max().unwrap_or(0));
                    self.pick_area((left, top, (right - left).max(1), (bottom - top).max(1)), Some(&points));
                }
            }
            Mode::Window | Mode::Screen => {
                if std::mem::take(&mut self.armed) {
                    if let Some(area) = self.highlight() {
                        self.pick_area(area, None);
                    }
                }
            }
        }
    }

    pub fn key_pressed(&mut self, key: Key, fast: bool) {
        let step = if fast { 10 } else { 1 };
        match key {
            Key::Escape => self.outcome = Some(Outcome::Cancelled),
            Key::Enter if self.tool == Tool::Color => self.pick_color(),
            Key::Enter => {
                let area = self.drag_rect().or(self.highlight()).unwrap_or_else(|| self.screen_area(self.screen_at(self.cursor)));
                self.pick_area(area, None);
            }
            Key::Char(character) => self.character_pressed(character.to_ascii_lowercase()),
            Key::Left => self.nudge(-step, 0),
            Key::Right => self.nudge(step, 0),
            Key::Up => self.nudge(0, -step),
            Key::Down => self.nudge(0, step),
            Key::Backspace => {}
        }
    }

    fn character_pressed(&mut self, character: char) {
        let item = TOOLBAR_ITEMS
            .iter()
            .filter(|(item, _, _)| self.options.extra_tools || matches!(item, ToolbarItem::Mode(_)))
            .find(|(_, key, _)| key.starts_with(character))
            .map(|(item, _, _)| *item);
        match item {
            Some(ToolbarItem::Tool(Tool::Color)) => self.pick_color(),
            Some(item) => self.choose(item),
            None => {}
        }
    }

    fn nudge(&mut self, dx: i32, dy: i32) {
        self.mouse_moved(self.cursor.0 + dx, self.cursor.1 + dy);
    }

    // ------------------------------------------------------------ drawing

    /// Draws screen `index` into `pixmap`, which is that screen's size.
    pub fn render(&self, index: usize, fonts: &Fonts, pixmap: &mut Pixmap) {
        canvas::draw_fitted(pixmap, &self.frames[index]);
        let scale = self.screens[index].scale;
        let (width, height) = (pixmap.width() as f32, pixmap.height() as f32);
        let cursor_here = self.screen_at(self.cursor) == index;
        let (cursor_x, cursor_y) = self.to_local(index, self.cursor);

        let hole = if self.tool == Tool::Color {
            None
        } else {
            self.drag_rect().or(if cursor_here { self.highlight() } else { None }).and_then(|area| intersect(area, self.screen_area(index)))
        };
        if self.tool != Tool::Color {
            self.draw_shade(pixmap, index, hole);
        }
        if let Some(area) = hole {
            let local = Rect::new((area.0 - self.screens[index].x) as f32, (area.1 - self.screens[index].y) as f32, area.2 as f32, area.3 as f32);
            canvas::stroke_rounded(pixmap, local, 0.0, theme::accent(), 2.0 * scale);
            self.draw_size_label(pixmap, fonts, local, area, scale);
        }
        if self.mode == Mode::Freeform && self.outline.len() > 1 {
            let points: Vec<(f32, f32)> = self.outline.iter().map(|&point| self.to_local(index, point)).collect();
            canvas::polyline(pixmap, &points, theme::accent(), 2.0 * scale);
        }
        if self.options.show_crosshair && self.tool != Tool::Color && cursor_here {
            canvas::line(pixmap, (0.0, cursor_y + 0.5), (width, cursor_y + 0.5), theme::crosshair(), 1.0);
            canvas::line(pixmap, (cursor_x + 0.5, 0.0), (cursor_x + 0.5, height), theme::crosshair(), 1.0);
        }
        if cursor_here {
            let toolbar = Toolbar::layout(fonts, self.options.extra_tools, width, scale);
            self.draw_toolbar(pixmap, fonts, &toolbar, (cursor_x, cursor_y));
            if self.tool == Tool::Color {
                self.draw_loupe(pixmap, fonts, scale);
            }
        }
    }

    fn draw_shade(&self, pixmap: &mut Pixmap, index: usize, hole: Option<Area>) {
        let (width, height) = (pixmap.width() as f32, pixmap.height() as f32);
        let Some(area) = hole else {
            canvas::fill_rect(pixmap, Rect::new(0.0, 0.0, width, height), theme::shade());
            return;
        };
        let left = (area.0 - self.screens[index].x) as f32;
        let top = (area.1 - self.screens[index].y) as f32;
        let (right, bottom) = (left + area.2 as f32, top + area.3 as f32);
        let strips = [
            Rect::new(0.0, 0.0, width, top),
            Rect::new(0.0, bottom, width, height - bottom),
            Rect::new(0.0, top, left, bottom - top),
            Rect::new(right, top, width - right, bottom - top),
        ];
        for strip in strips.into_iter().filter(|strip| strip.width > 0.0 && strip.height > 0.0) {
            canvas::fill_rect(pixmap, strip, theme::shade());
        }
    }

    fn draw_size_label(&self, pixmap: &mut Pixmap, fonts: &Fonts, hole: Rect, area: Area, scale: f32) {
        let size = theme::LABEL_SIZE * scale;
        let text = format!("[ {} × {} ]", area.2, area.3);
        let padding = 6.0 * scale;
        let panel_width = fonts.width(&text, size) + padding * 2.0;
        let panel_height = fonts.line_height(size) + padding;
        let above = hole.y - panel_height - 6.0 * scale;
        let top = if above >= 0.0 { above } else { hole.y + 6.0 * scale };
        let panel = Rect::new(hole.x.max(0.0), top, panel_width, panel_height);
        canvas::fill_rounded(pixmap, panel, theme::BUTTON_RADIUS * scale, theme::ink_panel());
        fonts.draw(pixmap, &text, panel.x + padding, panel.y + padding / 2.0, size, theme::text());
    }

    fn draw_toolbar(&self, pixmap: &mut Pixmap, fonts: &Fonts, toolbar: &Toolbar, cursor: (f32, f32)) {
        let scale = toolbar.scale;
        canvas::fill_rounded(pixmap, toolbar.bar, theme::PANEL_RADIUS * scale, theme::ink_panel());
        canvas::stroke_rounded(pixmap, toolbar.bar, theme::PANEL_RADIUS * scale, theme::rule(), 1.0);
        canvas::spark(pixmap, toolbar.spark_center, 8.0 * scale, theme::accent());
        let hovered = toolbar.item_at(cursor.0, cursor.1);
        for button in &toolbar.buttons {
            let active = match button.item {
                ToolbarItem::Mode(mode) => mode == self.mode && self.tool != Tool::Color,
                ToolbarItem::Tool(tool) => tool == self.tool,
            };
            if active {
                canvas::fill_rounded(pixmap, button.rect, theme::BUTTON_RADIUS * scale, theme::accent_soft());
            } else if hovered == Some(button.item) {
                canvas::fill_rounded(pixmap, button.rect, theme::BUTTON_RADIUS * scale, theme::hover());
            }
            let (key_color, name_color) = if active { (theme::accent(), theme::accent()) } else { (theme::text_dim(), theme::text()) };
            let text_left = button.rect.x + toolbar.padding_x;
            let text_top = button.rect.y + toolbar.padding_y;
            fonts.draw(pixmap, &button.key, text_left, text_top, toolbar.text_size, key_color);
            fonts.draw(pixmap, &button.name, text_left + button.key_width, text_top, toolbar.text_size, name_color);
        }
        if toolbar.over_close(cursor.0, cursor.1) {
            canvas::fill_rounded(pixmap, toolbar.close, theme::BUTTON_RADIUS * scale, theme::hover());
        }
        let text_left = toolbar.close.x + toolbar.padding_x;
        let text_top = toolbar.close.y + toolbar.padding_y;
        fonts.draw(pixmap, "[esc]", text_left, text_top, toolbar.text_size, theme::text_dim());
        fonts.draw(pixmap, " close", text_left + toolbar.close_key_width, text_top, toolbar.text_size, theme::text());
    }

    /// A pixel-grid magnifier with the colour, its hex value and the position.
    fn draw_loupe(&self, pixmap: &mut Pixmap, fonts: &Fonts, scale: f32) {
        let index = self.screen_at(self.cursor);
        let (cursor_x, cursor_y) = self.to_local(index, self.cursor);
        let cells = theme::LOUPE_CELLS;
        let cell = theme::LOUPE_CELL_SIZE * scale;
        let padding = 6.0 * scale;
        let size = theme::LABEL_SIZE * scale;
        let grid = if self.options.show_loupe { cells as f32 * cell } else { 0.0 };
        let info_height = fonts.line_height(size);
        let color = self.pixel_at(self.cursor);
        let info = format!("{}  {},{}", crate::output::color_hex(color), self.cursor.0, self.cursor.1);
        let panel_width = grid.max(fonts.width(&info, size) + 18.0 * scale) + padding * 2.0;
        let panel_height = grid + info_height + padding * 2.0 + if grid > 0.0 { padding } else { 0.0 };

        let mut left = cursor_x + 24.0 * scale;
        let mut top = cursor_y + 24.0 * scale;
        if left + panel_width > pixmap.width() as f32 {
            left = cursor_x - 24.0 * scale - panel_width;
        }
        if top + panel_height > pixmap.height() as f32 {
            top = cursor_y - 24.0 * scale - panel_height;
        }
        let panel = Rect::new(left.round(), top.round(), panel_width.round(), panel_height.round());
        canvas::fill_rounded(pixmap, panel, theme::PANEL_RADIUS * scale, theme::ink_panel());
        canvas::stroke_rounded(pixmap, panel, theme::PANEL_RADIUS * scale, theme::rule(), 1.0);

        let mut text_top = panel.y + padding;
        if self.options.show_loupe {
            let grid_left = (panel.x + (panel_width - grid) / 2.0).round();
            for row in 0..cells {
                for column in 0..cells {
                    let sample = (self.cursor.0 + column - cells / 2, self.cursor.1 + row - cells / 2);
                    let [red, green, blue] = self.pixel_at(sample);
                    let fill = tiny_skia::Color::from_rgba8(red, green, blue, 255);
                    let rect = Rect::new(grid_left + column as f32 * cell, (panel.y + padding).round() + row as f32 * cell, cell, cell);
                    canvas::fill_rect(pixmap, rect, fill);
                }
            }
            let middle = Rect::new(grid_left + (cells / 2) as f32 * cell, panel.y + padding + (cells / 2) as f32 * cell, cell, cell);
            canvas::stroke_rounded(pixmap, middle, 0.0, theme::text(), 2.0 * scale);
            text_top += grid + padding;
        }
        let swatch = Rect::new(panel.x + padding, text_top + 2.0 * scale, 12.0 * scale, 12.0 * scale);
        canvas::fill_rounded(pixmap, swatch, 2.0 * scale, tiny_skia::Color::from_rgba8(color[0], color[1], color[2], 255));
        fonts.draw(pixmap, &info, swatch.right() + 6.0 * scale, text_top, size, theme::text());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::ui::testing;

    fn options() -> OverlayOptions {
        OverlayOptions { extra_tools: true, snap_to_windows: true, show_loupe: true, show_crosshair: true }
    }

    fn state(mode: Mode, tool: Tool) -> OverlayState {
        OverlayState::new(testing::two_screens(), testing::sample_windows(), options(), mode, tool)
    }

    fn drag(state: &mut OverlayState, fonts: &Fonts, from: (i32, i32), to: (i32, i32)) {
        state.mouse_moved(from.0, from.1);
        state.mouse_pressed(fonts);
        state.mouse_moved(to.0, to.1);
        state.mouse_released();
    }

    #[test]
    fn dragging_a_region_picks_it() {
        let fonts = Fonts::load();
        let mut state = state(Mode::Region, Tool::Snip);
        drag(&mut state, &fonts, (100, 100), (260, 190));
        let expected = Pick { screen: 0, area: (100, 100, 160, 90), outline: None, tool: Tool::Snip };
        assert_eq!(state.outcome(), Some(&Outcome::Picked(expected)));
    }

    #[test]
    fn a_region_on_the_second_screen_is_local_to_it() {
        let fonts = Fonts::load();
        let mut state = state(Mode::Region, Tool::Text);
        drag(&mut state, &fonts, (1000, 200), (1100, 300));
        let Some(Outcome::Picked(pick)) = state.outcome() else { panic!("nothing picked") };
        assert_eq!((pick.screen, pick.area, pick.tool), (1, (1000 - 800, 200, 100, 100), Tool::Text));
    }

    #[test]
    fn a_click_without_a_drag_selects_nothing() {
        let fonts = Fonts::load();
        let mut state = state(Mode::Region, Tool::Snip);
        drag(&mut state, &fonts, (300, 300), (301, 300));
        assert_eq!(state.outcome(), None);
    }

    #[test]
    fn window_mode_snaps_to_the_window_under_the_cursor() {
        let fonts = Fonts::load();
        let mut state = state(Mode::Window, Tool::Snip);
        let window = &testing::sample_windows()[0];
        state.mouse_moved(window.x + 20, window.y + 20);
        state.mouse_pressed(&fonts);
        state.mouse_released();
        let Some(Outcome::Picked(pick)) = state.outcome() else { panic!("nothing picked") };
        assert_eq!(pick.area, (window.x, window.y, window.width, window.height));
    }

    #[test]
    fn screen_mode_takes_the_whole_screen_under_the_cursor() {
        let fonts = Fonts::load();
        let mut state = state(Mode::Screen, Tool::Snip);
        state.mouse_moved(900, 300);
        state.mouse_pressed(&fonts);
        state.mouse_released();
        let Some(Outcome::Picked(pick)) = state.outcome() else { panic!("nothing picked") };
        assert_eq!((pick.screen, pick.area), (1, (0, 0, 800, 500)));
    }

    #[test]
    fn freeform_keeps_an_outline_inside_its_box() {
        let fonts = Fonts::load();
        let mut state = state(Mode::Freeform, Tool::Snip);
        state.mouse_moved(100, 100);
        state.mouse_pressed(&fonts);
        for point in [(150, 110), (160, 160), (110, 150)] {
            state.mouse_moved(point.0, point.1);
        }
        state.mouse_released();
        let Some(Outcome::Picked(pick)) = state.outcome() else { panic!("nothing picked") };
        assert_eq!(pick.area, (100, 100, 60, 60));
        assert_eq!(pick.outline.as_ref().map(Vec::len), Some(4));
    }

    #[test]
    fn keys_switch_modes_and_escape_cancels() {
        let mut state = state(Mode::Region, Tool::Snip);
        state.key_pressed(Key::Char('w'), false);
        assert_eq!(state.mode, Mode::Window);
        state.key_pressed(Key::Char('t'), false);
        assert_eq!(state.tool, Tool::Text);
        state.key_pressed(Key::Escape, false);
        assert_eq!(state.outcome(), Some(&Outcome::Cancelled));
    }

    #[test]
    fn the_color_tool_copies_the_pixel_under_the_cursor() {
        let fonts = Fonts::load();
        let mut state = state(Mode::Region, Tool::Color);
        state.mouse_moved(300, 300);
        let expected = state.pixel_at((300, 300));
        state.mouse_pressed(&fonts);
        assert_eq!(state.outcome(), Some(&Outcome::Color(expected)));
    }

    #[test]
    fn clicking_a_toolbar_button_changes_the_mode() {
        let fonts = Fonts::load();
        let mut state = state(Mode::Region, Tool::Snip);
        let toolbar = Toolbar::layout(&fonts, true, 800.0, 1.0);
        let screen_button = toolbar.buttons.iter().find(|button| button.item == ToolbarItem::Mode(Mode::Screen)).unwrap();
        state.mouse_moved((screen_button.rect.x + 5.0) as i32, (screen_button.rect.y + 5.0) as i32);
        state.mouse_pressed(&fonts);
        assert_eq!(state.mode, Mode::Screen);
        assert_eq!(state.outcome(), None);
    }

    #[test]
    fn the_toolbar_fits_and_close_is_last() {
        let fonts = Fonts::load();
        let toolbar = Toolbar::layout(&fonts, true, 1280.0, 1.0);
        assert!(toolbar.bar.x > 0.0 && toolbar.bar.right() < 1280.0);
        assert!(toolbar.close.x > toolbar.buttons.last().unwrap().rect.right());
        let plain = Toolbar::layout(&fonts, false, 1280.0, 1.0);
        assert_eq!(plain.buttons.len(), 4);
    }

    #[test]
    fn renders_the_overlay_states_to_png() {
        let fonts = Fonts::load();
        let wide = || {
            let screen = Screen { x: 0, y: 0, scale: 1.0, image: testing::sample_image(1280, 720) };
            OverlayState::new(vec![screen], testing::sample_windows(), options(), Mode::Region, Tool::Snip)
        };
        let mut pixmap = Pixmap::new(1280, 720).unwrap();

        let mut region = wide();
        region.mouse_moved(120, 140);
        region.mouse_pressed(&fonts);
        region.mouse_moved(520, 360);
        region.render(0, &fonts, &mut pixmap);
        testing::write_preview("overlay-region", &pixmap);

        let mut color = wide();
        color.tool = Tool::Color;
        color.mouse_moved(330, 260);
        color.render(0, &fonts, &mut pixmap);
        testing::write_preview("overlay-color", &pixmap);
    }
}
