//! Plain logic behind the macOS platform: geometry, names, arguments and plist text.
//! Nothing here touches the OS or any other crate module, so it is tested on any machine.

use std::path::{Path, PathBuf};

/// The shared desktop space is physical pixels. Displays can have different scales, so every
/// position is points times the largest scale: displays then never overlap in that space.
pub fn desktop_scale(display_scales: &[f64]) -> f64 {
    display_scales.iter().copied().fold(1.0, f64::max)
}

/// A rectangle in points, origin at the top left of the main display.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A rectangle in physical pixels of the shared desktop space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

pub fn points_to_pixels(rect: PointRect, scale: f64) -> PixelRect {
    PixelRect {
        x: (rect.x * scale).round() as i32,
        y: (rect.y * scale).round() as i32,
        width: (rect.width * scale).round().max(0.0) as u32,
        height: (rect.height * scale).round().max(0.0) as u32,
    }
}

pub fn pixels_to_points(area: (i32, i32, u32, u32), scale: f64) -> PointRect {
    PointRect {
        x: f64::from(area.0) / scale,
        y: f64::from(area.1) / scale,
        width: f64::from(area.2) / scale,
        height: f64::from(area.3) / scale,
    }
}

/// How much of `area` lies inside `display`, in square points.
pub fn overlap_area(area: PointRect, display: PointRect) -> f64 {
    let left = area.x.max(display.x);
    let top = area.y.max(display.y);
    let right = (area.x + area.width).min(display.x + display.width);
    let bottom = (area.y + area.height).min(display.y + display.height);
    (right - left).max(0.0) * (bottom - top).max(0.0)
}

/// The display that holds most of `area`, as an index into `displays`.
pub fn display_for_area(area: PointRect, displays: &[PointRect]) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (index, display) in displays.iter().enumerate() {
        let overlap = overlap_area(area, *display);
        if overlap > 0.0 && best.is_none_or(|(_, best_overlap)| overlap > best_overlap) {
            best = Some((index, overlap));
        }
    }
    best.map(|(index, _)| index)
}

/// Video encoders want even sizes.
pub fn even(value: u32) -> u32 {
    value.max(2) & !1
}

/// "Screen Recording 1 trimmed.mp4", then "... trimmed 2.mp4", like the Windows trim.
pub fn trimmed_file_name(stem: &str, copy_number: u32) -> String {
    if copy_number <= 1 {
        format!("{stem} trimmed.mp4")
    } else {
        format!("{stem} trimmed {copy_number}.mp4")
    }
}

pub fn trimmed_path(recording: &Path, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let stem = recording.file_stem().map(|stem| stem.to_string_lossy().to_string()).unwrap_or_default();
    let mut copy_number = 1;
    loop {
        let path = recording.with_file_name(trimmed_file_name(&stem, copy_number));
        if !exists(&path) {
            return path;
        }
        copy_number += 1;
    }
}

pub const GIF_FRAMES_PER_SECOND: u32 = 15;
pub const GIF_MAX_WIDTH: u32 = 960;

/// The moments in a recording to take GIF frames from.
pub fn gif_frame_times(duration_seconds: f64) -> Vec<f64> {
    let count = ((duration_seconds * f64::from(GIF_FRAMES_PER_SECOND)).floor() as usize).max(1);
    (0..count).map(|index| index as f64 / f64::from(GIF_FRAMES_PER_SECOND)).collect()
}

/// Seconds between GIF frames.
pub fn gif_delay_seconds() -> f64 {
    1.0 / f64::from(GIF_FRAMES_PER_SECOND)
}

/// The menu-bar icon: four corner brackets, black on see-through, as a template image.
pub fn tray_icon_rgba(size: u32) -> Vec<u8> {
    let margin = size / 8;
    let length = size / 3;
    let thickness = (size / 12).max(1);
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let from_left = x.saturating_sub(margin);
            let from_top = y.saturating_sub(margin);
            let from_right = (size - 1).saturating_sub(x).saturating_sub(margin);
            let from_bottom = (size - 1).saturating_sub(y).saturating_sub(margin);
            let inside = x >= margin && y >= margin && x < size - margin && y < size - margin;
            let on_horizontal = |distance: u32| distance < thickness;
            let near_corner_x = from_left < length || from_right < length;
            let near_corner_y = from_top < length || from_bottom < length;
            let on_edge_x = on_horizontal(from_left) || on_horizontal(from_right);
            let on_edge_y = on_horizontal(from_top) || on_horizontal(from_bottom);
            let drawn = inside && ((on_edge_y && near_corner_x) || (on_edge_x && near_corner_y));
            if drawn {
                let offset = ((y * size + x) * 4) as usize;
                rgba[offset + 3] = 255;
            }
        }
    }
    rgba
}

pub const BUNDLE_ID: &str = "com.codepawl.oh-snap";
pub const APP_NAME: &str = "oh-snap.app";
pub const RELEASE_ASSET: &str = "oh-snap-macos.zip";

/// `/x/oh-snap.app/Contents/MacOS/oh-snap` gives `/x/oh-snap.app`.
pub fn bundle_of_executable(executable: &Path) -> Option<PathBuf> {
    let bundle = executable.parent()?.parent()?.parent()?;
    let is_app = bundle.extension().is_some_and(|extension| extension == "app");
    is_app.then(|| bundle.to_path_buf())
}

pub fn executable_in_bundle(bundle: &Path) -> PathBuf {
    bundle.join("Contents").join("MacOS").join("oh-snap")
}

pub fn launch_agent_path(home: &Path) -> PathBuf {
    home.join("Library").join("LaunchAgents").join(format!("{BUNDLE_ID}.plist"))
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The LaunchAgent that starts the tray program at sign-in.
pub fn launch_agent_plist(executable: &Path) -> String {
    let executable = escape_xml(&executable.to_string_lossy());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
\t<key>Label</key>\n\t<string>{BUNDLE_ID}</string>\n\
\t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>{executable}</string>\n\t\t<string>daemon</string>\n\t</array>\n\
\t<key>RunAtLoad</key>\n\t<true/>\n\
\t<key>ProcessType</key>\n\t<string>Interactive</string>\n\
</dict>\n\
</plist>\n"
    )
}

/// One system screenshot shortcut: its id, and macOS's default ascii code, key code and modifier mask.
pub type SystemShortcut = (u32, u32, u32, u32);

pub const SYSTEM_SHORTCUTS: [SystemShortcut; 5] = [
    (28, 51, 20, 1_179_648),  // Shift+Cmd+3  screen to file
    (29, 51, 20, 1_441_792),  // Ctrl+Shift+Cmd+3  screen to clipboard
    (30, 52, 21, 1_179_648),  // Shift+Cmd+4  area to file
    (31, 52, 21, 1_441_792),  // Ctrl+Shift+Cmd+4  area to clipboard
    (184, 53, 23, 1_179_648), // Shift+Cmd+5  screenshot and recording options
];

/// The plist value for one shortcut. It keeps the key combination, so turning it back on
/// restores what macOS shipped with.
pub fn shortcut_entry(shortcut: SystemShortcut, enabled: bool) -> String {
    let (_, ascii, key_code, modifiers) = shortcut;
    format!(
        "<dict><key>enabled</key><{enabled}/><key>value</key><dict><key>parameters</key>\
<array><integer>{ascii}</integer><integer>{key_code}</integer><integer>{modifiers}</integer></array>\
<key>type</key><string>standard</string></dict></dict>"
    )
}

/// Arguments for `defaults` that turn one system shortcut on or off.
pub fn shortcut_defaults_arguments(shortcut: SystemShortcut, enabled: bool) -> Vec<String> {
    vec![
        "write".into(),
        "com.apple.symbolichotkeys".into(),
        "AppleSymbolicHotKeys".into(),
        "-dict-add".into(),
        shortcut.0.to_string(),
        shortcut_entry(shortcut, enabled),
    ]
}

pub const SETTINGS_PANE_URL: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

pub const PERMISSION_MESSAGE: &str = "oh-snap needs permission to record the screen. Allow oh-snap in \
System Settings > Privacy & Security > Screen & System Audio Recording, then start oh-snap again.";

/// Where the unzipped update waits before it replaces the installed bundle.
pub fn staging_dir(installed_bundle: &Path) -> PathBuf {
    installed_bundle.with_file_name(".oh-snap-update")
}

pub fn old_bundle_path(installed_bundle: &Path) -> PathBuf {
    installed_bundle.with_file_name("oh-snap.app.old")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_is_the_largest_display_scale() {
        assert_eq!(desktop_scale(&[]), 1.0);
        assert_eq!(desktop_scale(&[1.0, 2.0]), 2.0);
    }

    #[test]
    fn points_and_pixels_round_trip() {
        let rect = PointRect { x: 10.5, y: 20.0, width: 100.0, height: 50.25 };
        let pixels = points_to_pixels(rect, 2.0);
        assert_eq!(pixels, PixelRect { x: 21, y: 40, width: 200, height: 101 });
        let back = pixels_to_points((pixels.x, pixels.y, pixels.width, pixels.height), 2.0);
        assert_eq!(back.x, 10.5);
    }

    #[test]
    fn picks_the_display_with_most_overlap() {
        let left = PointRect { x: 0.0, y: 0.0, width: 1440.0, height: 900.0 };
        let right = PointRect { x: 1440.0, y: 0.0, width: 1920.0, height: 1080.0 };
        let area = PointRect { x: 1400.0, y: 100.0, width: 500.0, height: 300.0 };
        assert_eq!(display_for_area(area, &[left, right]), Some(1));
        let outside = PointRect { x: 9000.0, y: 0.0, width: 10.0, height: 10.0 };
        assert_eq!(display_for_area(outside, &[left, right]), None);
    }

    #[test]
    fn sizes_become_even() {
        assert_eq!(even(101), 100);
        assert_eq!(even(0), 2);
    }

    #[test]
    fn trimmed_names_count_up() {
        let recording = Path::new("/v/Screen Recording 1.mp4");
        let taken = ["Screen Recording 1 trimmed.mp4"];
        let path = trimmed_path(recording, |candidate| {
            taken.iter().any(|name| candidate.file_name().is_some_and(|file| file == *name))
        });
        assert_eq!(path.file_name().unwrap(), "Screen Recording 1 trimmed 2.mp4");
        assert_eq!(trimmed_file_name("a", 1), "a trimmed.mp4");
    }

    #[test]
    fn gif_frames_and_size() {
        assert_eq!(gif_frame_times(0.0), vec![0.0]);
        assert_eq!(gif_frame_times(2.0).len(), 30);
    }

    #[test]
    fn menu_bar_icon_has_corners_and_an_empty_middle() {
        let size = 44;
        let rgba = tray_icon_rgba(size);
        assert_eq!(rgba.len(), (size * size * 4) as usize);
        let alpha = |x: u32, y: u32| rgba[((y * size + x) * 4 + 3) as usize];
        assert_eq!(alpha(5, 5), 255);
        assert_eq!(alpha(size - 6, size - 6), 255);
        assert_eq!(alpha(size / 2, size / 2), 0);
    }

    #[test]
    fn finds_the_bundle() {
        let exe = Path::new("/Applications/oh-snap.app/Contents/MacOS/oh-snap");
        assert_eq!(bundle_of_executable(exe), Some(PathBuf::from("/Applications/oh-snap.app")));
        assert_eq!(bundle_of_executable(Path::new("/usr/local/bin/oh-snap")), None);
        assert_eq!(
            executable_in_bundle(Path::new("/A/oh-snap.app")),
            Path::new("/A/oh-snap.app/Contents/MacOS/oh-snap")
        );
    }

    #[test]
    fn launch_agent_text() {
        let text = launch_agent_plist(Path::new("/A & B/oh-snap.app/Contents/MacOS/oh-snap"));
        assert!(text.contains("<string>/A &amp; B/oh-snap.app/Contents/MacOS/oh-snap</string>"));
        assert!(text.contains("<key>RunAtLoad</key>\n\t<true/>"));
        assert!(text.contains("<string>daemon</string>"));
    }

    #[test]
    fn shortcut_arguments() {
        let arguments = shortcut_defaults_arguments(SYSTEM_SHORTCUTS[2], false);
        assert_eq!(arguments[4], "30");
        assert!(arguments[5].contains("<key>enabled</key><false/>"));
        assert!(arguments[5].contains("<integer>52</integer><integer>21</integer>"));
        assert!(shortcut_entry(SYSTEM_SHORTCUTS[0], true).contains("<true/>"));
    }
}
