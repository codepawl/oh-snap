//! What runs while the tray process lives: the command socket, the tray icon and the hotkeys.
//! `start` returns at once; every piece has its own thread.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ksni::blocking::TrayMethods;
use ksni::menu::{MenuItem, StandardItem, SubMenu};

use super::logic::command::{self, Command};
use super::logic::session::Session;
use super::{wayland, xorg};
use crate::desktop::platform::Trigger;
use crate::mode::Mode;

type Sender = Arc<dyn Fn(Trigger) + Send + Sync>;

pub fn socket_file() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").ok();
    // SAFETY: getuid has no arguments and cannot fail.
    let user_id = unsafe { libc::getuid() };
    command::socket_path(runtime_dir.as_deref(), user_id)
}

/// Sends one command line to the running daemon.
pub fn send_command(line: &str) -> Result<(), String> {
    let mut stream = UnixStream::connect(socket_file()).map_err(|_| "oh-snap is not running".to_string())?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).map_err(|error| error.to_string())?;
    stream.write_all(format!("{line}\n").as_bytes()).map_err(|error| error.to_string())?;
    let mut reply = String::new();
    let _ = stream.read_to_string(&mut reply);
    if reply.starts_with("ok") { Ok(()) } else { Err(format!("oh-snap did not understand `{line}`")) }
}

pub fn is_running() -> bool {
    UnixStream::connect(socket_file()).is_ok()
}

fn trigger_for(command: Command) -> Option<Trigger> {
    Some(match command {
        Command::Snip(word) => Trigger::Snip(Mode::parse(&word)?),
        Command::LastRegion => Trigger::LastRegion,
        Command::Record => Trigger::Record,
        Command::Quit => Trigger::Quit,
        Command::OpenSettings => Trigger::OpenSettings,
        Command::OpenFolder => Trigger::OpenFolder,
        Command::Recent(path) => Trigger::Recent(path),
        Command::CheckUpdate => Trigger::CheckUpdate,
    })
}

fn bind_socket() -> Result<UnixListener, String> {
    let path = socket_file();
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            if is_running() {
                return Err("oh-snap is already running".into());
            }
            // A crashed run left its socket file behind.
            let _ = std::fs::remove_file(&path);
            UnixListener::bind(&path).map_err(|error| format!("cannot open {}: {error}", path.display()))?
        }
        Err(error) => return Err(format!("cannot open {}: {error}", path.display())),
    };
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    Ok(listener)
}

fn serve_socket(listener: UnixListener, send: Sender) {
    for stream in listener.incoming().flatten() {
        let mut line = String::new();
        let mut reader = BufReader::new(stream.try_clone().expect("a socket can be cloned")).take(4096);
        if reader.read_line(&mut line).is_err() {
            continue;
        }
        let trigger = command::parse(&line).and_then(trigger_for);
        let mut stream = stream;
        match trigger {
            Some(trigger) => {
                let _ = stream.write_all(b"ok\n");
                send(trigger);
            }
            None => {
                let _ = stream.write_all(b"error\n");
            }
        }
    }
}

/// Starts the socket, the tray and the hotkeys, and returns. Only a socket that cannot be opened
/// (usually: another oh-snap runs) is an error; a missing tray or hotkey is logged.
pub fn start(session: Session, send: Box<dyn Fn(Trigger) + Send + Sync>) -> Result<(), String> {
    let listener = bind_socket()?;
    let inner: Sender = Arc::from(send);
    let send: Sender = Arc::new(move |trigger| {
        if trigger == Trigger::Quit {
            let _ = std::fs::remove_file(socket_file());
        }
        inner(trigger);
    });

    let socket_send = send.clone();
    std::thread::spawn(move || serve_socket(listener, socket_send));

    let tray_send = send.clone();
    std::thread::spawn(move || run_tray(tray_send));

    let hotkey_send = send;
    std::thread::spawn(move || {
        let outcome = match session {
            Session::X11 => xorg::run_hotkeys(hotkey_send),
            Session::Wayland => wayland::run_shortcuts(hotkey_send),
        };
        if let Err(error) = outcome {
            crate::log::write(&format!("hotkeys unavailable: {error}"));
        }
    });
    Ok(())
}

struct SnapTray {
    send: Sender,
    icon: Vec<ksni::Icon>,
}

fn item(label: &str, trigger: Trigger) -> MenuItem<SnapTray> {
    StandardItem {
        label: label.into(),
        activate: Box::new(move |tray: &mut SnapTray| (tray.send)(trigger.clone())),
        ..Default::default()
    }
    .into()
}

impl ksni::Tray for SnapTray {
    fn id(&self) -> String {
        "oh-snap".into()
    }

    fn title(&self) -> String {
        "oh-snap".into()
    }

    fn icon_name(&self) -> String {
        "oh-snap".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon.clone()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        (self.send)(Trigger::Snip(Mode::Region));
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let recent: Vec<PathBuf> = crate::history::list().into_iter().take(crate::history::LISTED).collect();
        let recent_items: Vec<MenuItem<Self>> =
            recent.iter().map(|path| item(&crate::history::label(path), Trigger::Recent(path.clone()))).collect();
        vec![
            item("snip region", Trigger::Snip(Mode::Region)),
            item("snip window", Trigger::Snip(Mode::Window)),
            item("snip screen", Trigger::Snip(Mode::Screen)),
            item("snip freeform", Trigger::Snip(Mode::Freeform)),
            item("snip last region (Shift+Print)", Trigger::LastRegion),
            item("record video", Trigger::Record),
            MenuItem::Separator,
            SubMenu { label: "recent snips".into(), enabled: !recent.is_empty(), submenu: recent_items, ..Default::default() }.into(),
            item("open screenshots", Trigger::OpenFolder),
            item("settings…", Trigger::OpenSettings),
            item("check for updates", Trigger::CheckUpdate),
            MenuItem::Separator,
            item("quit", Trigger::Quit),
        ]
    }
}

/// The logo shrunk 4x4 into 128 pixels, as the ARGB bytes a tray wants, for desktops without the icon theme entry.
fn tray_icon() -> Vec<ksni::Icon> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(super::ICON_PNG));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let Ok(mut reader) = decoder.read_info() else {
        return Vec::new();
    };
    let Some(size) = reader.output_buffer_size() else {
        return Vec::new();
    };
    let mut buffer = vec![0; size];
    let Ok(frame) = reader.next_frame(&mut buffer) else {
        return Vec::new();
    };
    if frame.color_type != png::ColorType::Rgba || frame.width % 4 != 0 || frame.height % 4 != 0 {
        return Vec::new();
    }
    let (width, height) = (frame.width as usize, frame.height as usize);
    let mut data = Vec::with_capacity(width / 4 * (height / 4) * 4);
    for block_y in 0..height / 4 {
        for block_x in 0..width / 4 {
            let mut sums = [0u32; 4];
            for y in 0..4 {
                for x in 0..4 {
                    let offset = ((block_y * 4 + y) * width + block_x * 4 + x) * 4;
                    for channel in 0..4 {
                        sums[channel] += buffer[offset + channel] as u32;
                    }
                }
            }
            let [red, green, blue, alpha] = sums.map(|sum| (sum / 16) as u8);
            data.extend_from_slice(&[alpha, red, green, blue]);
        }
    }
    vec![ksni::Icon { width: (width / 4) as i32, height: (height / 4) as i32, data }]
}

fn run_tray(send: Sender) {
    let tray = SnapTray { send, icon: tray_icon() };
    match tray.spawn() {
        // The handle keeps the tray alive; this thread has nothing else to do.
        Ok(_handle) => loop {
            std::thread::park();
        },
        Err(error) => crate::log::write(&format!("no tray icon: {error}")),
    }
}
