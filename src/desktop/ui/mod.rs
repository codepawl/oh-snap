//! The windows of the shared app, drawn into `tiny_skia::Pixmap`s by plain functions so the
//! layout, hit-testing and look can be tested (and written out as PNGs) on any OS. The
//! winit shell in `desktop::app` only feeds these functions events and blits the pixmaps.

pub mod canvas;
pub mod card;
pub mod geometry;
pub mod markup;
pub mod overlay;
pub mod recording_bar;
pub mod text;
pub mod trim;
pub mod theme;

#[cfg(any(test, windows))]
pub mod testing;
