//! The macOS and Linux app. `main` reads the same command words as the Windows program;
//! what an OS cannot do yet says so. The OS itself sits behind `platform::Platform`.

pub mod platform;
pub mod ui;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

use std::path::PathBuf;

use crate::mode::Mode;
use crate::output;
use platform::Platform;

const HELP: &str = "\
oh-snap — fast screen snips

usage
  oh-snap                          start in the tray (same as `daemon`)
  oh-snap capture [region|window|screen|freeform|last]
                  [--rect X,Y,W,H] capture that area without the overlay
                  [--out FILE]     write the PNG there instead of the screenshots folder
                  [--no-clipboard] [--no-save]
  oh-snap install                  run at sign-in and take the shortcuts
  oh-snap uninstall                undo install
  oh-snap record --rect X,Y,W,H [--seconds N] [--fps N] [--no-sound] [--microphone] [--out FILE]
                                   record an area without any window
  oh-snap text FILE                print the QR code in a PNG
  oh-snap quit                     stop the tray process

Only `capture --out FILE screen` works on this OS so far.
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
}

fn dispatch(arguments: &[String]) -> Result<(), String> {
    let first = arguments.first().map(String::as_str);
    match first {
        Some("--version" | "-V") => {
            println!("oh-snap {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("help" | "--help" | "-h") => {
            print!("{HELP}");
            Ok(())
        }
        Some("capture") => capture_command(&arguments[1..]),
        Some(
            "daemon" | "update" | "record" | "gif" | "trim" | "text" | "install" | "edit" | "addons" | "settings"
            | "uninstall" | "quit" | "bench",
        )
        | None => Err("not yet on this OS".into()),
        Some(other) => Err(format!("unknown command `{other}`\n\n{HELP}")),
    }
}

/// What `capture` was asked for.
struct CaptureRequest {
    mode: Mode,
    last: bool,
    area: Option<String>,
    out: Option<PathBuf>,
}

fn parse_capture(arguments: &[String]) -> Result<CaptureRequest, String> {
    let mut request = CaptureRequest { mode: Mode::Region, last: false, area: None, out: None };
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].as_str();
        match argument {
            "last" => request.last = true,
            "--rect" => {
                index += 1;
                request.area = Some(arguments.get(index).ok_or("--rect needs X,Y,W,H")?.clone());
            }
            "--out" => {
                index += 1;
                request.out = Some(PathBuf::from(arguments.get(index).ok_or("--out needs a file name")?));
            }
            "--no-clipboard" | "--no-save" => {}
            other => request.mode = Mode::parse(other).ok_or_else(|| format!("unknown capture option `{other}`"))?,
        }
        index += 1;
    }
    Ok(request)
}

fn capture_command(arguments: &[String]) -> Result<(), String> {
    let request = parse_capture(arguments)?;
    let whole_screen_to_file = request.mode == Mode::Screen && request.out.is_some();
    if request.last || request.area.is_some() || !whole_screen_to_file {
        return Err("not yet on this OS".into());
    }
    let path = request.out.expect("checked above");
    let screens = current_platform().capture_screens()?;
    let first = screens.first().ok_or("no screen to capture")?;
    let png = output::encode_png(&first.image).map_err(|error| error.to_string())?;
    output::write_png_to(&png, &path).map_err(|error| error.to_string())?;
    println!("{}x{} -> {}", first.image.width, first.image.height, path.display());
    Ok(())
}
