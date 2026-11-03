//! The text the running daemon understands on its socket, and where that socket lives.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// A capture mode word such as `region`; the caller turns it into a `Mode`.
    Snip(String),
    LastRegion,
    Record,
    Quit,
    OpenSettings,
    OpenFolder,
    Recent(PathBuf),
    CheckUpdate,
}

pub fn parse(line: &str) -> Option<Command> {
    let line = line.trim();
    let (word, rest) = match line.split_once(' ') {
        Some((word, rest)) => (word, rest.trim()),
        None => (line, ""),
    };
    match (word, rest) {
        ("capture", "last") => Some(Command::LastRegion),
        ("capture", mode) if !mode.is_empty() => Some(Command::Snip(mode.to_string())),
        ("record", "") => Some(Command::Record),
        ("quit", "") => Some(Command::Quit),
        ("settings", "") => Some(Command::OpenSettings),
        ("folder", "") => Some(Command::OpenFolder),
        ("check-update", "") => Some(Command::CheckUpdate),
        ("recent", path) if !path.is_empty() => Some(Command::Recent(PathBuf::from(path))),
        _ => None,
    }
}

/// `$XDG_RUNTIME_DIR/oh-snap.sock`, or a per-user file in /tmp when there is no runtime dir.
pub fn socket_path(runtime_dir: Option<&str>, user_id: u32) -> PathBuf {
    match runtime_dir.filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("oh-snap.sock"),
        None => PathBuf::from(format!("/tmp/oh-snap-{user_id}.sock")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        assert_eq!(parse("capture region\n"), Some(Command::Snip("region".into())));
        assert_eq!(parse("capture last"), Some(Command::LastRegion));
        assert_eq!(parse("recent /a b/c.png"), Some(Command::Recent(PathBuf::from("/a b/c.png"))));
        assert_eq!(parse("quit"), Some(Command::Quit));
        assert_eq!(parse("capture"), None);
        assert_eq!(parse("nonsense"), None);
    }

    #[test]
    fn picks_the_socket() {
        assert_eq!(socket_path(Some("/run/user/1000"), 1000), PathBuf::from("/run/user/1000/oh-snap.sock"));
        assert_eq!(socket_path(None, 7), PathBuf::from("/tmp/oh-snap-7.sock"));
    }
}
