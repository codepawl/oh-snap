//! Drawing on a snip right on the overlay, before it is kept: the screen stays frozen, the
//! picked area stays bright, and a bar next to it holds the tools. `Session` is the pure part
//! (what was drawn, what the pointer is doing, in image pixels); `MarkupState` puts it on a
//! screen. Annotations are drawn by one function, so the live view and the final image match.

use tiny_skia::{Color, Pixmap};

use super::canvas;
use super::geometry::Rect;
use super::overlay::Key;
use super::text::Fonts;
use super::theme;
use crate::output::Image;

pub type Point = (f32, f32);

const STROKE_UNITS: f32 = 2.0;
const TEXT_SIZE_PER_WIDTH: f32 = 7.0;
const SMALLEST_SHAPE: f32 = 3.0;
const MARKER_ALPHA: f32 = 0.4;
const MARKER_WIDTH_FACTOR: f32 = 5.0;
const SWATCH_SLOT: f32 = 24.0;
const GROUP_GAP: f32 = 12.0;

/// Accent first, then red, yellow, green and ink.
const COLOR_VALUES: [[u8; 3]; 5] = [[0xD9, 0x77, 0x57], [0xE5, 0x53, 0x4B], [0xF2, 0xC9, 0x4C], [0x4C, 0xC3, 0x8A], [0x1F, 0x1E, 0x1D]];

fn annotation_color(index: usize) -> Color {
    let [red, green, blue] = COLOR_VALUES[index.min(COLOR_VALUES.len() - 1)];
    Color::from_rgba8(red, green, blue, 255)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawTool {
    Arrow,
    Box,
    Pen,
    Marker,
    Text,
    Hide,
}

/// Toolbar order, the shortcut key and the name after the `[k]`.
pub const DRAW_TOOLS: [(DrawTool, char, &str); 6] = [
    (DrawTool::Arrow, 'a', "arrow"),
    (DrawTool::Box, 'b', "box"),
    (DrawTool::Pen, 'p', "pen"),
    (DrawTool::Marker, 'm', "mark"),
    (DrawTool::Text, 't', "text"),
    (DrawTool::Hide, 'h', "hide"),
];

#[derive(Clone, Debug, PartialEq)]
pub enum Annotation {
    Arrow { from: Point, to: Point, color: usize, width: f32 },
    Box { rect: Rect, color: usize, width: f32 },
    Stroke { points: Vec<Point>, color: usize, width: f32, marker: bool },
    Text { at: Point, text: String, color: usize, size: f32 },
    /// Shows the matching part of the pixelated copy.
    Hide { rect: Rect },
}

// ---------------------------------------------------------------- session

struct Drag {
    start: Point,
    points: Vec<Point>,
}

struct TextEntry {
    at: Point,
    text: String,
}

/// What the user has drawn so far and what the pointer is doing, in image pixels. It knows
/// nothing about windows, so it can be tested without a screen.
pub struct Session {
    size: (f32, f32),
    stroke_width: f32,
    tool: DrawTool,
    color: usize,
    annotations: Vec<Annotation>,
    undone: Vec<Annotation>,
    drag: Option<Drag>,
    entry: Option<TextEntry>,
}

impl Session {
    pub fn new(width: u32, height: u32) -> Session {
        let unit = (width.min(height) as f32 / 300.0).clamp(1.5, 4.0);
        Session {
            size: (width as f32, height as f32),
            stroke_width: unit * STROKE_UNITS,
            tool: DrawTool::Arrow,
            color: 0,
            annotations: Vec::new(),
            undone: Vec::new(),
            drag: None,
            entry: None,
        }
    }

    pub fn annotations(&self) -> &[Annotation] {
        &self.annotations
    }

    pub fn tool(&self) -> DrawTool {
        self.tool
    }

    pub fn color(&self) -> usize {
        self.color
    }

    pub fn set_color(&mut self, color: usize) {
        self.color = color.min(COLOR_VALUES.len() - 1);
    }

    pub fn is_typing(&self) -> bool {
        self.entry.is_some()
    }

    fn clamped(&self, point: Point) -> Point {
        (point.0.clamp(0.0, self.size.0), point.1.clamp(0.0, self.size.1))
    }

    pub fn contains(&self, point: Point) -> bool {
        point.0 >= 0.0 && point.1 >= 0.0 && point.0 < self.size.0 && point.1 < self.size.1
    }

    fn add(&mut self, annotation: Annotation) {
        self.annotations.push(annotation);
        self.undone.clear();
    }

    /// A press inside the snip starts a shape, or a piece of text with the text tool.
    pub fn press(&mut self, point: Point) {
        if !self.contains(point) {
            return;
        }
        self.commit_text();
        if self.tool == DrawTool::Text {
            self.entry = Some(TextEntry { at: point, text: String::new() });
            return;
        }
        self.drag = Some(Drag { start: point, points: vec![point] });
    }

    pub fn pointer_moved(&mut self, point: Point) {
        let point = self.clamped(point);
        if let Some(drag) = self.drag.as_mut() {
            drag.points.push(point);
        }
    }

    pub fn release(&mut self) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if let Some(annotation) = self.annotation_from(&drag) {
            self.add(annotation);
        }
    }

    /// What a drag makes, or nothing when it was too small. The live drag draws through this too.
    fn annotation_from(&self, drag: &Drag) -> Option<Annotation> {
        let end = *drag.points.last()?;
        let rect = Rect::from_corners(drag.start, end);
        let big_enough = rect.width >= SMALLEST_SHAPE && rect.height >= SMALLEST_SHAPE;
        let moved = (end.0 - drag.start.0).hypot(end.1 - drag.start.1) >= SMALLEST_SHAPE;
        let (width, color) = (self.stroke_width, self.color);
        match self.tool {
            DrawTool::Arrow if moved => Some(Annotation::Arrow { from: drag.start, to: end, color, width }),
            DrawTool::Box if big_enough => Some(Annotation::Box { rect, color, width }),
            DrawTool::Pen | DrawTool::Marker => Some(Annotation::Stroke { points: drag.points.clone(), color, width, marker: self.tool == DrawTool::Marker }),
            DrawTool::Hide if big_enough => Some(Annotation::Hide { rect }),
            _ => None,
        }
    }

    /// The shape being dragged, and the text being typed with a caret, for the live view.
    pub fn live_annotations(&self) -> Vec<Annotation> {
        let mut live = Vec::new();
        if let Some(annotation) = self.drag.as_ref().and_then(|drag| self.annotation_from(drag)) {
            live.push(annotation);
        }
        if let Some(entry) = &self.entry {
            live.push(Annotation::Text { at: entry.at, text: format!("{}|", entry.text), color: self.color, size: self.text_size() });
        }
        live
    }

    pub fn select_tool(&mut self, tool: DrawTool) {
        self.commit_text();
        self.tool = tool;
    }

    pub fn undo(&mut self) {
        self.commit_text();
        if let Some(annotation) = self.annotations.pop() {
            self.undone.push(annotation);
        }
    }

    pub fn redo(&mut self) {
        if let Some(annotation) = self.undone.pop() {
            self.annotations.push(annotation);
        }
    }

    fn text_size(&self) -> f32 {
        self.stroke_width * TEXT_SIZE_PER_WIDTH
    }

    pub fn type_character(&mut self, character: char) {
        if let Some(entry) = self.entry.as_mut().filter(|_| !character.is_control()) {
            entry.text.push(character);
        }
    }

    pub fn type_newline(&mut self) {
        if let Some(entry) = self.entry.as_mut() {
            entry.text.push('\n');
        }
    }

    pub fn type_backspace(&mut self) {
        if let Some(entry) = self.entry.as_mut() {
            entry.text.pop();
        }
    }

    pub fn cancel_typing(&mut self) {
        self.entry = None;
    }

    /// Keeps the text being typed, unless it is empty.
    pub fn commit_text(&mut self) {
        let Some(entry) = self.entry.take() else {
            return;
        };
        if entry.text.trim().is_empty() {
            return;
        }
        let size = self.text_size();
        self.add(Annotation::Text { at: entry.at, text: entry.text, color: self.color, size });
    }
}

// ---------------------------------------------------------------- drawing

/// A blocky copy of the image; `Hide` shows the matching part of it.
pub fn pixelate(image: &Image) -> Image {
    let block = (image.width.min(image.height) / 60).max(8) as usize;
    let (width, height) = (image.width as usize, image.height as usize);
    let mut output = image.bgra.clone();
    for block_top in (0..height).step_by(block) {
        for block_left in (0..width).step_by(block) {
            let bottom = (block_top + block).min(height);
            let right = (block_left + block).min(width);
            let count = ((bottom - block_top) * (right - block_left)) as u32;
            let mut sums = [0u32; 3];
            for row in block_top..bottom {
                for column in block_left..right {
                    let offset = (row * width + column) * 4;
                    for (channel, sum) in sums.iter_mut().enumerate() {
                        *sum += u32::from(image.bgra[offset + channel]);
                    }
                }
            }
            for row in block_top..bottom {
                for column in block_left..right {
                    let offset = (row * width + column) * 4;
                    for (channel, sum) in sums.iter().enumerate() {
                        output[offset + channel] = (sum / count) as u8;
                    }
                }
            }
        }
    }
    Image { width: image.width, height: image.height, bgra: output }
}

fn copy_region(target: &mut Pixmap, source: &Pixmap, rect: Rect) {
    let width = target.width() as usize;
    let left = rect.x.max(0.0) as usize;
    let right = (rect.right().max(0.0) as usize).min(width);
    let top = rect.y.max(0.0) as usize;
    let bottom = (rect.bottom().max(0.0) as usize).min(target.height() as usize);
    if right <= left {
        return;
    }
    for row in top..bottom {
        let range = (row * width + left) * 4..(row * width + right) * 4;
        target.data_mut()[range.clone()].copy_from_slice(&source.data()[range]);
    }
}

/// Draws one annotation onto the image-sized `pixmap`; `hidden` is the pixelated copy.
pub fn draw_annotation(pixmap: &mut Pixmap, hidden: &Pixmap, fonts: &Fonts, annotation: &Annotation) {
    match annotation {
        Annotation::Arrow { from, to, color, width } => draw_arrow(pixmap, *from, *to, annotation_color(*color), *width),
        Annotation::Box { rect, color, width } => {
            canvas::stroke_rounded(pixmap, rect.inset(-width / 2.0), width * 1.5, annotation_color(*color), *width);
        }
        Annotation::Stroke { points, color, width, marker } => {
            let mut paint_color = annotation_color(*color);
            let mut stroke_width = *width;
            if *marker {
                paint_color.set_alpha(MARKER_ALPHA);
                stroke_width *= MARKER_WIDTH_FACTOR;
            }
            match points.as_slice() {
                [only] => canvas::fill_circle(pixmap, *only, stroke_width / 2.0, paint_color),
                _ => canvas::polyline(pixmap, points, paint_color, stroke_width),
            }
        }
        Annotation::Text { at, text, color, size } => {
            let line_height = fonts.line_height(*size);
            for (index, line) in text.split('\n').enumerate() {
                fonts.draw(pixmap, line, at.0, at.1 + line_height * index as f32, *size, annotation_color(*color));
            }
        }
        Annotation::Hide { rect } => copy_region(pixmap, hidden, *rect),
    }
}

fn draw_arrow(pixmap: &mut Pixmap, from: Point, to: Point, color: Color, width: f32) {
    let length = (to.0 - from.0).hypot(to.1 - from.1);
    if length < 1.0 {
        return;
    }
    let direction = ((to.0 - from.0) / length, (to.1 - from.1) / length);
    let head_length = (width * 4.5).min(length * 0.6);
    let head_half_width = head_length * 0.55;
    let head_base = (to.0 - direction.0 * head_length, to.1 - direction.1 * head_length);
    let normal = (-direction.1, direction.0);
    canvas::line(pixmap, from, (head_base.0 + direction.0, head_base.1 + direction.1), color, width);
    let head = [
        to,
        (head_base.0 + normal.0 * head_half_width, head_base.1 + normal.1 * head_half_width),
        (head_base.0 - normal.0 * head_half_width, head_base.1 - normal.1 * head_half_width),
    ];
    canvas::fill_polygon(pixmap, &head, color);
}

/// The snip with every annotation burned in.
pub fn flatten(image: &Image, annotations: &[Annotation], fonts: &Fonts) -> Image {
    let flat = || Image { width: image.width, height: image.height, bgra: image.bgra.clone() };
    if annotations.is_empty() {
        return flat();
    }
    let (Some(mut pixmap), Some(hidden)) = (canvas::pixmap_from_image(image), canvas::pixmap_from_image(&pixelate(image))) else {
        return flat();
    };
    for annotation in annotations {
        draw_annotation(&mut pixmap, &hidden, fonts, annotation);
    }
    canvas::image_from_pixmap(&pixmap)
}

// ---------------------------------------------------------------- bar

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Tool(DrawTool),
    Color(usize),
    Undo,
    Done,
    Cancel,
}

pub struct BarButton {
    pub control: Control,
    pub rect: Rect,
    pub key: String,
    pub name: String,
    pub key_width: f32,
}

/// The tools, colours, undo, done and cancel in one row, in the screen's pixels.
pub struct MarkupBar {
    pub bar: Rect,
    pub buttons: Vec<BarButton>,
    pub swatches: Vec<(usize, Rect)>,
    pub text_size: f32,
    pub padding_x: f32,
    pub padding_y: f32,
    pub scale: f32,
}

enum Slot {
    Button(Control, String, String),
    Swatch(usize),
    Gap,
}

impl MarkupBar {
    /// Below `area` (above it, or inside it, when there is no room), centred, kept on the screen.
    pub fn layout(fonts: &Fonts, scale: f32, area: Rect, screen: (f32, f32)) -> MarkupBar {
        let text_size = theme::LABEL_SIZE * scale;
        let (padding_x, padding_y, inset) = (9.0 * scale, 6.0 * scale, 4.0 * scale);
        let button_height = fonts.line_height(text_size) + padding_y * 2.0;
        let height = button_height + inset * 2.0;

        let mut slots = Vec::new();
        for (tool, key, name) in DRAW_TOOLS {
            slots.push(Slot::Button(Control::Tool(tool), format!("[{key}]"), format!(" {name}")));
        }
        slots.push(Slot::Gap);
        slots.extend((0..COLOR_VALUES.len()).map(Slot::Swatch));
        slots.push(Slot::Gap);
        slots.push(Slot::Button(Control::Undo, String::new(), "undo".into()));
        slots.push(Slot::Gap);
        slots.push(Slot::Button(Control::Done, "[enter]".into(), " done".into()));
        slots.push(Slot::Button(Control::Cancel, "[esc]".into(), " cancel".into()));

        let slot_width = |slot: &Slot| match slot {
            Slot::Button(_, key, name) => fonts.width(key, text_size) + fonts.width(name, text_size) + padding_x * 2.0,
            Slot::Swatch(_) => SWATCH_SLOT * scale,
            Slot::Gap => GROUP_GAP * scale,
        };
        let width = slots.iter().map(slot_width).sum::<f32>() + inset * 2.0;

        let left = (area.x + (area.width - width) / 2.0).clamp(0.0, (screen.0 - width).max(0.0)).round();
        let margin = 8.0 * scale;
        let below = area.bottom() + margin;
        let top = if below + height <= screen.1 {
            below
        } else if area.y - margin - height >= 0.0 {
            area.y - margin - height
        } else {
            screen.1 - height - margin
        };
        let bar = Rect::new(left, top.round(), width, height);

        let mut cursor = bar.x + inset;
        let mut buttons = Vec::new();
        let mut swatches = Vec::new();
        for slot in slots {
            let slot_extent = slot_width(&slot);
            match slot {
                Slot::Button(control, key, name) => {
                    let key_width = fonts.width(&key, text_size);
                    buttons.push(BarButton { control, rect: Rect::new(cursor, bar.y + inset, slot_extent, button_height), key, name, key_width });
                }
                Slot::Swatch(index) => swatches.push((index, Rect::new(cursor, bar.y + inset, slot_extent, button_height))),
                Slot::Gap => {}
            }
            cursor += slot_extent;
        }
        MarkupBar { bar, buttons, swatches, text_size, padding_x, padding_y, scale }
    }

    pub fn control_at(&self, x: f32, y: f32) -> Option<Control> {
        let button = self.buttons.iter().find(|button| button.rect.contains(x, y)).map(|button| button.control);
        button.or_else(|| self.swatches.iter().find(|(_, rect)| rect.contains(x, y)).map(|(index, _)| Control::Color(*index)))
    }
}

// ---------------------------------------------------------------- state

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    Done,
    Cancelled,
}

/// The draw step of one picked area, on the screen it was picked on. Events come in that
/// screen's pixels.
pub struct MarkupState {
    frame: Pixmap,
    scale: f32,
    area: Rect,
    image: Image,
    base: Pixmap,
    hidden: Pixmap,
    session: Session,
    cursor: (f32, f32),
    ending: Option<Ending>,
}

impl MarkupState {
    /// `screen_image` is the whole frozen screen, `area` x, y, width, height inside it and
    /// `image` the pixels of that area.
    pub fn new(screen_image: &Image, scale: f32, area: (i32, i32, u32, u32), image: Image) -> Option<MarkupState> {
        Some(MarkupState {
            frame: canvas::pixmap_from_image(screen_image)?,
            scale,
            area: Rect::new(area.0 as f32, area.1 as f32, area.2 as f32, area.3 as f32),
            base: canvas::pixmap_from_image(&image)?,
            hidden: canvas::pixmap_from_image(&pixelate(&image))?,
            session: Session::new(image.width, image.height),
            image,
            cursor: (-1.0, -1.0),
            ending: None,
        })
    }

    pub fn ending(&self) -> Option<Ending> {
        self.ending
    }

    #[cfg(test)]
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// The snip with the drawing burned in, for `Ending::Done`.
    pub fn flattened(&self, fonts: &Fonts) -> Image {
        flatten(&self.image, self.session.annotations(), fonts)
    }

    fn bar(&self, fonts: &Fonts) -> MarkupBar {
        MarkupBar::layout(fonts, self.scale, self.area, (self.frame.width() as f32, self.frame.height() as f32))
    }

    fn to_image(&self, point: Point) -> Point {
        (point.0 - self.area.x, point.1 - self.area.y)
    }

    pub fn mouse_moved(&mut self, x: f32, y: f32) {
        self.cursor = (x, y);
        self.session.pointer_moved(self.to_image(self.cursor));
    }

    pub fn mouse_pressed(&mut self, fonts: &Fonts) {
        match self.bar(fonts).control_at(self.cursor.0, self.cursor.1) {
            Some(control) => self.activate(control),
            None => self.session.press(self.to_image(self.cursor)),
        }
    }

    pub fn mouse_released(&mut self) {
        self.session.release();
    }

    fn activate(&mut self, control: Control) {
        match control {
            Control::Tool(tool) => self.session.select_tool(tool),
            Control::Color(index) => self.session.set_color(index),
            Control::Undo => self.session.undo(),
            Control::Done => self.finish(Ending::Done),
            Control::Cancel => self.finish(Ending::Cancelled),
        }
    }

    /// Closing the window: drops the drawing, typed text included.
    pub fn cancel(&mut self) {
        self.ending = Some(Ending::Cancelled);
    }

    fn finish(&mut self, ending: Ending) {
        self.session.commit_text();
        self.ending = Some(ending);
    }

    /// `command` is Ctrl on Linux and Cmd on macOS.
    pub fn key_pressed(&mut self, key: Key, shift: bool, command: bool) {
        if self.session.is_typing() {
            match key {
                Key::Escape => self.session.cancel_typing(),
                Key::Enter if shift => self.session.type_newline(),
                Key::Enter => self.session.commit_text(),
                Key::Backspace => self.session.type_backspace(),
                Key::Char(character) if !command => self.session.type_character(character),
                _ => {}
            }
            return;
        }
        match key {
            Key::Escape => self.finish(Ending::Cancelled),
            Key::Enter => self.finish(Ending::Done),
            Key::Char(character) if command => match character.to_ascii_lowercase() {
                'z' if shift => self.session.redo(),
                'z' => self.session.undo(),
                'y' => self.session.redo(),
                _ => {}
            },
            Key::Char(character) => self.character_pressed(character.to_ascii_lowercase()),
            _ => {}
        }
    }

    fn character_pressed(&mut self, character: char) {
        if let Some((tool, _, _)) = DRAW_TOOLS.iter().find(|(_, key, _)| *key == character) {
            self.session.select_tool(*tool);
        } else if let Some(index) = character.to_digit(10).and_then(|digit| (digit as usize).checked_sub(1)).filter(|index| *index < COLOR_VALUES.len()) {
            self.session.set_color(index);
        }
    }

    pub fn render(&self, fonts: &Fonts, pixmap: &mut Pixmap) {
        canvas::draw_fitted(pixmap, &self.frame);
        self.draw_shade(pixmap);

        let mut layer = self.base.clone();
        for annotation in self.session.annotations().iter().chain(self.session.live_annotations().iter()) {
            draw_annotation(&mut layer, &self.hidden, fonts, annotation);
        }
        pixmap.draw_pixmap(self.area.x as i32, self.area.y as i32, layer.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
        canvas::stroke_rounded(pixmap, self.area.inset(-self.scale), 0.0, theme::accent(), self.scale);

        self.draw_bar(pixmap, fonts, &self.bar(fonts));
    }

    fn draw_shade(&self, pixmap: &mut Pixmap) {
        let (width, height) = (pixmap.width() as f32, pixmap.height() as f32);
        let hole = self.area;
        let strips = [
            Rect::new(0.0, 0.0, width, hole.y),
            Rect::new(0.0, hole.bottom(), width, height - hole.bottom()),
            Rect::new(0.0, hole.y, hole.x, hole.height),
            Rect::new(hole.right(), hole.y, width - hole.right(), hole.height),
        ];
        for strip in strips.into_iter().filter(|strip| strip.width > 0.0 && strip.height > 0.0) {
            canvas::fill_rect(pixmap, strip, theme::shade());
        }
    }

    fn draw_bar(&self, pixmap: &mut Pixmap, fonts: &Fonts, bar: &MarkupBar) {
        let scale = bar.scale;
        canvas::fill_rounded(pixmap, bar.bar, theme::PANEL_RADIUS * scale, theme::ink_panel());
        canvas::stroke_rounded(pixmap, bar.bar, theme::PANEL_RADIUS * scale, theme::rule(), 1.0);
        let hovered = bar.control_at(self.cursor.0, self.cursor.1);
        for button in &bar.buttons {
            let primary = button.control == Control::Done;
            let active = primary || button.control == Control::Tool(self.session.tool());
            if active {
                canvas::fill_rounded(pixmap, button.rect, theme::BUTTON_RADIUS * scale, theme::accent_soft());
            } else if hovered == Some(button.control) {
                canvas::fill_rounded(pixmap, button.rect, theme::BUTTON_RADIUS * scale, theme::hover());
            }
            let (key_color, name_color) = if active { (theme::accent(), theme::accent()) } else { (theme::text_dim(), theme::text()) };
            let text_left = button.rect.x + bar.padding_x;
            let text_top = button.rect.y + bar.padding_y;
            fonts.draw(pixmap, &button.key, text_left, text_top, bar.text_size, key_color);
            fonts.draw(pixmap, &button.name, text_left + button.key_width, text_top, bar.text_size, name_color);
        }
        for (index, rect) in &bar.swatches {
            let center = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
            if hovered == Some(Control::Color(*index)) {
                canvas::fill_circle(pixmap, center, 11.0 * scale, theme::hover());
            }
            canvas::fill_circle(pixmap, center, 7.0 * scale, annotation_color(*index));
            canvas::stroke_circle(pixmap, center, 7.0 * scale, theme::rule(), 1.0);
            if self.session.color() == *index {
                canvas::stroke_circle(pixmap, center, 10.0 * scale, theme::text(), 1.5 * scale);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::ui::testing;

    fn session() -> Session {
        Session::new(600, 400)
    }

    fn drag(session: &mut Session, from: Point, to: Point) {
        session.press(from);
        session.pointer_moved(to);
        session.release();
    }

    #[test]
    fn a_drag_makes_the_chosen_shape() {
        let mut markup = session();
        drag(&mut markup, (10.0, 10.0), (100.0, 80.0));
        markup.select_tool(DrawTool::Box);
        drag(&mut markup, (10.0, 10.0), (100.0, 80.0));
        markup.select_tool(DrawTool::Hide);
        drag(&mut markup, (10.0, 10.0), (100.0, 80.0));
        markup.select_tool(DrawTool::Pen);
        drag(&mut markup, (10.0, 10.0), (100.0, 80.0));
        let kinds = markup.annotations();
        assert!(matches!(kinds[0], Annotation::Arrow { .. }));
        assert!(matches!(kinds[1], Annotation::Box { .. }));
        assert!(matches!(kinds[2], Annotation::Hide { .. }));
        assert!(matches!(&kinds[3], Annotation::Stroke { marker: false, points, .. } if points.len() == 2));
    }

    #[test]
    fn a_tiny_drag_makes_nothing_but_a_pen_dot() {
        let mut markup = session();
        drag(&mut markup, (10.0, 10.0), (11.0, 10.0));
        assert!(markup.annotations().is_empty());
        markup.select_tool(DrawTool::Pen);
        drag(&mut markup, (10.0, 10.0), (10.0, 10.0));
        assert_eq!(markup.annotations().len(), 1);
    }

    #[test]
    fn a_press_outside_the_snip_does_nothing() {
        let mut markup = session();
        markup.press((-5.0, 10.0));
        markup.press((700.0, 10.0));
        markup.pointer_moved((50.0, 50.0));
        markup.release();
        assert!(markup.annotations().is_empty());
        assert!(markup.live_annotations().is_empty());
    }

    #[test]
    fn a_drag_past_the_edge_stops_at_the_edge() {
        let mut markup = session();
        drag(&mut markup, (10.0, 10.0), (5000.0, -300.0));
        let Annotation::Arrow { to, .. } = markup.annotations()[0] else { panic!("expected an arrow") };
        assert_eq!(to, (600.0, 0.0));
    }

    #[test]
    fn undo_and_redo_walk_the_history_and_a_new_shape_clears_redo() {
        let mut markup = session();
        drag(&mut markup, (10.0, 10.0), (100.0, 80.0));
        markup.undo();
        assert!(markup.annotations().is_empty());
        markup.redo();
        assert_eq!(markup.annotations().len(), 1);
        markup.undo();
        drag(&mut markup, (20.0, 20.0), (100.0, 80.0));
        markup.redo();
        assert_eq!(markup.annotations().len(), 1);
    }

    #[test]
    fn text_is_typed_edited_and_kept_on_enter() {
        let mut markup = session();
        markup.select_tool(DrawTool::Text);
        markup.press((50.0, 50.0));
        assert!(markup.is_typing());
        for character in "hix".chars() {
            markup.type_character(character);
        }
        markup.type_backspace();
        markup.type_newline();
        markup.type_character('!');
        assert!(matches!(&markup.live_annotations()[0], Annotation::Text { text, .. } if text == "hi\n!|"));
        markup.commit_text();
        assert!(!markup.is_typing());
        assert!(matches!(&markup.annotations()[0], Annotation::Text { text, .. } if text == "hi\n!"));
    }

    #[test]
    fn empty_text_leaves_nothing_and_escape_drops_it() {
        let mut markup = session();
        markup.select_tool(DrawTool::Text);
        markup.press((50.0, 50.0));
        markup.commit_text();
        assert!(markup.annotations().is_empty());
        markup.press((50.0, 50.0));
        markup.type_character('a');
        markup.cancel_typing();
        assert!(markup.annotations().is_empty());
    }

    #[test]
    fn changing_tool_keeps_the_text_being_typed() {
        let mut markup = session();
        markup.select_tool(DrawTool::Text);
        markup.press((50.0, 50.0));
        markup.type_character('a');
        markup.select_tool(DrawTool::Arrow);
        assert_eq!(markup.annotations().len(), 1);
    }

    fn state() -> MarkupState {
        state_on(800, 500)
    }

    fn state_on(width: u32, height: u32) -> MarkupState {
        let screen = testing::sample_image(width, height);
        let area = (60, 80, 360, 260);
        let image = crate::desktop::ui::canvas::crop_image(&screen, area);
        MarkupState::new(&screen, 1.0, area, image).expect("state")
    }

    fn drag_on_screen(state: &mut MarkupState, fonts: &Fonts, from: (f32, f32), to: (f32, f32)) {
        state.mouse_moved(from.0, from.1);
        state.mouse_pressed(fonts);
        state.mouse_moved(to.0, to.1);
        state.mouse_released();
    }

    #[test]
    fn keys_pick_tools_and_colours_and_enter_keeps() {
        let mut state = state();
        state.key_pressed(Key::Char('b'), false, false);
        state.key_pressed(Key::Char('3'), false, false);
        assert_eq!((state.session().tool(), state.session().color()), (DrawTool::Box, 2));
        state.key_pressed(Key::Enter, false, false);
        assert_eq!(state.ending(), Some(Ending::Done));
    }

    #[test]
    fn escape_cancels_and_command_z_undoes() {
        let fonts = Fonts::load();
        let mut state = state();
        drag_on_screen(&mut state, &fonts, (100.0, 120.0), (300.0, 250.0));
        assert_eq!(state.session().annotations().len(), 1);
        state.key_pressed(Key::Char('z'), false, true);
        assert!(state.session().annotations().is_empty());
        state.key_pressed(Key::Char('Z'), true, true);
        assert_eq!(state.session().annotations().len(), 1);
        state.key_pressed(Key::Escape, false, false);
        assert_eq!(state.ending(), Some(Ending::Cancelled));
    }

    #[test]
    fn typing_text_swallows_tool_keys_and_escape_only_drops_the_text() {
        let fonts = Fonts::load();
        let mut state = state();
        state.key_pressed(Key::Char('t'), false, false);
        state.mouse_moved(100.0, 120.0);
        state.mouse_pressed(&fonts);
        state.key_pressed(Key::Char('b'), false, false);
        assert_eq!(state.session().tool(), DrawTool::Text);
        state.key_pressed(Key::Escape, false, false);
        assert_eq!(state.ending(), None);
        assert!(state.session().annotations().is_empty());
    }

    #[test]
    fn the_bar_buttons_work_and_stay_on_the_screen() {
        let fonts = Fonts::load();
        let mut state = state_on(1280, 720);
        let bar = state.bar(&fonts);
        assert!(bar.bar.x >= 0.0 && bar.bar.right() <= 1280.0 && bar.bar.bottom() <= 720.0);
        assert!(bar.bar.y >= state.area.bottom(), "below the area when there is room");
        let pen = bar.buttons.iter().find(|button| button.control == Control::Tool(DrawTool::Pen)).unwrap().rect;
        state.mouse_moved(pen.x + 4.0, pen.y + 4.0);
        state.mouse_pressed(&fonts);
        assert_eq!(state.session().tool(), DrawTool::Pen);
        let red = bar.swatches[1].1;
        state.mouse_moved(red.x + 4.0, red.y + 4.0);
        state.mouse_pressed(&fonts);
        assert_eq!(state.session().color(), 1);
        let done = bar.buttons.iter().find(|button| button.control == Control::Done).unwrap().rect;
        state.mouse_moved(done.x + 4.0, done.y + 4.0);
        state.mouse_pressed(&fonts);
        assert_eq!(state.ending(), Some(Ending::Done));
    }

    #[test]
    fn the_bar_goes_above_an_area_at_the_bottom() {
        let fonts = Fonts::load();
        let bar = MarkupBar::layout(&fonts, 1.0, Rect::new(100.0, 300.0, 300.0, 190.0), (800.0, 500.0));
        assert!(bar.bar.bottom() <= 300.0);
    }

    #[test]
    fn flattening_burns_in_the_drawing_and_hide_pixelates() {
        let fonts = Fonts::load();
        let mut state = state();
        assert_eq!(state.flattened(&fonts).bgra, state.image.bgra);
        state.key_pressed(Key::Char('h'), false, false);
        drag_on_screen(&mut state, &fonts, (80.0, 100.0), (200.0, 160.0));
        let flat = state.flattened(&fonts);
        assert_eq!((flat.width, flat.height), (360, 260));
        assert_ne!(flat.bgra, state.image.bgra);
        let hidden = pixelate(&state.image);
        let inside = ((30 * 360 + 30) * 4) as usize;
        assert_eq!(flat.bgra[inside..inside + 3], hidden.bgra[inside..inside + 3]);
        let outside = ((200 * 360 + 300) * 4) as usize;
        assert_eq!(flat.bgra[outside..outside + 3], state.image.bgra[outside..outside + 3]);
    }

    #[test]
    fn renders_the_draw_toolbar_with_a_few_shapes_to_png() {
        let fonts = Fonts::load();
        let screen = testing::sample_image(1280, 720);
        let area = (120, 100, 640, 400);
        let image = crate::desktop::ui::canvas::crop_image(&screen, area);
        let mut state = MarkupState::new(&screen, 1.0, area, image).expect("state");
        state.key_pressed(Key::Char('2'), false, false);
        drag_on_screen(&mut state, &fonts, (180.0, 420.0), (330.0, 280.0));
        state.key_pressed(Key::Char('b'), false, false);
        state.key_pressed(Key::Char('4'), false, false);
        drag_on_screen(&mut state, &fonts, (400.0, 160.0), (640.0, 250.0));
        state.key_pressed(Key::Char('h'), false, false);
        drag_on_screen(&mut state, &fonts, (430.0, 300.0), (700.0, 380.0));
        state.key_pressed(Key::Char('t'), false, false);
        state.key_pressed(Key::Char('3'), false, false);
        state.mouse_moved(160.0, 140.0);
        state.mouse_pressed(&fonts);
        for character in "look here".chars() {
            state.key_pressed(Key::Char(character), false, false);
        }
        state.key_pressed(Key::Enter, false, false);
        state.key_pressed(Key::Char('m'), false, false);
        state.key_pressed(Key::Char('1'), false, false);
        drag_on_screen(&mut state, &fonts, (200.0, 470.0), (480.0, 475.0));
        state.key_pressed(Key::Char('a'), false, false);
        state.mouse_moved(520.0, 560.0);
        let mut pixmap = Pixmap::new(1280, 720).unwrap();
        state.render(&fonts, &mut pixmap);
        testing::write_preview("draw-toolbar", &pixmap);
        assert_eq!(state.session().annotations().len(), 5);
    }
}
