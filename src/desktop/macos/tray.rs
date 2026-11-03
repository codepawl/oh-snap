//! Global hotkeys and the menu-bar icon. Everything is registered once and kept alive;
//! `listen` returns right away and events arrive through callbacks on the main thread.

use std::path::PathBuf;
use std::sync::OnceLock;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

use super::pure;
use crate::desktop::platform::Trigger;
use crate::mode::Mode;

type Sender = Box<dyn Fn(Trigger) + Send + Sync>;

static SENDER: OnceLock<Sender> = OnceLock::new();
/// Hotkey id and what it asks for, filled once at registration.
static HOTKEY_TRIGGERS: OnceLock<Vec<(u32, Trigger)>> = OnceLock::new();

/// Every snip action: its menu id, label, hotkey key (with Cmd+Shift) and trigger.
/// This table is the only place the shortcuts are written down.
struct Action {
    id: &'static str,
    label: &'static str,
    key: Code,
    trigger: fn() -> Trigger,
}

const SHORTCUT_MODIFIERS: Modifiers = Modifiers::META.union(Modifiers::SHIFT);

const ACTIONS: [Action; 6] = [
    Action { id: "region", label: "snip region", key: Code::Digit4, trigger: || Trigger::Snip(Mode::Region) },
    Action { id: "window", label: "snip window", key: Code::Digit2, trigger: || Trigger::Snip(Mode::Window) },
    Action { id: "screen", label: "snip screen", key: Code::Digit3, trigger: || Trigger::Snip(Mode::Screen) },
    Action { id: "freeform", label: "snip freeform", key: Code::Digit7, trigger: || Trigger::Snip(Mode::Freeform) },
    Action { id: "last", label: "snip last region", key: Code::Digit6, trigger: || Trigger::LastRegion },
    Action { id: "record", label: "record video", key: Code::Digit5, trigger: || Trigger::Record },
];

const RECENT_PREFIX: &str = "recent:";
const MAX_RECENT: usize = 10;

/// What the fixed menu items (the ones without a hotkey) ask for.
fn fixed_trigger(id: &str) -> Option<Trigger> {
    match id {
        "folder" => Some(Trigger::OpenFolder),
        "settings" => Some(Trigger::OpenSettings),
        "update" => Some(Trigger::CheckUpdate),
        "quit" => Some(Trigger::Quit),
        _ => None,
    }
}

fn trigger_for_menu_id(id: &str) -> Option<Trigger> {
    if let Some(action) = ACTIONS.iter().find(|action| action.id == id) {
        return Some((action.trigger)());
    }
    if let Some(index) = id.strip_prefix(RECENT_PREFIX).and_then(|index| index.parse::<usize>().ok()) {
        let path: PathBuf = crate::history::list().into_iter().nth(index)?;
        return Some(Trigger::Recent(path));
    }
    fixed_trigger(id)
}

fn send(trigger: Trigger) {
    if let Some(sender) = SENDER.get() {
        sender(trigger);
    }
}

fn register_hotkeys() -> Result<GlobalHotKeyManager, String> {
    let manager = GlobalHotKeyManager::new().map_err(|error| format!("hotkeys: {error}"))?;
    let mut registered = Vec::new();
    for action in &ACTIONS {
        let hotkey = HotKey::new(Some(SHORTCUT_MODIFIERS), action.key);
        // A shortcut another program owns is skipped: the menu still has the action.
        match manager.register(hotkey) {
            Ok(()) => registered.push((hotkey.id(), (action.trigger)())),
            Err(error) => crate::log::write(&format!("hotkey {} not taken: {error}", action.id)),
        }
    }
    let _ = HOTKEY_TRIGGERS.set(registered);
    GlobalHotKeyEvent::set_event_handler(Some(|event: GlobalHotKeyEvent| {
        if event.state != HotKeyState::Pressed {
            return;
        }
        let triggers = HOTKEY_TRIGGERS.get().map(Vec::as_slice).unwrap_or_default();
        if let Some((_, trigger)) = triggers.iter().find(|(id, _)| *id == event.id) {
            send(trigger.clone());
        }
    }));
    Ok(manager)
}

fn action_item(action: &Action) -> MenuItem {
    let accelerator = tray_icon::menu::accelerator::Accelerator::new(SHORTCUT_MODIFIERS, action.key);
    MenuItem::with_id(action.id, action.label, true, Some(accelerator))
}

/// Puts the latest snips in the "recent snips" submenu.
fn fill_recent(submenu: &Submenu) {
    while submenu.remove_at(0).is_some() {}
    let recent = crate::history::list();
    for (index, path) in recent.iter().take(MAX_RECENT).enumerate() {
        let id = format!("{RECENT_PREFIX}{index}");
        let _ = submenu.append(&MenuItem::with_id(id, crate::history::label(path), true, None));
    }
    submenu.set_enabled(!recent.is_empty());
}

fn build_menu() -> Result<(Menu, Submenu), String> {
    let menu = Menu::new();
    let recent = Submenu::new("recent snips", false);
    fill_recent(&recent);
    let separator = || PredefinedMenuItem::separator();
    let mut result = Ok(());
    for action in &ACTIONS[..4] {
        result = result.and(menu.append(&action_item(action)));
    }
    result = result.and(menu.append(&separator()));
    result = result.and(menu.append(&action_item(&ACTIONS[4])));
    result = result.and(menu.append(&action_item(&ACTIONS[5])));
    result = result.and(menu.append(&separator()));
    result = result.and(menu.append(&recent));
    result = result.and(menu.append(&MenuItem::with_id("folder", "open screenshots", true, None)));
    result = result.and(menu.append(&MenuItem::with_id("settings", "settings…", true, None)));
    result = result.and(menu.append(&MenuItem::with_id("update", "check for updates", true, None)));
    result = result.and(menu.append(&separator()));
    result = result.and(menu.append(&MenuItem::with_id("quit", "quit", true, None)));
    result.map_err(|error| format!("menu: {error}"))?;
    Ok((menu, recent))
}

fn menu_bar_icon() -> Result<Icon, String> {
    const SIZE: u32 = 44;
    Icon::from_rgba(pure::tray_icon_rgba(SIZE), SIZE, SIZE).map_err(|error| format!("tray icon: {error}"))
}

thread_local! {
    /// The recent-snips submenu, refreshed from the main thread where menu events arrive.
    static RECENT_MENU: std::cell::RefCell<Option<Submenu>> = const { std::cell::RefCell::new(None) };
}

fn refresh_recent() {
    RECENT_MENU.with(|slot| {
        if let Some(submenu) = slot.borrow().as_ref() {
            fill_recent(submenu);
        }
    });
}

/// Handles that must outlive `listen`: dropping either removes it from the system.
struct Registered {
    _hotkeys: GlobalHotKeyManager,
    _tray: TrayIcon,
}

/// Registers the hotkeys and the menu-bar icon and returns. Call it on the main thread,
/// inside the running event loop.
pub fn listen(sender: Sender) -> Result<(), String> {
    SENDER.set(sender).map_err(|_| "already listening".to_string())?;
    let hotkeys = register_hotkeys()?;
    let (menu, recent) = build_menu()?;
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon_templated(menu_bar_icon()?)
        .with_tooltip("oh-snap")
        .build()
        .map_err(|error| format!("menu bar icon: {error}"))?;
    RECENT_MENU.with(|slot| *slot.borrow_mut() = Some(recent));

    MenuEvent::set_event_handler(Some(|event: MenuEvent| {
        if let Some(trigger) = trigger_for_menu_id(&event.id.0) {
            send(trigger);
        }
        refresh_recent();
    }));
    // The menu opens on a click, so the list is fresh when it shows.
    TrayIconEvent::set_event_handler(Some(|_: TrayIconEvent| refresh_recent()));

    // Neither handle may be dropped while the program runs.
    std::mem::forget(Registered { _hotkeys: hotkeys, _tray: tray });
    Ok(())
}
