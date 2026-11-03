//! The local date and time, for file names.

pub struct LocalTime {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millisecond: u32,
}

#[cfg(windows)]
pub fn local_time() -> LocalTime {
    let time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    LocalTime {
        year: u32::from(time.wYear),
        month: u32::from(time.wMonth),
        day: u32::from(time.wDay),
        hour: u32::from(time.wHour),
        minute: u32::from(time.wMinute),
        second: u32::from(time.wSecond),
        millisecond: u32::from(time.wMilliseconds),
    }
}

#[cfg(not(windows))]
pub fn local_time() -> LocalTime {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = since_epoch.as_secs() as libc::time_t;
    let mut broken_down: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&seconds, &mut broken_down) };
    LocalTime {
        year: (broken_down.tm_year + 1900) as u32,
        month: (broken_down.tm_mon + 1) as u32,
        day: broken_down.tm_mday as u32,
        hour: broken_down.tm_hour as u32,
        minute: broken_down.tm_min as u32,
        second: broken_down.tm_sec as u32,
        millisecond: since_epoch.subsec_millis(),
    }
}
