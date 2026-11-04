//! X11: monitors, windows, the Print key grabs and the clipboard, over x11rb (pure Rust).

use std::sync::Arc;
use std::sync::mpsc;

use x11rb::connection::{Connection, RequestConnection};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::protocol::Event;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, GrabMode, ImageFormat, ImageOrder, MapState, ModMask,
    PropMode, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::{COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, CURRENT_TIME, NONE};

use super::logic::x11 as numbers;
use super::logic::x11::Payload;
use super::display_error;
use crate::desktop::platform::{Screen, Trigger, WindowInfo};
use crate::mode::Mode;
use crate::output::Image;

fn connect() -> Result<(RustConnection, usize), String> {
    RustConnection::connect(None).map_err(|error| format!("cannot reach the X server: {error}"))
}

fn atom(connection: &RustConnection, name: &str) -> Result<Atom, String> {
    let reply = connection.intern_atom(false, name.as_bytes()).map_err(display_error)?;
    Ok(reply.reply().map_err(display_error)?.atom)
}

/// Monitor rectangles in desktop pixels, left to right; the whole root when RandR has none.
fn monitor_areas(connection: &RustConnection, root: Window, root_size: (u16, u16)) -> Vec<(i32, i32, u32, u32)> {
    let monitors = connection
        .randr_get_monitors(root, true)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|reply| reply.monitors)
        .unwrap_or_default();
    let mut areas: Vec<(i32, i32, u32, u32)> = monitors
        .iter()
        .filter(|monitor| monitor.width > 0 && monitor.height > 0)
        .map(|monitor| (monitor.x as i32, monitor.y as i32, monitor.width as u32, monitor.height as u32))
        .collect();
    areas.sort_by_key(|area| (area.0, area.1));
    if areas.is_empty() {
        areas.push((0, 0, root_size.0 as u32, root_size.1 as u32));
    }
    areas
}

fn desktop_scale(connection: &RustConnection, root: Window) -> f32 {
    let resources = connection
        .get_property(false, root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, 0, 65536)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|reply| String::from_utf8_lossy(&reply.value).into_owned())
        .unwrap_or_default();
    numbers::scale_from_resources(&resources)
}

/// One GetImage per monitor, all sent before the first reply is read.
pub fn capture_screens() -> Result<Vec<Screen>, String> {
    let (connection, screen_number) = connect()?;
    let setup = connection.setup();
    let screen = &setup.roots[screen_number];
    let has_32_bit_pixels = setup
        .pixmap_formats
        .iter()
        .any(|format| format.depth == screen.root_depth && format.bits_per_pixel == 32);
    if setup.image_byte_order != ImageOrder::LSB_FIRST || !has_32_bit_pixels {
        return Err("this X server does not use 32-bit little-endian pixels".into());
    }
    let root = screen.root;
    let areas = monitor_areas(&connection, root, (screen.width_in_pixels, screen.height_in_pixels));
    let scale = desktop_scale(&connection, root);

    let mut requests = Vec::new();
    for &(x, y, width, height) in &areas {
        let cookie = connection
            .get_image(ImageFormat::Z_PIXMAP, root, x as i16, y as i16, width as u16, height as u16, !0)
            .map_err(display_error)?;
        requests.push(cookie);
    }
    let mut screens = Vec::new();
    for (cookie, &(x, y, width, height)) in requests.into_iter().zip(&areas) {
        let mut bgra = cookie.reply().map_err(display_error)?.data;
        numbers::make_opaque(&mut bgra);
        screens.push(Screen { x, y, scale, image: Image { width, height, bgra } });
    }
    Ok(screens)
}

struct WindowAtoms {
    stacking: Atom,
    frame_extents: Atom,
    state: Atom,
    hidden: Atom,
}

fn window_atoms(connection: &RustConnection) -> Result<WindowAtoms, String> {
    Ok(WindowAtoms {
        stacking: atom(connection, "_NET_CLIENT_LIST_STACKING")?,
        frame_extents: atom(connection, "_NET_FRAME_EXTENTS")?,
        state: atom(connection, "_NET_WM_STATE")?,
        hidden: atom(connection, "_NET_WM_STATE_HIDDEN")?,
    })
}

fn read_u32s(connection: &RustConnection, window: Window, property: Atom, kind: impl Into<Atom>, length: u32) -> Vec<u32> {
    connection
        .get_property(false, window, property, kind, 0, length)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .and_then(|reply| reply.value32().map(|values| values.collect()))
        .unwrap_or_default()
}

fn describe_window(connection: &RustConnection, root: Window, atoms: &WindowAtoms, window: Window) -> Option<WindowInfo> {
    let attributes = connection.get_window_attributes(window).ok()?.reply().ok()?;
    if attributes.map_state != MapState::VIEWABLE {
        return None;
    }
    if read_u32s(connection, window, atoms.state, AtomEnum::ATOM, 64).contains(&atoms.hidden) {
        return None;
    }
    let geometry = connection.get_geometry(window).ok()?.reply().ok()?;
    let origin = connection.translate_coordinates(window, root, 0, 0).ok()?.reply().ok()?;
    let extents: Vec<u32> = read_u32s(connection, window, atoms.frame_extents, AtomEnum::CARDINAL, 4);
    let extents = <[u32; 4]>::try_from(extents).unwrap_or([0; 4]);
    let (x, y, width, height) =
        numbers::with_frame_extents(origin.dst_x as i32, origin.dst_y as i32, geometry.width as u32, geometry.height as u32, extents);
    if width == 0 || height == 0 {
        return None;
    }
    Some(WindowInfo { x, y, width, height })
}

/// Visible top-level windows from the window manager's stacking list, front to back.
pub fn windows_on_screen() -> Vec<WindowInfo> {
    let Ok((connection, screen_number)) = connect() else {
        return Vec::new();
    };
    let Ok(atoms) = window_atoms(&connection) else {
        return Vec::new();
    };
    let root = connection.setup().roots[screen_number].root;
    let stacking = read_u32s(&connection, root, atoms.stacking, AtomEnum::WINDOW, 4096);
    stacking
        .into_iter()
        .rev()
        .filter_map(|window| describe_window(&connection, root, &atoms, window))
        .collect()
}

/// Print, Shift+Print and Ctrl+Print on the root window, with every Caps Lock and Num Lock combination.
pub fn run_hotkeys(send: Arc<dyn Fn(Trigger) + Send + Sync>) -> Result<(), String> {
    let (connection, screen_number) = connect()?;
    let root = connection.setup().roots[screen_number].root;
    let setup = connection.setup();
    let count = setup.max_keycode - setup.min_keycode + 1;
    let mapping = connection
        .get_keyboard_mapping(setup.min_keycode, count)
        .map_err(display_error)?
        .reply()
        .map_err(display_error)?;
    let keycode = numbers::keycode_for_keysym(setup.min_keycode, mapping.keysyms_per_keycode, &mapping.keysyms, numbers::KEYSYM_PRINT)
        .ok_or("this keyboard has no Print key")?;

    let hotkeys = [
        (0, Trigger::Snip(Mode::Region)),
        (numbers::SHIFT, Trigger::LastRegion),
        (numbers::CONTROL, Trigger::Snip(Mode::Screen)),
    ];
    for (modifiers, _) in &hotkeys {
        for variant in numbers::with_lock_variants(*modifiers) {
            let grab = connection
                .grab_key(false, root, ModMask::from(variant), keycode, GrabMode::ASYNC, GrabMode::ASYNC)
                .map_err(display_error)?;
            grab.check().map_err(|error| format!("another program holds Print: {error}"))?;
        }
    }
    loop {
        if let Event::KeyPress(event) = connection.wait_for_event().map_err(display_error)? {
            if event.detail != keycode {
                continue;
            }
            let held = numbers::without_locks(u16::from(event.state));
            if let Some((_, trigger)) = hotkeys.iter().find(|(modifiers, _)| *modifiers == held) {
                send(trigger.clone());
            }
        }
    }
}

/// Owns the CLIPBOARD selection and keeps serving it from a thread until another program takes it.
pub fn copy_to_clipboard(payload: Payload) -> Result<(), String> {
    let (ready_sender, ready_receiver) = mpsc::channel();
    std::thread::spawn(move || match claim_clipboard(&payload) {
        Ok(owner) => {
            let _ = ready_sender.send(Ok(()));
            serve_clipboard(owner, &payload);
        }
        Err(error) => {
            let _ = ready_sender.send(Err(error));
        }
    });
    ready_receiver.recv().map_err(|_| "the clipboard thread stopped".to_string())?
}

struct ClipboardOwner {
    connection: RustConnection,
    clipboard: Atom,
    targets: Atom,
    offered: Vec<(Atom, &'static str)>,
}

fn claim_clipboard(payload: &Payload) -> Result<ClipboardOwner, String> {
    let (connection, screen_number) = connect()?;
    let root = connection.setup().roots[screen_number].root;
    let window = connection.generate_id().map_err(display_error)?;
    connection
        .create_window(COPY_DEPTH_FROM_PARENT, window, root, 0, 0, 1, 1, 0, WindowClass::INPUT_ONLY, COPY_FROM_PARENT, &CreateWindowAux::new())
        .map_err(display_error)?;
    let clipboard = atom(&connection, "CLIPBOARD")?;
    let targets = atom(&connection, "TARGETS")?;
    let mut offered = Vec::new();
    for name in numbers::offered_targets(payload) {
        offered.push((atom(&connection, name)?, *name));
    }
    connection.set_selection_owner(window, clipboard, CURRENT_TIME).map_err(display_error)?;
    connection.flush().map_err(display_error)?;
    let owner = connection.get_selection_owner(clipboard).map_err(display_error)?.reply().map_err(display_error)?.owner;
    if owner != window {
        return Err("could not take the clipboard".into());
    }
    Ok(ClipboardOwner { connection, clipboard, targets, offered })
}

fn serve_clipboard(owner: ClipboardOwner, payload: &Payload) {
    // Larger data would need the INCR protocol; PNGs of a few screens fit one request.
    let largest = owner.connection.maximum_request_bytes().saturating_sub(64);
    loop {
        match owner.connection.wait_for_event() {
            Ok(Event::SelectionRequest(request)) if request.selection == owner.clipboard => {
                let property = if request.property == NONE { request.target } else { request.property };
                let answered = answer_request(&owner, payload, &request, property, largest);
                let notify = SelectionNotifyEvent {
                    response_type: SELECTION_NOTIFY_EVENT,
                    sequence: 0,
                    time: request.time,
                    requestor: request.requestor,
                    selection: request.selection,
                    target: request.target,
                    property: if answered { property } else { NONE },
                };
                let _ = owner.connection.send_event(false, request.requestor, EventMask::NO_EVENT, notify);
                let _ = owner.connection.flush();
            }
            Ok(Event::SelectionClear(_)) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

fn answer_request(
    owner: &ClipboardOwner,
    payload: &Payload,
    request: &x11rb::protocol::xproto::SelectionRequestEvent,
    property: Atom,
    largest: usize,
) -> bool {
    let connection = &owner.connection;
    if request.target == owner.targets {
        let mut list = vec![owner.targets];
        list.extend(owner.offered.iter().map(|(atom, _)| *atom));
        return connection
            .change_property32(PropMode::REPLACE, request.requestor, property, AtomEnum::ATOM, &list)
            .is_ok();
    }
    let Some((_, name)) = owner.offered.iter().find(|(atom, _)| *atom == request.target) else {
        return false;
    };
    match numbers::bytes_for(payload, name) {
        Some(bytes) if bytes.len() <= largest => connection
            .change_property8(PropMode::REPLACE, request.requestor, property, request.target, bytes)
            .is_ok(),
        _ => false,
    }
}

