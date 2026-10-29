//! What the editor draws on a snip, and how a finished edit becomes a flat image.
//! Annotations live in image pixels, so the same drawing code serves the window and export.

use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F,
    D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN,
    D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
    D2D1_BITMAP_PROPERTIES, D2D1_CAP_STYLE_ROUND, D2D1_DASH_STYLE_SOLID, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_FEATURE_LEVEL_DEFAULT, D2D1_LINE_JOIN_ROUND, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_STROKE_STYLE_PROPERTIES, ID2D1Bitmap,
    ID2D1RenderTarget, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapCacheOnLoad,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::core::{Result, w};
use windows_numerics::{Matrix3x2, Vector2};

use crate::capture::PixelRect;
use crate::output::Image;
use crate::overlay::Graphics;
use crate::style;

pub type Point = (f32, f32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Arrow,
    Box,
    Pen,
    Marker,
    Text,
    Hide,
    Crop,
}

/// Toolbar order, the shortcut key and the `[k]ey` style label.
pub const TOOLS: [(Tool, u8, &str); 7] = [
    (Tool::Arrow, b'A', "[a]rrow"),
    (Tool::Box, b'B', "[b]ox"),
    (Tool::Pen, b'P', "[p]en"),
    (Tool::Marker, b'M', "[m]ark"),
    (Tool::Text, b'T', "[t]ext"),
    (Tool::Hide, b'H', "[h]ide"),
    (Tool::Crop, b'C', "[c]rop"),
];

pub const COLORS: [D2D1_COLOR_F; 5] = [
    style::ACCENT,
    style::rgb(0xE5, 0x53, 0x4B, 1.0),
    style::rgb(0xF2, 0xC9, 0x4C, 1.0),
    style::rgb(0x4C, 0xC3, 0x8A, 1.0),
    style::rgb(0x1F, 0x1E, 0x1D, 1.0),
];

/// Stroke widths and text sizes, as multiples of the image's unit.
pub const WIDTH_STEPS: [f32; 3] = [1.0, 2.0, 4.0];
const TEXT_SIZE_PER_WIDTH: f32 = 7.0;

#[derive(Clone, Debug)]
pub enum Annotation {
    Arrow { from: Point, to: Point, color: usize, width: f32 },
    Box { rect: D2D_RECT_F, color: usize, width: f32 },
    Stroke { points: Vec<Point>, color: usize, width: f32, marker: bool },
    Text { at: Point, text: String, color: usize, size: f32 },
    Hide { rect: D2D_RECT_F },
    Crop { rect: D2D_RECT_F },
}

/// One stroke unit for this image: thin lines on small snips, thicker on 4K.
pub fn unit_for(image: &Image) -> f32 {
    (image.width.min(image.height) as f32 / 300.0).clamp(1.5, 4.0)
}

pub fn text_size(width: f32) -> f32 {
    width * TEXT_SIZE_PER_WIDTH
}

pub fn normalized(from: Point, to: Point) -> D2D_RECT_F {
    D2D_RECT_F {
        left: from.0.min(to.0),
        top: from.1.min(to.1),
        right: from.0.max(to.0),
        bottom: from.1.max(to.1),
    }
}

/// The part of the image that is kept: the last crop, or all of it.
pub fn view_rect(image: &Image, annotations: &[Annotation]) -> PixelRect {
    let full = PixelRect {
        left: 0,
        top: 0,
        width: image.width as i32,
        height: image.height as i32,
    };
    let last_crop = annotations.iter().rev().find_map(|annotation| match annotation {
        Annotation::Crop { rect } => Some(*rect),
        _ => None,
    });
    let Some(rect) = last_crop else {
        return full;
    };
    let cropped = PixelRect::from_edges(
        rect.left.round() as i32,
        rect.top.round() as i32,
        rect.right.round() as i32,
        rect.bottom.round() as i32,
    );
    cropped.intersect(&full).unwrap_or(full)
}

/// A blocky copy of the image; `Hide` shows the matching part of it.
pub fn pixelate(image: &Image) -> Vec<u8> {
    let block = (image.width.min(image.height) / 60).max(8) as usize;
    let width = image.width as usize;
    let height = image.height as usize;
    let mut output = image.bgra.clone();
    for block_top in (0..height).step_by(block) {
        for block_left in (0..width).step_by(block) {
            let bottom = (block_top + block).min(height);
            let right = (block_left + block).min(width);
            let mut sums = [0u32; 3];
            let count = ((bottom - block_top) * (right - block_left)) as u32;
            for row in block_top..bottom {
                for column in block_left..right {
                    let offset = (row * width + column) * 4;
                    for channel in 0..3 {
                        sums[channel] += image.bgra[offset + channel] as u32;
                    }
                }
            }
            for row in block_top..bottom {
                for column in block_left..right {
                    let offset = (row * width + column) * 4;
                    for channel in 0..3 {
                        output[offset + channel] = (sums[channel] / count) as u8;
                    }
                }
            }
        }
    }
    output
}

/// Device resources for one render target: the snip and its pixelated copy.
pub struct Canvas {
    pub base: ID2D1Bitmap,
    pub hidden: ID2D1Bitmap,
    round: ID2D1StrokeStyle,
}

impl Canvas {
    pub fn new(graphics: &Graphics, target: &ID2D1RenderTarget, image: &Image, hidden_pixels: &[u8]) -> Result<Self> {
        let round_properties = D2D1_STROKE_STYLE_PROPERTIES {
            startCap: D2D1_CAP_STYLE_ROUND,
            endCap: D2D1_CAP_STYLE_ROUND,
            dashCap: D2D1_CAP_STYLE_ROUND,
            lineJoin: D2D1_LINE_JOIN_ROUND,
            miterLimit: 10.0,
            dashStyle: D2D1_DASH_STYLE_SOLID,
            dashOffset: 0.0,
        };
        Ok(Self {
            base: bitmap_from(target, image.width, image.height, &image.bgra)?,
            hidden: bitmap_from(target, image.width, image.height, hidden_pixels)?,
            round: unsafe { graphics.d2d.CreateStrokeStyle(&round_properties, None)? },
        })
    }
}

fn bitmap_from(target: &ID2D1RenderTarget, width: u32, height: u32, bgra: &[u8]) -> Result<ID2D1Bitmap> {
    let properties = D2D1_BITMAP_PROPERTIES {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_IGNORE,
        },
        dpiX: 96.0,
        dpiY: 96.0,
    };
    let size = D2D_SIZE_U { width, height };
    unsafe { target.CreateBitmap(size, Some(bgra.as_ptr() as *const _), width * 4, &properties) }
}

/// Draws the snip and every annotation, in image pixels under the target's transform.
pub fn draw_scene(graphics: &Graphics, target: &ID2D1RenderTarget, canvas: &Canvas, size: (u32, u32), annotations: &[Annotation]) {
    let full = D2D_RECT_F {
        left: 0.0,
        top: 0.0,
        right: size.0 as f32,
        bottom: size.1 as f32,
    };
    unsafe { target.DrawBitmap(&canvas.base, Some(&full), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None) };
    for annotation in annotations {
        draw_annotation(graphics, target, canvas, annotation);
    }
}

pub fn draw_annotation(graphics: &Graphics, target: &ID2D1RenderTarget, canvas: &Canvas, annotation: &Annotation) {
    let brush_for = |color: D2D1_COLOR_F| unsafe { target.CreateSolidColorBrush(&color, None) };
    match annotation {
        Annotation::Arrow { from, to, color, width } => {
            let Ok(brush) = brush_for(COLORS[*color]) else {
                return;
            };
            let length = ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt();
            if length < 1.0 {
                return;
            }
            let direction = ((to.0 - from.0) / length, (to.1 - from.1) / length);
            let head_length = (width * 4.5).min(length * 0.6);
            let head_half_width = head_length * 0.55;
            let head_base = (to.0 - direction.0 * head_length, to.1 - direction.1 * head_length);
            let normal = (-direction.1, direction.0);
            unsafe {
                target.DrawLine(
                    Vector2 { X: from.0, Y: from.1 },
                    Vector2 { X: head_base.0 + direction.0, Y: head_base.1 + direction.1 },
                    &brush,
                    *width,
                    &canvas.round,
                );
            }
            let head = [
                (to.0, to.1),
                (head_base.0 + normal.0 * head_half_width, head_base.1 + normal.1 * head_half_width),
                (head_base.0 - normal.0 * head_half_width, head_base.1 - normal.1 * head_half_width),
            ];
            if let Ok(geometry) = path(graphics, &head, true) {
                unsafe { target.FillGeometry(&geometry, &brush, None) };
            }
        }
        Annotation::Box { rect, color, width } => {
            let Ok(brush) = brush_for(COLORS[*color]) else {
                return;
            };
            let shape = windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT {
                rect: *rect,
                radiusX: width * 1.5,
                radiusY: width * 1.5,
            };
            unsafe { target.DrawRoundedRectangle(&shape, &brush, *width, None) };
        }
        Annotation::Stroke { points, color, width, marker } => {
            let mut color_value = COLORS[*color];
            let mut stroke_width = *width;
            if *marker {
                color_value.a = 0.4;
                stroke_width = width * 5.0;
            }
            let Ok(brush) = brush_for(color_value) else {
                return;
            };
            if points.len() == 1 {
                let point = Vector2 { X: points[0].0, Y: points[0].1 };
                unsafe { target.DrawLine(point, point, &brush, stroke_width, &canvas.round) };
                return;
            }
            if let Ok(geometry) = path(graphics, points, false) {
                unsafe { target.DrawGeometry(&geometry, &brush, stroke_width, &canvas.round) };
            }
        }
        Annotation::Text { at, text, color, size } => {
            let Ok(brush) = brush_for(COLORS[*color]) else {
                return;
            };
            draw_text(graphics, target, text, *at, *size, &brush);
        }
        Annotation::Hide { rect } => unsafe {
            target.DrawBitmap(
                &canvas.hidden,
                Some(rect),
                1.0,
                D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                Some(rect),
            );
        },
        Annotation::Crop { .. } => {}
    }
}

pub fn draw_text(
    graphics: &Graphics,
    target: &ID2D1RenderTarget,
    text: &str,
    at: Point,
    size: f32,
    brush: &windows::Win32::Graphics::Direct2D::ID2D1SolidColorBrush,
) {
    let format = unsafe {
        graphics.dwrite.CreateTextFormat(
            w!("Segoe UI"),
            None,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!("en-us"),
        )
    };
    let Ok(format) = format else {
        return;
    };
    let utf16: Vec<u16> = text.encode_utf16().collect();
    let layout_rect = D2D_RECT_F {
        left: at.0,
        top: at.1,
        right: at.0 + 100_000.0,
        bottom: at.1 + 100_000.0,
    };
    unsafe {
        target.DrawText(&utf16, &format, &layout_rect, brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
    }
}

fn path(
    graphics: &Graphics,
    points: &[Point],
    closed: bool,
) -> Result<windows::Win32::Graphics::Direct2D::ID2D1PathGeometry> {
    let geometry = unsafe { graphics.d2d.CreatePathGeometry()? };
    let sink = unsafe { geometry.Open()? };
    let vectors: Vec<Vector2> = points.iter().map(|point| Vector2 { X: point.0, Y: point.1 }).collect();
    let begin = if closed { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW };
    let end = if closed { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN };
    unsafe {
        sink.BeginFigure(vectors[0], begin);
        sink.AddLines(&vectors[1..]);
        sink.EndFigure(end);
        sink.Close()?;
    }
    Ok(geometry)
}

/// Flattens the snip and its annotations into an image the size of the kept area.
/// COM must be initialised on this thread.
pub fn compose(graphics: &Graphics, image: &Image, hidden_pixels: &[u8], annotations: &[Annotation]) -> Result<Image> {
    let view = view_rect(image, annotations);
    let width = view.width as u32;
    let height = view.height as u32;
    let imaging: IWICImagingFactory = unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? };
    let bitmap = unsafe { imaging.CreateBitmap(width, height, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad)? };
    let properties = D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0,
        dpiY: 96.0,
        usage: D2D1_RENDER_TARGET_USAGE_NONE,
        minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
    };
    let target = unsafe { graphics.d2d.CreateWicBitmapRenderTarget(&bitmap, &properties)? };
    let canvas = Canvas::new(graphics, &target, image, hidden_pixels)?;
    let shift = Matrix3x2 {
        M11: 1.0,
        M12: 0.0,
        M21: 0.0,
        M22: 1.0,
        M31: -view.left as f32,
        M32: -view.top as f32,
    };
    unsafe {
        target.BeginDraw();
        target.SetTransform(&shift);
        draw_scene(graphics, &target, &canvas, (image.width, image.height), annotations);
        target.EndDraw(None, None)?;
    }
    let stride = width * 4;
    let mut bgra = vec![0u8; (stride * height) as usize];
    unsafe { bitmap.CopyPixels(std::ptr::null(), stride, &mut bgra)? };
    for pixel in bgra.chunks_exact_mut(4) {
        pixel[3] = 255;
    }
    Ok(Image { width, height, bgra })
}
