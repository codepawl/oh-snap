//! Programs oh-snap drives when they are installed: tesseract for text, ffmpeg for video,
//! wf-recorder for Wayland video.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use super::logic::media::{self, RecordSettings};
use super::logic::session::Session;
use crate::desktop::platform::{RecordOptions, Recording};
use crate::output::Image;

pub fn find_program(name: &str) -> Option<PathBuf> {
    let search_path = std::env::var_os("PATH")?;
    std::env::split_paths(&search_path).map(|folder| folder.join(name)).find(|candidate| candidate.is_file())
}

fn require(name: &str, package_hint: &str) -> Result<PathBuf, String> {
    find_program(name).ok_or_else(|| format!("install {package_hint}"))
}

pub fn read_text(image: &Image) -> Result<String, String> {
    let tesseract = require("tesseract", "tesseract-ocr for text reading")?;
    let png = crate::output::encode_png(image).map_err(|error| error.to_string())?;
    let mut child = Command::new(tesseract)
        .args(["stdin", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("tesseract did not start: {error}"))?;
    let mut input = child.stdin.take().ok_or("tesseract has no input")?;
    // Written from a thread so a full output pipe cannot stall the write.
    let writer = std::thread::spawn(move || input.write_all(&png));
    let output = child.wait_with_output().map_err(|error| error.to_string())?;
    let _ = writer.join();
    if !output.status.success() {
        return Err("tesseract could not read the image".into());
    }
    Ok(media::clean_ocr(&String::from_utf8_lossy(&output.stdout)))
}

fn run_ffmpeg(arguments: &[String]) -> Result<(), String> {
    let ffmpeg = require("ffmpeg", "ffmpeg")?;
    let output = Command::new(ffmpeg).args(arguments).stdin(Stdio::null()).output().map_err(|error| error.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr);
    Err(format!("ffmpeg failed: {}", message.lines().last().unwrap_or("no message")))
}

pub fn make_gif(recording: &Path) -> Result<PathBuf, String> {
    let gif = media::gif_path(recording);
    run_ffmpeg(&media::gif_args(recording, &gif))?;
    Ok(gif)
}

pub fn trim(recording: &Path, from: f64, to: f64) -> Result<PathBuf, String> {
    let output = media::trimmed_path(recording, &|path| path.exists());
    run_ffmpeg(&media::trim_args(recording, from, to, &output))?;
    Ok(output)
}

struct ProcessRecording {
    child: Child,
    session: Session,
    paused: bool,
    output: PathBuf,
}

fn signal(child: &Child, signal: i32) {
    // SAFETY: kill only sends a signal to a process id this program started.
    unsafe {
        libc::kill(child.id() as i32, signal);
    }
}

impl Recording for ProcessRecording {
    /// ffmpeg keeps its clock while stopped, so the paused time shows as a frozen frame.
    fn pause(&mut self, paused: bool) {
        if paused == self.paused {
            return;
        }
        signal(&self.child, if paused { libc::SIGSTOP } else { libc::SIGCONT });
        self.paused = paused;
    }

    fn stop(mut self: Box<Self>) -> Result<PathBuf, String> {
        if self.paused {
            signal(&self.child, libc::SIGCONT);
        }
        match self.session {
            // ffmpeg finishes the file when it reads `q`.
            Session::X11 => {
                if let Some(mut input) = self.child.stdin.take() {
                    let _ = input.write_all(b"q");
                }
            }
            Session::Wayland => signal(&self.child, libc::SIGINT),
        }
        self.child.wait().map_err(|error| error.to_string())?;
        if self.output.is_file() { Ok(self.output) } else { Err("the recording was not written".into()) }
    }
}

pub fn start_recording(
    session: Session,
    area: (i32, i32, u32, u32),
    options: RecordOptions,
    output: &Path,
) -> Result<Box<dyn Recording>, String> {
    let settings = RecordSettings {
        area,
        frames_per_second: options.frames_per_second.max(1),
        system_sound: options.system_sound,
        microphone: options.microphone,
    };
    let (program, arguments) = match session {
        Session::X11 => {
            let display = std::env::var("DISPLAY").map_err(|_| "no DISPLAY to record")?;
            (require("ffmpeg", "ffmpeg to record")?, media::x11_record_args(&display, &settings, output))
        }
        Session::Wayland => (
            require("wf-recorder", "wf-recorder to record on Wayland (GNOME: record with its own Ctrl+Alt+Shift+R)")?,
            media::wayland_record_args(&settings, output),
        ),
    };
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("the recorder did not start: {error}"))?;
    // A bad device or option makes the recorder quit at once; report that instead of a silent no-file.
    std::thread::sleep(Duration::from_millis(500));
    if let Ok(Some(_)) = child.try_wait() {
        let mut message = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut message);
        }
        return Err(format!("the recorder stopped at once: {}", message.lines().last().unwrap_or("no message")));
    }
    Ok(Box::new(ProcessRecording { child, session, paused: false, output: output.to_path_buf() }))
}
