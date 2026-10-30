//! The last snips, kept as PNG files in %LOCALAPPDATA%\oh-snap\history so the tray menu
//! can bring them back, also when saving to a folder is off. Only the newest are kept.

use std::path::{Path, PathBuf};

use windows::Win32::System::SystemInformation::GetLocalTime;

const KEPT: usize = 20;
/// How many the tray menu lists.
pub const LISTED: usize = 10;

pub fn folder() -> PathBuf {
    crate::log::data_folder().join("history")
}

/// Keeps a copy of a snip and drops the oldest copies past the limit.
pub fn record(png_bytes: &[u8]) -> Option<PathBuf> {
    let folder = folder();
    std::fs::create_dir_all(&folder).ok()?;
    let time = unsafe { GetLocalTime() };
    let name = format!(
        "{:04}-{:02}-{:02} {:02}{:02}{:02}-{:03}.png",
        time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute, time.wSecond, time.wMilliseconds
    );
    let path = folder.join(name);
    std::fs::write(&path, png_bytes).ok()?;
    for old in newest_first(&folder).into_iter().skip(KEPT) {
        let _ = std::fs::remove_file(old);
    }
    Some(path)
}

/// The kept snips, newest first.
pub fn list() -> Vec<PathBuf> {
    newest_first(&folder())
}

fn newest_first(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "png"))
        .collect();
    // The names start with the date and time, so name order is time order.
    paths.sort();
    paths.reverse();
    paths
}

/// "10-30 14:05:22  800×600" for the tray menu.
pub fn label(path: &Path) -> String {
    let stem = path.file_stem().map(|stem| stem.to_string_lossy().to_string()).unwrap_or_default();
    let time = time_label(&stem).unwrap_or_else(|| stem.clone());
    match png_size(path) {
        Some((width, height)) => format!("{time}  {width}×{height}"),
        None => time,
    }
}

/// "2026-10-30 140522-123" becomes "10-30 14:05:22".
fn time_label(stem: &str) -> Option<String> {
    let (date, clock) = stem.split_once(' ')?;
    let month_day = date.get(5..10)?;
    let clock = clock.get(0..6)?;
    Some(format!("{month_day} {}:{}:{}", &clock[0..2], &clock[2..4], &clock[4..6]))
}

/// Width and height from the PNG header, without decoding the image.
fn png_size(path: &Path) -> Option<(u32, u32)> {
    use std::io::Read;
    let mut header = [0u8; 24];
    std::fs::File::open(path).ok()?.read_exact(&mut header).ok()?;
    if &header[1..4] != b"PNG" {
        return None;
    }
    let width = u32::from_be_bytes(header[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(header[20..24].try_into().ok()?);
    Some((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_come_from_the_file_name() {
        assert_eq!(time_label("2026-10-30 140522-123").as_deref(), Some("10-30 14:05:22"));
        assert_eq!(time_label("notes"), None);
    }

    #[test]
    fn keeps_only_the_newest() {
        let folder = std::env::temp_dir().join(format!("oh-snap-history-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        for second in 0..25 {
            std::fs::write(folder.join(format!("2026-10-30 1400{second:02}-000.png")), b"x").unwrap();
        }
        let listed = newest_first(&folder);
        assert_eq!(listed.len(), 25);
        assert!(listed[0].ends_with("2026-10-30 140024-000.png"));
        let _ = std::fs::remove_dir_all(&folder);
    }
}
