//! Cuts a recording to a part of it, as a new file next to it. Each frame is decoded and
//! encoded again, because a cut that only copies data can start at key frames alone, seconds
//! off from where the user asked.

use std::path::{Path, PathBuf};
use std::time::Duration;

use windows::Win32::Media::MediaFoundation::{
    IMFSourceReader, MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
    MF_MT_FRAME_RATE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_PD_DURATION, MF_SOURCE_READER_ANY_STREAM,
    MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_SOURCE_READER_MEDIASOURCE,
    MF_SOURCE_READERF_ENDOFSTREAM, MFAudioFormat_PCM, MFCreateMediaType, MFMediaType_Audio,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Variant::VT_I8;
use windows::core::GUID;

use crate::capture::PixelRect;
use crate::gif_export::{self, FrameLayout};
use crate::output::Image;
use crate::recorder::{self, AUDIO_BYTES_PER_FRAME, AUDIO_CHANNELS, AUDIO_RATE, TICKS_PER_SECOND};

type TrimResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Shows single frames of a recording, for the trim window.
pub struct FramePreview {
    reader: IMFSourceReader,
    layout: FrameLayout,
    pub duration: Duration,
}

impl FramePreview {
    pub fn open(recording: &Path) -> TrimResult<Self> {
        recorder::start_media_foundation()?;
        let video_stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        let reader = gif_export::open_reader(recording, video_stream)?;
        let layout = gif_export::frame_layout(&reader, video_stream)?;
        let duration = source_duration(&reader)?;
        Ok(Self { reader, layout, duration })
    }

    pub fn size(&self) -> (u32, u32) {
        self.layout.shown
    }

    /// The frame showing at `time`: the last one that started by then, the same frame a cut
    /// from `time` opens on.
    pub fn frame_at(&mut self, time: Duration) -> TrimResult<Image> {
        seek(&self.reader, time)?;
        let wanted = ticks(time);
        let video_stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        let mut latest = None;
        loop {
            let mut flags = 0;
            let mut sample_time = 0;
            let mut sample = None;
            unsafe { self.reader.ReadSample(video_stream, 0, None, Some(&mut flags), Some(&mut sample_time), Some(&mut sample))? };
            let at_the_end = flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0;
            if at_the_end || (sample_time > wanted && latest.is_some()) {
                break;
            }
            if let Some(sample) = sample {
                latest = Some(unsafe { sample.ConvertToContiguousBuffer()? });
            }
        }
        let buffer = latest.ok_or("the recording has no frame there")?;
        let bgra = gif_export::read_frame(&buffer, &self.layout, |top_row, pitch| top_down_rows(top_row, pitch, self.layout.shown))?;
        Ok(Image {
            width: self.layout.shown.0,
            height: self.layout.shown.1,
            bgra,
        })
    }
}

fn ticks(time: Duration) -> i64 {
    (time.as_secs_f64() * TICKS_PER_SECOND as f64) as i64
}

fn seek(reader: &IMFSourceReader, time: Duration) -> TrimResult<()> {
    let mut position = PROPVARIANT::default();
    unsafe {
        let value = &mut *position.Anonymous.Anonymous;
        value.vt = VT_I8;
        value.Anonymous.hVal = ticks(time);
        reader.SetCurrentPosition(&GUID::zeroed(), &position)?;
    }
    Ok(())
}

fn source_duration(reader: &IMFSourceReader) -> TrimResult<Duration> {
    let value = unsafe { reader.GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)? };
    let duration_ticks = unsafe { value.Anonymous.Anonymous.Anonymous.uhVal };
    Ok(Duration::from_nanos(duration_ticks.saturating_mul(100)))
}

/// Tight rows from top to bottom, opaque, in BGRA.
fn top_down_rows(top_row: *const u8, pitch: isize, size: (u32, u32)) -> Vec<u8> {
    let row_bytes = size.0 as usize * 4;
    let mut bgra = Vec::with_capacity(row_bytes * size.1 as usize);
    for row in 0..size.1 as isize {
        let start = unsafe { top_row.offset(row * pitch) };
        bgra.extend_from_slice(unsafe { std::slice::from_raw_parts(start, row_bytes) });
    }
    for pixel in bgra.chunks_exact_mut(4) {
        pixel[3] = 255;
    }
    bgra
}

/// "Screen Recording 1 trimmed.mp4", or with a number when that exists.
fn trimmed_path(recording: &Path) -> PathBuf {
    let stem = recording.file_stem().map(|stem| stem.to_string_lossy().to_string()).unwrap_or_default();
    let mut path = recording.with_file_name(format!("{stem} trimmed.mp4"));
    let mut copy_number = 2;
    while path.exists() {
        path = recording.with_file_name(format!("{stem} trimmed {copy_number}.mp4"));
        copy_number += 1;
    }
    path
}

/// A finished cut.
pub struct Trimmed {
    pub path: PathBuf,
    pub duration: Duration,
    pub first_frame: Image,
}

/// Writes the part from `from` to `to` to a new file. The recording itself stays as it is.
pub fn trim(recording: &Path, from: Duration, to: Duration) -> Result<Trimmed, String> {
    recorder::start_media_foundation().map_err(|error| error.message())?;
    let path = trimmed_path(recording);
    match write_part(recording, &path, from, to) {
        Ok(first_frame) => Ok(Trimmed {
            path,
            duration: to.saturating_sub(from),
            first_frame,
        }),
        Err(error) => {
            let _ = std::fs::remove_file(&path);
            Err(error.to_string())
        }
    }
}

/// The part of an audio buffer inside the cut, as (bytes to skip, bytes to keep).
fn audio_part(start: i64, byte_count: usize, from: i64, to: i64) -> (usize, usize) {
    // Whole numbers, because rounding must not leave a sample split between two channels.
    let bytes_up_to = |time: i64| {
        let sound_frames = (time - start).max(0) * i64::from(AUDIO_RATE) / TICKS_PER_SECOND;
        (sound_frames as usize * AUDIO_BYTES_PER_FRAME as usize).min(byte_count)
    };
    let skip = bytes_up_to(from);
    (skip, bytes_up_to(to).saturating_sub(skip))
}

/// Asks the reader for PCM in the format the writer takes. Recordings without such sound,
/// or without any, are cut as picture alone.
fn select_audio(reader: &IMFSourceReader) -> Option<u32> {
    let stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
    unsafe {
        reader.SetStreamSelection(stream, true).ok()?;
        let wanted = MFCreateMediaType().ok()?;
        wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).ok()?;
        wanted.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).ok()?;
        wanted.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AUDIO_RATE).ok()?;
        wanted.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, AUDIO_CHANNELS).ok()?;
        wanted.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).ok()?;
        reader.SetCurrentMediaType(stream, None, &wanted).ok()?;
    }
    Some(stream)
}

fn write_part(recording: &Path, path: &Path, from: Duration, to: Duration) -> TrimResult<Image> {
    let video_stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let reader = gif_export::open_reader(recording, video_stream)?;
    let layout = gif_export::frame_layout(&reader, video_stream)?;
    let packed_rate = unsafe { reader.GetCurrentMediaType(video_stream)?.GetUINT64(&MF_MT_FRAME_RATE)? };
    let frames_per_second = ((packed_rate >> 32) as u32 / (packed_rate as u32).max(1)).max(1);
    let audio_stream = select_audio(&reader);
    let video_index = video_stream_index(&reader);

    let area = PixelRect {
        left: 0,
        top: 0,
        width: layout.shown.0 as i32,
        height: layout.shown.1 as i32,
    };
    let writer = recorder::open_writer(path)?;
    let written_video = recorder::add_video_stream(&writer, area, frames_per_second)?;
    let written_audio = match audio_stream {
        Some(_) => Some(recorder::add_audio_stream(&writer)?),
        None => None,
    };
    unsafe { writer.BeginWriting()? };
    seek(&reader, from)?;

    let from_ticks = ticks(from);
    let to_ticks = ticks(to);
    let frame_ticks = TICKS_PER_SECOND / i64::from(frames_per_second);
    let mut video_open = true;
    let mut audio_open = audio_stream.is_some();
    let mut first_frame: Option<Image> = None;
    let mut last_video_ticks = -1;
    // The cut opens on the frame showing at `from`, which usually started a little earlier.
    let mut frame_before_cut: Option<Vec<u8>> = None;
    while video_open || audio_open {
        let mut stream = 0;
        let mut flags = 0;
        let mut time = 0;
        let mut sample = None;
        unsafe {
            reader.ReadSample(
                MF_SOURCE_READER_ANY_STREAM.0 as u32,
                0,
                Some(&mut stream),
                Some(&mut flags),
                Some(&mut time),
                Some(&mut sample),
            )?;
        }
        let is_video = stream == video_index;
        let ended = flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0;
        if ended || time >= to_ticks {
            if is_video {
                video_open = false;
            } else {
                audio_open = false;
            }
            // A stream past the cut stops here, and one at its end has already stopped.
            if !ended {
                let selected_stream = if is_video { video_stream } else { audio_stream.unwrap_or(video_stream) };
                unsafe { reader.SetStreamSelection(selected_stream, false)? };
            }
            continue;
        }
        let Some(sample) = sample else {
            continue;
        };
        let buffer = unsafe { sample.ConvertToContiguousBuffer()? };
        if is_video {
            let bgra = gif_export::read_frame(&buffer, &layout, |top_row, pitch| top_down_rows(top_row, pitch, layout.shown))?;
            if time <= from_ticks {
                frame_before_cut = Some(bgra);
                continue;
            }
            if first_frame.is_none() {
                // Nothing at or before `from` means the cut opens on this frame.
                let opening = frame_before_cut.take().unwrap_or_else(|| bgra.clone());
                let out = recorder::bytes_sample(&opening, 0, frame_ticks)?;
                unsafe { writer.WriteSample(written_video, &out)? };
                last_video_ticks = 0;
                first_frame = Some(Image {
                    width: layout.shown.0,
                    height: layout.shown.1,
                    bgra: opening,
                });
            }
            let shifted = (time - from_ticks).max(last_video_ticks + 1);
            last_video_ticks = shifted;
            let out = recorder::bytes_sample(&bgra, shifted, frame_ticks)?;
            unsafe { writer.WriteSample(written_video, &out)? };
        } else if let Some(written_audio) = written_audio {
            let mut start = std::ptr::null_mut();
            let mut length = 0;
            unsafe { buffer.Lock(&mut start, None, Some(&mut length))? };
            let (skip, keep) = audio_part(time, length as usize, from_ticks, to_ticks);
            let result = if keep > 0 {
                let bytes = unsafe { std::slice::from_raw_parts(start.add(skip), keep) };
                let start_ticks = (time - from_ticks).max(0);
                let keep_ticks = keep as i64 * TICKS_PER_SECOND / i64::from(AUDIO_RATE * AUDIO_BYTES_PER_FRAME);
                recorder::bytes_sample(bytes, start_ticks, keep_ticks).map(Some)
            } else {
                Ok(None)
            };
            unsafe { buffer.Unlock()? };
            if let Some(out) = result? {
                unsafe { writer.WriteSample(written_audio, &out)? };
            }
        }
    }
    // A cut shorter than one frame, or at the very end, has only the frame showing there.
    if first_frame.is_none() {
        if let Some(opening) = frame_before_cut.take() {
            let out = recorder::bytes_sample(&opening, 0, frame_ticks)?;
            unsafe { writer.WriteSample(written_video, &out)? };
            first_frame = Some(Image {
                width: layout.shown.0,
                height: layout.shown.1,
                bgra: opening,
            });
        }
    }
    let first_frame = first_frame.ok_or("that part of the recording has no frame")?;
    unsafe { writer.Finalize()? };
    Ok(first_frame)
}

/// The index the reader reports for the first video stream: it numbers streams in file order.
fn video_stream_index(reader: &IMFSourceReader) -> u32 {
    let mut index = 0;
    loop {
        let found = unsafe { reader.GetNativeMediaType(index, 0) };
        let Ok(media_type) = found else {
            return 0;
        };
        let major = unsafe { media_type.GetGUID(&MF_MT_MAJOR_TYPE) };
        if major.is_ok_and(|major| major == windows::Win32::Media::MediaFoundation::MFMediaType_Video) {
            return index;
        }
        index += 1;
    }
}

/// `oh-snap trim FILE --from SECONDS --to SECONDS`: cuts without any window.
pub fn command(arguments: &[String]) -> TrimResult<()> {
    let recording = arguments.first().ok_or("usage: oh-snap trim FILE --from SECONDS --to SECONDS")?;
    let mut from = 0.0;
    let mut to = None;
    let mut index = 1;
    while index < arguments.len() {
        let value: f64 = arguments.get(index + 1).ok_or("--from and --to need a number of seconds")?.parse()?;
        match arguments[index].as_str() {
            "--from" => from = value,
            "--to" => to = Some(value),
            other => return Err(format!("unknown trim option `{other}`").into()),
        }
        index += 2;
    }
    let recording = Path::new(recording);
    if to.is_some_and(|to| to <= from) {
        return Err("--to must be after --from".into());
    }
    let to = match to {
        Some(seconds) => Duration::from_secs_f64(seconds),
        None => FramePreview::open(recording)?.duration,
    };
    let trimmed = trim(recording, Duration::from_secs_f64(from), to)?;
    println!("{}", trimmed.path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_new_file_never_overwrites_one() {
        let folder = std::env::temp_dir().join(format!("oh-snap-trim-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let recording = folder.join("clip.mp4");
        assert_eq!(trimmed_path(&recording), folder.join("clip trimmed.mp4"));
        std::fs::write(folder.join("clip trimmed.mp4"), b"x").unwrap();
        assert_eq!(trimmed_path(&recording), folder.join("clip trimmed 2.mp4"));
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn audio_is_cut_on_whole_frames_at_both_ends() {
        // One second of sound starting at 0, cut from 0.5 s to 0.75 s.
        let second = TICKS_PER_SECOND;
        let bytes = (AUDIO_RATE * AUDIO_BYTES_PER_FRAME) as usize;
        let (skip, keep) = audio_part(0, bytes, second / 2, second * 3 / 4);
        assert_eq!(skip, bytes / 2);
        assert_eq!(keep, bytes / 4);
        assert_eq!(skip % AUDIO_BYTES_PER_FRAME as usize, 0);
    }

    #[test]
    fn audio_outside_the_cut_keeps_nothing() {
        let second = TICKS_PER_SECOND;
        let bytes = (AUDIO_RATE * AUDIO_BYTES_PER_FRAME) as usize;
        assert_eq!(audio_part(0, bytes, second * 2, second * 3).1, 0);
    }

    #[test]
    fn rows_come_out_top_down_and_opaque() {
        // Two rows stored bottom to top: the pointer starts at the top row and walks backwards.
        let stored: [u8; 8] = [3, 3, 3, 0, 1, 1, 1, 0];
        let top_row = stored[4..].as_ptr();
        let rows = top_down_rows(top_row, -4, (1, 2));
        assert_eq!(rows, vec![1, 1, 1, 255, 3, 3, 3, 255]);
    }
}
