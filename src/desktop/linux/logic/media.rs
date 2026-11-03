//! Command lines for ffmpeg and friends, and the names of the files they write.

use std::path::{Path, PathBuf};

pub struct RecordSettings {
    pub area: (i32, i32, u32, u32),
    pub frames_per_second: u32,
    pub system_sound: bool,
    pub microphone: bool,
}

/// yuv420p needs even sizes.
pub fn even(size: u32) -> u32 {
    (size & !1).max(2)
}

fn text(value: impl ToString) -> String {
    value.to_string()
}

/// ffmpeg arguments that record an X11 area. `display` is `$DISPLAY`, such as `:0`.
pub fn x11_record_args(display: &str, settings: &RecordSettings, output: &Path) -> Vec<String> {
    let (x, y, width, height) = settings.area;
    let mut args: Vec<String> = ["-y", "-loglevel", "error", "-f", "x11grab", "-framerate"].map(text).into();
    args.push(text(settings.frames_per_second));
    args.extend(["-video_size", &format!("{}x{}", even(width), even(height)), "-draw_mouse", "1", "-i"].map(text));
    args.push(format!("{display}+{},{}", x.max(0), y.max(0)));

    let mut audio_inputs = 0;
    if settings.system_sound {
        args.extend(["-f", "pulse", "-i", "@DEFAULT_MONITOR@"].map(text));
        audio_inputs += 1;
    }
    if settings.microphone {
        args.extend(["-f", "pulse", "-i", "default"].map(text));
        audio_inputs += 1;
    }
    if audio_inputs == 2 {
        args.extend(["-filter_complex", "[1:a][2:a]amix=inputs=2:duration=longest[mixed]", "-map", "0:v", "-map", "[mixed]"].map(text));
    }
    args.extend(["-c:v", "libx264", "-preset", "ultrafast", "-crf", "23", "-pix_fmt", "yuv420p"].map(text));
    if audio_inputs > 0 {
        args.extend(["-c:a", "aac"].map(text));
    }
    args.extend(["-movflags", "+faststart"].map(text));
    args.push(output.display().to_string());
    args
}

/// wf-recorder arguments for a Wayland area.
pub fn wayland_record_args(settings: &RecordSettings, output: &Path) -> Vec<String> {
    let (x, y, width, height) = settings.area;
    let mut args = vec![
        "-g".to_string(),
        format!("{x},{y} {width}x{height}"),
        "-r".to_string(),
        settings.frames_per_second.to_string(),
        "-f".to_string(),
        output.display().to_string(),
    ];
    if settings.system_sound || settings.microphone {
        args.push("--audio".to_string());
    }
    args
}

/// GIF: about 15 fps, at most 960 wide, one palette for the whole clip.
pub fn gif_args(recording: &Path, gif: &Path) -> Vec<String> {
    let filter = "fps=15,scale='min(960,iw)':-2:flags=lanczos,split[a][b];[a]palettegen[palette];[b][palette]paletteuse";
    let mut args: Vec<String> = ["-y", "-loglevel", "error", "-i"].map(text).into();
    args.push(recording.display().to_string());
    args.extend(["-vf", filter, "-loop", "0"].map(text));
    args.push(gif.display().to_string());
    args
}

pub fn gif_path(recording: &Path) -> PathBuf {
    recording.with_extension("gif")
}

/// Frame-accurate trim: seek on the input, then re-encode.
pub fn trim_args(recording: &Path, from: f64, to: f64, output: &Path) -> Vec<String> {
    let mut args: Vec<String> = ["-y", "-loglevel", "error", "-ss"].map(text).into();
    args.push(format!("{from:.3}"));
    args.push("-i".to_string());
    args.push(recording.display().to_string());
    args.push("-t".to_string());
    args.push(format!("{:.3}", (to - from).max(0.0)));
    args.extend(["-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-c:a", "aac"].map(text));
    args.push(output.display().to_string());
    args
}

/// `X trimmed.mp4`, then `X trimmed 2.mp4`, `X trimmed 3.mp4` ... the first that does not exist.
pub fn trimmed_path(recording: &Path, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
    let stem = recording.file_stem().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let extension = recording.extension().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "mp4".into());
    let folder = recording.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut number = 1;
    loop {
        let name = if number == 1 { format!("{stem} trimmed.{extension}") } else { format!("{stem} trimmed {number}.{extension}") };
        let candidate = folder.join(name);
        if !exists(&candidate) {
            return candidate;
        }
        number += 1;
    }
}

/// Tesseract's text without trailing blank space.
pub fn clean_ocr(output: &str) -> String {
    output.lines().map(str::trim_end).collect::<Vec<_>>().join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(system_sound: bool, microphone: bool) -> RecordSettings {
        RecordSettings { area: (10, -5, 801, 600), frames_per_second: 30, system_sound, microphone }
    }

    #[test]
    fn x11_args_without_sound() {
        let args = x11_record_args(":0", &settings(false, false), Path::new("out.mp4"));
        assert!(args.windows(2).any(|pair| pair == ["-video_size", "800x600"]));
        assert!(args.windows(2).any(|pair| pair == ["-i", ":0+10,0"]));
        assert!(!args.contains(&"pulse".to_string()));
        assert_eq!(args.last().unwrap(), "out.mp4");
    }

    #[test]
    fn x11_args_mix_both_audio_inputs() {
        let args = x11_record_args(":1", &settings(true, true), Path::new("out.mp4"));
        assert_eq!(args.iter().filter(|argument| *argument == "pulse").count(), 2);
        assert!(args.contains(&"-filter_complex".to_string()));
        let single = x11_record_args(":1", &settings(true, false), Path::new("out.mp4"));
        assert!(!single.contains(&"-filter_complex".to_string()));
        assert!(single.contains(&"aac".to_string()));
    }

    #[test]
    fn wayland_args() {
        let args = wayland_record_args(&settings(false, true), Path::new("o.mp4"));
        assert_eq!(args[..2], ["-g", "10,-5 801x600"]);
        assert!(args.contains(&"--audio".to_string()));
    }

    #[test]
    fn gif_and_trim() {
        let args = gif_args(Path::new("a.mp4"), Path::new("a.gif"));
        assert!(args.iter().any(|argument| argument.contains("paletteuse") && argument.contains("960")));
        assert_eq!(gif_path(Path::new("/v/a.mp4")), PathBuf::from("/v/a.gif"));
        let args = trim_args(Path::new("a.mp4"), 1.5, 4.0, Path::new("b.mp4"));
        assert!(args.windows(2).any(|pair| pair == ["-ss", "1.500"]));
        assert!(args.windows(2).any(|pair| pair == ["-t", "2.500"]));
    }

    #[test]
    fn trimmed_names_count_up() {
        let taken = [PathBuf::from("/v/X trimmed.mp4"), PathBuf::from("/v/X trimmed 2.mp4")];
        let exists = |path: &Path| taken.iter().any(|name| name == path);
        assert_eq!(trimmed_path(Path::new("/v/X.mp4"), &exists), PathBuf::from("/v/X trimmed 3.mp4"));
        assert_eq!(trimmed_path(Path::new("/v/Y.mp4"), &exists), PathBuf::from("/v/Y trimmed.mp4"));
    }

    #[test]
    fn cleans_ocr() {
        assert_eq!(clean_ocr("hello  \nworld\n\n\x0c"), "hello\nworld");
    }
}
