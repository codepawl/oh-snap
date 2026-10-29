//! Bits that break off the pointer while a region is dragged, like mining a block:
//! small squares in the colour of the pixels under the pointer, and ASCII glyphs.
//! They fly up and out, fall, and fade.

use std::time::Instant;

use windows::Win32::Graphics::Direct2D::Common::{D2D_RECT_F, D2D1_COLOR_F};
use windows::Win32::Graphics::Direct2D::{D2D1_DRAW_TEXT_OPTIONS_NONE, ID2D1RenderTarget, ID2D1SolidColorBrush};
use windows::Win32::Graphics::DirectWrite::{DWRITE_MEASURING_MODE_NATURAL, IDWriteTextFormat};

use crate::style;

const GRAVITY: f32 = 1700.0;
const GLYPHS: [char; 9] = ['#', '@', '%', '*', '+', '=', '-', ':', '.'];
const MAX_PARTICLES: usize = 400;
/// One particle for this many pixels of pointer travel.
const SPACING: f32 = 3.5;

struct Particle {
    x: f32,
    y: f32,
    velocity_x: f32,
    velocity_y: f32,
    age: f32,
    lifetime: f32,
    size: f32,
    color: D2D1_COLOR_F,
    glyph: Option<char>,
}

pub struct ParticleSystem {
    particles: Vec<Particle>,
    random_state: u32,
    last_update: Instant,
    travel: f32,
    last_point: Option<(f32, f32)>,
}

impl ParticleSystem {
    pub fn new(seed: u32) -> Self {
        Self {
            particles: Vec::new(),
            random_state: seed | 1,
            last_update: Instant::now(),
            travel: 0.0,
            last_point: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.particles.is_empty()
    }

    /// Called as the pointer moves during a drag. `pixel` is the colour under it.
    pub fn trail(&mut self, point: (f32, f32), pixel: [u8; 3]) {
        let Some(previous) = self.last_point.replace(point) else {
            return;
        };
        self.travel += ((point.0 - previous.0).powi(2) + (point.1 - previous.1).powi(2)).sqrt();
        while self.travel >= SPACING && self.particles.len() < MAX_PARTICLES {
            self.travel -= SPACING;
            self.spawn(point, pixel);
        }
    }

    /// A single burst at one point, for the "start and end" setting.
    pub fn burst(&mut self, point: (f32, f32), pixel: [u8; 3], count: usize) {
        for _ in 0..count {
            if self.particles.len() >= MAX_PARTICLES {
                break;
            }
            self.spawn(point, pixel);
        }
    }

    fn spawn(&mut self, point: (f32, f32), pixel: [u8; 3]) {
        let is_glyph = self.random() < 0.4;
        let glyph = if is_glyph {
            Some(GLYPHS[(self.random() * GLYPHS.len() as f32) as usize % GLYPHS.len()])
        } else {
            None
        };
        // Squares take the colour of what is being "mined"; glyphs are clay or ivory.
        let color = if is_glyph {
            if self.random() < 0.6 { style::ACCENT } else { style::TEXT }
        } else {
            let shade = 0.85 + self.random() * 0.3;
            let channel = |value: u8| ((value as f32 * shade).min(255.0)) / 255.0;
            D2D1_COLOR_F {
                r: channel(pixel[0]),
                g: channel(pixel[1]),
                b: channel(pixel[2]),
                a: 1.0,
            }
        };
        let particle = Particle {
            x: point.0,
            y: point.1,
            velocity_x: (self.random() - 0.5) * 640.0,
            velocity_y: -160.0 - self.random() * 460.0,
            age: 0.0,
            lifetime: 0.55 + self.random() * 0.4,
            size: if is_glyph { 0.0 } else { 4.0 + (self.random() * 4.0).floor() * 2.0 },
            color,
            glyph,
        };
        self.particles.push(particle);
    }

    /// Moves everything by the real time since the last call and drops dead particles.
    pub fn update(&mut self) {
        let now = Instant::now();
        let elapsed = (now - self.last_update).as_secs_f32().min(0.05);
        self.last_update = now;
        self.advance(elapsed);
    }

    /// Steps the simulation by a fixed time; the offscreen preview uses this.
    pub fn advance(&mut self, seconds: f32) {
        for particle in &mut self.particles {
            particle.velocity_y += GRAVITY * seconds;
            particle.x += particle.velocity_x * seconds;
            particle.y += particle.velocity_y * seconds;
            particle.age += seconds;
        }
        self.particles.retain(|particle| particle.age < particle.lifetime);
    }

    pub fn draw(&self, target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, glyph_format: &IDWriteTextFormat) {
        for particle in &self.particles {
            let progress = particle.age / particle.lifetime;
            let mut color = particle.color;
            color.a = 1.0 - progress * progress;
            unsafe { brush.SetColor(&color) };
            match particle.glyph {
                Some(glyph) => {
                    let mut buffer = [0u16; 2];
                    let utf16 = glyph.encode_utf16(&mut buffer);
                    let area = D2D_RECT_F {
                        left: particle.x - 6.0,
                        top: particle.y - 9.0,
                        right: particle.x + 12.0,
                        bottom: particle.y + 12.0,
                    };
                    unsafe {
                        target.DrawText(utf16, glyph_format, &area, brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                    }
                }
                None => {
                    // Snapped to whole pixels, so the squares stay crisp like voxels.
                    let left = particle.x.round();
                    let top = particle.y.round();
                    let square = D2D_RECT_F {
                        left,
                        top,
                        right: left + particle.size,
                        bottom: top + particle.size,
                    };
                    unsafe { target.FillRectangle(&square, brush) };
                }
            }
        }
    }

    fn random(&mut self) -> f32 {
        self.random_state ^= self.random_state << 13;
        self.random_state ^= self.random_state >> 17;
        self.random_state ^= self.random_state << 5;
        self.random_state as f32 / u32::MAX as f32
    }
}
