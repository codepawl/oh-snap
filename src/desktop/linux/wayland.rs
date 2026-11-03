//! Wayland: the Screenshot portal, the data-control clipboard and the GlobalShortcuts portal.
//! A Wayland client cannot read other windows' pixels or grab keys itself, so each of these
//! asks the compositor, through D-Bus.

use std::sync::Arc;
use std::time::Duration;

use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use ashpd::desktop::screenshot::Screenshot;
use futures_lite::StreamExt;
use futures_lite::future::block_on;
use wl_clipboard_rs::copy::{MimeType, Options, Source};

use super::display_error;
use super::logic::uri;
use crate::desktop::platform::{Screen, Trigger};
use crate::mode::Mode;
use crate::output::Image;

/// The portal gives one picture of the whole desktop, so it is one `Screen` at the origin.
pub fn capture_screens() -> Result<Vec<Screen>, String> {
    let response = block_on(async {
        let request = Screenshot::request().interactive(false).modal(false).send().await?;
        request.response()
    })
    .map_err(|error| format!("the screenshot portal refused: {error}"))?;
    let path = uri::file_uri_to_path(response.uri().as_str()).ok_or("the screenshot portal returned a non-file address")?;
    let loaded = crate::image_file::load_png(&path).map_err(display_error);
    // The portal wrote this file for us; do not leave it in the user's Pictures folder.
    let _ = std::fs::remove_file(&path);
    let image: Image = loaded?;
    Ok(vec![Screen { x: 0, y: 0, scale: 1.0, image }])
}

/// Offers `bytes` as `mime` on the Wayland clipboard from a thread that serves until it is replaced.
/// Needs the wlr-data-control protocol (wlroots, KDE); GNOME's Mutter does not offer it.
pub fn copy(bytes: Vec<u8>, mime: MimeType) -> Result<(), String> {
    let (result_sender, result_receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut options = Options::new();
        options.foreground(true);
        let outcome = options.copy(Source::Bytes(bytes.into_boxed_slice()), mime);
        // Only reached once something else took the clipboard, or when setting it up failed.
        let _ = result_sender.send(outcome.map_err(display_error));
    });
    match result_receiver.recv_timeout(Duration::from_millis(400)) {
        Ok(result) => result,
        Err(_) => Ok(()),
    }
}

pub fn copy_png(png: &[u8]) -> Result<(), String> {
    copy(png.to_vec(), MimeType::Specific("image/png".into()))
}

pub fn copy_text(text: &str) -> Result<(), String> {
    copy(text.as_bytes().to_vec(), MimeType::Text)
}

/// Binds Print, Shift+Print and Ctrl+Print through the portal and reports each press.
pub fn run_shortcuts(send: Arc<dyn Fn(Trigger) + Send + Sync>) -> Result<(), String> {
    block_on(async {
        let proxy = GlobalShortcuts::new().await?;
        let session = proxy.create_session(Default::default()).await?;
        let shortcuts = [
            NewShortcut::new("region", "Snip a region").preferred_trigger("Print"),
            NewShortcut::new("last", "Snip the last region").preferred_trigger("SHIFT+Print"),
            NewShortcut::new("screen", "Snip the screen").preferred_trigger("CTRL+Print"),
        ];
        proxy.bind_shortcuts(&session, &shortcuts, None, Default::default()).await?.response()?;
        let mut activations = proxy.receive_activated().await?;
        while let Some(activation) = activations.next().await {
            match activation.shortcut_id() {
                "region" => send(Trigger::Snip(Mode::Region)),
                "last" => send(Trigger::LastRegion),
                "screen" => send(Trigger::Snip(Mode::Screen)),
                _ => {}
            }
        }
        Ok::<(), ashpd::Error>(())
    })
    .map_err(|error| format!("global shortcuts portal: {error}"))
}
