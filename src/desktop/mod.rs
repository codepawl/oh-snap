//! The macOS and Linux app. `main` reads the same command words as the Windows program.
//! The OS sits behind `platform::Platform`; the windows are `ui` (drawn into pixmaps) and
//! `app` (the winit shell that shows them).

pub mod finish;
pub mod platform;
pub mod socket;
pub mod ui;
pub mod update;

#[cfg(not(windows))]
mod app;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

use std::path::PathBuf;
use std::time::Duration;

use crate::mode::Mode;
use finish::Overrides;
use platform::{Platform, RecordOptions, Trigger};

const HELP: &str = "\
oh-snap — fast screen snips

usage
  oh-snap                          start in the tray (same as `daemon`)
  oh-snap capture [region|window|screen|freeform|last]
                  [--rect X,Y,W,H] capture that area without the overlay
                  [--out FILE]     write the PNG there instead of the screenshots folder
                  [--no-clipboard] [--no-save]
  oh-snap record --rect X,Y,W,H [--seconds N] [--fps N] [--no-sound] [--microphone] [--out FILE]
                                   record an area without any window
  oh-snap gif FILE                 make a GIF next to a recording
  oh-snap trim FILE --from S --to S
                                   save the part of a recording between two seconds
  oh-snap text FILE                print the QR code (or the text) in a PNG
  oh-snap install                  run at sign-in and take the shortcuts
  oh-snap uninstall                undo install
  oh-snap update                   install the newest release now
  oh-snap settings                 open the settings file
  oh-snap quit                     stop the tray process
  oh-snap --version

With a running daemon, `capture` and `quit` go to it through its socket (bind them to a
desktop shortcut on Wayland). Without one, the command does the work itself.
";

pub fn main() {
    crate::crash::install();
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = dispatch(&arguments) {
        eprintln!("oh-snap: {error}");
        crate::log::write(&format!("error: {error}"));
        std::process::exit(1);
    }
}

/// The Platform for the OS this binary was built for.
pub fn current_platform() -> Box<dyn Platform> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacPlatform)
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxPlatform)
    }
    #[cfg(windows)]
    {
        Box::new(ui::testing::FakePlatform::default())
    }
}

fn dispatch(arguments: &[String]) -> Result<(), String> {
    let first = arguments.first().map(String::as_str);
    let rest = arguments.get(1..).unwrap_or(&[]);
    match first {
        Some("--version" | "-V") => {
            println!("oh-snap {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("help" | "--help" | "-h") => {
            print!("{HELP}");
            Ok(())
        }
        // Opened by hand (Finder, `open`): offer setup. The sign-in agent passes `daemon`.
        None => run_app(None, Overrides::default(), true),
        Some("daemon") => run_app(None, Overrides::default(), false),
        Some("capture") => capture_command(rest),
        Some("record") => record_command(rest),
        Some("gif") => {
            let recording = PathBuf::from(rest.first().ok_or("gif needs a recording file")?);
            println!("{}", current_platform().make_gif(&recording)?.display());
            Ok(())
        }
        Some("trim") => trim_command(rest),
        Some("text") => text_command(rest),
        Some("install") => print_text(current_platform().install()),
        Some("uninstall") => print_text(current_platform().uninstall()),
        Some("update") => update_command(),
        Some("settings") => {
            let file = crate::folders::config_folder().join("settings.toml");
            current_platform().open(&file);
            Ok(())
        }
        Some("quit") => forward("quit").map(|_| ()),
        Some("edit" | "addons" | "bench") => Err("not available on this OS".into()),
        Some(other) => Err(format!("unknown command `{other}`\n\n{HELP}")),
    }
}

fn print_text(result: Result<String, String>) -> Result<(), String> {
    println!("{}", result?);
    Ok(())
}

/// Sends a command to the running daemon.
fn forward(line: &str) -> Result<String, String> {
    #[cfg(unix)]
    {
        socket::send(line).map_err(|error| format!("no oh-snap daemon is running ({error})"))
    }
    #[cfg(not(unix))]
    {
        Err(format!("cannot send `{line}`: no socket on this OS"))
    }
}

fn daemon_is_running() -> bool {
    #[cfg(unix)]
    {
        socket::daemon_is_running()
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn run_app(first: Option<(Trigger, bool)>, overrides: Overrides, offer_setup: bool) -> Result<(), String> {
    #[cfg(not(windows))]
    {
        app::run(first, overrides, offer_setup)
    }
    #[cfg(windows)]
    {
        let _ = (first, overrides, offer_setup);
        Err("the desktop app does not run on Windows".into())
    }
}

/// What `capture` was asked for.
struct CaptureRequest {
    mode: Mode,
    last: bool,
    area: Option<(i32, i32, u32, u32)>,
    overrides: Overrides,
}

fn parse_capture(arguments: &[String]) -> Result<CaptureRequest, String> {
    let mut request = CaptureRequest { mode: Mode::Region, last: false, area: None, overrides: Overrides::default() };
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "last" => request.last = true,
            "--rect" => {
                index += 1;
                request.area = Some(finish::parse_rect(arguments.get(index).ok_or("--rect needs X,Y,W,H")?)?);
            }
            "--out" => {
                index += 1;
                request.overrides.out = Some(PathBuf::from(arguments.get(index).ok_or("--out needs a file name")?));
            }
            "--no-clipboard" => request.overrides.no_clipboard = true,
            "--no-save" => request.overrides.no_save = true,
            other => request.mode = Mode::parse(other).ok_or_else(|| format!("unknown capture option `{other}`"))?,
        }
        index += 1;
    }
    Ok(request)
}

fn capture_command(arguments: &[String]) -> Result<(), String> {
    let request = parse_capture(arguments)?;
    let plain = request.area.is_none() && request.overrides.out.is_none() && !request.overrides.no_clipboard && !request.overrides.no_save;
    let trigger = if request.last { Trigger::LastRegion } else { Trigger::Snip(request.mode) };
    if plain && daemon_is_running() {
        forward(&socket::command_for(&trigger))?;
        return Ok(());
    }
    let platform = current_platform();
    let settings = crate::settings::load();
    let direct_area = if let Some(area) = request.area {
        Some(area)
    } else if request.last {
        Some(finish::last_region().ok_or("no last region yet")?)
    } else {
        None
    };
    let image = match direct_area {
        Some(area) => finish::capture_area(platform.as_ref(), area)?,
        None if request.mode == Mode::Screen && request.overrides.out.is_some() => {
            let screens = platform.capture_screens()?;
            screens.into_iter().next().ok_or("no screen to capture")?.image
        }
        None => return run_app(Some((trigger, true)), request.overrides, false),
    };
    let finished = finish::finish_snip(platform.as_ref(), &image, &settings, &request.overrides)?;
    if let Some(error) = &finished.copy_error {
        eprintln!("oh-snap: clipboard: {error}");
    }
    match (&finished.saved_path, &finished.save_error) {
        (Some(path), _) => println!("{}x{} -> {}", image.width, image.height, path.display()),
        (None, Some(error)) => return Err(format!("could not save: {error}")),
        (None, None) => println!("{}x{}", image.width, image.height),
    }
    Ok(())
}

fn value_after<'a>(arguments: &'a [String], flag: &str) -> Result<Option<&'a str>, String> {
    match arguments.iter().position(|argument| argument == flag) {
        Some(index) => arguments.get(index + 1).map(|value| Some(value.as_str())).ok_or_else(|| format!("{flag} needs a value")),
        None => Ok(None),
    }
}

fn record_command(arguments: &[String]) -> Result<(), String> {
    let area = finish::parse_rect(value_after(arguments, "--rect")?.ok_or("record needs --rect X,Y,W,H")?)?;
    let settings = crate::settings::load();
    let seconds: Option<f64> = value_after(arguments, "--seconds")?.map(|text| text.parse().map_err(|_| "--seconds needs a number")).transpose()?;
    let frames_per_second = value_after(arguments, "--fps")?
        .map(|text| text.parse().map_err(|_| "--fps needs a number"))
        .transpose()?
        .unwrap_or(settings.record_frames_per_second);
    let options = RecordOptions {
        frames_per_second,
        system_sound: settings.record_system_sound && !arguments.iter().any(|argument| argument == "--no-sound"),
        microphone: settings.record_microphone || arguments.iter().any(|argument| argument == "--microphone"),
    };
    let output = value_after(arguments, "--out")?.map(PathBuf::from).unwrap_or_else(|| finish::recording_path(&settings));
    if let Some(folder) = output.parent() {
        std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    }
    let recording = current_platform().start_recording(area, options, &output)?;
    match seconds {
        Some(seconds) => std::thread::sleep(Duration::from_secs_f64(seconds.max(0.0))),
        None => {
            eprintln!("recording; press Enter to stop");
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
        }
    }
    println!("{}", recording.stop()?.display());
    Ok(())
}

fn trim_command(arguments: &[String]) -> Result<(), String> {
    let recording = PathBuf::from(arguments.first().ok_or("trim needs a recording file")?);
    let seconds = |flag: &str| -> Result<f64, String> {
        value_after(arguments, flag)?.ok_or_else(|| format!("trim needs {flag} S"))?.parse().map_err(|_| format!("{flag} needs a number"))
    };
    println!("{}", current_platform().trim(&recording, seconds("--from")?, seconds("--to")?)?.display());
    Ok(())
}

fn text_command(arguments: &[String]) -> Result<(), String> {
    let file = arguments.first().ok_or("text needs a PNG file")?;
    let image = crate::image_file::load_png(std::path::Path::new(file)).map_err(|error| error.to_string())?;
    let text = match crate::qr::read_qr_code(&image) {
        Some(payload) => payload,
        None => current_platform().read_text(&image)?,
    };
    println!("{text}");
    Ok(())
}

fn update_command() -> Result<(), String> {
    #[cfg(not(windows))]
    {
        let platform = current_platform();
        match update::check(platform.as_ref(), crate::settings::UpdateMode::Auto)? {
            update::UpdateResult::UpToDate => println!("oh-snap {} is the newest release", update::CURRENT_VERSION),
            update::UpdateResult::Available(version) => println!("oh-snap {version} is out"),
            update::UpdateResult::Installed(version) => println!("installed {version}; it runs after a restart"),
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        Err("update does not run on Windows here".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn parses_capture_options() {
        let request = parse_capture(&words("window --out a.png --no-clipboard --rect 1,2,3,4")).unwrap();
        assert_eq!(request.mode, Mode::Window);
        assert_eq!(request.area, Some((1, 2, 3, 4)));
        assert!(request.overrides.no_clipboard && !request.overrides.no_save);
        assert!(parse_capture(&words("--bogus")).is_err());
        assert!(parse_capture(&words("last")).unwrap().last);
    }

    #[test]
    fn reads_flag_values() {
        let arguments = words("clip.mp4 --from 1.5 --to 4");
        assert_eq!(value_after(&arguments, "--from").unwrap(), Some("1.5"));
        assert_eq!(value_after(&arguments, "--seconds").unwrap(), None);
        assert!(value_after(&words("x --to"), "--to").is_err());
    }
}
