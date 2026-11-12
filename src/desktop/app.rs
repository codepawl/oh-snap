//! The winit shell: the event loop, the windows, and what a finished snip leads to. All
//! drawing is in `ui`; all OS work goes through `Platform`. Not built on Windows.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use softbuffer::{Context, Surface};
use tiny_skia::Pixmap;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy, OwnedDisplayHandle};
use winit::keyboard::{Key as LogicalKey, NamedKey};
use winit::window::{CursorIcon, Window, WindowId, WindowLevel};

use super::finish::{self, Overrides};
use super::pending;
use super::platform::{Platform, RecordOptions, Recording, Screen, Trigger};
use super::socket;
use super::ui::card::{self, CardAction, CardLayout, CardModel};
use super::ui::canvas;
use super::ui::markup::{Ending, MarkupState};
use super::ui::overlay::{Key, Outcome, OverlayOptions, OverlayState, Pick};
use super::ui::recording_bar::{self, BarAction, BarLayout, BarModel};
use super::ui::text::Fonts;
use super::ui::trim::{TrimOutcome, TrimState};
use super::update::{self, UpdateResult};
use crate::mode::{Mode, Tool};
use crate::output::{self, Image};
use crate::settings::{self, Settings, UpdateMode};

const TICK: Duration = Duration::from_millis(33);
const UPDATE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const SCREEN_MARGIN: f64 = 16.0;

enum UserEvent {
    Trigger(Trigger),
    FileReady { label: &'static str, result: Result<PathBuf, String> },
    Updated(Result<UpdateResult, String>),
}

enum Kind {
    Overlay(usize),
    Card,
    Bar,
    Trim,
}

struct Managed {
    window: Arc<Window>,
    surface: Surface<OwnedDisplayHandle, Arc<Window>>,
    kind: Kind,
    cursor: (f32, f32),
}

/// What a card shows and what its buttons act on.
#[derive(Default)]
struct CardContent {
    headline: String,
    detail: String,
    thumbnail: Option<Pixmap>,
    swatch: Option<[u8; 3]>,
    actions: Vec<CardAction>,
    failed: bool,
    path: Option<PathBuf>,
    image: Option<Image>,
    /// A recording's length, which the trim window needs.
    recording_length: Option<Duration>,
}

struct CardState {
    window: WindowId,
    model: CardModel,
    layout: CardLayout,
    path: Option<PathBuf>,
    image: Option<Image>,
    recording_length: Option<Duration>,
    hovered: Option<CardAction>,
    shown_for: Duration,
    lifetime: Duration,
    last_tick: Instant,
}

struct ActiveRecording {
    handle: Box<dyn Recording>,
    bar: WindowId,
    layout: BarLayout,
    started: Instant,
    paused_since: Option<Instant>,
    paused_total: Duration,
    hovered: Option<BarAction>,
}

impl ActiveRecording {
    fn elapsed(&self) -> Duration {
        let paused_now = self.paused_since.map_or(Duration::ZERO, |since| since.elapsed());
        self.started.elapsed().saturating_sub(self.paused_total + paused_now)
    }
}

/// The trim window and the recording it cuts.
struct TrimSession {
    window: WindowId,
    state: TrimState,
    path: PathBuf,
}

struct App {
    platform: Arc<dyn Platform>,
    proxy: EventLoopProxy<UserEvent>,
    settings: Settings,
    overrides: Overrides,
    one_shot: bool,
    fonts: Fonts,
    context: Option<Context<OwnedDisplayHandle>>,
    windows: HashMap<WindowId, Managed>,
    session: Option<OverlayState>,
    /// Drawing on the picked area, in the overlay window of its screen.
    markup: Option<MarkupState>,
    trim: Option<TrimSession>,
    card: Option<CardState>,
    recording: Option<ActiveRecording>,
    delayed: Option<(Instant, Trigger)>,
    first: Option<Trigger>,
    started: bool,
    shift: bool,
    /// Ctrl on Linux, Cmd on macOS.
    command: bool,
    next_update_check: Option<Instant>,
    update_running: bool,
}

/// Runs the app until `Trigger::Quit`. `first` is a trigger to act on at once; with
/// `one_shot` the app quits when that is done (the command line without a daemon).
pub fn run(first: Option<(Trigger, bool)>, overrides: Overrides) -> Result<(), String> {
    let one_shot = first.as_ref().is_some_and(|(_, one_shot)| *one_shot);
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }
    let event_loop = builder.build().map_err(|error| format!("no window system: {error}"))?;
    let proxy = event_loop.create_proxy();
    if !one_shot {
        if crate::crash::started_after_crash() {
            wait_for_crashed_daemon();
        }
        let socket_proxy = proxy.clone();
        socket::serve(move |trigger| {
            let _ = socket_proxy.send_event(UserEvent::Trigger(trigger));
        })?;
    }
    let settings = settings::load();
    let next_update_check = (!one_shot && settings.updates != UpdateMode::Off).then(Instant::now);
    let mut app = App {
        platform: Arc::from(super::current_platform()),
        proxy,
        settings,
        overrides,
        one_shot,
        fonts: Fonts::load(),
        context: None,
        windows: HashMap::new(),
        session: None,
        markup: None,
        trim: None,
        card: None,
        recording: None,
        delayed: None,
        first: first.map(|(trigger, _)| trigger),
        started: false,
        shift: false,
        command: false,
        next_update_check,
        update_running: false,
    };
    let result = event_loop.run_app(&mut app).map_err(|error| error.to_string());
    socket::remove();
    result
}

/// A daemon that a crash started can come up while the crashed one still holds the socket.
fn wait_for_crashed_daemon() {
    const PATIENCE: Duration = Duration::from_secs(10);
    let started = Instant::now();
    while socket::daemon_is_running() && started.elapsed() < PATIENCE {
        std::thread::sleep(Duration::from_millis(100));
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.started {
            return;
        }
        self.started = true;
        self.context = Context::new(event_loop.owned_display_handle()).ok();
        if !self.one_shot {
            // Registers hotkeys and the tray; returns at once.
            let proxy = self.proxy.clone();
            let send = Box::new(move |trigger| {
                let _ = proxy.send_event(UserEvent::Trigger(trigger));
            });
            if let Err(error) = self.platform.listen(send) {
                crate::log::write(&format!("listen: {error}"));
                eprintln!("oh-snap: {error}");
            }
            if let Some(crash) = crate::crash::take_pending() {
                let content = CardContent {
                    headline: "oh-snap crashed".into(),
                    detail: crash.chars().take(40).collect(),
                    actions: vec![CardAction::Report],
                    failed: true,
                    ..CardContent::default()
                };
                self.show_card(event_loop, content);
            }
        }
        if let Some(trigger) = self.first.take() {
            self.handle_trigger(event_loop, trigger);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Trigger(trigger) => self.handle_trigger(event_loop, trigger),
            UserEvent::FileReady { label, result } => match result {
                Ok(path) => {
                    let content = CardContent { headline: label.into(), detail: finish::file_name(&path), actions: vec![CardAction::Open, CardAction::Folder], path: Some(path), ..CardContent::default() };
                    self.show_card(event_loop, content);
                }
                Err(error) => self.show_failure(event_loop, label, &error),
            },
            UserEvent::Updated(result) => {
                self.update_running = false;
                match result {
                    Ok(UpdateResult::UpToDate) => {}
                    Ok(UpdateResult::Available(version)) => self.show_note(event_loop, &format!("oh-snap {version} is out"), "download it from GitHub releases"),
                    Ok(UpdateResult::Installed(version)) => self.show_note(event_loop, &format!("oh-snap {version} installed"), "it runs after a restart"),
                    Err(error) => crate::log::write(&format!("update: {error}")),
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(kind) = self.windows.get(&id).map(|managed| match managed.kind {
            Kind::Overlay(index) => Some(index),
            _ => None,
        }) else {
            return;
        };
        if let WindowEvent::ModifiersChanged(modifiers) = &event {
            self.shift = modifiers.state().shift_key();
            self.command = modifiers.state().control_key() || modifiers.state().super_key();
            return;
        }
        if matches!(event, WindowEvent::RedrawRequested) {
            return self.paint(id);
        }
        if self.trim.as_ref().is_some_and(|trim| trim.window == id) {
            return self.trim_event(event_loop, event);
        }
        if self.markup.is_some() && kind.is_some() {
            return self.markup_event(event_loop, event);
        }
        match event {
            WindowEvent::CursorMoved { position, .. } => self.cursor_moved(id, kind, position),
            WindowEvent::CursorLeft { .. } => self.cursor_left(id),
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => self.pressed(event_loop, id, kind),
            WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. } => self.released(event_loop, kind),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed && kind.is_some() => {
                if let Some(key) = overlay_key(&event.logical_key) {
                    if let Some(state) = self.session.as_mut() {
                        state.key_pressed(key, self.shift);
                    }
                    self.after_overlay_input(event_loop);
                }
            }
            WindowEvent::CloseRequested => match kind {
                Some(_) => {
                    if let Some(state) = self.session.as_mut() {
                        state.key_pressed(Key::Escape, false);
                    }
                    self.after_overlay_input(event_loop);
                }
                None => self.close_card_if(id),
            },
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if self.delayed.as_ref().is_some_and(|(due, _)| *due <= now) {
            if let Some((_, trigger)) = self.delayed.take() {
                self.run_delayed(event_loop, trigger);
            }
        }
        self.tick_card(event_loop);
        self.tick_recording();
        self.maybe_check_for_update(now);

        let mut wake: Option<Instant> = None;
        let mut want = |at: Instant| wake = Some(wake.map_or(at, |current| current.min(at)));
        if self.card.is_some() || self.recording.is_some() {
            want(now + TICK);
        }
        if let Some((due, _)) = &self.delayed {
            want(*due);
        }
        if let Some(at) = self.next_update_check {
            want(at);
        }
        event_loop.set_control_flow(wake.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }
}

fn overlay_key(key: &LogicalKey) -> Option<Key> {
    match key {
        LogicalKey::Named(NamedKey::Escape) => Some(Key::Escape),
        LogicalKey::Named(NamedKey::Enter) => Some(Key::Enter),
        LogicalKey::Named(NamedKey::ArrowLeft) => Some(Key::Left),
        LogicalKey::Named(NamedKey::ArrowRight) => Some(Key::Right),
        LogicalKey::Named(NamedKey::ArrowUp) => Some(Key::Up),
        LogicalKey::Named(NamedKey::ArrowDown) => Some(Key::Down),
        LogicalKey::Named(NamedKey::Backspace) => Some(Key::Backspace),
        LogicalKey::Named(NamedKey::Space) => Some(Key::Char(' ')),
        LogicalKey::Character(text) => text.chars().next().map(Key::Char),
        _ => None,
    }
}

impl App {
    fn busy(&self) -> bool {
        self.session.is_some() || self.markup.is_some() || self.trim.is_some() || self.recording.is_some() || self.delayed.is_some()
    }

    fn handle_trigger(&mut self, event_loop: &ActiveEventLoop, trigger: Trigger) {
        match trigger {
            Trigger::Quit => {
                if self.recording.is_some() {
                    self.stop_recording(event_loop);
                }
                event_loop.exit();
            }
            Trigger::Record if self.recording.is_some() => self.stop_recording(event_loop),
            Trigger::Report => self.open_report(),
            Trigger::CrashTest => crate::crash::test(None),
            // A capture already waiting (delay or card) ignores new ones until it runs.
            _ if self.session.is_some() || self.markup.is_some() || self.recording.is_some() || self.delayed.is_some() => {}
            Trigger::Snip(mode) => self.start_snip(event_loop, mode, Tool::Snip, false),
            Trigger::Record => self.start_snip(event_loop, Mode::Region, Tool::Record, false),
            Trigger::LastRegion => self.capture_last_region(event_loop, false),
            Trigger::OpenSettings => self.platform.open(&crate::folders::config_folder().join("settings.toml")),
            Trigger::OpenFolder => self.platform.open(&self.settings.save_folder()),
            Trigger::Recent(path) => self.copy_recent(event_loop, path),
            Trigger::CheckUpdate => self.start_update_check(true),
        }
    }

    // ------------------------------------------------------------ windows

    fn make_window(&mut self, event_loop: &ActiveEventLoop, window: Window, kind: Kind) -> Option<WindowId> {
        let window = Arc::new(window);
        let context = self.context.as_ref()?;
        let surface = Surface::new(context, window.clone()).ok()?;
        let id = window.id();
        self.windows.insert(id, Managed { window, surface, kind, cursor: (0.0, 0.0) });
        let _ = event_loop;
        Some(id)
    }

    fn open_overlay_window(&mut self, event_loop: &ActiveEventLoop, index: usize, screen: &Screen) {
        let attributes = Window::default_attributes()
            .with_title("oh-snap")
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_position(PhysicalPosition::new(screen.x, screen.y))
            .with_inner_size(PhysicalSize::new(screen.image.width, screen.image.height));
        // Wayland places windows itself; a borderless fullscreen window covers the monitor.
        #[cfg(not(target_os = "macos"))]
        let attributes = {
            let monitor = event_loop.available_monitors().find(|monitor| monitor.position().x == screen.x && monitor.position().y == screen.y);
            attributes.with_fullscreen(Some(winit::window::Fullscreen::Borderless(monitor)))
        };
        match event_loop.create_window(attributes) {
            Ok(window) => {
                window.set_cursor(CursorIcon::Crosshair);
                if let Some(id) = self.make_window(event_loop, window, Kind::Overlay(index)) {
                    if let Some(managed) = self.windows.get(&id) {
                        managed.window.focus_window();
                    }
                }
            }
            Err(error) => crate::log::write(&format!("overlay window: {error}")),
        }
    }

    fn close_window(&mut self, id: WindowId) {
        if let Some(managed) = self.windows.remove(&id) {
            managed.window.set_visible(false);
        }
    }

    fn redraw_all_overlays(&self) {
        for managed in self.windows.values().filter(|managed| matches!(managed.kind, Kind::Overlay(_))) {
            managed.window.request_redraw();
        }
    }

    fn paint(&mut self, id: WindowId) {
        let Some(managed) = self.windows.get_mut(&id) else {
            return;
        };
        let size = managed.window.inner_size();
        let (Some(width), Some(height)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        let Some(mut pixmap) = Pixmap::new(size.width, size.height) else {
            return;
        };
        match managed.kind {
            Kind::Overlay(index) => {
                if let Some(markup) = &self.markup {
                    markup.render(&self.fonts, &mut pixmap);
                } else if let Some(state) = &self.session {
                    state.render(index, &self.fonts, &mut pixmap);
                }
            }
            Kind::Card => {
                if let Some(card) = &self.card {
                    card::render(&card.model, &card.layout, card.hovered, &self.fonts, &mut pixmap);
                }
            }
            Kind::Bar => {
                if let Some(recording) = &self.recording {
                    let model = BarModel { elapsed_milliseconds: recording.elapsed().as_millis() as u64, paused: recording.paused_since.is_some() };
                    recording_bar::render(&model, &recording.layout, recording.hovered, &self.fonts, &mut pixmap);
                }
            }
            Kind::Trim => {
                if let Some(trim) = &self.trim {
                    trim.state.render(&self.fonts, &mut pixmap);
                }
            }
        }
        if managed.surface.resize(width, height).is_err() {
            return;
        }
        if let Ok(mut buffer) = managed.surface.buffer_mut() {
            buffer.copy_from_slice(&canvas::to_argb_words(&pixmap));
            let _ = buffer.present();
        }
    }

    // ------------------------------------------------------------ input

    fn cursor_moved(&mut self, id: WindowId, overlay: Option<usize>, position: PhysicalPosition<f64>) {
        let Some(managed) = self.windows.get_mut(&id) else {
            return;
        };
        managed.cursor = (position.x as f32, position.y as f32);
        match overlay {
            Some(index) => {
                let (origin_x, origin_y) = self.session.as_ref().map_or((0, 0), |state| (state.screens()[index].x, state.screens()[index].y));
                if let Some(state) = self.session.as_mut() {
                    let before = state.screen_at(state.cursor());
                    state.mouse_moved(origin_x + position.x as i32, origin_y + position.y as i32);
                    if state.screen_at(state.cursor()) != before {
                        self.redraw_all_overlays();
                        return;
                    }
                }
                managed.window.request_redraw();
            }
            None => self.hover_small_window(id),
        }
    }

    fn cursor_left(&mut self, id: WindowId) {
        if let Some(card) = self.card.as_mut().filter(|card| card.window == id) {
            card.hovered = None;
        }
        if let Some(recording) = self.recording.as_mut().filter(|recording| recording.bar == id) {
            recording.hovered = None;
        }
    }

    fn hover_small_window(&mut self, id: WindowId) {
        let Some((x, y)) = self.windows.get(&id).map(|managed| managed.cursor) else {
            return;
        };
        if let Some(card) = self.card.as_mut().filter(|card| card.window == id) {
            card.hovered = card.layout.action_at(x, y);
        }
        if let Some(recording) = self.recording.as_mut().filter(|recording| recording.bar == id) {
            recording.hovered = recording.layout.action_at(x, y);
        }
    }

    fn pressed(&mut self, event_loop: &ActiveEventLoop, id: WindowId, overlay: Option<usize>) {
        if overlay.is_some() {
            if let Some(state) = self.session.as_mut() {
                state.mouse_pressed(&self.fonts);
            }
            self.after_overlay_input(event_loop);
            return;
        }
        let Some((x, y)) = self.windows.get(&id).map(|managed| managed.cursor) else {
            return;
        };
        let card_action = self.card.as_ref().filter(|card| card.window == id).and_then(|card| card.layout.action_at(x, y));
        if let Some(action) = card_action {
            self.run_card_action(event_loop, action);
        }
        let bar_action = self.recording.as_ref().filter(|recording| recording.bar == id).and_then(|recording| recording.layout.action_at(x, y));
        match bar_action {
            Some(BarAction::Pause) => self.toggle_pause(),
            Some(BarAction::Stop) => self.stop_recording(event_loop),
            None => {}
        }
    }

    fn released(&mut self, event_loop: &ActiveEventLoop, overlay: Option<usize>) {
        if overlay.is_some() {
            if let Some(state) = self.session.as_mut() {
                state.mouse_released();
            }
            self.after_overlay_input(event_loop);
        }
    }

    // ------------------------------------------------------------ snipping

    /// A capture whose wait (`defer_capture`) is over.
    fn run_delayed(&mut self, event_loop: &ActiveEventLoop, trigger: Trigger) {
        match trigger {
            Trigger::Snip(mode) => self.start_snip(event_loop, mode, Tool::Snip, true),
            Trigger::Record => self.start_snip(event_loop, Mode::Region, Tool::Record, true),
            Trigger::LastRegion => self.capture_last_region(event_loop, true),
            other => self.handle_trigger(event_loop, other),
        }
    }

    /// Hides the card and, when the capture has to wait for the delay or for the card to
    /// leave the screen, schedules `trigger` and returns true. `waited` says this is the
    /// return from such a wait.
    fn defer_capture(&mut self, delay: Duration, waited: bool, trigger: Trigger) -> bool {
        let card_on_screen = self.card.is_some();
        if let Some(card) = self.card.take() {
            self.close_window(card.window);
        }
        match pending::wait_before_capture(delay, card_on_screen, waited) {
            Some(wait) => {
                self.delayed = Some((Instant::now() + wait, trigger));
                true
            }
            None => false,
        }
    }

    fn start_snip(&mut self, event_loop: &ActiveEventLoop, mode: Mode, tool: Tool, waited: bool) {
        self.settings = settings::load();
        let delay = match tool {
            Tool::Snip => Duration::from_secs(u64::from(self.settings.delay_seconds)),
            _ => Duration::ZERO,
        };
        let trigger = match tool {
            Tool::Record => Trigger::Record,
            _ => Trigger::Snip(mode),
        };
        if self.defer_capture(delay, waited, trigger) {
            return;
        }
        // The screens are captured before any of our windows exists.
        let screens = match self.platform.capture_screens() {
            Ok(screens) if !screens.is_empty() => screens,
            Ok(_) => return self.show_failure(event_loop, "snip failed", "no screen to capture"),
            Err(error) => return self.show_failure(event_loop, "snip failed", &error),
        };
        let options = OverlayOptions {
            extra_tools: true,
            snap_to_windows: self.settings.snap_to_windows,
            show_loupe: self.settings.show_loupe,
            show_crosshair: self.settings.show_crosshair,
        };
        let state = OverlayState::new(screens, self.platform.windows_on_screen(), options, mode, tool);
        for (index, screen) in state.screens().iter().enumerate() {
            self.open_overlay_window(event_loop, index, screen);
        }
        self.session = Some(state);
        self.redraw_all_overlays();
    }

    fn after_overlay_input(&mut self, event_loop: &ActiveEventLoop) {
        let finished = self.session.as_ref().is_some_and(|state| state.outcome().is_some());
        if !finished {
            self.redraw_all_overlays();
            return;
        }
        let outcome = self.session.as_ref().and_then(|state| state.outcome().cloned()).unwrap_or(Outcome::Cancelled);
        if let Outcome::Picked(pick) = &outcome {
            if pick.tool == Tool::Draw && self.start_markup(pick) {
                return;
            }
        }
        // Shutter feel: the shade goes away before anything slow happens.
        self.close_overlay_windows(None);
        let Some(state) = self.session.take() else {
            return;
        };
        let screens = state.into_screens();
        match outcome {
            Outcome::Cancelled => self.finish_one_shot(event_loop),
            Outcome::Color(rgb) => self.finish_color(event_loop, rgb),
            Outcome::Picked(pick) => self.finish_pick(event_loop, pick, &screens),
        }
    }

    fn finish_one_shot(&mut self, event_loop: &ActiveEventLoop) {
        if self.one_shot && self.recording.is_none() {
            event_loop.exit();
        }
    }

    fn finish_color(&mut self, event_loop: &ActiveEventLoop, rgb: [u8; 3]) {
        let hex = output::color_hex(rgb);
        let copied = self.platform.copy_text(&hex);
        let content = CardContent {
            headline: if copied.is_ok() { format!("{hex} copied") } else { hex },
            detail: output::color_details(rgb),
            swatch: Some(rgb),
            failed: copied.is_err(),
            ..CardContent::default()
        };
        self.show_card(event_loop, content);
        self.finish_one_shot(event_loop);
    }

    fn finish_pick(&mut self, event_loop: &ActiveEventLoop, pick: Pick, screens: &[Screen]) {
        let screen = &screens[pick.screen];
        let mut image = canvas::crop_image(&screen.image, pick.area);
        if let Some(outline) = &pick.outline {
            output::keep_inside(&mut image, outline);
        }
        let global = (screen.x + pick.area.0, screen.y + pick.area.1, pick.area.2, pick.area.3);
        finish::remember_region(global);
        match pick.tool {
            Tool::Text => self.finish_text(event_loop, &image),
            Tool::Record => self.start_recording(event_loop, global, screen.scale),
            // A draw pick normally went to `start_markup`; it is saved like a snip only when
            // the drawing step could not open.
            Tool::Snip | Tool::Draw | Tool::Color => self.finish_snip(event_loop, image),
        }
        self.finish_one_shot(event_loop);
    }

    // ------------------------------------------------------------ drawing

    fn close_overlay_windows(&mut self, keep: Option<usize>) {
        let ids: Vec<WindowId> = self
            .windows
            .iter()
            .filter(|(_, managed)| matches!(managed.kind, Kind::Overlay(index) if Some(index) != keep))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.close_window(id);
        }
    }

    /// Keeps the overlay window of the picked screen and draws on the picked area in it.
    /// False when that cannot be set up.
    fn start_markup(&mut self, pick: &Pick) -> bool {
        let Some(state) = self.session.as_ref() else {
            return false;
        };
        let screen = &state.screens()[pick.screen];
        let mut image = canvas::crop_image(&screen.image, pick.area);
        if let Some(outline) = &pick.outline {
            output::keep_inside(&mut image, outline);
        }
        let global = (screen.x + pick.area.0, screen.y + pick.area.1, pick.area.2, pick.area.3);
        let Some(markup) = MarkupState::new(&screen.image, screen.scale, pick.area, image) else {
            return false;
        };
        finish::remember_region(global);
        self.close_overlay_windows(Some(pick.screen));
        self.session = None;
        self.markup = Some(markup);
        self.redraw_all_overlays();
        true
    }

    fn markup_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        let (shift, command) = (self.shift, self.command);
        let Some(markup) = self.markup.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CursorMoved { position, .. } => markup.mouse_moved(position.x as f32, position.y as f32),
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => markup.mouse_pressed(&self.fonts),
            WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. } => markup.mouse_released(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => match overlay_key(&event.logical_key) {
                Some(key) => markup.key_pressed(key, shift, command),
                None => return,
            },
            WindowEvent::CloseRequested => markup.cancel(),
            _ => return,
        }
        self.after_markup_input(event_loop);
    }

    fn after_markup_input(&mut self, event_loop: &ActiveEventLoop) {
        let Some(ending) = self.markup.as_ref().and_then(|markup| markup.ending()) else {
            return self.redraw_all_overlays();
        };
        self.close_overlay_windows(None);
        let Some(markup) = self.markup.take() else {
            return;
        };
        if ending == Ending::Done {
            let image = markup.flattened(&self.fonts);
            self.finish_snip(event_loop, image);
        }
        self.finish_one_shot(event_loop);
    }

    fn finish_snip(&mut self, event_loop: &ActiveEventLoop, image: Image) {
        let finished = match finish::finish_snip(self.platform.as_ref(), &image, &self.settings, &self.overrides) {
            Ok(finished) => finished,
            Err(error) => return self.show_failure(event_loop, "snip failed", &error),
        };
        if self.one_shot {
            match (&finished.saved_path, &finished.save_error) {
                (Some(path), _) => println!("{}x{} -> {}", image.width, image.height, path.display()),
                (None, Some(error)) => eprintln!("oh-snap: could not save: {error}"),
                (None, None) => println!("{}x{}", image.width, image.height),
            }
            return;
        }
        let headline = match (finished.copied, finished.saved_path.is_some()) {
            (true, true) => "copied · saved",
            (true, false) => "copied to clipboard",
            (false, true) => "saved",
            (false, false) => "snipped",
        };
        let mut actions = Vec::new();
        if finished.saved_path.is_some() {
            actions.push(CardAction::Open);
            actions.push(CardAction::Folder);
        }
        actions.push(CardAction::Copy);
        let error = finished.copy_error.or(finished.save_error);
        let content = CardContent {
            headline: headline.into(),
            detail: error.clone().unwrap_or_else(|| format!("{} × {}", image.width, image.height)),
            thumbnail: canvas::pixmap_from_image(&image),
            actions,
            failed: error.is_some(),
            path: finished.saved_path,
            image: Some(image),
            ..CardContent::default()
        };
        self.show_card(event_loop, content);
    }

    fn finish_text(&mut self, event_loop: &ActiveEventLoop, image: &Image) {
        // A QR code first: it is exact, OCR is a guess.
        let (headline, text) = match crate::qr::read_qr_code(image) {
            Some(payload) => ("QR code copied".to_string(), payload),
            None => match self.platform.read_text(image) {
                Ok(text) if !text.trim().is_empty() => ("text copied".to_string(), text),
                Ok(_) => return self.show_failure(event_loop, "no text found", "try a tighter area"),
                Err(error) => return self.show_failure(event_loop, "text failed", &error),
            },
        };
        if let Err(error) = self.platform.copy_text(&text) {
            return self.show_failure(event_loop, "text failed", &error);
        }
        let first_line = text.lines().next().unwrap_or_default().chars().take(36).collect::<String>();
        let content = CardContent { headline, detail: first_line, thumbnail: canvas::pixmap_from_image(image), ..CardContent::default() };
        if self.one_shot {
            println!("{text}");
        } else {
            self.show_card(event_loop, content);
        }
    }

    fn capture_last_region(&mut self, event_loop: &ActiveEventLoop, waited: bool) {
        let Some(area) = finish::last_region() else {
            return self.show_note(event_loop, "no last region yet", "snip one first");
        };
        if self.defer_capture(Duration::ZERO, waited, Trigger::LastRegion) {
            return;
        }
        match finish::capture_area(self.platform.as_ref(), area) {
            Ok(image) => self.finish_snip(event_loop, image),
            Err(error) => self.show_failure(event_loop, "snip failed", &error),
        }
        self.finish_one_shot(event_loop);
    }

    fn copy_recent(&mut self, event_loop: &ActiveEventLoop, path: PathBuf) {
        let image = match crate::image_file::load_png(&path) {
            Ok(image) => image,
            Err(error) => return self.show_failure(event_loop, "cannot open it", &error.to_string()),
        };
        let png = output::encode_png(&image).unwrap_or_default();
        if let Err(error) = self.platform.copy_image(&image, &png) {
            return self.show_failure(event_loop, "copy failed", &error);
        }
        let content = CardContent {
            headline: "copied again".into(),
            detail: format!("{} × {}", image.width, image.height),
            thumbnail: canvas::pixmap_from_image(&image),
            actions: vec![CardAction::Open, CardAction::Folder, CardAction::Copy],
            path: Some(path),
            image: Some(image),
            ..CardContent::default()
        };
        self.show_card(event_loop, content);
    }

    // ------------------------------------------------------------ recording

    fn start_recording(&mut self, event_loop: &ActiveEventLoop, area: (i32, i32, u32, u32), scale: f32) {
        let options = RecordOptions {
            frames_per_second: self.settings.record_frames_per_second,
            system_sound: self.settings.record_system_sound,
            microphone: self.settings.record_microphone,
        };
        let path = finish::recording_path(&self.settings);
        if let Some(folder) = path.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let handle = match self.platform.start_recording(area, options, &path) {
            Ok(handle) => handle,
            Err(error) => return self.show_failure(event_loop, "recording failed", &error),
        };
        let layout = BarLayout::new(&self.fonts, scale);
        // Under the area, or above it when there is no room below.
        let below = f64::from(area.1) + f64::from(area.3) + SCREEN_MARGIN;
        let left = f64::from(area.0) + (f64::from(area.2) - f64::from(layout.width)) / 2.0;
        let attributes = Window::default_attributes()
            .with_title("oh-snap recording")
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_position(PhysicalPosition::new(left.max(0.0), below))
            .with_inner_size(PhysicalSize::new(layout.width, layout.height));
        let bar = event_loop.create_window(attributes).ok().and_then(|window| self.make_window(event_loop, window, Kind::Bar));
        let Some(bar) = bar else {
            let _ = handle.stop();
            return self.show_failure(event_loop, "recording failed", "cannot open the recording bar");
        };
        self.recording = Some(ActiveRecording { handle, bar, layout, started: Instant::now(), paused_since: None, paused_total: Duration::ZERO, hovered: None });
    }

    fn toggle_pause(&mut self) {
        let Some(recording) = self.recording.as_mut() else {
            return;
        };
        match recording.paused_since.take() {
            Some(since) => {
                recording.paused_total += since.elapsed();
                recording.handle.pause(false);
            }
            None => {
                recording.paused_since = Some(Instant::now());
                recording.handle.pause(true);
            }
        }
    }

    fn stop_recording(&mut self, event_loop: &ActiveEventLoop) {
        let Some(recording) = self.recording.take() else {
            return;
        };
        let length = recording.elapsed();
        self.close_window(recording.bar);
        match recording.handle.stop() {
            Ok(path) => {
                if self.one_shot {
                    println!("{}", path.display());
                    return event_loop.exit();
                }
                let content = CardContent {
                    headline: "recorded".into(),
                    detail: finish::file_name(&path),
                    actions: vec![CardAction::Open, CardAction::Gif, CardAction::Trim, CardAction::Folder],
                    path: Some(path),
                    recording_length: Some(length),
                    ..CardContent::default()
                };
                self.show_card(event_loop, content);
            }
            Err(error) => self.show_failure(event_loop, "recording failed", &error),
        }
        self.finish_one_shot(event_loop);
    }

    fn tick_recording(&mut self) {
        if let Some(recording) = &self.recording {
            if let Some(managed) = self.windows.get(&recording.bar) {
                managed.window.request_redraw();
            }
        }
    }

    // ------------------------------------------------------------ the card

    fn show_failure(&mut self, event_loop: &ActiveEventLoop, headline: &str, detail: &str) {
        crate::log::write(&format!("{headline}: {detail}"));
        if self.one_shot {
            eprintln!("oh-snap: {headline}: {detail}");
            return event_loop.exit();
        }
        self.show_card(event_loop, CardContent { headline: headline.into(), detail: detail.chars().take(40).collect(), failed: true, ..CardContent::default() });
    }

    /// The browser takes the link the same way the file manager takes a path.
    fn open_report(&self) {
        self.platform.open(std::path::Path::new(&crate::report::link()));
    }

    fn show_note(&mut self, event_loop: &ActiveEventLoop, headline: &str, detail: &str) {
        self.show_card(event_loop, CardContent { headline: headline.into(), detail: detail.into(), ..CardContent::default() });
    }

    fn show_card(&mut self, event_loop: &ActiveEventLoop, content: CardContent) {
        if self.one_shot || !self.settings.show_card && !content.failed {
            return;
        }
        if let Some(old) = self.card.take() {
            self.close_window(old.window);
        }
        let monitor = event_loop.primary_monitor().or_else(|| event_loop.available_monitors().next());
        let scale = monitor.as_ref().map_or(1.0, |monitor| monitor.scale_factor() as f32);
        let model = CardModel {
            headline: content.headline,
            detail: content.detail,
            thumbnail: content.thumbnail,
            swatch: content.swatch,
            actions: content.actions,
            remaining: 1.0,
            failed: content.failed,
        };
        let layout = CardLayout::new(&model, &self.fonts, scale);
        let (left, top) = match &monitor {
            Some(monitor) => {
                let margin = SCREEN_MARGIN * f64::from(scale);
                let origin = monitor.position();
                let size = monitor.size();
                (
                    f64::from(origin.x) + f64::from(size.width) - f64::from(layout.width) - margin,
                    f64::from(origin.y) + f64::from(size.height) - f64::from(layout.height) - margin * 3.0,
                )
            }
            None => (0.0, 0.0),
        };
        let attributes = Window::default_attributes()
            .with_title("oh-snap")
            .with_decorations(false)
            .with_resizable(false)
            .with_active(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_position(PhysicalPosition::new(left, top))
            .with_inner_size(PhysicalSize::new(layout.width, layout.height));
        let Ok(window) = event_loop.create_window(attributes) else {
            return;
        };
        let Some(id) = self.make_window(event_loop, window, Kind::Card) else {
            return;
        };
        let lifetime = Duration::from_secs(u64::from(self.settings.card_seconds.max(1)));
        self.card = Some(CardState { window: id, model, layout, path: content.path, image: content.image, recording_length: content.recording_length, hovered: None, shown_for: Duration::ZERO, lifetime, last_tick: Instant::now() });
    }

    fn close_card_if(&mut self, id: WindowId) {
        if self.card.as_ref().is_some_and(|card| card.window == id) {
            self.close_window(id);
            self.card = None;
        }
    }

    fn tick_card(&mut self, event_loop: &ActiveEventLoop) {
        let Some(card) = self.card.as_mut() else {
            return;
        };
        let now = Instant::now();
        let delta = now - card.last_tick;
        card.last_tick = now;
        // The countdown waits while the pointer is on a button.
        if card.hovered.is_none() {
            card.shown_for += delta;
        }
        card.model.remaining = 1.0 - card.shown_for.as_secs_f32() / card.lifetime.as_secs_f32();
        let id = card.window;
        if card.model.remaining <= 0.0 {
            self.close_card_if(id);
            self.finish_one_shot(event_loop);
        } else if let Some(managed) = self.windows.get(&id) {
            managed.window.request_redraw();
        }
    }

    fn run_card_action(&mut self, event_loop: &ActiveEventLoop, action: CardAction) {
        let Some(card) = self.card.as_mut() else {
            return;
        };
        // Using a button keeps the card for a while longer.
        card.shown_for = Duration::ZERO;
        let path = card.path.clone();
        let length = card.recording_length;
        match (action, path) {
            (CardAction::Open, Some(path)) => self.platform.open(&path),
            (CardAction::Folder, Some(path)) => self.platform.reveal(&path),
            (CardAction::Copy, _) => self.copy_card_image(),
            (CardAction::Report, _) => self.open_report(),
            (CardAction::Gif, Some(path)) => self.make_in_background("gif ready", move |platform| platform.make_gif(&path)),
            (CardAction::Trim, Some(path)) => match length {
                Some(length) => self.open_trim(event_loop, path, length),
                None => self.show_note(event_loop, "cannot trim this", "its length is not known"),
            },
            _ => {}
        }
    }

    // ------------------------------------------------------------ trimming

    fn open_trim(&mut self, event_loop: &ActiveEventLoop, path: PathBuf, length: Duration) {
        if self.trim.is_some() {
            return;
        }
        let monitor = event_loop.primary_monitor().or_else(|| event_loop.available_monitors().next());
        let scale = monitor.as_ref().map_or(1.0, |monitor| monitor.scale_factor() as f32);
        let state = TrimState::new(&self.fonts, scale, length, finish::file_name(&path));
        let (width, height) = (state.layout().width, state.layout().height);
        let (left, top) = match &monitor {
            Some(monitor) => (
                f64::from(monitor.position().x) + (f64::from(monitor.size().width) - f64::from(width)) / 2.0,
                f64::from(monitor.position().y) + (f64::from(monitor.size().height) - f64::from(height)) / 2.0,
            ),
            None => (0.0, 0.0),
        };
        let attributes = Window::default_attributes()
            .with_title("oh-snap trim")
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_position(PhysicalPosition::new(left, top))
            .with_inner_size(PhysicalSize::new(width, height));
        let Ok(window) = event_loop.create_window(attributes) else {
            return self.show_failure(event_loop, "trim failed", "cannot open the trim window");
        };
        let Some(id) = self.make_window(event_loop, window, Kind::Trim) else {
            return;
        };
        if let Some(managed) = self.windows.get(&id) {
            managed.window.focus_window();
            managed.window.request_redraw();
        }
        self.trim = Some(TrimSession { window: id, state, path });
    }

    fn trim_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        let Some(trim) = self.trim.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CursorMoved { position, .. } => trim.state.mouse_moved(position.x as f32, position.y as f32),
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => trim.state.mouse_pressed(),
            WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. } => trim.state.mouse_released(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => match overlay_key(&event.logical_key) {
                Some(key) => trim.state.key_pressed(key),
                None => return,
            },
            WindowEvent::CloseRequested => trim.state.key_pressed(Key::Escape),
            _ => return,
        }
        let Some(outcome) = trim.state.outcome() else {
            if let Some(managed) = self.windows.get(&trim.window) {
                managed.window.request_redraw();
            }
            return;
        };
        let Some(trim) = self.trim.take() else {
            return;
        };
        self.close_window(trim.window);
        if outcome != TrimOutcome::Keep {
            return;
        }
        if trim.state.range().is_whole() {
            return self.show_note(event_loop, "nothing to trim", "move a handle first");
        }
        let (from, to) = trim.state.seconds();
        let path = trim.path;
        self.make_in_background("trim saved", move |platform| platform.trim(&path, from, to));
    }

    fn copy_card_image(&mut self) {
        let Some(image) = self.card.as_ref().and_then(|card| card.image.as_ref()) else {
            return;
        };
        let png = output::encode_png(image).unwrap_or_default();
        if let Err(error) = self.platform.copy_image(image, &png) {
            crate::log::write(&format!("copy: {error}"));
        }
    }

    fn make_in_background(&self, label: &'static str, work: impl FnOnce(&dyn Platform) -> Result<PathBuf, String> + Send + 'static) {
        let platform = self.platform.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let result = work(platform.as_ref());
            let _ = proxy.send_event(UserEvent::FileReady { label, result });
        });
    }

    // ------------------------------------------------------------ updates

    fn maybe_check_for_update(&mut self, now: Instant) {
        if self.next_update_check.is_some_and(|at| at <= now) {
            if self.busy() {
                // Never while a snip or recording is active: look again shortly.
                self.next_update_check = Some(now + Duration::from_secs(60));
            } else {
                self.start_update_check(false);
            }
        }
    }

    fn start_update_check(&mut self, forced: bool) {
        if self.update_running {
            return;
        }
        let mode = match (self.settings.updates, forced) {
            (UpdateMode::Off, true) => UpdateMode::Notify,
            (mode, _) => mode,
        };
        if mode == UpdateMode::Off {
            self.next_update_check = None;
            return;
        }
        self.update_running = true;
        self.next_update_check = (self.settings.updates != UpdateMode::Off).then(|| Instant::now() + UPDATE_INTERVAL);
        let platform = self.platform.clone();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let result = update::check(platform.as_ref(), mode);
            let _ = proxy.send_event(UserEvent::Updated(result));
        });
    }
}
