//! Leaves a line in the log when the program dies, so a report can say where: the tray process
//! has no console, and Windows often keeps no record of its own. A tray process that dies
//! also starts a new one, which shows a card that leads to the report.

#[cfg(windows)]
use windows::Win32::System::Diagnostics::Debug::{EXCEPTION_POINTERS, SetUnhandledExceptionFilter};
#[cfg(windows)]
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
    GetModuleHandleExW,
};
#[cfg(windows)]
use windows::core::PCWSTR;

#[cfg(windows)]
const CONTINUE_SEARCH: i32 = 0;

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// When the tray process started, and whether a crash started it. Only the tray process
/// sets this, so only it comes back after a crash.
static TRAY_STARTED: OnceLock<(Instant, bool)> = OnceLock::new();
/// A panic can end in an exception too; the first report is the one that counts.
static ALREADY_RECORDED: AtomicBool = AtomicBool::new(false);
/// A tray process that a crash started and that dies again this soon stays down: bringing
/// it back would loop.
const CRASH_LOOP_WINDOW: Duration = Duration::from_secs(60);
/// What the last crash was, kept until the next tray process shows it.
const PENDING_FILE: &str = "last-crash.txt";

/// Called by the tray process: from here on a crash is kept for a report and the tray
/// comes back by itself.
pub fn watch_tray(started_after_crash: bool) {
    let _ = TRAY_STARTED.set((Instant::now(), started_after_crash));
}

/// The crash the last tray process died of, once.
pub fn take_pending() -> Option<String> {
    let path = crate::log::data_folder().join(PENDING_FILE);
    let summary = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let summary = summary.trim().to_string();
    (!summary.is_empty()).then_some(summary)
}

fn record(summary: &str) {
    if ALREADY_RECORDED.swap(true, Ordering::SeqCst) {
        return;
    }
    crate::log::write(&format!("crash in {}: {summary}", env!("CARGO_PKG_VERSION")));
    let Some((started, started_after_crash)) = TRAY_STARTED.get() else {
        return;
    };
    let _ = std::fs::write(crate::log::data_folder().join(PENDING_FILE), summary);
    if *started_after_crash && started.elapsed() < CRASH_LOOP_WINDOW {
        crate::log::write("crashed again right after a restart, staying down");
        return;
    }
    restart_tray();
}

/// Starts a new tray process. It waits for this one to let go of the single-instance lock.
fn restart_tray() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if let Err(error) = std::process::Command::new(exe).args(["daemon", "--after-crash"]).spawn() {
        crate::log::write(&format!("could not restart after the crash: {error}"));
    }
}

/// Hooks everything this OS can report: native exceptions on Windows, panics everywhere.
pub fn install() {
    #[cfg(windows)]
    unsafe {
        let _ = SetUnhandledExceptionFilter(Some(on_exception));
    }
    #[cfg(unix)]
    install_signal_handlers();
    install_panic_hook();
}

/// The signals a broken program dies of. A panic ends in SIGABRT, after the hook wrote it down.
#[cfg(unix)]
const FATAL_SIGNALS: [(libc::c_int, &str); 5] = [
    (libc::SIGSEGV, "SIGSEGV"),
    (libc::SIGBUS, "SIGBUS"),
    (libc::SIGILL, "SIGILL"),
    (libc::SIGFPE, "SIGFPE"),
    (libc::SIGABRT, "SIGABRT"),
];

#[cfg(unix)]
fn install_signal_handlers() {
    // A tray process that a crash started was spawned inside the signal handler and
    // inherits its mask, with that signal blocked. On macOS a blocked SIGSEGV does not end
    // the process: the faulting instruction runs again, forever.
    unsafe {
        let mut no_signals: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut no_signals);
        libc::sigprocmask(libc::SIG_SETMASK, &no_signals, std::ptr::null_mut());
    }
    for (signal, _) in FATAL_SIGNALS {
        unsafe {
            libc::signal(signal, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
        }
    }
}

/// Writing a file and starting a process are not on the list of things a signal handler
/// may do. The process is dying either way, and a report that usually arrives beats none.
#[cfg(unix)]
extern "C" fn on_signal(signal: libc::c_int) {
    let name = FATAL_SIGNALS.iter().find(|(number, _)| *number == signal).map_or("signal", |(_, name)| name);
    record(&format!("{name} ({signal})"));
    // Back to the default action, so the process ends the way it would have.
    unsafe {
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
    }
}

/// Whether a tray process that crashed started this one.
#[cfg(not(windows))]
pub fn started_after_crash() -> bool {
    TRAY_STARTED.get().is_some_and(|(_, started_after_crash)| *started_after_crash)
}

fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let one_line = info.to_string().replace('\n', " ");
        record(&format!("panic: {one_line}"));
    }));
}

#[cfg(windows)]
unsafe extern "system" fn on_exception(info: *const EXCEPTION_POINTERS) -> i32 {
    let record = unsafe { &*(*info).ExceptionRecord };
    let address = record.ExceptionAddress as usize;
    let mut module = Default::default();
    let found = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(record.ExceptionAddress as *const u16),
            &mut module,
        )
    }
    .is_ok();
    let place = if found {
        let mut name = [0u16; 400];
        let length = unsafe { GetModuleFileNameW(Some(module), &mut name) } as usize;
        let path = String::from_utf16_lossy(&name[..length]);
        let file = path.rsplit('\\').next().unwrap_or(&path).to_string();
        format!("{file}+{:#x}", address - module.0 as usize)
    } else {
        format!("{address:#x}")
    };
    self::record(&format!("exception {:#x} at {place}", record.ExceptionCode.0 as u32));
    CONTINUE_SEARCH
}

/// `oh-snap crash-test [panic]`: dies on purpose, to prove that the log gets its line.
pub fn test(kind: Option<&str>) -> ! {
    if kind == Some("panic") {
        panic!("crash test");
    }
    let nowhere = std::hint::black_box(std::ptr::null_mut::<u8>());
    unsafe { nowhere.write_volatile(1) };
    unreachable!("the write above faults");
}
