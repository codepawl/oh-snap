//! The result card, bottom right: thumbnail, what happened, a countdown ring and buttons.

use tiny_skia::Pixmap;

use super::canvas;
use super::geometry::Rect;
use super::text::Fonts;
use super::theme;

pub const CARD_WIDTH: f32 = 320.0;
const PADDING: f32 = 12.0;
const THUMBNAIL_MAX_HEIGHT: f32 = 168.0;
const SWATCH_HEIGHT: f32 = 48.0;
const BUTTON_HEIGHT: f32 = 28.0;
const RING_RADIUS: f32 = 7.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardAction {
    Open,
    Folder,
    Copy,
    Gif,
    Trim,
    SetUp,
    NotNow,
    /// On the card that follows a crash: opens the filled-in bug report.
    Report,
}

impl CardAction {
    pub fn label(self) -> &'static str {
        match self {
            CardAction::Open => "open",
            CardAction::Folder => "folder",
            CardAction::Copy => "copy",
            CardAction::Gif => "gif",
            CardAction::Trim => "trim",
            CardAction::SetUp => "set up",
            CardAction::NotNow => "not now",
            CardAction::Report => "report",
        }
    }
}

pub struct CardModel {
    pub headline: String,
    pub detail: String,
    pub thumbnail: Option<Pixmap>,
    /// A colour pick shows its colour instead of a thumbnail.
    pub swatch: Option<[u8; 3]>,
    pub actions: Vec<CardAction>,
    /// 1.0 when the card appears, 0.0 when it goes away.
    pub remaining: f32,
    pub failed: bool,
}

pub struct CardLayout {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub media: Option<Rect>,
    pub ring_center: (f32, f32),
    pub headline_at: (f32, f32),
    pub detail_at: Option<(f32, f32)>,
    pub buttons: Vec<(CardAction, Rect)>,
}

impl CardLayout {
    pub fn new(model: &CardModel, fonts: &Fonts, scale: f32) -> CardLayout {
        let padding = PADDING * scale;
        let width = CARD_WIDTH * scale;
        let text_size = theme::LABEL_SIZE * scale;
        let line = fonts.line_height(text_size);
        let mut top = padding;

        let media = if let Some(thumbnail) = &model.thumbnail {
            let inner = width - padding * 2.0;
            let natural = inner * thumbnail.height() as f32 / thumbnail.width() as f32;
            Some(Rect::new(padding, top, inner, natural.min(THUMBNAIL_MAX_HEIGHT * scale)))
        } else if model.swatch.is_some() {
            Some(Rect::new(padding, top, width - padding * 2.0, SWATCH_HEIGHT * scale))
        } else {
            None
        };
        if let Some(media) = media {
            top = media.bottom() + padding * 0.75;
        }

        let ring_center = (padding + RING_RADIUS * scale, top + line / 2.0);
        let text_left = padding + RING_RADIUS * scale * 2.0 + 8.0 * scale;
        let headline_at = (text_left, top);
        top += line;
        let detail_at = (!model.detail.is_empty()).then_some((text_left, top));
        if detail_at.is_some() {
            top += line;
        }
        top += padding * 0.75;

        let button_gap = 4.0 * scale;
        let mut left = padding;
        let mut buttons = Vec::new();
        for &action in &model.actions {
            let button_width = fonts.width(action.label(), text_size) + 20.0 * scale;
            buttons.push((action, Rect::new(left, top, button_width, BUTTON_HEIGHT * scale)));
            left += button_width + button_gap;
        }
        if !buttons.is_empty() {
            top += BUTTON_HEIGHT * scale;
        }
        CardLayout {
            width: width.round() as u32,
            height: (top + padding).round() as u32,
            scale,
            media,
            ring_center,
            headline_at,
            detail_at,
            buttons,
        }
    }

    pub fn action_at(&self, x: f32, y: f32) -> Option<CardAction> {
        self.buttons.iter().find(|(_, rect)| rect.contains(x, y)).map(|(action, _)| *action)
    }
}

pub fn render(model: &CardModel, layout: &CardLayout, hovered: Option<CardAction>, fonts: &Fonts, pixmap: &mut Pixmap) {
    let scale = layout.scale;
    let size = theme::LABEL_SIZE * scale;
    let whole = Rect::new(0.0, 0.0, pixmap.width() as f32, pixmap.height() as f32);
    pixmap.fill(theme::ink());
    canvas::fill_rounded(pixmap, whole, theme::PANEL_RADIUS * scale, theme::ink_panel());
    canvas::stroke_rounded(pixmap, whole, theme::PANEL_RADIUS * scale, theme::rule(), 1.0);

    if let Some(media) = layout.media {
        if let Some(thumbnail) = &model.thumbnail {
            canvas::fill_rounded(pixmap, media, theme::BUTTON_RADIUS * scale, theme::ink());
            canvas::draw_thumbnail(pixmap, thumbnail, media);
        } else if let Some([red, green, blue]) = model.swatch {
            canvas::fill_rounded(pixmap, media, theme::BUTTON_RADIUS * scale, tiny_skia::Color::from_rgba8(red, green, blue, 255));
        }
    }

    let mark = if model.failed { theme::failure() } else { theme::accent() };
    if model.failed {
        canvas::fill_circle(pixmap, layout.ring_center, RING_RADIUS * scale, mark);
    } else {
        canvas::progress_ring(pixmap, layout.ring_center, RING_RADIUS * scale - 1.0, model.remaining, theme::rule(), mark, 2.0 * scale);
    }
    fonts.draw(pixmap, &model.headline, layout.headline_at.0, layout.headline_at.1, size, theme::text());
    if let Some(at) = layout.detail_at {
        fonts.draw(pixmap, &model.detail, at.0, at.1, size, theme::text_dim());
    }
    for (action, rect) in &layout.buttons {
        if hovered == Some(*action) {
            canvas::fill_rounded(pixmap, *rect, theme::BUTTON_RADIUS * scale, theme::hover());
        } else {
            canvas::stroke_rounded(pixmap, *rect, theme::BUTTON_RADIUS * scale, theme::rule(), 1.0);
        }
        let label_width = fonts.width(action.label(), size);
        let text_top = rect.y + (rect.height - fonts.line_height(size)) / 2.0;
        fonts.draw(pixmap, action.label(), rect.x + (rect.width - label_width) / 2.0, text_top, size, theme::text());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::ui::testing;

    fn snip_model() -> CardModel {
        CardModel {
            headline: "copied · saved".into(),
            detail: "800 × 600".into(),
            thumbnail: Some(testing::sample_thumbnail()),
            swatch: None,
            actions: vec![CardAction::Open, CardAction::Folder, CardAction::Copy],
            remaining: 0.6,
            failed: false,
        }
    }

    #[test]
    fn buttons_fit_inside_the_card_and_hit_test() {
        let fonts = Fonts::load();
        let model = snip_model();
        let layout = CardLayout::new(&model, &fonts, 1.0);
        for (_, rect) in &layout.buttons {
            assert!(rect.right() <= layout.width as f32 && rect.bottom() <= layout.height as f32);
        }
        let (action, rect) = layout.buttons[1];
        assert_eq!(action, CardAction::Folder);
        assert_eq!(layout.action_at(rect.x + 3.0, rect.y + 3.0), Some(CardAction::Folder));
        assert_eq!(layout.action_at(1.0, 1.0), None);
    }

    #[test]
    fn a_recording_card_has_gif_trim_and_folder() {
        let fonts = Fonts::load();
        let model = CardModel { actions: vec![CardAction::Gif, CardAction::Trim, CardAction::Folder], ..snip_model() };
        let layout = CardLayout::new(&model, &fonts, 2.0);
        assert_eq!(layout.buttons.len(), 3);
        assert_eq!(layout.width, 640);
    }

    #[test]
    fn renders_cards_to_png() {
        let fonts = Fonts::load();
        let model = snip_model();
        let layout = CardLayout::new(&model, &fonts, 1.0);
        let mut pixmap = Pixmap::new(layout.width, layout.height).unwrap();
        render(&model, &layout, Some(CardAction::Open), &fonts, &mut pixmap);
        testing::write_preview("card-snip", &pixmap);

        let color = CardModel { thumbnail: None, swatch: Some([0xD9, 0x77, 0x57]), headline: "#D97757 copied".into(), detail: "rgb(217, 119, 87)".into(), actions: vec![], ..snip_model() };
        let layout = CardLayout::new(&color, &fonts, 1.0);
        let mut pixmap = Pixmap::new(layout.width, layout.height).unwrap();
        render(&color, &layout, None, &fonts, &mut pixmap);
        testing::write_preview("card-color", &pixmap);
    }

    #[test]
    fn the_setup_card_fits_its_text_and_buttons() {
        let fonts = Fonts::load();
        let model = CardModel {
            headline: "set up oh-snap?".into(),
            detail: "run at login, take Cmd+Shift+3/4/5".into(),
            thumbnail: None,
            swatch: None,
            actions: vec![CardAction::SetUp, CardAction::NotNow],
            remaining: 1.0,
            failed: false,
        };
        let layout = CardLayout::new(&model, &fonts, 2.0);
        let detail_at = layout.detail_at.expect("a detail line");
        assert!(detail_at.0 + fonts.width(&model.detail, theme::LABEL_SIZE * 2.0) <= layout.width as f32);
        let (action, rect) = layout.buttons[0];
        assert_eq!(action, CardAction::SetUp);
        assert_eq!(layout.action_at(rect.x + 3.0, rect.y + 3.0), Some(CardAction::SetUp));
        let mut pixmap = Pixmap::new(layout.width, layout.height).unwrap();
        render(&model, &layout, Some(CardAction::SetUp), &fonts, &mut pixmap);
        testing::write_preview("card-setup", &pixmap);
    }
}
