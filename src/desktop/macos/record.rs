//! Screen recording with ScreenCaptureKit's own mp4 writer (macOS 15 and later).
//!
//! The writer has no pause, so a pause ends the current piece and a resume starts the next.
//! Stopping joins the pieces without re-encoding.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_av_foundation::{AVFileTypeMPEG4, AVVideoCodecTypeH264};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_media::CMTime;
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol, NSOperatingSystemVersion, NSProcessInfo, NSString, NSURL};
use objc2_screen_capture_kit::{
    SCContentFilter, SCRecordingOutput, SCRecordingOutputConfiguration, SCRecordingOutputDelegate, SCStream,
    SCStreamConfiguration, SCWindow,
};

use super::pure::{self, PointRect};
use super::{sck, video};
use crate::desktop::platform::{RecordOptions, Recording};

/// 'BGRA' as a four character code.
const PIXEL_FORMAT_BGRA: u32 = 0x4247_5241;
const FINISH_TIMEOUT: Duration = Duration::from_secs(15);

/// What the system told us about one piece of the recording.
#[derive(Default)]
struct PieceProgress {
    finished: bool,
    failure: Option<String>,
}

struct PieceState {
    progress: Mutex<PieceProgress>,
    changed: Condvar,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = Arc<PieceState>]
    struct PieceDelegate;

    unsafe impl NSObjectProtocol for PieceDelegate {}

    unsafe impl SCRecordingOutputDelegate for PieceDelegate {
        #[unsafe(method(recordingOutputDidFinishRecording:))]
        fn recording_output_did_finish_recording(&self, _output: &SCRecordingOutput) {
            let state = self.ivars();
            state.progress.lock().unwrap().finished = true;
            state.changed.notify_all();
        }

        #[unsafe(method(recordingOutput:didFailWithError:))]
        fn recording_output_did_fail_with_error(&self, _output: &SCRecordingOutput, error: &NSError) {
            let state = self.ivars();
            {
                let mut progress = state.progress.lock().unwrap();
                progress.failure = Some(error.localizedDescription().to_string());
                progress.finished = true;
            }
            state.changed.notify_all();
        }
    }
);

impl PieceDelegate {
    fn new(state: Arc<PieceState>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(state);
        unsafe { msg_send![super(this), init] }
    }
}

struct ActivePiece {
    output: Retained<SCRecordingOutput>,
    // The output keeps only a weak reference to its delegate.
    _delegate: Retained<PieceDelegate>,
    state: Arc<PieceState>,
}

pub struct MacRecording {
    stream: Retained<SCStream>,
    final_path: PathBuf,
    pieces: Vec<PathBuf>,
    active: Option<ActivePiece>,
}

// The stream is only touched from the thread that owns the recording; ScreenCaptureKit
// objects may be used from any thread.
unsafe impl Send for MacRecording {}

fn piece_path(final_path: &Path, number: usize) -> PathBuf {
    let stem = final_path.file_stem().map(|stem| stem.to_string_lossy().to_string()).unwrap_or_default();
    final_path.with_file_name(format!("{stem}.part{number}.mp4"))
}

fn at_least_macos_15() -> bool {
    let version = NSOperatingSystemVersion { majorVersion: 15, minorVersion: 0, patchVersion: 0 };
    NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(version)
}

fn wait_for_completion(start: impl FnOnce(&RcBlock<dyn Fn(*mut NSError)>)) -> Result<(), String> {
    let (sender, receiver) = mpsc::channel();
    let handler: RcBlock<dyn Fn(*mut NSError)> = RcBlock::new(move |error: *mut NSError| {
        let _ = sender.send(if error.is_null() { Ok(()) } else { Err(sck::describe_error(error)) });
    });
    start(&handler);
    receiver.recv_timeout(sck::ANSWER_TIMEOUT).map_err(|_| "ScreenCaptureKit did not answer".to_string())?
}

impl MacRecording {
    /// Starts writing the next piece of the recording.
    fn begin_piece(&mut self) -> Result<(), String> {
        let path = piece_path(&self.final_path, self.pieces.len() + 1);
        let _ = std::fs::remove_file(&path);
        let configuration = unsafe { SCRecordingOutputConfiguration::new() };
        unsafe {
            configuration.setOutputURL(&NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy())));
            if let Some(file_type) = AVFileTypeMPEG4 {
                configuration.setOutputFileType(file_type);
            }
            if let Some(codec) = AVVideoCodecTypeH264 {
                configuration.setVideoCodecType(codec);
            }
        }
        let state = Arc::new(PieceState { progress: Mutex::new(PieceProgress::default()), changed: Condvar::new() });
        let delegate = PieceDelegate::new(state.clone());
        let output = unsafe {
            SCRecordingOutput::initWithConfiguration_delegate(
                SCRecordingOutput::alloc(),
                &configuration,
                ProtocolObject::from_ref(&*delegate),
            )
        };
        unsafe { self.stream.addRecordingOutput_error(&output) }.map_err(|error| error.localizedDescription().to_string())?;
        self.pieces.push(path);
        self.active = Some(ActivePiece { output, _delegate: delegate, state });
        Ok(())
    }

    /// Ends the piece being written and waits until its file is complete.
    fn end_piece(&mut self) -> Result<(), String> {
        let Some(piece) = self.active.take() else {
            return Ok(());
        };
        unsafe { self.stream.removeRecordingOutput_error(&piece.output) }
            .map_err(|error| error.localizedDescription().to_string())?;
        let mut progress = piece.state.progress.lock().unwrap();
        while !progress.finished {
            let (guard, timeout) = piece.state.changed.wait_timeout(progress, FINISH_TIMEOUT).unwrap();
            progress = guard;
            if timeout.timed_out() && !progress.finished {
                return Err("the recording file did not finish".into());
            }
        }
        match progress.failure.take() {
            Some(failure) => Err(failure),
            None => Ok(()),
        }
    }

    /// Puts the pieces into the final file and removes them.
    fn assemble(&self) -> Result<(), String> {
        let _ = std::fs::remove_file(&self.final_path);
        if let [only] = self.pieces.as_slice() {
            return std::fs::rename(only, &self.final_path).map_err(|error| format!("could not name the recording: {error}"));
        }
        video::join_segments(&self.pieces, &self.final_path)?;
        for piece in &self.pieces {
            let _ = std::fs::remove_file(piece);
        }
        Ok(())
    }
}

impl Recording for MacRecording {
    fn pause(&mut self, paused: bool) {
        let result = if paused {
            self.end_piece()
        } else if self.active.is_none() {
            self.begin_piece()
        } else {
            Ok(())
        };
        if let Err(error) = result {
            crate::log::write(&format!("pause: {error}"));
        }
    }

    fn stop(mut self: Box<Self>) -> Result<PathBuf, String> {
        let ended = self.end_piece();
        let stopped = wait_for_completion(|handler| unsafe { self.stream.stopCaptureWithCompletionHandler(Some(handler)) });
        ended?;
        stopped?;
        self.assemble()?;
        Ok(self.final_path.clone())
    }
}

/// The windows of this program, so its own recording bar stays out of the video.
fn own_windows(windows: &NSArray<SCWindow>) -> Retained<NSArray<SCWindow>> {
    let own_process = std::process::id() as i32;
    let own: Vec<Retained<SCWindow>> = windows
        .iter()
        .filter(|window| unsafe { window.owningApplication() }.is_some_and(|application| unsafe { application.processID() } == own_process))
        .collect();
    NSArray::from_retained_slice(&own)
}

pub fn start_recording(
    area: (i32, i32, u32, u32),
    options: RecordOptions,
    output: &Path,
) -> Result<Box<dyn Recording>, String> {
    if !at_least_macos_15() {
        return Err("recording the screen needs macOS 15 or later".into());
    }
    let content = sck::shareable_content()?;
    let displays = sck::displays_of(&content);
    let display_rects: Vec<PointRect> = displays.iter().map(|display| sck::display_rect(display)).collect();
    let scales: Vec<f64> = displays.iter().map(|display| sck::pixel_scale(display)).collect();
    let desktop_scale = pure::desktop_scale(&scales);

    let area_points = pure::pixels_to_points(area, desktop_scale);
    let index = pure::display_for_area(area_points, &display_rects).ok_or("the area is not on any screen")?;
    let display_rect = display_rects[index];
    let display_scale = scales[index];

    let excluded = own_windows(&*unsafe { content.windows() });
    let filter = unsafe { SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &displays[index], &excluded) };

    let configuration = unsafe { SCStreamConfiguration::new() };
    unsafe {
        configuration.setSourceRect(CGRect::new(
            CGPoint::new(area_points.x - display_rect.x, area_points.y - display_rect.y),
            CGSize::new(area_points.width, area_points.height),
        ));
        configuration.setWidth(pure::even((area_points.width * display_scale).round() as u32) as usize);
        configuration.setHeight(pure::even((area_points.height * display_scale).round() as u32) as usize);
        configuration.setPixelFormat(PIXEL_FORMAT_BGRA);
        configuration.setMinimumFrameInterval(CMTime::new(1, options.frames_per_second.max(1) as i32));
        configuration.setShowsCursor(true);
        configuration.setCapturesAudio(options.system_sound);
        configuration.setExcludesCurrentProcessAudio(true);
        configuration.setCaptureMicrophone(options.microphone);
    }
    let stream = unsafe { objc2_screen_capture_kit::SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &configuration, None) };

    let mut recording = MacRecording { stream, final_path: output.to_path_buf(), pieces: Vec::new(), active: None };
    recording.begin_piece()?;
    wait_for_completion(|handler| unsafe { recording.stream.startCaptureWithCompletionHandler(Some(handler)) })?;
    Ok(Box::new(recording))
}

fn display_pixel_scale(display: &objc2_screen_capture_kit::SCDisplay) -> f64 {
    let filter = unsafe {
        SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), display, &NSArray::<SCWindow>::new())
    };
    f64::from(unsafe { objc2_screen_capture_kit::SCShareableContent::infoForFilter(&filter).pointPixelScale() })
}

