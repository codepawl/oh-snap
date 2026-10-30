//! Turns a recording into a GIF next to it. Media Foundation decodes the MP4. Each frame
//! keeps only the pixels that changed, which suits screen content and keeps files small.

use std::path::{Path, PathBuf};

use windows::Win32::Media::MediaFoundation::{
    IMFMediaBuffer, IMFSourceReader, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_MINIMUM_DISPLAY_APERTURE,
    MF_MT_SUBTYPE, MF_SOURCE_READER_ALL_STREAMS, MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_SOURCE_READERF_ENDOFSTREAM, MFCreateAttributes, MFCreateMediaType,
    MFCreateSourceReaderFromURL, MFMediaType_Video, MFVideoFormat_RGB32,
};
use windows::core::{Error, HSTRING};

const LARGEST_WIDTH: u32 = 960;
/// GIF timing is in hundredths of a second; about 15 frames a second looks smooth enough.
const SMALLEST_DELAY_TICKS: i64 = 660_000;
const TICKS_PER_CENTISECOND: i64 = 100_000;
/// The decoder leaves faint noise on still content; this much counts as no change.
const SAME_COLOR_TOLERANCE: i16 = 6;
const QUANTIZER_SPEED: i32 = 10;

type ExportResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Writes "<recording>.gif" and returns its path. Blocks, so run it off the window thread.
pub fn export(recording: &Path) -> std::result::Result<PathBuf, String> {
    let gif_path = recording.with_extension("gif");
    crate::recorder::start_media_foundation().map_err(|error| error.message())?;
    match convert(recording, &gif_path) {
        Ok(()) => Ok(gif_path),
        Err(error) => {
            let _ = std::fs::remove_file(&gif_path);
            Err(error.to_string())
        }
    }
}

fn convert(recording: &Path, gif_path: &Path) -> ExportResult<()> {
    let video_stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let reader = open_reader(recording, video_stream)?;
    let layout = frame_layout(&reader, video_stream)?;
    let shown_size = layout.shown;
    let (gif_width, gif_height) = gif_size(shown_size);

    let file = std::io::BufWriter::new(std::fs::File::create(gif_path)?);
    let mut encoder = gif::Encoder::new(file, gif_width as u16, gif_height as u16, &[])?;
    encoder.set_repeat(gif::Repeat::Infinite)?;

    let mut shown = vec![0u8; gif_width as usize * gif_height as usize * 4];
    let mut has_shown_frame = false;
    let mut pending: Option<(gif::Frame<'static>, i64)> = None;
    let mut last_taken_time = None;
    let mut last_time = 0;
    loop {
        let mut flags = 0;
        let mut time = 0;
        let mut sample = None;
        unsafe { reader.ReadSample(video_stream, 0, None, Some(&mut flags), Some(&mut time), Some(&mut sample))? };
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            break;
        }
        let Some(sample) = sample else {
            continue;
        };
        last_time = time;
        if last_taken_time.is_some_and(|taken: i64| time - taken < SMALLEST_DELAY_TICKS) {
            continue;
        }
        last_taken_time = Some(time);
        let buffer = unsafe { sample.ConvertToContiguousBuffer()? };
        let picture = read_picture(&buffer, &layout, (gif_width, gif_height))?;
        let mut changes = changed_pixels(&picture, &mut shown, has_shown_frame);
        has_shown_frame = true;
        if !changes.iter().skip(3).step_by(4).any(|alpha| *alpha != 0) {
            continue;
        }
        let mut frame = gif::Frame::from_rgba_speed(gif_width as u16, gif_height as u16, &mut changes, QUANTIZER_SPEED);
        frame.dispose = gif::DisposalMethod::Keep;
        if let Some((mut earlier, started)) = pending.take() {
            earlier.delay = centiseconds(time - started);
            encoder.write_frame(&earlier)?;
        }
        pending = Some((frame, time));
    }
    if let Some((mut last, started)) = pending.take() {
        last.delay = centiseconds(last_time - started).max(10);
        encoder.write_frame(&last)?;
    }
    Ok(())
}

fn centiseconds(ticks: i64) -> u16 {
    (ticks / TICKS_PER_CENTISECOND).clamp(1, i64::from(u16::MAX)) as u16
}

/// The frame's RGBA with unchanged pixels see-through, and `shown` updated to match.
fn changed_pixels(picture: &[u8], shown: &mut [u8], has_shown_frame: bool) -> Vec<u8> {
    let mut changes = vec![0u8; picture.len()];
    let pixels = picture.chunks_exact(4).zip(shown.chunks_exact_mut(4)).zip(changes.chunks_exact_mut(4));
    for ((new_pixel, shown_pixel), changed_pixel) in pixels {
        if has_shown_frame && colors_match(new_pixel, shown_pixel) {
            continue;
        }
        shown_pixel.copy_from_slice(new_pixel);
        changed_pixel.copy_from_slice(new_pixel);
    }
    changes
}

fn colors_match(left: &[u8], right: &[u8]) -> bool {
    left[..3]
        .iter()
        .zip(&right[..3])
        .all(|(left_channel, right_channel)| (i16::from(*left_channel) - i16::from(*right_channel)).abs() <= SAME_COLOR_TOLERANCE)
}

/// The size of the GIF: the recording's, or narrower when it is wide.
fn gif_size(shown_size: (u32, u32)) -> (u32, u32) {
    let (width, height) = shown_size;
    if width <= LARGEST_WIDTH {
        return (width, height);
    }
    let scaled_height = u64::from(height) * u64::from(LARGEST_WIDTH) / u64::from(width);
    (LARGEST_WIDTH, (scaled_height as u32).max(1))
}

fn open_reader(recording: &Path, video_stream: u32) -> ExportResult<IMFSourceReader> {
    unsafe {
        let mut attributes = None;
        MFCreateAttributes(&mut attributes, 1)?;
        let attributes = attributes.ok_or_else(Error::empty)?;
        // Lets the reader convert H.264 frames to RGB by itself.
        attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
        let reader = MFCreateSourceReaderFromURL(&HSTRING::from(recording.as_os_str()), &attributes)?;
        reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
        reader.SetStreamSelection(video_stream, true)?;
        let wanted = MFCreateMediaType()?;
        wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        wanted.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        reader.SetCurrentMediaType(video_stream, None, &wanted)?;
        Ok(reader)
    }
}

struct FrameLayout {
    /// The part to show. H.264 codes 1080 rows as 1088, and the aperture says which is the picture.
    shown: (u32, u32),
    /// Bytes from one row to the next. Negative when the rows run bottom to top.
    stride: i32,
}

fn frame_layout(reader: &IMFSourceReader, video_stream: u32) -> ExportResult<FrameLayout> {
    unsafe {
        let media_type = reader.GetCurrentMediaType(video_stream)?;
        let packed = media_type.GetUINT64(&MF_MT_FRAME_SIZE)?;
        let coded = ((packed >> 32) as u32, packed as u32);
        let mut aperture = [0u8; 16];
        let mut aperture_length = 0;
        let shown = match media_type.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, &mut aperture, Some(&mut aperture_length)) {
            Ok(()) if aperture_length == 16 => {
                let width = u32::from_le_bytes([aperture[8], aperture[9], aperture[10], aperture[11]]);
                let height = u32::from_le_bytes([aperture[12], aperture[13], aperture[14], aperture[15]]);
                (width.min(coded.0), height.min(coded.1))
            }
            _ => coded,
        };
        // RGB buffers run bottom to top unless the type says otherwise.
        let stride = media_type
            .GetUINT32(&MF_MT_DEFAULT_STRIDE)
            .map(|stride| stride as i32)
            .unwrap_or(-((coded.0 * 4) as i32));
        Ok(FrameLayout { shown, stride })
    }
}

/// One decoded frame as top-down RGBA at the GIF's size. The stride's sign covers both
/// row orders.
fn read_picture(buffer: &IMFMediaBuffer, layout: &FrameLayout, gif_size: (u32, u32)) -> ExportResult<Vec<u8>> {
    let mut start = std::ptr::null_mut();
    let mut length = 0;
    unsafe { buffer.Lock(&mut start, None, Some(&mut length))? };
    let row_bytes = layout.stride.unsigned_abs() as usize;
    let row_count = length as usize / row_bytes.max(1);
    let result = if row_count < layout.shown.1 as usize || row_bytes < layout.shown.0 as usize * 4 {
        Err(format!("a decoded frame has {length} bytes, too few for {:?}", layout.shown).into())
    } else {
        let top_row = if layout.stride < 0 {
            unsafe { start.add((row_count - 1) * row_bytes) }
        } else {
            start
        };
        Ok(scale_to_rgba(top_row, layout.stride as isize, layout.shown, gif_size))
    };
    unsafe { buffer.Unlock()? };
    result
}

/// Averages the source pixels behind each GIF pixel, so thin text survives shrinking.
fn scale_to_rgba(first_row: *const u8, pitch: isize, shown_size: (u32, u32), gif_size: (u32, u32)) -> Vec<u8> {
    let (shown_width, shown_height) = (shown_size.0 as usize, shown_size.1 as usize);
    let (gif_width, gif_height) = (gif_size.0 as usize, gif_size.1 as usize);
    let mut rgba = Vec::with_capacity(gif_width * gif_height * 4);
    for gif_y in 0..gif_height {
        let source_top = gif_y * shown_height / gif_height;
        let source_bottom = ((gif_y + 1) * shown_height / gif_height).max(source_top + 1);
        for gif_x in 0..gif_width {
            let source_left = gif_x * shown_width / gif_width;
            let source_right = ((gif_x + 1) * shown_width / gif_width).max(source_left + 1);
            let mut totals = [0u32; 3];
            for source_y in source_top..source_bottom {
                let row = unsafe { first_row.offset(source_y as isize * pitch) };
                for source_x in source_left..source_right {
                    let pixel = unsafe { row.add(source_x * 4) };
                    for (channel, total) in totals.iter_mut().enumerate() {
                        *total += u32::from(unsafe { *pixel.add(channel) });
                    }
                }
            }
            let count = ((source_bottom - source_top) * (source_right - source_left)) as u32;
            rgba.extend_from_slice(&[(totals[2] / count) as u8, (totals[1] / count) as u8, (totals[0] / count) as u8, 255]);
        }
    }
    rgba
}

/// `oh-snap gif FILE`: makes a GIF from a recording and prints where it went.
pub fn command(path: &str) -> ExportResult<()> {
    let gif_path = export(Path::new(path))?;
    println!("{}", gif_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_recordings_shrink_and_narrow_ones_do_not() {
        assert_eq!(gif_size((400, 300)), (400, 300));
        assert_eq!(gif_size((1920, 1080)), (960, 540));
    }

    #[test]
    fn only_changed_pixels_stay_visible() {
        let mut shown = vec![10, 10, 10, 255, 10, 10, 10, 255];
        let picture = vec![13, 10, 10, 255, 200, 10, 10, 255];
        let changes = changed_pixels(&picture, &mut shown, true);
        assert_eq!(changes[3], 0);
        assert_eq!(&changes[4..8], &[200, 10, 10, 255]);
        assert_eq!(&shown[..4], &[10, 10, 10, 255]);
        assert_eq!(&shown[4..8], &[200, 10, 10, 255]);
    }

    #[test]
    fn the_first_frame_shows_everything() {
        let mut shown = vec![0, 0, 0, 0];
        let changes = changed_pixels(&[0, 0, 0, 255], &mut shown, false);
        assert_eq!(changes, vec![0, 0, 0, 255]);
    }

    #[test]
    fn scaling_averages_and_swaps_blue_and_red() {
        // Two BGRA pixels shrink to one: blue 100 and 200 average to 150.
        let pixels: [u8; 8] = [100, 0, 50, 255, 200, 0, 50, 255];
        let rgba = scale_to_rgba(pixels.as_ptr(), 8, (2, 1), (1, 1));
        assert_eq!(rgba, vec![50, 0, 150, 255]);
    }
}
