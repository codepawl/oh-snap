//! The local socket of a running daemon. `oh-snap capture region` and a desktop shortcut on
//! Wayland (which has no global hotkeys for apps) send one line of text to it; without a
//! daemon the command line does the work itself.

use std::path::PathBuf;

use super::platform::Trigger;
use crate::mode::Mode;

pub fn socket_path() -> PathBuf {
    crate::folders::data_folder().join("oh-snap.sock")
}

/// The line the daemon understands for a trigger.
pub fn command_for(trigger: &Trigger) -> String {
    match trigger {
        Trigger::Snip(mode) => format!("capture {}", mode.name()),
        Trigger::LastRegion => "capture last".into(),
        Trigger::Record => "record".into(),
        Trigger::Quit => "quit".into(),
        Trigger::OpenSettings => "settings".into(),
        Trigger::OpenFolder => "folder".into(),
        Trigger::Recent(path) => format!("recent {}", path.display()),
        Trigger::CheckUpdate => "update".into(),
        Trigger::Report => "report".into(),
        Trigger::CrashTest => "crash-test".into(),
    }
}

pub fn parse_command(line: &str) -> Option<Trigger> {
    let line = line.trim();
    let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
    match word {
        "capture" if rest == "last" => Some(Trigger::LastRegion),
        "capture" => Mode::parse(rest).map(Trigger::Snip),
        "record" => Some(Trigger::Record),
        "quit" => Some(Trigger::Quit),
        "settings" => Some(Trigger::OpenSettings),
        "folder" => Some(Trigger::OpenFolder),
        "update" => Some(Trigger::CheckUpdate),
        "report" => Some(Trigger::Report),
        "crash-test" => Some(Trigger::CrashTest),
        "recent" if !rest.is_empty() => Some(Trigger::Recent(PathBuf::from(rest))),
        _ => None,
    }
}

#[cfg(unix)]
mod unix {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};

    use super::{parse_command, socket_path};
    use crate::desktop::platform::Trigger;

    /// Sends one command to a running daemon and returns its answer.
    pub fn send(line: &str) -> Result<String, String> {
        let mut stream = UnixStream::connect(socket_path()).map_err(|error| error.to_string())?;
        stream.write_all(format!("{line}\n").as_bytes()).map_err(|error| error.to_string())?;
        let mut answer = String::new();
        BufReader::new(stream).read_line(&mut answer).map_err(|error| error.to_string())?;
        Ok(answer.trim().to_string())
    }

    pub fn daemon_is_running() -> bool {
        UnixStream::connect(socket_path()).is_ok()
    }

    /// Binds the socket and answers on a thread; each line becomes one call of `deliver`.
    /// Fails when another daemon already owns it.
    pub fn serve(deliver: impl Fn(Trigger) + Send + 'static) -> Result<(), String> {
        let path = socket_path();
        if daemon_is_running() {
            return Err("oh-snap is already running".into());
        }
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
        }
        // A socket file left by a daemon that did not shut down cleanly.
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).map_err(|error| format!("cannot bind {}: {error}", path.display()))?;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                answer(stream, &deliver);
            }
        });
        Ok(())
    }

    fn answer(mut stream: UnixStream, deliver: &impl Fn(Trigger)) {
        let mut line = String::new();
        let Ok(reader) = stream.try_clone() else {
            return;
        };
        if BufReader::new(reader).read_line(&mut line).is_err() {
            return;
        }
        let reply = match parse_command(&line) {
            Some(trigger) => {
                deliver(trigger);
                "ok"
            }
            None if line.trim() == "ping" => "ok",
            None => "unknown command",
        };
        let _ = stream.write_all(format!("{reply}\n").as_bytes());
    }

    pub fn remove() {
        let _ = std::fs::remove_file(socket_path());
    }
}

#[cfg(unix)]
pub use unix::{daemon_is_running, remove, send, serve};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_trigger_round_trips_through_its_line() {
        let triggers = [
            Trigger::Snip(Mode::Region),
            Trigger::Snip(Mode::Freeform),
            Trigger::LastRegion,
            Trigger::Record,
            Trigger::Quit,
            Trigger::OpenSettings,
            Trigger::OpenFolder,
            Trigger::Recent(PathBuf::from("/tmp/a b.png")),
            Trigger::CheckUpdate,
            Trigger::Report,
            Trigger::CrashTest,
        ];
        for trigger in triggers {
            assert_eq!(parse_command(&command_for(&trigger)), Some(trigger.clone()), "{trigger:?}");
        }
    }

    #[test]
    fn unknown_words_are_refused() {
        assert_eq!(parse_command("capture nonsense"), None);
        assert_eq!(parse_command("dance"), None);
    }
}
