//! Add-ons: programs in their own folder that get a snip as a PNG and hand back text,
//! an image, a link or a message. Each folder holds an `addon.toml` like:
//!
//! ```toml
//! name = "Copy text (OCR)"
//! description = "Reads the text in the snip and copies it"
//! command = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File ocr.ps1 \"{file}\""
//! result = "text"      # text | image | url | message | none
//! ```
//!
//! `{file}` is the snip as PNG, `{out}` is where an image add-on writes its result and
//! `{dir}` is the add-on's own folder. The command runs in that folder, without a window.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MANIFEST: &str = "addon.toml";
const TIMEOUT: Duration = Duration::from_secs(60);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Add-ons that ship with oh-snap; `install` writes them when they are missing.
const BUNDLED: [(&str, &[(&str, &str)]); 2] = [
    (
        "ocr",
        &[
            ("addon.toml", include_str!("../addons/ocr/addon.toml")),
            ("ocr.ps1", include_str!("../addons/ocr/ocr.ps1")),
        ],
    ),
    ("data-uri", &[("addon.toml", include_str!("../addons/data-uri/addon.toml"))]),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultKind {
    Text,
    Image,
    Url,
    Message,
    Nothing,
}

#[derive(Clone, Debug)]
pub struct Addon {
    pub folder: PathBuf,
    pub name: String,
    pub description: String,
    pub command: String,
    pub result: ResultKind,
}

pub enum AddonOutput {
    Text(String),
    Image(PathBuf),
    Url(String),
    Message(String),
    Nothing,
}

pub fn addons_folder() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("oh-snap").join("addons")
}

/// Every add-on folder with a readable manifest, sorted by name. The bundled add-ons
/// are written first when missing, so they exist even without `oh-snap install`.
pub fn discover() -> Vec<Addon> {
    if let Err(error) = install_bundled() {
        crate::log::write(&format!("could not write the bundled add-ons: {error}"));
    }
    let Ok(entries) = std::fs::read_dir(addons_folder()) else {
        return Vec::new();
    };
    let mut addons: Vec<Addon> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| match read_manifest(&entry.path()) {
            Ok(addon) => Some(addon),
            Err(error) => {
                crate::log::write(&format!("add-on {} skipped: {error}", entry.path().display()));
                None
            }
        })
        .collect();
    addons.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    addons
}

fn read_manifest(folder: &Path) -> Result<Addon, String> {
    let text = std::fs::read_to_string(folder.join(MANIFEST)).map_err(|error| format!("no {MANIFEST}: {error}"))?;
    let mut name = None;
    let mut description = String::new();
    let mut command = None;
    let mut result = ResultKind::Message;
    for line in text.lines() {
        let line = line.split(" #").next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("cannot read line `{line}`"));
        };
        let value = unquote(value.trim())?;
        match key.trim() {
            "name" => name = Some(value),
            "description" => description = value,
            "command" => command = Some(value),
            "result" => result = parse_result(&value)?,
            _ => {}
        }
    }
    Ok(Addon {
        folder: folder.to_path_buf(),
        name: name.ok_or("missing `name`")?,
        description,
        command: command.ok_or("missing `command`")?,
        result,
    })
}

/// A TOML basic string: double quotes, with \" and \\ escapes.
fn unquote(value: &str) -> Result<String, String> {
    let inner = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or_else(|| format!("expected a quoted string, got `{value}`"))?;
    let mut text = String::new();
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            match characters.next() {
                Some('"') => text.push('"'),
                Some('\\') => text.push('\\'),
                Some('n') => text.push('\n'),
                Some(other) => {
                    text.push('\\');
                    text.push(other);
                }
                None => text.push('\\'),
            }
        } else {
            text.push(character);
        }
    }
    Ok(text)
}

fn parse_result(value: &str) -> Result<ResultKind, String> {
    match value {
        "text" => Ok(ResultKind::Text),
        "image" => Ok(ResultKind::Image),
        "url" => Ok(ResultKind::Url),
        "message" => Ok(ResultKind::Message),
        "none" => Ok(ResultKind::Nothing),
        other => Err(format!("unknown result `{other}`")),
    }
}

/// Runs the add-on on a PNG file and turns its output into what the manifest promises.
pub fn run(addon: &Addon, png_path: &Path) -> Result<AddonOutput, String> {
    let out_path = std::env::temp_dir().join(format!("oh-snap-addon-{}.png", std::process::id()));
    let _ = std::fs::remove_file(&out_path);
    let command_line = addon
        .command
        .replace("{file}", &png_path.display().to_string())
        .replace("{out}", &out_path.display().to_string())
        .replace("{dir}", &addon.folder.display().to_string());

    let mut child = Command::new("cmd.exe")
        .raw_arg("/d /s /c \"")
        .raw_arg(&command_line)
        .raw_arg("\"")
        .current_dir(&addon.folder)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|error| format!("could not start: {error}"))?;

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > TIMEOUT => {
                let _ = child.kill();
                return Err(format!("stopped after {} s", TIMEOUT.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(30)),
            Err(error) => return Err(error.to_string()),
        }
    }
    let output = child.wait_with_output().map_err(|error| error.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr.lines().chain(stdout.lines()).find(|line| !line.trim().is_empty()).unwrap_or("");
        return Err(format!("exited with {}: {}", output.status.code().unwrap_or(-1), reason.trim()));
    }
    match addon.result {
        ResultKind::Text => Ok(AddonOutput::Text(stdout)),
        ResultKind::Image if out_path.exists() => Ok(AddonOutput::Image(out_path)),
        ResultKind::Image => Err("wrote no image to {out}".into()),
        ResultKind::Url => {
            let url = stdout.lines().next().unwrap_or("").trim().to_string();
            if url.starts_with("http://") || url.starts_with("https://") {
                Ok(AddonOutput::Url(url))
            } else {
                Err(format!("printed no http(s) link: `{url}`"))
            }
        }
        ResultKind::Message => Ok(AddonOutput::Message(stdout.lines().next().unwrap_or("done").to_string())),
        ResultKind::Nothing => Ok(AddonOutput::Nothing),
    }
}

/// Writes the bundled add-ons that are not there yet; never overwrites a user's edits.
pub fn install_bundled() -> std::io::Result<()> {
    for (folder_name, files) in BUNDLED {
        let folder = addons_folder().join(folder_name);
        if folder.join(MANIFEST).exists() {
            continue;
        }
        std::fs::create_dir_all(&folder)?;
        for (file_name, contents) in files {
            std::fs::write(folder.join(file_name), contents)?;
        }
    }
    Ok(())
}

/// `oh-snap addons [list|add FOLDER|remove NAME|folder]`.
pub fn command(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    match arguments.first().map(String::as_str) {
        None | Some("list") => {
            install_bundled()?;
            let addons = discover();
            if addons.is_empty() {
                println!("no add-ons in {}", addons_folder().display());
            }
            for addon in addons {
                let folder_name = addon.folder.file_name().unwrap_or_default().to_string_lossy();
                println!("{folder_name:<16} {:<24} {}", addon.name, addon.description);
            }
            Ok(())
        }
        Some("add") => {
            let source = PathBuf::from(arguments.get(1).ok_or("add needs a folder with an addon.toml")?);
            let addon = read_manifest(&source)?;
            let folder_name = source.file_name().ok_or("the folder has no name")?;
            let destination = addons_folder().join(folder_name);
            copy_folder(&source, &destination)?;
            println!("added {} to {}", addon.name, destination.display());
            Ok(())
        }
        Some("remove") => {
            let name = arguments.get(1).ok_or("remove needs the add-on's folder name")?;
            let folder = addons_folder().join(name);
            if !folder.join(MANIFEST).exists() {
                return Err(format!("no add-on folder named `{name}`").into());
            }
            std::fs::remove_dir_all(&folder)?;
            println!("removed {name}");
            Ok(())
        }
        Some("run") => {
            let name = arguments.get(1).ok_or("run needs an add-on folder name and a PNG file")?;
            let file = arguments.get(2).ok_or("run needs a PNG file after the name")?;
            install_bundled()?;
            let addon = read_manifest(&addons_folder().join(name))?;
            let started = Instant::now();
            let output = run(&addon, &std::path::absolute(file)?)?;
            match output {
                AddonOutput::Text(text) => println!("{text}"),
                AddonOutput::Image(path) => println!("image: {}", path.display()),
                AddonOutput::Url(url) => println!("{url}"),
                AddonOutput::Message(message) => println!("{message}"),
                AddonOutput::Nothing => {}
            }
            eprintln!("({} in {} ms)", addon.name, started.elapsed().as_millis());
            Ok(())
        }
        Some("folder") => {
            println!("{}", addons_folder().display());
            Ok(())
        }
        Some(other) => Err(format!("unknown addons command `{other}`").into()),
    }
}

fn copy_folder(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_folder(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
