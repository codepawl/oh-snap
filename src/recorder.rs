//! Records an area of the screen to MP4: frames from the capture device, the pointer drawn
//! on top, H.264 and AAC through Media Foundation, and system sound through WASAPI loopback.
//! All of it ships with Windows, so the program stays small.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{POINT, RPC_E_CHANGED_MODE};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    HBITMAP, HDC, HGDIOBJ, SelectObject,
};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, EDataFlow, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
    MMDeviceEnumerator, WAVE_FORMAT_PCM, WAVEFORMATEX, eCapture, eConsole, eRender,
};
use windows::Win32::Media::MediaFoundation::{
    IMFMediaType, IMFSample, IMFSinkWriter, MF_MT_AAC_PAYLOAD_TYPE, MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
    MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND,
    MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_VERSION, MFAudioFormat_AAC,
    MFAudioFormat_PCM, MFCreateAttributes, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFCreateSinkWriterFromURL, MFMediaType_Audio, MFMediaType_Video, MFSTARTUP_FULL, MFStartup,
    MFVideoFormat_H264, MFVideoFormat_RGB32, MFVideoInterlace_Progressive,
};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree};
use windows::Win32::UI::Shell::{FOLDERID_Videos, KF_FLAG_CREATE, SHGetKnownFolderPath};
use windows::Win32::UI::WindowsAndMessaging::{
    CURSOR_SHOWING, CURSORINFO, DI_NORMAL, DrawIconEx, GetCursorInfo, GetIconInfo, HCURSOR, HICON, ICONINFO,
};
use windows::core::{Error, HSTRING, Result};

use crate::capture::{Capturer, PixelRect, Screenshot};
use crate::output::Image;

/// Media Foundation counts time in units of 100 nanoseconds.
const TICKS_PER_SECOND: i64 = 10_000_000;
const AUDIO_RATE: u32 = 48_000;
const AUDIO_CHANNELS: u32 = 2;
const AUDIO_BYTES_PER_FRAME: u32 = 4;
const AAC_BYTES_PER_SECOND: u32 = 24_000;
/// Screen content is mostly still, so this is plenty and keeps files small.
const BITS_PER_PIXEL_FRAME: f64 = 0.12;
/// Encoders refuse anything smaller.
const SMALLEST_SIDE: i32 = 16;
/// How far the sound stays behind the picture, so a source that delivers late still mixes in.
const SOUND_HELD_BACK: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug)]
pub struct RecordOptions {
    pub frames_per_second: u32,
    pub system_sound: bool,
    pub microphone: bool,
}

/// Shared with the thread that records: set these and it follows.
#[derive(Default)]
pub struct Controls {
    /// The engine takes close to a second to start, so it gets ready during the countdown
    /// and waits for this, and the first second is not lost.
    pub go: AtomicBool,
    pub stop: AtomicBool,
    pub paused: AtomicBool,
    /// Set by the engine: the time recorded so far, without pauses.
    pub recorded_milliseconds: AtomicU64,
    /// Set by the engine when the first frame is written.
    pub recording: AtomicBool,
}

pub struct Recording {
    pub path: PathBuf,
    pub duration: Duration,
    /// The first frame, for the result card.
    pub first_frame: Image,
    /// False when sound was asked for and no output device could give it.
    pub has_sound: bool,
}

/// Starts recording on its own thread. `on_done` runs on that thread when it has ended.
pub fn start(
    area: PixelRect,
    options: RecordOptions,
    on_done: impl FnOnce(std::result::Result<Recording, String>) + Send + 'static,
) -> Arc<Controls> {
    let controls = Arc::new(Controls::default());
    let thread_controls = controls.clone();
    std::thread::spawn(move || {
        let path = new_recording_path();
        on_done(record(area, options, &path, &thread_controls).map_err(|error| error.message()));
    });
    controls
}

/// "0:12", or "1:02:03" past an hour.
pub fn clock_text(milliseconds: u64) -> String {
    let seconds = milliseconds / 1000;
    if seconds >= 3600 {
        return format!("{}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60);
    }
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// Windows' own folder for recordings, so Photos and Explorer list ours too.
pub fn recordings_folder() -> PathBuf {
    let known = unsafe { SHGetKnownFolderPath(&FOLDERID_Videos, KF_FLAG_CREATE, None) };
    let videos = match known {
        Ok(raw_path) => {
            let path = unsafe { raw_path.to_string() }.unwrap_or_default();
            unsafe { CoTaskMemFree(Some(raw_path.0 as *const _)) };
            PathBuf::from(path)
        }
        Err(_) => Path::new(&std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into())).join("Videos"),
    };
    videos.join("Screen Recordings")
}

fn new_recording_path() -> PathBuf {
    let folder = recordings_folder();
    let _ = std::fs::create_dir_all(&folder);
    let stem = crate::output::screenshot_file_stem().replacen("Screenshot", "Screen Recording", 1);
    let mut path = folder.join(format!("{stem}.mp4"));
    let mut copy_number = 2;
    while path.exists() {
        path = folder.join(format!("{stem} ({copy_number}).mp4"));
        copy_number += 1;
    }
    path
}

/// Records until `controls.stop` is set, then finishes the file. A capture failure midway
/// also ends the recording, and what was recorded until then is kept.
pub fn record(area: PixelRect, options: RecordOptions, path: &Path, controls: &Controls) -> Result<Recording> {
    start_media_foundation()?;
    record_started(area, options, path, controls)
}

/// Readies this thread and, once per process, Media Foundation. It stays started: recordings
/// and GIFs can overlap, and the tray's idle trim gives its memory back anyway.
pub fn start_media_foundation() -> Result<()> {
    static STARTED: std::sync::OnceLock<std::result::Result<(), windows::core::HRESULT>> = std::sync::OnceLock::new();
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED);
        if initialized.is_err() && initialized != RPC_E_CHANGED_MODE {
            return Err(initialized.into());
        }
    }
    let started = STARTED.get_or_init(|| unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.map_err(|error| error.code()));
    started.map_err(Error::from_hresult)
}

fn record_started(area: PixelRect, options: RecordOptions, path: &Path, controls: &Controls) -> Result<Recording> {
    let mut capturer = Capturer::new()?;
    capturer.capture()?;
    let area = even_area(area, capturer.screenshot())?;
    let frames_per_second = options.frames_per_second.max(1);
    let writer = open_writer(path)?;
    let video_stream = add_video_stream(&writer, area, frames_per_second)?;
    let wants_sound = options.system_sound || options.microphone;
    let audio_stream = if wants_sound { add_audio_stream(&writer).ok() } else { None };
    let mut canvas = Canvas::new(area)?;
    unsafe { writer.BeginWriting()? };

    while !controls.go.load(Ordering::SeqCst) {
        if controls.stop.load(Ordering::SeqCst) {
            // Stopped during the countdown: nothing was recorded, so no file stays behind.
            drop(writer);
            let _ = std::fs::remove_file(path);
            return Err(Error::new(windows::Win32::Foundation::E_ABORT, "stopped before it started"));
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    let mut sound = audio_stream.and_then(|stream| SoundMixer::start(stream, options));
    let has_sound = sound.is_some();
    let frame_time = Duration::from_secs_f64(1.0 / f64::from(frames_per_second));
    let started = Instant::now();
    let mut paused_for = Duration::ZERO;
    let mut next_frame = started;
    let mut last_frame_ticks = None;
    let mut first_frame = None;

    while !controls.stop.load(Ordering::SeqCst) {
        if controls.paused.load(Ordering::SeqCst) {
            let pause_started = Instant::now();
            while controls.paused.load(Ordering::SeqCst) && !controls.stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(20));
                if let Some(sound) = sound.as_mut() {
                    sound.throw_away();
                }
            }
            paused_for += pause_started.elapsed();
            next_frame = Instant::now();
            continue;
        }
        let now = Instant::now();
        if now < next_frame {
            std::thread::sleep(next_frame - now);
        }
        let screenshot = match capturer.capture() {
            Ok(screenshot) => screenshot,
            Err(error) => {
                crate::log::write(&format!("recording ended early: {}", error.message()));
                break;
            }
        };
        if !canvas.draw(screenshot) {
            crate::log::write("recording ended early: the area left the screen");
            break;
        }
        if first_frame.is_none() {
            first_frame = Some(canvas.image());
            controls.recording.store(true, Ordering::SeqCst);
        }
        let timeline = started.elapsed().saturating_sub(paused_for);
        controls.recorded_milliseconds.store(timeline.as_millis() as u64, Ordering::SeqCst);
        let ticks = duration_ticks(timeline);
        // Two frames must not share a time, or the encoder drops the second.
        let ticks = last_frame_ticks.map_or(ticks, |last: i64| ticks.max(last + 1));
        last_frame_ticks = Some(ticks);
        let sample = bytes_sample(canvas.pixels(), ticks, duration_ticks(frame_time))?;
        unsafe { writer.WriteSample(video_stream, &sample)? };
        if let Some(sound) = sound.as_mut() {
            if let Err(error) = sound.drain(&writer, timeline, SOUND_HELD_BACK) {
                crate::log::write(&format!("sound stopped: {}", error.message()));
            }
        }
        next_frame += frame_time;
        let now = Instant::now();
        if next_frame < now {
            // Behind, so skip the frames that were missed instead of rushing to catch up.
            next_frame = now;
        }
    }

    let duration = started.elapsed().saturating_sub(paused_for);
    if let Some(sound) = sound.as_mut() {
        let _ = sound.drain(&writer, duration, Duration::ZERO);
        sound.stop();
    }
    unsafe { writer.Finalize()? };
    let first_frame = first_frame.ok_or_else(|| Error::new(windows::Win32::Foundation::E_FAIL, "no frame was recorded"))?;
    Ok(Recording {
        path: path.to_path_buf(),
        duration,
        first_frame,
        has_sound,
    })
}

/// H.264 needs even sizes, so the area loses a pixel on an odd side.
fn even_area(area: PixelRect, screenshot: &Screenshot) -> Result<PixelRect> {
    let area = area
        .intersect(&screenshot.bounds)
        .ok_or_else(|| Error::new(windows::Win32::Foundation::E_INVALIDARG, "the area is off screen"))?;
    let width = area.width & !1;
    let height = area.height & !1;
    if width < SMALLEST_SIDE || height < SMALLEST_SIDE {
        return Err(Error::new(windows::Win32::Foundation::E_INVALIDARG, "the area is too small to record"));
    }
    Ok(PixelRect { width, height, ..area })
}

fn open_writer(path: &Path) -> Result<IMFSinkWriter> {
    unsafe {
        let mut attributes = None;
        MFCreateAttributes(&mut attributes, 1)?;
        let attributes = attributes.ok_or_else(Error::empty)?;
        attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
        MFCreateSinkWriterFromURL(&HSTRING::from(path.as_os_str()), None, &attributes)
    }
}

fn pair(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

fn add_video_stream(writer: &IMFSinkWriter, area: PixelRect, frames_per_second: u32) -> Result<u32> {
    let width = area.width as u32;
    let height = area.height as u32;
    let bitrate = f64::from(width) * f64::from(height) * f64::from(frames_per_second) * BITS_PER_PIXEL_FRAME;
    let set_common = |media_type: &IMFMediaType| -> Result<()> {
        unsafe {
            media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            media_type.SetUINT64(&MF_MT_FRAME_SIZE, pair(width, height))?;
            media_type.SetUINT64(&MF_MT_FRAME_RATE, pair(frames_per_second, 1))?;
            media_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pair(1, 1))
        }
    };
    unsafe {
        let output = MFCreateMediaType()?;
        set_common(&output)?;
        output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        output.SetUINT32(&MF_MT_AVG_BITRATE, bitrate as u32)?;
        let stream = writer.AddStream(&output)?;
        let input = MFCreateMediaType()?;
        set_common(&input)?;
        input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        // A positive stride means the rows run top to bottom, as ours do.
        input.SetUINT32(&MF_MT_DEFAULT_STRIDE, width * 4)?;
        writer.SetInputMediaType(stream, &input, None)?;
        Ok(stream)
    }
}

fn add_audio_stream(writer: &IMFSinkWriter) -> Result<u32> {
    let set_common = |media_type: &IMFMediaType| -> Result<()> {
        unsafe {
            media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            media_type.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AUDIO_RATE)?;
            media_type.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, AUDIO_CHANNELS)?;
            media_type.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
        }
    };
    unsafe {
        let output = MFCreateMediaType()?;
        set_common(&output)?;
        output.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
        output.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AAC_BYTES_PER_SECOND)?;
        output.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?;
        let stream = writer.AddStream(&output)?;
        let input = MFCreateMediaType()?;
        set_common(&input)?;
        input.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
        input.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, AUDIO_BYTES_PER_FRAME)?;
        input.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AUDIO_RATE * AUDIO_BYTES_PER_FRAME)?;
        writer.SetInputMediaType(stream, &input, None)?;
        Ok(stream)
    }
}

fn duration_ticks(duration: Duration) -> i64 {
    (duration.as_secs_f64() * TICKS_PER_SECOND as f64) as i64
}

fn bytes_sample(bytes: &[u8], time_ticks: i64, duration_ticks: i64) -> Result<IMFSample> {
    unsafe {
        let buffer = MFCreateMemoryBuffer(bytes.len() as u32)?;
        let mut destination = std::ptr::null_mut();
        buffer.Lock(&mut destination, None, None)?;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), destination, bytes.len());
        buffer.Unlock()?;
        buffer.SetCurrentLength(bytes.len() as u32)?;
        let sample = MFCreateSample()?;
        sample.AddBuffer(&buffer)?;
        sample.SetSampleTime(time_ticks)?;
        sample.SetSampleDuration(duration_ticks)?;
        Ok(sample)
    }
}

/// The frame being made: the area's pixels with the pointer drawn over them. The capture
/// device leaves the pointer out, and GDI draws every pointer type correctly, so this is
/// a device context over the pixels.
struct Canvas {
    area: PixelRect,
    device_context: HDC,
    bitmap: HBITMAP,
    previous_object: HGDIOBJ,
    pixels: *mut u8,
    /// The pointer whose hot spot is known, and that hot spot.
    known_pointer: Option<(HCURSOR, (i32, i32))>,
}

impl Canvas {
    fn new(area: PixelRect) -> Result<Self> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: area.width,
                // Negative height: top row first, the same as the screenshot.
                biHeight: -area.height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        unsafe {
            let device_context = CreateCompatibleDC(None);
            let mut pixels = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(Some(device_context), &info, DIB_RGB_COLORS, &mut pixels, None, 0) {
                Ok(bitmap) => bitmap,
                Err(error) => {
                    let _ = DeleteDC(device_context);
                    return Err(error);
                }
            };
            let previous_object = SelectObject(device_context, bitmap.into());
            Ok(Self {
                area,
                device_context,
                bitmap,
                previous_object,
                pixels: pixels as *mut u8,
                known_pointer: None,
            })
        }
    }

    fn byte_count(&self) -> usize {
        self.area.width as usize * self.area.height as usize * 4
    }

    fn pixels(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.pixels, self.byte_count()) }
    }

    /// Returns false when the area is no longer on the desktop.
    fn draw(&mut self, screenshot: &Screenshot) -> bool {
        let bounds = screenshot.bounds;
        let area = self.area;
        let inside = area.left >= bounds.left && area.top >= bounds.top && area.right() <= bounds.right() && area.bottom() <= bounds.bottom();
        if !inside {
            return false;
        }
        let row_bytes = area.width as usize * 4;
        let source_stride = screenshot.stride();
        let first_column = (area.left - bounds.left) as usize * 4;
        for row in 0..area.height as usize {
            let source_start = (row + (area.top - bounds.top) as usize) * source_stride + first_column;
            let source = &screenshot.pixels[source_start..source_start + row_bytes];
            unsafe {
                std::ptr::copy_nonoverlapping(source.as_ptr(), self.pixels.add(row * row_bytes), row_bytes);
            }
        }
        self.draw_pointer();
        true
    }

    fn draw_pointer(&mut self) {
        let mut pointer = CURSORINFO {
            cbSize: size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetCursorInfo(&mut pointer) }.is_err() || pointer.flags.0 & CURSOR_SHOWING.0 == 0 {
            return;
        }
        let hot_spot = self.hot_spot(pointer.hCursor);
        let position: POINT = pointer.ptScreenPos;
        let left = position.x - hot_spot.0 - self.area.left;
        let top = position.y - hot_spot.1 - self.area.top;
        unsafe {
            let _ = DrawIconEx(self.device_context, left, top, HICON(pointer.hCursor.0), 0, 0, 0, None, DI_NORMAL);
        }
    }

    fn hot_spot(&mut self, pointer: HCURSOR) -> (i32, i32) {
        if let Some((known, hot_spot)) = self.known_pointer {
            if known == pointer {
                return hot_spot;
            }
        }
        let mut info = ICONINFO::default();
        let mut hot_spot = (0, 0);
        unsafe {
            if GetIconInfo(HICON(pointer.0), &mut info).is_ok() {
                hot_spot = (info.xHotspot as i32, info.yHotspot as i32);
                if !info.hbmMask.is_invalid() {
                    let _ = DeleteObject(info.hbmMask.into());
                }
                if !info.hbmColor.is_invalid() {
                    let _ = DeleteObject(info.hbmColor.into());
                }
            }
        }
        self.known_pointer = Some((pointer, hot_spot));
        hot_spot
    }

    /// The frame as an opaque image. GDI clears alpha where it draws the pointer.
    fn image(&self) -> Image {
        let mut bgra = self.pixels().to_vec();
        for pixel in bgra.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        Image {
            width: self.area.width as u32,
            height: self.area.height as u32,
            bgra,
        }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.device_context, self.previous_object);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.device_context);
        }
    }
}

/// One source's samples waiting to be mixed, as interleaved 16-bit stereo.
#[derive(Default)]
struct SampleQueue {
    samples: std::collections::VecDeque<i16>,
}

impl SampleQueue {
    fn push_bytes(&mut self, bytes: &[u8]) {
        let samples = bytes.chunks_exact(2).map(|pair| i16::from_le_bytes([pair[0], pair[1]]));
        self.samples.extend(samples);
    }

    fn push_silence(&mut self, frames: usize) {
        self.samples.extend(std::iter::repeat_n(0, frames * AUDIO_CHANNELS as usize));
    }

    /// A source that runs ahead of the timeline loses its oldest sound, so it cannot drift.
    fn drop_backlog(&mut self, largest_frames: usize) {
        let largest_samples = largest_frames * AUDIO_CHANNELS as usize;
        let extra = self.samples.len().saturating_sub(largest_samples);
        self.samples.drain(..extra);
    }

    fn pop(&mut self) -> i16 {
        self.samples.pop_front().unwrap_or(0)
    }
}

/// Adds the queues into `frames` frames as little-endian bytes. A queue that runs out counts
/// as silence, and loud sums clip instead of wrapping around.
fn mix(queues: &mut [SampleQueue], frames: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(frames * AUDIO_BYTES_PER_FRAME as usize);
    for _ in 0..frames * AUDIO_CHANNELS as usize {
        let sum: i32 = queues.iter_mut().map(|queue| i32::from(queue.pop())).sum();
        let clipped = sum.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        bytes.extend_from_slice(&clipped.to_le_bytes());
    }
    bytes
}

/// One Windows audio stream, opened in the format the file needs. Windows converts from
/// whatever the device runs at.
struct SoundSource {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    queue: SampleQueue,
}

impl SoundSource {
    fn open(data_flow: EDataFlow, extra_flags: u32) -> Result<Self> {
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: AUDIO_CHANNELS as u16,
            nSamplesPerSec: AUDIO_RATE,
            nAvgBytesPerSec: AUDIO_RATE * AUDIO_BYTES_PER_FRAME,
            nBlockAlign: AUDIO_BYTES_PER_FRAME as u16,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        unsafe {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = enumerator.GetDefaultAudioEndpoint(data_flow, eConsole)?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let flags = extra_flags | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
            client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, TICKS_PER_SECOND, 0, &format, None)?;
            let capture: IAudioCaptureClient = client.GetService()?;
            client.Start()?;
            Ok(Self {
                client,
                capture,
                queue: SampleQueue::default(),
            })
        }
    }

    /// Moves what Windows has ready into the queue.
    fn collect(&mut self) -> Result<()> {
        unsafe {
            while self.capture.GetNextPacketSize()? > 0 {
                let mut data = std::ptr::null_mut();
                let mut frames = 0;
                let mut flags = 0;
                self.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                if silent || data.is_null() {
                    self.queue.push_silence(frames as usize);
                } else {
                    let byte_count = frames as usize * AUDIO_BYTES_PER_FRAME as usize;
                    self.queue.push_bytes(std::slice::from_raw_parts(data, byte_count));
                }
                self.capture.ReleaseBuffer(frames)?;
            }
        }
        Ok(())
    }

    fn throw_away(&mut self) {
        let _ = self.collect();
        self.queue.samples.clear();
    }
}

/// The sound of a recording as one unbroken stream that follows the timeline. Windows sends
/// nothing from the speakers while no app makes a sound, so the mix fills the gaps with
/// silence, which keeps sound and picture together.
struct SoundMixer {
    stream: u32,
    sources: Vec<SoundSource>,
    frames_written: u64,
}

impl SoundMixer {
    /// Opens what was asked for. A source that cannot open, such as a missing or blocked
    /// microphone, is left out and logged. Returns nothing when no source opens.
    fn start(stream: u32, options: RecordOptions) -> Option<Self> {
        let mut sources = Vec::new();
        if options.system_sound {
            match SoundSource::open(eRender, AUDCLNT_STREAMFLAGS_LOOPBACK) {
                Ok(source) => sources.push(source),
                Err(error) => crate::log::write(&format!("recording without system sound: {}", error.message())),
            }
        }
        if options.microphone {
            match SoundSource::open(eCapture, 0) {
                Ok(source) => sources.push(source),
                Err(error) => crate::log::write(&format!("recording without the microphone: {}", error.message())),
            }
        }
        if sources.is_empty() {
            return None;
        }
        Some(Self {
            stream,
            sources,
            frames_written: 0,
        })
    }

    /// Drops what arrived during a pause.
    fn throw_away(&mut self) {
        for source in &mut self.sources {
            source.throw_away();
        }
    }

    /// Writes the mix up to `timeline`, less `held_back`, which gives late sound time to arrive.
    fn drain(&mut self, writer: &IMFSinkWriter, timeline: Duration, held_back: Duration) -> Result<()> {
        for source in &mut self.sources {
            source.collect()?;
        }
        let target = (timeline.saturating_sub(held_back).as_secs_f64() * f64::from(AUDIO_RATE)) as u64;
        let frames = target.saturating_sub(self.frames_written);
        if frames == 0 {
            return Ok(());
        }
        let bytes = {
            let mut queues: Vec<SampleQueue> = self.sources.iter_mut().map(|source| std::mem::take(&mut source.queue)).collect();
            let bytes = mix(&mut queues, frames as usize);
            for (source, mut queue) in self.sources.iter_mut().zip(queues) {
                queue.drop_backlog(AUDIO_RATE as usize);
                source.queue = queue;
            }
            bytes
        };
        let rate = i64::from(AUDIO_RATE);
        let time = self.frames_written as i64 * TICKS_PER_SECOND / rate;
        let duration = frames as i64 * TICKS_PER_SECOND / rate;
        let sample = bytes_sample(&bytes, time, duration)?;
        unsafe { writer.WriteSample(self.stream, &sample)? };
        self.frames_written += frames;
        Ok(())
    }

    fn stop(&mut self) {
        for source in &self.sources {
            unsafe {
                let _ = source.client.Stop();
            }
        }
    }
}

/// `oh-snap record --rect X,Y,W,H [--seconds N] [--fps N] [--no-sound] [--out FILE]`: records
/// without any window, for scripts and tests. Ctrl+C is not handled; give it a time.
pub fn command(
    area: PixelRect,
    seconds: u64,
    options: RecordOptions,
    out: Option<PathBuf>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let path = out.unwrap_or_else(new_recording_path);
    let controls = Arc::new(Controls::default());
    controls.go.store(true, Ordering::SeqCst);
    let timer_controls = controls.clone();
    std::thread::spawn(move || {
        while !timer_controls.recording.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_secs(seconds));
        timer_controls.stop.store(true, Ordering::SeqCst);
    });
    let recording = record(area, options, &path, &controls)?;
    let sound = if recording.has_sound { "with sound" } else { "no sound" };
    println!("{:.1} s, {sound} -> {}", recording.duration.as_secs_f64(), recording.path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(width: i32, height: i32) -> Screenshot {
        Screenshot {
            bounds: PixelRect { left: 0, top: 0, width, height },
            monitors: Vec::new(),
            pixels: Vec::new(),
        }
    }

    #[test]
    fn areas_become_even_and_stay_on_screen() {
        let screenshot = screen(1920, 1080);
        let area = PixelRect { left: 10, top: 20, width: 301, height: 201 };
        let even = even_area(area, &screenshot).unwrap();
        assert_eq!((even.left, even.top, even.width, even.height), (10, 20, 300, 200));
        assert!(even_area(PixelRect { left: 0, top: 0, width: 8, height: 100 }, &screenshot).is_err());
        assert!(even_area(PixelRect { left: 5000, top: 0, width: 100, height: 100 }, &screenshot).is_err());
    }

    /// Draws over a flat grey screen around the real pointer. Skipped when no pointer shows,
    /// as on a build server.
    #[test]
    fn the_pointer_is_drawn_on_the_frame() {
        let mut pointer = CURSORINFO {
            cbSize: size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        let found = unsafe { GetCursorInfo(&mut pointer) }.is_ok();
        if !found || pointer.flags.0 & CURSOR_SHOWING.0 == 0 {
            return;
        }
        let area = PixelRect {
            left: pointer.ptScreenPos.x - 32,
            top: pointer.ptScreenPos.y - 32,
            width: 96,
            height: 96,
        };
        let screenshot = Screenshot {
            bounds: area,
            monitors: vec![area],
            pixels: vec![128; 96 * 96 * 4],
        };
        let mut canvas = Canvas::new(area).unwrap();
        assert!(canvas.draw(&screenshot));
        let changed = canvas.pixels().chunks_exact(4).filter(|pixel| pixel[..3] != [128, 128, 128]).count();
        assert!(changed > 10, "only {changed} pixels changed");
    }

    fn test_recording_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("oh-snap-test-{}-{name}.mp4", std::process::id()))
    }

    fn small_area() -> PixelRect {
        PixelRect { left: 0, top: 0, width: 320, height: 240 }
    }

    /// Records the real screen, so it needs a desktop: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn a_pause_leaves_no_gap_in_the_recording() {
        let path = test_recording_path("pause");
        let controls = Arc::new(Controls::default());
        controls.go.store(true, Ordering::SeqCst);
        let driver = controls.clone();
        std::thread::spawn(move || {
            while !driver.recording.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            std::thread::sleep(Duration::from_secs(1));
            driver.paused.store(true, Ordering::SeqCst);
            std::thread::sleep(Duration::from_secs(2));
            driver.paused.store(false, Ordering::SeqCst);
            std::thread::sleep(Duration::from_secs(1));
            driver.stop.store(true, Ordering::SeqCst);
        });
        let options = RecordOptions { frames_per_second: 30, system_sound: true, microphone: false };
        let recording = record(small_area(), options, &path, &controls).unwrap();
        let seconds = recording.duration.as_secs_f64();
        assert!((1.8..2.6).contains(&seconds), "recorded {seconds} s");
        println!("{}", path.display());
    }

    /// Needs a desktop too: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn stopping_during_the_countdown_leaves_no_file() {
        let path = test_recording_path("countdown");
        let controls = Controls::default();
        controls.stop.store(true, Ordering::SeqCst);
        let options = RecordOptions { frames_per_second: 30, system_sound: false, microphone: false };
        assert!(record(small_area(), options, &path, &controls).is_err());
        assert!(!path.exists());
    }

    fn queue_of(samples: &[i16]) -> SampleQueue {
        SampleQueue {
            samples: samples.iter().copied().collect(),
        }
    }

    fn samples_of(bytes: &[u8]) -> Vec<i16> {
        bytes.chunks_exact(2).map(|pair| i16::from_le_bytes([pair[0], pair[1]])).collect()
    }

    #[test]
    fn sources_add_up_and_a_short_one_counts_as_silence() {
        let mut queues = [queue_of(&[100, -100, 200, -200]), queue_of(&[1, 2])];
        assert_eq!(samples_of(&mix(&mut queues, 2)), vec![101, -98, 200, -200]);
    }

    #[test]
    fn loud_sound_clips_instead_of_wrapping() {
        let mut queues = [queue_of(&[30_000, -30_000]), queue_of(&[30_000, -30_000])];
        assert_eq!(samples_of(&mix(&mut queues, 1)), vec![32_767, -32_768]);
    }

    #[test]
    fn silence_fills_the_timeline_when_no_source_has_sound() {
        let mut queues = [SampleQueue::default()];
        assert_eq!(samples_of(&mix(&mut queues, 2)), vec![0, 0, 0, 0]);
    }

    #[test]
    fn a_source_that_runs_ahead_loses_its_oldest_sound() {
        let mut queue = queue_of(&[1, 2, 3, 4, 5, 6]);
        queue.drop_backlog(1);
        assert_eq!(queue.samples.iter().copied().collect::<Vec<_>>(), vec![5, 6]);
    }

    #[test]
    fn clock_text_reads_like_a_clock() {
        assert_eq!(clock_text(12_400), "0:12");
        assert_eq!(clock_text(83_000), "1:23");
        assert_eq!(clock_text(3_723_000), "1:02:03");
    }

    #[test]
    fn time_pairs_put_the_first_number_high() {
        assert_eq!(pair(1920, 1080), (1920u64 << 32) | 1080);
    }
}
