//! A bug report the user sends themselves: the repository's bug report form, filled in with
//! the version and what the log says about crashes and failures. Nothing leaves the machine
//! until they press submit in the browser, where they can read and edit every line first.

const NEW_ISSUE: &str = "https://github.com/codepawl/oh-snap/issues/new";
/// Enough to see a pattern, short enough for a link.
const MAX_LINES: usize = 8;
const MAX_LINE_CHARACTERS: usize = 300;

/// The link to a filled-in issue form, from the log on this machine.
pub fn link() -> String {
    let log = std::fs::read_to_string(crate::log::data_folder().join("oh-snap.log")).unwrap_or_default();
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).ok();
    issue_link(&problem_lines(&log, home.as_deref()))
}

/// Opens the filled-in issue form in the browser.
#[cfg(windows)]
pub fn open() {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, w};
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(link()), None, None, SW_SHOWNORMAL);
    }
}

/// The newest log lines about a crash or a failure, with the home folder taken out: a
/// path under it carries the user's name.
fn problem_lines(log: &str, home: Option<&str>) -> Vec<String> {
    let problems: Vec<&str> = log.lines().filter(|line| is_problem(line)).collect();
    let first_kept = problems.len().saturating_sub(MAX_LINES);
    problems[first_kept..].iter().map(|line| without_home(line, home)).collect()
}

fn is_problem(line: &str) -> bool {
    ["crash in", "failed", "error"].iter().any(|word| line.contains(word))
}

fn without_home(line: &str, home: Option<&str>) -> String {
    let shortened: String = line.chars().take(MAX_LINE_CHARACTERS).collect();
    let Some(home) = home.filter(|home| home.len() > 3) else {
        return shortened;
    };
    let cleaned = replace_ignoring_case(&shortened, home, "~");
    // The name alone can sit in other paths too. A very short one would hit ordinary words.
    match home.rsplit(['\\', '/']).next().filter(|name| name.len() >= 4) {
        Some(name) => replace_ignoring_case(&cleaned, name, "user"),
        None => cleaned,
    }
}

/// Windows paths match in any case. ASCII lowercase keeps every byte offset.
fn replace_ignoring_case(text: &str, needle: &str, replacement: &str) -> String {
    let lower_needle = needle.to_ascii_lowercase();
    let mut replaced = String::new();
    let mut rest = text;
    while let Some(found) = rest.to_ascii_lowercase().find(&lower_needle) {
        replaced.push_str(&rest[..found]);
        replaced.push_str(replacement);
        rest = &rest[found + needle.len()..];
    }
    replaced.push_str(rest);
    replaced
}

fn issue_link(lines: &[String]) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let crashed = lines.iter().any(|line| line.contains("crash in"));
    let title = if crashed { format!("Crash in {version}") } else { format!("Problem in {version}") };
    // The names are the field ids of the bug report form, which GitHub fills from the link.
    // "What happened" stays empty: only the user knows.
    let mut fields = vec![
        ("template", "bug_report.yml".to_string()),
        ("title", title),
        ("version", format!("oh-snap {version}")),
        ("system-details", format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)),
        ("log", lines.join("\n")),
    ];
    if cfg!(target_os = "macos") {
        fields.push(("system", "macOS".to_string()));
    }
    let query: Vec<String> = fields.iter().map(|(name, value)| format!("{name}={}", percent_encode(value))).collect();
    format!("{NEW_ISSUE}?{}", query.join("&"))
}

/// Everything but letters, digits and `-._~` becomes %XX, byte by byte.
fn percent_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_problem_lines_are_kept_and_only_the_newest() {
        let mut log = String::from("1 snip 10x10 at 0,0 delivered in 3 ms\n2 updating to 0.13.0\n");
        for number in 0..12 {
            log.push_str(&format!("{number} snip failed: reason {number}\n"));
        }
        log.push_str("99 crash in 0.13.0: exception 0xc0000005 at oh-snap.exe+0x1234\n");
        let lines = problem_lines(&log, None);
        assert_eq!(lines.len(), MAX_LINES);
        assert_eq!(lines[0], "5 snip failed: reason 5");
        assert!(lines[MAX_LINES - 1].starts_with("99 crash in"));
    }

    #[test]
    fn the_home_folder_never_reaches_the_report() {
        let line = r"7 could not edit C:\Users\Someone\Pictures\a.png, kept c:\users\someone\b.png";
        let cleaned = without_home(line, Some(r"C:\Users\Someone"));
        assert_eq!(cleaned, r"7 could not edit ~\Pictures\a.png, kept ~\b.png");
        assert_eq!(without_home(line, None), line);
        let elsewhere = without_home(r"8 failed: D:\work\C--Users-Someone-Documents\a.png", Some(r"C:\Users\Someone"));
        assert_eq!(elsewhere, r"8 failed: D:\work\C--Users-user-Documents\a.png");
    }

    #[test]
    fn the_link_is_one_safe_line() {
        let link = issue_link(&["9 crash in 0.13.0: panic: a & b = c #1".to_string()]);
        assert!(link.starts_with("https://github.com/codepawl/oh-snap/issues/new?template=bug_report.yml&title=Crash%20in%20"));
        let query = link.split_once('?').unwrap().1;
        assert!(query.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-._~%&=".contains(&byte)));
        // One `&` between fields and one `=` in each: nothing in a value splits the query.
        let field_count = query.split('&').count();
        assert_eq!(query.matches('=').count(), field_count);
        assert!(link.contains("&log=9%20crash%20in%200.13.0%3A%20panic%3A%20a%20%26%20b%20%3D%20c%20%231"));
    }

    #[test]
    fn a_report_without_a_crash_says_problem() {
        assert!(issue_link(&[]).contains("title=Problem%20in%20"));
    }
}
