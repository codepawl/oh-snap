//! What runs while the tray process lives: the tray icon and the hotkeys. The command socket
//! is `desktop::socket`, shared with macOS. `start` returns at once; each piece has its own thread.

use std::path::PathBuf;
use std::sync::Arc;

use ksni::blocking::TrayMethods;
use ksni::menu::{MenuItem, StandardItem, SubMenu};

use super::logic::session::Session;
use super::{wayland, xorg};
use crate::desktop::platform::Trigger;
use crate::mode::Mode;

type Sender = Arc<dyn Fn(Trigger) + Send + Sync>;

/// Starts the tray and the hotkeys, and returns. A missing tray or hotkey is logged, not an error.
pub fn start(session: Session, send: Box<dyn Fn(Trigger) + Send + Sync>) -> Result<(), String> {
    let send: Sender = Arc::from(send);

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
