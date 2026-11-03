//! Finished recordings: the GIF, the trimmed copy and joining the pieces of a paused recording.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_av_foundation::{
    AVAsset, AVAssetExportPresetHighestQuality, AVAssetExportPresetPassthrough, AVAssetExportSession,
    AVAssetExportSessionStatus, AVAssetImageGenerator, AVFileTypeMPEG4, AVMutableComposition,
};
use objc2_core_foundation::{CFString, CFURL, CGSize};
use objc2_core_media::{CMTime, CMTimeRange};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};
use objc2_image_io::{CGImageDestination, kCGImagePropertyGIFDelayTime, kCGImagePropertyGIFDictionary, kCGImagePropertyGIFLoopCount};

use super::pure;

/// A time with the resolution video frames need.
const TIMESCALE: i32 = 600;

fn seconds(value: f64) -> CMTime {
    unsafe { CMTime::with_seconds(value, TIMESCALE) }
}

fn file_url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}

fn as_core_foundation_url(url: &NSURL) -> &CFURL {
    // NSURL and CFURL are the same object.
    unsafe { &*(url as *const NSURL).cast::<CFURL>() }
}

pub fn duration_seconds(asset: &AVAsset) -> f64 {
    unsafe { asset.duration().seconds() }
}

fn open_asset(path: &Path) -> Retained<AVAsset> {
    unsafe { AVAsset::assetWithURL(&file_url(path)) }
}

/// Runs an export session to its end. A cut that takes a moment is expected, so this waits.
fn run_export(session: &AVAssetExportSession, output: &Path) -> Result<(), String> {
    let _ = std::fs::remove_file(output);
    unsafe {
        session.setOutputURL(Some(&file_url(output)));
        session.setOutputFileType(AVFileTypeMPEG4);
    }
    let (sender, receiver) = mpsc::channel();
    let handler = RcBlock::new(move || {
        let _ = sender.send(());
    });
    unsafe { session.exportAsynchronouslyWithCompletionHandler(&handler) };
    receiver.recv_timeout(Duration::from_secs(60 * 30)).map_err(|_| "the export did not finish".to_string())?;
    if unsafe { session.status() } == AVAssetExportSessionStatus::Completed {
        return Ok(());
    }
    let reason = unsafe { session.error() }
        .map(|error| error.localizedDescription().to_string())
        .unwrap_or_else(|| "the export failed".into());
    let _ = std::fs::remove_file(output);
    Err(reason)
}

pub fn trim(recording: &Path, from: f64, to: f64) -> Result<PathBuf, String> {
    if to <= from {
        return Err("the end of the cut is before its start".into());
    }
    let asset = open_asset(recording);
    // A re-encoding preset, so the cut starts on the exact frame and not the one before.
    let session = unsafe {
        AVAssetExportSession::initWithAsset_presetName(AVAssetExportSession::alloc(), &asset, AVAssetExportPresetHighestQuality)
    }
    .ok_or("this recording cannot be exported")?;
    let range: CMTimeRange = unsafe { CMTimeRange::new(seconds(from), seconds(to - from)) };
    unsafe { session.setTimeRange(range) };
    let path = pure::trimmed_path(recording, Path::exists);
    run_export(&session, &path)?;
    Ok(path)
}

/// Joins the pieces of a recording that was paused into `output`, without re-encoding.
pub fn join_segments(segments: &[PathBuf], output: &Path) -> Result<(), String> {
    let composition = unsafe { AVMutableComposition::composition() };
    let mut end = seconds(0.0);
    for segment in segments {
        let asset = open_asset(segment);
        let length = unsafe { asset.duration() };
        let range = unsafe { CMTimeRange::new(seconds(0.0), length) };
        #[allow(deprecated)]
        unsafe { composition.insertTimeRange_ofAsset_atTime_error(range, &asset, end) }
            .map_err(|error| error.localizedDescription().to_string())?;
        end = unsafe { end.add(length) };
    }
    let session = unsafe {
        AVAssetExportSession::initWithAsset_presetName(AVAssetExportSession::alloc(), &composition, AVAssetExportPresetPassthrough)
    }
    .ok_or("the pieces cannot be joined")?;
    run_export(&session, output)
}

fn number_value(value: f64) -> Retained<AnyObject> {
    unsafe { Retained::cast_unchecked(NSNumber::new_f64(value)) }
}

/// One-entry dictionaries for the GIF writer: {GIF: {key: value}}.
fn gif_properties(key: &CFString, value: f64) -> Retained<NSDictionary<NSString, AnyObject>> {
    let key_text: &NSString = unsafe { &*(key as *const CFString).cast::<NSString>() };
    let inner = NSDictionary::from_retained_objects(&[key_text], &[number_value(value)]);
    let inner: Retained<AnyObject> = unsafe { Retained::cast_unchecked(inner) };
    let outer_key: &NSString = unsafe { &*(kCGImagePropertyGIFDictionary as *const CFString).cast::<NSString>() };
    NSDictionary::from_retained_objects(&[outer_key], &[inner])
}

fn as_dictionary(dictionary: &NSDictionary<NSString, AnyObject>) -> &objc2_core_foundation::CFDictionary {
    unsafe { &*(dictionary as *const NSDictionary<NSString, AnyObject>).cast::<objc2_core_foundation::CFDictionary>() }
}

pub fn make_gif(recording: &Path) -> Result<PathBuf, String> {
    let asset = open_asset(recording);
    let times = pure::gif_frame_times(duration_seconds(&asset));
    let generator = unsafe { AVAssetImageGenerator::assetImageGeneratorWithAsset(&asset) };
    let tolerance = seconds(0.5 / f64::from(pure::GIF_FRAMES_PER_SECOND));
    unsafe {
        generator.setAppliesPreferredTrackTransform(true);
        generator.setMaximumSize(CGSize::new(f64::from(pure::GIF_MAX_WIDTH), 8192.0));
        generator.setRequestedTimeToleranceBefore(tolerance);
        generator.setRequestedTimeToleranceAfter(tolerance);
    }

    let path = recording.with_extension("gif");
    let gif_type = CFString::from_str("com.compuserve.gif");
    let url = file_url(&path);
    let destination = unsafe { CGImageDestination::with_url(as_core_foundation_url(&url), &gif_type, times.len(), None) }
        .ok_or("could not create the GIF file")?;
    let loop_forever = gif_properties(unsafe { kCGImagePropertyGIFLoopCount }, 0.0);
    unsafe { destination.set_properties(Some(as_dictionary(&loop_forever))) };
    let frame_delay = gif_properties(unsafe { kCGImagePropertyGIFDelayTime }, pure::gif_delay_seconds());

    for time in times {
        #[allow(deprecated)]
        let frame = unsafe { generator.copyCGImageAtTime_actualTime_error(seconds(time), std::ptr::null_mut()) }
            .map_err(|error| error.localizedDescription().to_string())?;
        unsafe { destination.add_image(&frame, Some(as_dictionary(&frame_delay))) };
    }
    if !unsafe { destination.finalize() } {
        let _ = std::fs::remove_file(&path);
        return Err("could not finish the GIF file".into());
    }
    Ok(path)
}

