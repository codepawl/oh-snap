#![windows_subsystem = "windows"]

mod addons;
mod annotate;
mod bench;
mod card;
mod compare;
mod confirm;
mod capture;
mod daemon;
mod editor;
mod icons;
mod image_file;
mod history;
mod image_menu;
mod install;
mod log;
mod logo;
mod markup;
mod ocr;
mod output;
mod particles;
mod pin;
mod overlay;
mod gif_export;
mod recorder;
mod recording_bar;
mod preview;
mod snip;
mod settings;
mod settings_window;
mod sound;
mod style;
mod updater;

use std::path::PathBuf;

use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};

use capture::{Capturer, PixelRect};
use overlay::{Graphics, Mode};
use snip::SnipRequest;

const HELP: &str = "\
oh-snap — fast screen snips

usage
  oh-snap                          start in the tray (same as `daemon`)
  oh-snap capture [region|window|screen|last]
                  [--rect X,Y,W,H] capture that area without the overlay
                  [--out FILE]     write the PNG there instead of Pictures\\Screenshots
                  [--no-clipboard] [--no-save]
  oh-snap install                  run at sign-in, take PrtSc, Win+Shift+S and ms-screenclip:
  oh-snap uninstall                give them back to Snipping Tool
  oh-snap record --rect X,Y,W,H [--seconds N] [--fps N] [--no-sound] [--microphone] [--out FILE]
                                   record an area without any window
  oh-snap gif FILE                 make a GIF from a recording
  oh-snap text FILE                print the QR code or the text in a PNG
  oh-snap update                   install the latest release from GitHub
  oh-snap bench [ROUNDS]           measure capture and overlay speed
  oh-snap quit                     stop the tray process
";

fn main() {
    unsafe {
        // Output reaches the terminal when started from one; ignored otherwise.
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        // Physical pixels everywhere, so captures and overlay line up on mixed-DPI setups.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = dispatch(&arguments) {
        eprintln!("oh-snap: {error}");
        log::write(&format!("error: {error}"));
        std::process::exit(1);
    }
}

fn dispatch(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let first = arguments.first().map(String::as_str);
    match first {
        None => Ok(daemon::run(false)?),
        Some("daemon") => Ok(daemon::run(arguments.get(1).map(String::as_str) == Some("--after-update"))?),
        Some("update") => updater::command(),
        Some("record") => record_command(&arguments[1..]),
        Some("gif") => gif_export::command(arguments.get(1).ok_or("usage: oh-snap gif FILE")?),
        Some("text") => ocr::command(arguments.get(1).ok_or("usage: oh-snap text FILE")?),
        Some("capture") => capture_command(&arguments[1..]),
        Some("install") => install::install(),
        Some("edit") => {
            let path = arguments.get(1).ok_or("edit needs a PNG file")?;
            let path = std::path::absolute(path)?;
            // The tray process opens it so the window shares its warm graphics.
            if let Some(daemon) = daemon::find_running() {
                daemon::send_command(daemon, &format!("edit {}", path.display()));
                return Ok(());
            }
            editor::run_standalone(path)
        }
        Some("addons") => addons::command(&arguments[1..]),
        Some("settings") => {
            if let Some(daemon) = daemon::find_running() {
                daemon::send_command(daemon, "settings");
                return Ok(());
            }
            settings_window::run_standalone()
        }
        Some("uninstall") => install::uninstall(),
        Some("logo") => {
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
            }
            let folder = arguments.get(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("assets"));
            logo::export(&Graphics::new()?, &folder)
        }
        Some("sound") => {
            let path = arguments.get(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("shutter.wav"));
            Ok(sound::write_shutter(&path)?)
        }
        Some("preview") => {
            let folder = arguments.get(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("preview"));
            preview::run(&folder)
        }
        Some("bench") if arguments.get(1).map(String::as_str) == Some("compare") => {
            let rounds = arguments.get(2).and_then(|text| text.parse().ok()).unwrap_or(5);
            compare::run(rounds)
        }
        Some("bench") => {
            let rounds = arguments.get(1).and_then(|text| text.parse().ok()).unwrap_or(10);
            bench::run(rounds)
        }
        Some("quit") => {
            if let Some(daemon) = daemon::find_running() {
                daemon::send_command(daemon, "quit");
            }
            Ok(())
        }
        Some("--version" | "-V") => {
            println!("oh-snap {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("help" | "--help" | "-h") => {
            print!("{HELP}");
            Ok(())
        }
        Some(uri) if is_snip_uri(uri) => {
            let mode = mode_from_uri(uri);
            capture_command(&[mode_name(mode).to_string()])
        }
        Some(other) => Err(format!("unknown command `{other}`\n\n{HELP}").into()),
    }
}

fn record_command(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut area = None;
    let mut seconds = 10;
    let mut options = recorder::RecordOptions { frames_per_second: 30, system_sound: true, microphone: false };
    let mut out = None;
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].as_str();
        index += 1;
        if argument == "--no-sound" {
            options.system_sound = false;
            continue;
        }
        if argument == "--microphone" {
            options.microphone = true;
            continue;
        }
        let value = arguments.get(index).ok_or_else(|| format!("{argument} needs a value"))?;
        index += 1;
        match argument {
            "--rect" => area = Some(parse_rect(value)?),
            "--seconds" => seconds = value.parse()?,
            "--fps" => options.frames_per_second = value.parse()?,
            "--out" => out = Some(PathBuf::from(value)),
            other => return Err(format!("unknown record option `{other}`").into()),
        }
    }
    let area = area.ok_or("record needs --rect X,Y,W,H")?;
    recorder::command(area, seconds, options, out)
}

fn capture_command(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if arguments.first().map(String::as_str) == Some("last") {
        // Only the tray process remembers the last area.
        let daemon = daemon::find_running().ok_or("`capture last` needs oh-snap running in the tray")?;
        daemon::send_command(daemon, "capture last");
        return Ok(());
    }
    let mut mode = Mode::Region;
    let mut area: Option<PixelRect> = None;
    let mut explicit_mode = None;
    let mut request = SnipRequest {
        mode,
        output_path: None,
        settings: settings::load(),
    };
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].as_str();
        match argument {
            "--rect" => {
                index += 1;
                area = Some(parse_rect(arguments.get(index).ok_or("--rect needs X,Y,W,H")?)?);
            }
            "--out" => {
                index += 1;
                let path = arguments.get(index).ok_or("--out needs a file name")?;
                request.output_path = Some(PathBuf::from(path));
            }
            "--no-clipboard" => request.settings.copy_to_clipboard = false,
            "--no-save" => request.settings.save_to_folder = false,
            other => explicit_mode = Some(Mode::parse(other).ok_or_else(|| format!("unknown capture option `{other}`"))?),
        }
        index += 1;
    }
    mode = explicit_mode.unwrap_or(request.settings.default_mode);
    request.mode = mode;

    let plain_interactive = area.is_none() && request.output_path.is_none() && request.settings.copy_to_clipboard && request.settings.save_to_folder;
    if plain_interactive {
        // The tray process is warm; let it snip instead of starting cold here.
        if let Some(daemon) = daemon::find_running() {
            let command = match explicit_mode {
                Some(mode) => format!("capture {}", mode_name(mode)),
                None => "capture".to_string(),
            };
            daemon::send_command(daemon, &command);
            return Ok(());
        }
    }

    let mut capturer = Capturer::new()?;
    let outcome = match area {
        Some(area) => snip::run_area(&mut capturer, None, &request, area)?,
        None => {
            let graphics = Graphics::new()?;
            snip::run(&mut capturer, &graphics, None, &request)?
        }
    };
    match outcome {
        Some(outcome) => {
            let area = outcome.area;
            match outcome.saved_to {
                Some(path) => println!("{}x{} -> {}", area.width, area.height, path.display()),
                None if outcome.copied => println!("{}x{} -> clipboard", area.width, area.height),
                None => println!("{}x{} -> recent snips only", area.width, area.height),
            }
        }
        None => println!("cancelled"),
    }
    Ok(())
}

fn parse_rect(text: &str) -> Result<PixelRect, Box<dyn std::error::Error>> {
    let numbers: Vec<i32> = text
        .split(',')
        .map(|part| part.trim().parse::<i32>())
        .collect::<Result<_, _>>()
        .map_err(|_| format!("--rect wants X,Y,W,H, got `{text}`"))?;
    let [left, top, width, height] = numbers[..] else {
        return Err(format!("--rect wants X,Y,W,H, got `{text}`").into());
    };
    if width <= 0 || height <= 0 {
        return Err("--rect width and height must be positive".into());
    }
    Ok(PixelRect { left, top, width, height })
}

fn is_snip_uri(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.starts_with("ms-screenclip:") || lower.starts_with("oh-snap:")
}

/// `ms-screenclip:?clippingMode=Window`, `oh-snap://capture/screen` and similar.
fn mode_from_uri(uri: &str) -> Mode {
    let lower = uri.to_ascii_lowercase();
    if lower.contains("window") {
        Mode::Window
    } else if lower.contains("fullscreen") || lower.contains("screen/") || lower.ends_with("screen") {
        Mode::Screen
    } else {
        Mode::Region
    }
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Region => "region",
        Mode::Window => "window",
        Mode::Screen => "screen",
    }
}
