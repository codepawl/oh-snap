//! A short, quiet shutter sound, synthesised at start-up so no audio file ships.

use std::sync::OnceLock;

use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
use windows::core::PCWSTR;

const SAMPLE_RATE: u32 = 44_100;

static SHUTTER_WAV: OnceLock<Vec<u8>> = OnceLock::new();

/// Plays without blocking; a sound already playing is cut off.
pub fn play_shutter() {
    let wav = SHUTTER_WAV.get_or_init(build_shutter_wav);
    unsafe {
        let _ = PlaySoundW(PCWSTR(wav.as_ptr() as *const u16), None, SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}

/// Two clicks 55 ms apart, like a leaf shutter opening and closing. Each click is a
/// burst of bright noise over a short low thump.
fn build_shutter_wav() -> Vec<u8> {
    let length = (SAMPLE_RATE as f32 * 0.14) as usize;
    let mut samples = vec![0.0f32; length];
    add_click(&mut samples, 0.0, 1.0, 190.0);
    add_click(&mut samples, 0.055, 0.7, 240.0);
    let pcm: Vec<i16> = samples
        .iter()
        .map(|sample| (sample.clamp(-1.0, 1.0) * 0.6 * i16::MAX as f32) as i16)
        .collect();
    wav_bytes(&pcm)
}

fn add_click(samples: &mut [f32], start_seconds: f32, loudness: f32, thump_hz: f32) {
    let start = (start_seconds * SAMPLE_RATE as f32) as usize;
    let mut noise_state: u32 = 0x9E37_79B9 ^ start as u32;
    let mut previous_noise = 0.0f32;
    for offset in 0..(SAMPLE_RATE as usize / 20) {
        let Some(sample) = samples.get_mut(start + offset) else {
            break;
        };
        let time = offset as f32 / SAMPLE_RATE as f32;
        noise_state ^= noise_state << 13;
        noise_state ^= noise_state >> 17;
        noise_state ^= noise_state << 5;
        let noise = (noise_state as f32 / u32::MAX as f32) * 2.0 - 1.0;
        // Differencing the noise removes its low end, so the click sounds crisp, not hissy.
        let bright_noise = noise - previous_noise;
        previous_noise = noise;
        let click = bright_noise * (-time * 900.0).exp();
        let thump = (std::f32::consts::TAU * thump_hz * time).sin() * (-time * 120.0).exp() * 0.5;
        *sample += (click + thump) * loudness;
    }
}

fn wav_bytes(pcm: &[i16]) -> Vec<u8> {
    let data_size = (pcm.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data_size as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    bytes.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_size.to_le_bytes());
    for sample in pcm {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

/// For checking the sound by ear or with a spectrogram: `oh-snap sound FILE.wav`.
pub fn write_shutter(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::write(path, build_shutter_wav())
}
