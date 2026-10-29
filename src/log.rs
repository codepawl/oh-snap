//! A small append-only log in %LOCALAPPDATA%\oh-snap, for the daemon that has no console.

use std::io::Write;
use std::path::PathBuf;

const MAX_LOG_BYTES: u64 = 1_000_000;

pub fn data_folder() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("oh-snap")
}

pub fn write(line: &str) {
    let folder = data_folder();
    if std::fs::create_dir_all(&folder).is_err() {
        return;
    }
    let path = folder.join("oh-snap.log");
    if let Ok(metadata) = std::fs::metadata(&path) {
        if metadata.len() > MAX_LOG_BYTES {
            let _ = std::fs::remove_file(&path);
        }
    }
    let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let _ = writeln!(file, "{seconds} {line}");
}
