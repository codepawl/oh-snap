//! The oh-snap mark: four clay corner brackets around an ivory shutter dot on a warm
//! ink tile. Drawn with Direct2D, so the tray icon, the .ico and the PNGs match.

use std::path::Path;

use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT};
use windows::Win32::Graphics::Direct2D::{
    D2D1_CAP_STYLE_ROUND, D2D1_DASH_STYLE_SOLID, D2D1_ELLIPSE, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_LINE_JOIN_ROUND,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE,
    D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapCacheOnLoad,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::core::Result;
use windows_numerics::Vector2;

use crate::overlay::Graphics;
use crate::style;

/// The same drawing as SVG, for the README and anything outside Windows.
pub const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256">
  <rect x="8" y="8" width="240" height="240" rx="56" fill="#1F1E1D"/>
  <rect x="9.5" y="9.5" width="237" height="237" rx="54.5" fill="none" stroke="#3E3C38" stroke-width="3"/>
  <g fill="none" stroke="#D97757" stroke-width="20" stroke-linecap="round" stroke-linejoin="round">
    <path d="M60 108V60h48"/>
    <path d="M148 60h48v48"/>
    <path d="M196 148v48h-48"/>
    <path d="M108 196H60v-48"/>
  </g>
  <circle cx="128" cy="128" r="22" fill="#F0EEE6"/>
</svg>
"##;

const ICON_SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];

/// Straight-alpha BGRA pixels of the mark at `size` x `size`. COM must be initialised.
pub fn render(graphics: &Graphics, size: u32) -> Result<Vec<u8>> {
    let imaging: IWICImagingFactory = unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? };
    let bitmap = unsafe { imaging.CreateBitmap(size, size, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad)? };
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
    let scale = size as f32 / 256.0;
    let at = |value: f32| value * scale;
    let round_caps = D2D1_STROKE_STYLE_PROPERTIES {
        startCap: D2D1_CAP_STYLE_ROUND,
        endCap: D2D1_CAP_STYLE_ROUND,
        dashCap: D2D1_CAP_STYLE_ROUND,
        lineJoin: D2D1_LINE_JOIN_ROUND,
        miterLimit: 10.0,
        dashStyle: D2D1_DASH_STYLE_SOLID,
        dashOffset: 0.0,
    };
    let caps = unsafe { graphics.d2d.CreateStrokeStyle(&round_caps, None)? };
    unsafe {
        let ink = target.CreateSolidColorBrush(&style::INK, None)?;
        let rule = target.CreateSolidColorBrush(&style::RULE, None)?;
        let accent = target.CreateSolidColorBrush(&style::ACCENT, None)?;
        let ivory = target.CreateSolidColorBrush(&style::TEXT, None)?;

        target.BeginDraw();
        target.Clear(None);
        let tile = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: at(8.0), top: at(8.0), right: at(248.0), bottom: at(248.0) },
            radiusX: at(56.0),
            radiusY: at(56.0),
        };
        target.FillRoundedRectangle(&tile, &ink);
        if size >= 48 {
            let edge = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F { left: at(9.5), top: at(9.5), right: at(246.5), bottom: at(246.5) },
                radiusX: at(54.5),
                radiusY: at(54.5),
            };
            target.DrawRoundedRectangle(&edge, &rule, at(3.0), None);
        }
        // Small sizes get thicker strokes so the brackets survive the downscale.
        let stroke = if size <= 24 { at(28.0) } else { at(20.0) };
        let brackets = [
            [(60.0, 108.0), (60.0, 60.0), (108.0, 60.0)],
            [(148.0, 60.0), (196.0, 60.0), (196.0, 108.0)],
            [(196.0, 148.0), (196.0, 196.0), (148.0, 196.0)],
            [(108.0, 196.0), (60.0, 196.0), (60.0, 148.0)],
        ];
        for [first, corner, last] in brackets {
            for (from, to) in [(first, corner), (corner, last)] {
                target.DrawLine(
                    Vector2 { X: at(from.0), Y: at(from.1) },
                    Vector2 { X: at(to.0), Y: at(to.1) },
                    &accent,
                    stroke,
                    &caps,
                );
            }
        }
        let dot = D2D1_ELLIPSE {
            point: Vector2 { X: at(128.0), Y: at(128.0) },
            radiusX: at(22.0),
            radiusY: at(22.0),
        };
        target.FillEllipse(&dot, &ivory);
        target.EndDraw(None, None)?;
    }

    let stride = size * 4;
    let mut pixels = vec![0u8; (stride * size) as usize];
    unsafe { bitmap.CopyPixels(std::ptr::null(), stride, &mut pixels)? };
    unpremultiply(&mut pixels);
    Ok(pixels)
}

fn unpremultiply(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = pixel[3] as u32;
        if alpha == 0 || alpha == 255 {
            continue;
        }
        for channel in &mut pixel[..3] {
            *channel = ((*channel as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
}

/// `oh-snap logo DIR`: writes oh-snap.svg, oh-snap.png (512 px) and oh-snap.ico.
pub fn export(graphics: &Graphics, folder: &Path) -> std::result::Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(folder)?;
    std::fs::write(folder.join("oh-snap.svg"), SVG)?;
    let large = render(graphics, 512)?;
    std::fs::write(folder.join("oh-snap.png"), encode_rgba_png(&large, 512)?)?;

    let mut entries = Vec::new();
    for size in ICON_SIZES {
        let pixels = render(graphics, size)?;
        entries.push((size, encode_rgba_png(&pixels, size)?));
    }
    std::fs::write(folder.join("oh-snap.ico"), ico_bytes(&entries))?;
    Ok(())
}

fn encode_rgba_png(bgra: &[u8], size: u32) -> std::io::Result<Vec<u8>> {
    let mut rgba = Vec::with_capacity(bgra.len());
    for pixel in bgra.chunks_exact(4) {
        rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
    }
    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, size, size);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
        writer.write_image_data(&rgba).map_err(std::io::Error::other)?;
    }
    Ok(png_bytes)
}

/// An .ico whose images are PNGs, which every Windows version since Vista reads.
fn ico_bytes(entries: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * entries.len() as u32;
    for (size, png_bytes) in entries {
        // A dimension of 256 is written as 0.
        let dimension = if *size >= 256 { 0 } else { *size as u8 };
        bytes.push(dimension);
        bytes.push(dimension);
        bytes.push(0);
        bytes.push(0);
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(&(png_bytes.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&offset.to_le_bytes());
        offset += png_bytes.len() as u32;
    }
    for (_, png_bytes) in entries {
        bytes.extend_from_slice(png_bytes);
    }
    bytes
}
