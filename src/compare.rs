//! `oh-snap bench compare [ROUNDS]`: oh-snap against Windows Snipping Tool. Both start
//! the way a script would: oh-snap by command, Snipping Tool by `ms-screenclip:`.
//! Each round records two moments: the overlay window exists, and the screen visibly dims.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_ESCAPE,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowW, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, SW_SHOWNORMAL,
    WM_KEYDOWN, WM_KEYUP,
};
use windows::core::{BOOL, PWSTR, w};

use crate::capture::{Capturer, Screenshot};

#[derive(Clone, Copy, PartialEq)]
enum Contender {
    OhSnap,
    SnippingTool,
}

struct Round {
    window_shown: Option<Duration>,
    screen_dimmed: Option<Duration>,
}

pub fn run(rounds: usize) -> Result<(), Box<dyn std::error::Error>> {
    let mut capturer = Capturer::new()?;
    let daemon = ensure_daemon()?;

    let mut oh_snap = Vec::new();
    let mut snipping_tool = Vec::new();
    for _ in 0..rounds {
        let started = Instant::now();
        crate::daemon::send_command(daemon, "capture region");
        oh_snap.push(watch(&mut capturer, started, Contender::OhSnap)?);
        std::thread::sleep(Duration::from_millis(700));

        let started = Instant::now();
        unsafe {
            ShellExecuteW(None, w!("open"), w!("ms-screenclip:"), None, None, SW_SHOWNORMAL);
        }
        snipping_tool.push(watch(&mut capturer, started, Contender::SnippingTool)?);
        std::thread::sleep(Duration::from_millis(700));
    }

    report("oh-snap", &oh_snap);
    report("Snipping Tool", &snipping_tool);
    Ok(())
}

fn ensure_daemon() -> Result<HWND, Box<dyn std::error::Error>> {
    if let Some(daemon) = crate::daemon::find_running() {
        return Ok(daemon);
    }
    std::process::Command::new(std::env::current_exe()?).arg("daemon").spawn()?;
    for _ in 0..100 {
        if let Some(daemon) = crate::daemon::find_running() {
            // Let it finish its warm-up before timing it.
            std::thread::sleep(Duration::from_millis(500));
            return Ok(daemon);
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    Err("the oh-snap tray process did not start".into())
}

/// Polls until both moments are seen or 5 s pass, then closes the overlay with Esc.
fn watch(capturer: &mut Capturer, started: Instant, contender: Contender) -> Result<Round, Box<dyn std::error::Error>> {
    let baseline = brightness(capturer.capture()?);
    let can_see_dimming = baseline > 30.0;
    let mut round = Round {
        window_shown: None,
        screen_dimmed: None,
    };
    let mut overlay = None;
    while started.elapsed() < Duration::from_secs(5) {
        if round.window_shown.is_none() {
            if let Some(hwnd) = overlay_window(contender) {
                round.window_shown = Some(started.elapsed());
                overlay = Some(hwnd);
            }
        }
        if round.screen_dimmed.is_none() && can_see_dimming {
            let current = brightness(capturer.capture()?);
            if current < baseline * 0.85 {
                round.screen_dimmed = Some(started.elapsed());
            }
        }
        let dimming_done = round.screen_dimmed.is_some() || !can_see_dimming;
        if round.window_shown.is_some() && dimming_done {
            break;
        }
    }
    if let Some(hwnd) = overlay.or_else(|| overlay_window(contender)) {
        close_overlay(hwnd, contender);
    }
    Ok(round)
}

/// oh-snap takes a posted Esc. Snipping Tool's XAML overlay ignores posted keys, so it
/// gets a real Esc, and only while one of its own windows has the focus.
fn close_overlay(hwnd: HWND, contender: Contender) {
    if contender == Contender::OhSnap {
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_KEYDOWN, WPARAM(VK_ESCAPE.0 as usize), LPARAM(0));
            let _ = PostMessageW(Some(hwnd), WM_KEYUP, WPARAM(VK_ESCAPE.0 as usize), LPARAM(0));
        }
        return;
    }
    for _ in 0..50 {
        let foreground = unsafe { GetForegroundWindow() };
        if belongs_to_snipping_tool(foreground) {
            press_escape();
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    for _ in 0..100 {
        if overlay_window(contender).is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn press_escape() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_ESCAPE,
                dwFlags: flags,
                ..Default::default()
            },
        },
    };
    let inputs = [key(Default::default()), key(KEYEVENTF_KEYUP)];
    unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
}

/// Mean luminance over a sparse grid; enough to see a full-screen shade arrive.
fn brightness(screenshot: &Screenshot) -> f64 {
    let stride = screenshot.stride();
    let mut total = 0.0f64;
    let mut count = 0.0f64;
    for row in (0..screenshot.bounds.height as usize).step_by(24) {
        for column in (0..screenshot.bounds.width as usize).step_by(24) {
            let offset = row * stride + column * 4;
            let blue = screenshot.pixels[offset] as f64;
            let green = screenshot.pixels[offset + 1] as f64;
            let red = screenshot.pixels[offset + 2] as f64;
            total += 0.2126 * red + 0.7152 * green + 0.0722 * blue;
            count += 1.0;
        }
    }
    total / count.max(1.0)
}

fn overlay_window(contender: Contender) -> Option<HWND> {
    match contender {
        Contender::OhSnap => {
            let hwnd = unsafe { FindWindowW(w!("oh-snap.overlay"), w!("oh-snap")) }.ok()?;
            unsafe { IsWindowVisible(hwnd) }.as_bool().then_some(hwnd)
        }
        Contender::SnippingTool => {
            let mut found: Option<HWND> = None;
            unsafe {
                let _ = EnumWindows(Some(check_snipping_window), LPARAM(&mut found as *mut Option<HWND> as isize));
            }
            found
        }
    }
}

/// Matches a visible, monitor-sized window that belongs to Snipping Tool.
unsafe extern "system" fn check_snipping_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let found = unsafe { &mut *(lparam.0 as *mut Option<HWND>) };
    // Snipping Tool keeps its overlay alive between snips, hidden by DWM cloaking.
    let visible = unsafe { IsWindowVisible(hwnd) }.as_bool() && !is_cloaked(hwnd);
    let mut rect = RECT::default();
    let _ = unsafe { GetWindowRect(hwnd, &mut rect) };
    let large = rect.right - rect.left >= 800 && rect.bottom - rect.top >= 600;
    if visible && large && belongs_to_snipping_tool(hwnd) {
        *found = Some(hwnd);
        return BOOL(0);
    }
    BOOL(1)
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    let read = unsafe {
        DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, size_of::<u32>() as u32)
    };
    read.is_ok() && cloaked != 0
}

fn belongs_to_snipping_tool(hwnd: HWND) -> bool {
    let mut process_id = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process_id)) };
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }) else {
        return false;
    };
    let mut buffer = [0u16; 512];
    let mut length = buffer.len() as u32;
    let named =
        unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut length) };
    unsafe {
        let _ = CloseHandle(process);
    }
    if named.is_err() {
        return false;
    }
    let path = String::from_utf16_lossy(&buffer[..length as usize]).to_ascii_lowercase();
    path.contains("screenclippinghost") || path.contains("snippingtool")
}

fn report(name: &str, rounds: &[Round]) {
    println!("{name}");
    let shown: Vec<Duration> = rounds.iter().filter_map(|round| round.window_shown).collect();
    let dimmed: Vec<Duration> = rounds.iter().filter_map(|round| round.screen_dimmed).collect();
    let first_shown = rounds.first().and_then(|round| round.window_shown);
    let first_dimmed = rounds.first().and_then(|round| round.screen_dimmed);
    println!("  window shown   {}", describe(shown, first_shown, rounds.len()));
    println!("  screen dimmed  {}", describe(dimmed, first_dimmed, rounds.len()));
}

fn describe(mut samples: Vec<Duration>, first: Option<Duration>, attempts: usize) -> String {
    if samples.is_empty() {
        return format!("not seen in {attempts} tries");
    }
    samples.sort();
    let median = samples[samples.len() / 2];
    let worst = samples[samples.len() - 1];
    let first_text = first.map(millis).unwrap_or_else(|| "-".into());
    format!(
        "median {}  worst {}  first {}  (seen {}/{})",
        millis(median),
        millis(worst),
        first_text,
        samples.len(),
        attempts
    )
}

fn millis(duration: Duration) -> String {
    format!("{:.0} ms", duration.as_secs_f64() * 1000.0)
}
