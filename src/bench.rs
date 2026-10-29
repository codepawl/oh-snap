//! `oh-snap bench`: the numbers the plan promises, measured on this machine.
//! It never touches the clipboard or the screenshots folder.

use std::time::{Duration, Instant};

use crate::capture::Capturer;
use crate::output;
use crate::overlay::{self, Graphics, Mode, OverlayOptions};

pub fn run(overlay_rounds: usize) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let mut capturer = Capturer::new()?;
    let init_time = started.elapsed();
    let first_capture_started = Instant::now();
    capturer.capture()?;
    let first_capture_time = first_capture_started.elapsed();

    let bounds = capturer.screenshot().bounds;
    println!(
        "desktop    {}x{} over {} monitor(s)",
        bounds.width,
        bounds.height,
        capturer.screenshot().monitors.len()
    );
    println!("start      duplication {} + first frame {}", millis(init_time), millis(first_capture_time));

    let mut warm_captures = Vec::new();
    for _ in 0..20 {
        let capture_started = Instant::now();
        capturer.capture()?;
        warm_captures.push(capture_started.elapsed());
        std::thread::sleep(Duration::from_millis(15));
    }
    println!("capture    {}", summary(&mut warm_captures));

    let graphics_started = Instant::now();
    let graphics = Graphics::new()?;
    overlay::warm_up(&graphics);
    println!("graphics   {} (once per daemon, with warm-up)", millis(graphics_started.elapsed()));

    let mut overlay_times = Vec::new();
    for _ in 0..overlay_rounds {
        let shortcut_pressed = Instant::now();
        let screenshot = capturer.capture()?;
        let options = OverlayOptions {
            start_mode: Mode::Region,
            close_after_first_frame: true,
            particles: false,
        };
        let outcome = overlay::select_area(&graphics, screenshot, options)?;
        if let Some(first_frame_at) = outcome.first_frame_at {
            overlay_times.push(first_frame_at - shortcut_pressed);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !overlay_times.is_empty() {
        println!("overlay    {}  (shortcut to first frame, goal < 50 ms)", summary(&mut overlay_times));
    }

    let monitor = capturer.screenshot().monitors[0];
    let deliver_started = Instant::now();
    let image = output::crop(capturer.screenshot(), monitor).ok_or("monitor is off the desktop")?;
    let crop_time = deliver_started.elapsed();
    let encode_started = Instant::now();
    let png_bytes = output::encode_png(&image)?;
    let encode_time = encode_started.elapsed();
    println!(
        "deliver    {}x{}: crop {} + png {} ({} KB)",
        image.width,
        image.height,
        millis(crop_time),
        millis(encode_time),
        png_bytes.len() / 1024
    );
    Ok(())
}

fn millis(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
}

fn summary(samples: &mut [Duration]) -> String {
    samples.sort();
    let median = samples[samples.len() / 2];
    let worst = samples[samples.len() - 1];
    format!("median {}  worst {}  (n={})", millis(median), millis(worst), samples.len())
}
