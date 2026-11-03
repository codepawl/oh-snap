//! Small drawing helpers over tiny-skia, so the window code reads as layout.

use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Stroke, Transform};

use super::geometry::Rect;
use crate::output::Image;

fn paint(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    paint
}

fn stroke_of(width: f32) -> Stroke {
    Stroke { width, line_join: tiny_skia::LineJoin::Round, line_cap: tiny_skia::LineCap::Round, ..Stroke::default() }
}

pub fn fill_rect(pixmap: &mut Pixmap, rect: Rect, color: Color) {
    if let Some(shape) = tiny_skia::Rect::from_xywh(rect.x, rect.y, rect.width.max(0.1), rect.height.max(0.1)) {
        pixmap.fill_rect(shape, &paint(color), Transform::identity(), None);
    }
}

fn rounded_path(rect: Rect, radius: f32) -> Option<tiny_skia::Path> {
    let radius = radius.min(rect.width / 2.0).min(rect.height / 2.0).max(0.0);
    let (left, top, right, bottom) = (rect.x, rect.y, rect.right(), rect.bottom());
    let mut builder = PathBuilder::new();
    builder.move_to(left + radius, top);
    builder.line_to(right - radius, top);
    builder.quad_to(right, top, right, top + radius);
    builder.line_to(right, bottom - radius);
    builder.quad_to(right, bottom, right - radius, bottom);
    builder.line_to(left + radius, bottom);
    builder.quad_to(left, bottom, left, bottom - radius);
    builder.line_to(left, top + radius);
    builder.quad_to(left, top, left + radius, top);
    builder.close();
    builder.finish()
}

pub fn fill_rounded(pixmap: &mut Pixmap, rect: Rect, radius: f32, color: Color) {
    if let Some(path) = rounded_path(rect, radius) {
        pixmap.fill_path(&path, &paint(color), FillRule::Winding, Transform::identity(), None);
    }
}

/// The outline sits inside `rect`.
pub fn stroke_rounded(pixmap: &mut Pixmap, rect: Rect, radius: f32, color: Color, width: f32) {
    if let Some(path) = rounded_path(rect.inset(width / 2.0), radius) {
        pixmap.stroke_path(&path, &paint(color), &stroke_of(width), Transform::identity(), None);
    }
}

pub fn line(pixmap: &mut Pixmap, from: (f32, f32), to: (f32, f32), color: Color, width: f32) {
    polyline(pixmap, &[from, to], color, width);
}

pub fn polyline(pixmap: &mut Pixmap, points: &[(f32, f32)], color: Color, width: f32) {
    let Some(first) = points.first() else {
        return;
    };
    let mut builder = PathBuilder::new();
    builder.move_to(first.0, first.1);
    for point in &points[1..] {
        builder.line_to(point.0, point.1);
    }
    if let Some(path) = builder.finish() {
        pixmap.stroke_path(&path, &paint(color), &stroke_of(width), Transform::identity(), None);
    }
}

pub fn fill_circle(pixmap: &mut Pixmap, center: (f32, f32), radius: f32, color: Color) {
    if let Some(path) = PathBuilder::from_circle(center.0, center.1, radius) {
        pixmap.fill_path(&path, &paint(color), FillRule::Winding, Transform::identity(), None);
    }
}

/// A ring with `fraction` (0 to 1) of it drawn clockwise from the top, over a dim track.
pub fn progress_ring(pixmap: &mut Pixmap, center: (f32, f32), radius: f32, fraction: f32, track: Color, color: Color, width: f32) {
    if let Some(path) = PathBuilder::from_circle(center.0, center.1, radius) {
        pixmap.stroke_path(&path, &paint(track), &stroke_of(width), Transform::identity(), None);
    }
    let steps = 48;
    let drawn = (fraction.clamp(0.0, 1.0) * steps as f32).round() as usize;
    if drawn == 0 {
        return;
    }
    let points: Vec<(f32, f32)> = (0..=drawn)
        .map(|step| {
            let angle = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * step as f32 / steps as f32;
            (center.0 + radius * angle.cos(), center.1 + radius * angle.sin())
        })
        .collect();
    polyline(pixmap, &points, color, width);
}

/// The toolbar's mark: six strokes through one point, like Claude Code's spinner.
pub fn spark(pixmap: &mut Pixmap, center: (f32, f32), radius: f32, color: Color) {
    for index in 0..3 {
        let angle = std::f32::consts::PI / 3.0 * index as f32 + std::f32::consts::FRAC_PI_6;
        let (dx, dy) = (radius * angle.cos(), radius * angle.sin());
        line(pixmap, (center.0 - dx, center.1 - dy), (center.0 + dx, center.1 + dy), color, radius * 0.28);
    }
}

/// A pixmap with the image's pixels (BGRA in, premultiplied RGBA out; snips are opaque).
pub fn pixmap_from_image(image: &Image) -> Option<Pixmap> {
    let mut pixmap = Pixmap::new(image.width, image.height)?;
    for (target, source) in pixmap.data_mut().chunks_exact_mut(4).zip(image.bgra.chunks_exact(4)) {
        target.copy_from_slice(&[source[2], source[1], source[0], source[3]]);
    }
    Some(pixmap)
}

/// Draws `source` over the whole of `destination`, scaled when the sizes differ.
pub fn draw_fitted(destination: &mut Pixmap, source: &Pixmap) {
    if destination.width() == source.width() && destination.height() == source.height() {
        destination.data_mut().copy_from_slice(source.data());
        return;
    }
    let scale_x = destination.width() as f32 / source.width() as f32;
    let scale_y = destination.height() as f32 / source.height() as f32;
    let quality = PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..PixmapPaint::default() };
    destination.draw_pixmap(0, 0, source.as_ref(), &quality, Transform::from_scale(scale_x, scale_y), None);
}

/// `source` scaled to fit inside `target` and centred in it.
pub fn draw_thumbnail(destination: &mut Pixmap, source: &Pixmap, target: Rect) {
    let scale = (target.width / source.width() as f32).min(target.height / source.height() as f32);
    let left = target.x + (target.width - source.width() as f32 * scale) / 2.0;
    let top = target.y + (target.height - source.height() as f32 * scale) / 2.0;
    let transform = Transform::from_translate(left, top).pre_scale(scale, scale);
    let quality = PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..PixmapPaint::default() };
    destination.draw_pixmap(0, 0, source.as_ref(), &quality, transform, None);
}

/// The window buffer softbuffer wants: one 0x00RRGGBB word per pixel.
pub fn to_argb_words(pixmap: &Pixmap) -> Vec<u32> {
    pixmap
        .data()
        .chunks_exact(4)
        .map(|pixel| (u32::from(pixel[0]) << 16) | (u32::from(pixel[1]) << 8) | u32::from(pixel[2]))
        .collect()
}

/// The part of `image` inside x, y, width, height (already inside the image).
pub fn crop_image(image: &Image, area: (i32, i32, u32, u32)) -> Image {
    let (left, top, width, height) = (area.0 as usize, area.1 as usize, area.2 as usize, area.3 as usize);
    let mut bgra = Vec::with_capacity(width * height * 4);
    for row in top..top + height {
        let start = (row * image.width as usize + left) * 4;
        bgra.extend_from_slice(&image.bgra[start..start + width * 4]);
    }
    Image { width: area.2, height: area.3, bgra }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_keeps_the_right_pixels() {
        let mut bgra = vec![0u8; 4 * 4 * 4];
        bgra[(2 * 4 + 1) * 4] = 99;
        let image = Image { width: 4, height: 4, bgra };
        let cropped = crop_image(&image, (1, 2, 2, 1));
        assert_eq!((cropped.width, cropped.height), (2, 1));
        assert_eq!(cropped.bgra[0], 99);
    }

    #[test]
    fn image_to_pixmap_swaps_red_and_blue() {
        let image = Image { width: 1, height: 1, bgra: vec![10, 20, 30, 255] };
        let pixmap = pixmap_from_image(&image).unwrap();
        assert_eq!(pixmap.data(), &[30, 20, 10, 255]);
    }
}
