//! Leaves a line in the log when the program dies, so a report can say where: the tray process
//! has no console, and Windows often keeps no record of its own.

use windows::Win32::System::Diagnostics::Debug::{EXCEPTION_POINTERS, SetUnhandledExceptionFilter};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
    GetModuleHandleExW,
};
use windows::core::PCWSTR;

const CONTINUE_SEARCH: i32 = 0;

pub fn install() {
    unsafe {
        let _ = SetUnhandledExceptionFilter(Some(on_exception));
    }
    std::panic::set_hook(Box::new(|info| {
        let one_line = info.to_string().replace('\n', " ");
        crate::log::write(&format!("crash in {}: panic: {one_line}", crate::updater::CURRENT_VERSION));
    }));
}

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
    crate::log::write(&format!(
        "crash in {}: exception {:#x} at {place}",
        crate::updater::CURRENT_VERSION,
        record.ExceptionCode.0 as u32
    ));
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
