//! The texts install writes: .desktop files, GNOME keybinding values and the undo record.

pub const MEDIA_KEYS_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys";
pub const CUSTOM_KEYBINDING_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";
pub const SHELL_KEYBINDINGS_SCHEMA: &str = "org.gnome.shell.keybindings";
/// GNOME's own screenshot keys, which would swallow Print before oh-snap sees it.
pub const GNOME_SCREENSHOT_KEYS: [&str; 4] = ["show-screenshot-ui", "screenshot", "screenshot-window", "screenshot-clip"];

pub struct Keybinding {
    pub id: &'static str,
    pub name: &'static str,
    pub binding: &'static str,
    /// Arguments after the program name.
    pub arguments: &'static str,
}

pub const KEYBINDINGS: [Keybinding; 3] = [
    Keybinding { id: "oh-snap-region", name: "oh-snap region", binding: "Print", arguments: "capture region" },
    Keybinding { id: "oh-snap-last", name: "oh-snap last region", binding: "<Shift>Print", arguments: "capture last" },
    Keybinding { id: "oh-snap-screen", name: "oh-snap screen", binding: "<Control>Print", arguments: "capture screen" },
];

pub fn keybinding_path(id: &str) -> String {
    format!("/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/{id}/")
}

/// The .desktop file for the menu (`autostart` false) or for sign-in start (`autostart` true).
pub fn desktop_entry(executable: &str, autostart: bool) -> String {
    let mut entry = String::from("[Desktop Entry]\nType=Application\nName=oh-snap\nGenericName=Screenshot tool\n");
    entry.push_str("Comment=Fast screen snipping tool\n");
    if autostart {
        entry.push_str(&format!("Exec={} daemon\n", quote_exec(executable)));
        entry.push_str("X-GNOME-Autostart-enabled=true\nNoDisplay=true\n");
    } else {
        entry.push_str(&format!("Exec={} capture region\n", quote_exec(executable)));
        entry.push_str("Categories=Utility;Graphics;\nKeywords=screenshot;snip;capture;record;\n");
        entry.push_str("Actions=Screen;Window;\n\n[Desktop Action Screen]\nName=Capture screen\n");
        entry.push_str(&format!("Exec={} capture screen\n\n", quote_exec(executable)));
        entry.push_str("[Desktop Action Window]\nName=Capture window\n");
        entry.push_str(&format!("Exec={} capture window\n", quote_exec(executable)));
    }
    entry.push_str("Icon=oh-snap\nTerminal=false\n");
    entry
}

/// A path as the Exec key wants it: quoted when it holds a space or another reserved character.
fn quote_exec(path: &str) -> String {
    if path.chars().any(|c| c.is_whitespace() || "\"'\\><~|&;$*?#()`".contains(c)) {
        let escaped: String = path
            .chars()
            .flat_map(|c| if "\"`$\\".contains(c) { vec!['\\', c] } else { vec![c] })
            .collect();
        format!("\"{escaped}\"")
    } else {
        path.to_string()
    }
}

/// `['a', 'b']` or `@as []` from gsettings to its strings.
pub fn parse_string_list(value: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut inside = false;
    let mut escaped = false;
    for character in value.chars() {
        if inside {
            if escaped {
                current.push(character);
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '\'' {
                items.push(std::mem::take(&mut current));
                inside = false;
            } else {
                current.push(character);
            }
        } else if character == '\'' {
            inside = true;
        }
    }
    items
}

pub fn format_string_list(items: &[String]) -> String {
    if items.is_empty() {
        return "@as []".to_string();
    }
    let quoted: Vec<String> = items.iter().map(|item| format!("'{}'", item.replace('\\', "\\\\").replace('\'', "\\'"))).collect();
    format!("[{}]", quoted.join(", "))
}

/// The custom-keybindings list with ours added once.
pub fn add_ours(existing: &[String]) -> Vec<String> {
    let mut list = existing.to_vec();
    for binding in &KEYBINDINGS {
        let path = keybinding_path(binding.id);
        if !list.contains(&path) {
            list.push(path);
        }
    }
    list
}

pub fn remove_ours(existing: &[String]) -> Vec<String> {
    let ours: Vec<String> = KEYBINDINGS.iter().map(|binding| keybinding_path(binding.id)).collect();
    existing.iter().filter(|path| !ours.contains(path)).cloned().collect()
}

/// A setting changed by install: `schema key value`, kept so uninstall can put it back.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct SavedSetting {
    pub schema: String,
    pub key: String,
    pub value: String,
}

pub fn serialize_saved(settings: &[SavedSetting]) -> String {
    settings.iter().map(|saved| format!("{}\t{}\t{}\n", saved.schema, saved.key, saved.value)).collect()
}

pub fn parse_saved(text: &str) -> Vec<SavedSetting> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            Some(SavedSetting { schema: parts.next()?.to_string(), key: parts.next()?.to_string(), value: parts.next()?.to_string() })
        })
        .collect()
}

pub fn is_gnome(current_desktop: &str) -> bool {
    current_desktop.split(':').any(|part| part.eq_ignore_ascii_case("gnome") || part.eq_ignore_ascii_case("ubuntu"))
}

pub fn is_kde(current_desktop: &str) -> bool {
    current_desktop.split(':').any(|part| part.eq_ignore_ascii_case("kde"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_lists_round_trip() {
        assert_eq!(parse_string_list("@as []"), Vec::<String>::new());
        assert_eq!(parse_string_list("['/a/', '/b/']"), vec!["/a/", "/b/"]);
        let items = vec!["it's".to_string(), "<Shift>Print".to_string()];
        assert_eq!(parse_string_list(&format_string_list(&items)), items);
        assert_eq!(format_string_list(&[]), "@as []");
    }

    #[test]
    fn adds_and_removes_our_paths_once() {
        let existing = vec!["/other/".to_string()];
        let added = add_ours(&existing);
        assert_eq!(added.len(), 4);
        assert_eq!(add_ours(&added), added);
        assert_eq!(remove_ours(&added), existing);
    }

    #[test]
    fn saved_settings_round_trip() {
        let saved = vec![SavedSetting { schema: "a.b".into(), key: "k".into(), value: "['Print']".into() }];
        assert_eq!(parse_saved(&serialize_saved(&saved)), saved);
    }

    #[test]
    fn desktop_entries() {
        let menu = desktop_entry("/home/a b/.local/bin/oh-snap", false);
        assert!(menu.contains("Exec=\"/home/a b/.local/bin/oh-snap\" capture region"));
        assert!(menu.contains("Icon=oh-snap"));
        let autostart = desktop_entry("/home/a/.local/bin/oh-snap", true);
        assert!(autostart.contains("Exec=/home/a/.local/bin/oh-snap daemon"));
        assert!(!autostart.contains("Actions="));
    }

    #[test]
    fn detects_desktops() {
        assert!(is_gnome("ubuntu:GNOME"));
        assert!(is_kde("KDE"));
        assert!(!is_gnome("KDE"));
    }
}
