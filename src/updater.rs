//! Self-update from GitHub Releases: find the latest tag, download oh-snap.exe and
//! SHA256SUMS.txt, verify the hash, swap the exe in place and restart.
//!
//! The latest tag comes from where github.com/<repo>/releases/latest redirects, not from
//! the REST API: the API allows 60 calls an hour per IP address, which people behind a
//! shared address use up.
//!
//! HTTP goes through WinHTTP and hashing through CNG, both part of Windows, so the
//! updater adds no dependency and almost no size.

use std::path::PathBuf;

use windows::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_OPTION_URL,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen,
    WinHttpOpenRequest, WinHttpQueryDataAvailable, WinHttpQueryHeaders, WinHttpQueryOption, WinHttpReadData,
    WinHttpReceiveResponse, WinHttpSendRequest,
};
use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
use windows::core::{HSTRING, PCWSTR};

const RELEASES: &str = "https://github.com/codepawl/oh-snap/releases";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    exe_url: String,
    sums_url: String,
}

pub enum Check {
    UpToDate,
    Available(Release),
}

/// Asks GitHub for the latest release and compares it with this build.
pub fn check() -> Result<Check, String> {
    let (final_url, _) = request("HEAD", &format!("{RELEASES}/latest"))?;
    let tag = tag_from_release_url(&final_url).ok_or_else(|| format!("no release tag in {final_url}"))?;
    let version = tag.trim_start_matches('v').to_string();
    if !is_newer(&version, CURRENT_VERSION) {
        return Ok(Check::UpToDate);
    }
    Ok(Check::Available(Release {
        version,
        exe_url: format!("{RELEASES}/download/{tag}/oh-snap.exe"),
        sums_url: format!("{RELEASES}/download/{tag}/SHA256SUMS.txt"),
    }))
}

/// "v0.5.0" from ".../releases/tag/v0.5.0".
fn tag_from_release_url(url: &str) -> Option<String> {
    let tag = url.split_once("/releases/tag/")?.1.trim_end_matches('/');
    (!tag.is_empty() && !tag.contains('/')).then(|| tag.to_string())
}

/// Downloads the release's exe and returns it only when its SHA256 matches the release's list.
pub fn download(release: &Release) -> Result<Vec<u8>, String> {
    let sums = String::from_utf8_lossy(&http_get(&release.sums_url)?).to_string();
    let expected = sums
        .lines()
        .find(|line| line.trim_end().ends_with("oh-snap.exe"))
        .and_then(|line| line.split_whitespace().next())
        .ok_or("SHA256SUMS.txt has no line for oh-snap.exe")?
        .to_ascii_lowercase();
    let exe = http_get(&release.exe_url)?;
    let actual = sha256_hex(&exe)?;
    if actual != expected {
        return Err(format!("checksum mismatch: expected {expected}, got {actual}"));
    }
    if exe.len() < 100_000 || &exe[..2] != b"MZ" {
        return Err("the download is not a Windows program".into());
    }
    Ok(exe)
}

/// Puts the new exe where this one is. Windows lets a running exe be renamed, so this one
/// moves aside to oh-snap.old.exe and is deleted by the next start.
pub fn install(new_exe: &[u8]) -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    let old = old_exe_path(&current);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&current, &old).map_err(|error| format!("could not move the running program aside: {error}"))?;
    if let Err(error) = std::fs::write(&current, new_exe) {
        // Put the working program back before reporting.
        let _ = std::fs::rename(&old, &current);
        return Err(format!("could not write the new program: {error}"));
    }
    Ok(current)
}

/// Removes the previous exe an update left behind.
pub fn clean_up_old_exe() {
    if let Ok(current) = std::env::current_exe() {
        let _ = std::fs::remove_file(old_exe_path(&current));
    }
}

fn old_exe_path(current: &std::path::Path) -> PathBuf {
    current.with_file_name("oh-snap.old.exe")
}

/// True when `candidate` (like "0.5.0") is a higher version than `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |text: &str| -> Vec<u64> {
        text.split(['.', '-'])
            .take(3)
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect()
    };
    parse(candidate) > parse(current)
}

fn sha256_hex(bytes: &[u8]) -> Result<String, String> {
    let mut digest = [0u8; 32];
    let status = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, bytes, &mut digest) };
    if status.is_err() {
        return Err(format!("hashing failed: {:#x}", status.0));
    }
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn http_get(url: &str) -> Result<Vec<u8>, String> {
    request("GET", url).map(|(_, body)| body)
}

/// An HTTPS request with WinHTTP, following redirects (release downloads redirect to
/// GitHub's file host). Returns the URL it ended at and the body of a 200 response.
fn request(method: &str, url: &str) -> Result<(String, Vec<u8>), String> {
    let without_scheme = url.strip_prefix("https://").ok_or("only https links are fetched")?;
    let (host, path) = without_scheme.split_once('/').map(|(host, path)| (host, format!("/{path}"))).unwrap_or((without_scheme, "/".into()));
    let agent = HSTRING::from(format!("oh-snap/{CURRENT_VERSION}"));
    unsafe {
        let session = WinHttpOpen(&agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0);
        if session.is_null() {
            return Err(windows::core::Error::from_thread().message());
        }
        let result = (|| {
            let connection = WinHttpConnect(session, &HSTRING::from(host), INTERNET_DEFAULT_HTTPS_PORT as u16, 0);
            if connection.is_null() {
                return Err(format!("could not reach {host}"));
            }
            let request = WinHttpOpenRequest(
                connection,
                &HSTRING::from(method),
                &HSTRING::from(path.as_str()),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            );
            let outcome = if request.is_null() {
                Err("could not open the request".to_string())
            } else {
                let read = read_response(request);
                let _ = WinHttpCloseHandle(request);
                read
            };
            let _ = WinHttpCloseHandle(connection);
            outcome
        })();
        let _ = WinHttpCloseHandle(session);
        result
    }
}

unsafe fn read_response(request: *mut std::ffi::c_void) -> Result<(String, Vec<u8>), String> {
    let headers: Vec<u16> = "Accept: application/vnd.github+json, application/octet-stream, */*\r\n"
        .encode_utf16()
        .collect();
    unsafe {
        WinHttpSendRequest(request, Some(&headers), None, 0, 0, 0).map_err(|error| error.message())?;
        WinHttpReceiveResponse(request, std::ptr::null_mut()).map_err(|error| error.message())?;
        let mut status = 0u32;
        let mut size = size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut _),
            &mut size,
            std::ptr::null_mut(),
        )
        .map_err(|error| error.message())?;
        if status != 200 {
            return Err(format!("the server answered {status}"));
        }
        let mut body = Vec::new();
        loop {
            let mut available = 0u32;
            WinHttpQueryDataAvailable(request, &mut available).map_err(|error| error.message())?;
            if available == 0 {
                break;
            }
            let start = body.len();
            body.resize(start + available as usize, 0);
            let mut read = 0u32;
            WinHttpReadData(request, body[start..].as_mut_ptr() as *mut _, available, &mut read)
                .map_err(|error| error.message())?;
            body.truncate(start + read as usize);
        }
        Ok((final_url(request)?, body))
    }
}

unsafe fn final_url(request: *mut std::ffi::c_void) -> Result<String, String> {
    let mut buffer = [0u16; 2048];
    let mut size = (buffer.len() * 2) as u32;
    unsafe {
        WinHttpQueryOption(request, WINHTTP_OPTION_URL, Some(buffer.as_mut_ptr() as *mut _), &mut size)
            .map_err(|error| error.message())?;
    }
    Ok(String::from_utf16_lossy(&buffer[..size as usize / 2]))
}

/// `oh-snap update`: updates the exe it is run from. A tray process running the same exe
/// is stopped first and started again on the new version.
pub fn command() -> Result<(), Box<dyn std::error::Error>> {
    println!("oh-snap {CURRENT_VERSION}, checking GitHub...");
    let release = match check()? {
        Check::UpToDate => {
            println!("up to date");
            return Ok(());
        }
        Check::Available(release) => release,
    };
    println!("downloading {}...", release.version);
    let new_exe = download(&release)?;
    let daemon_was_running = stop_own_daemon();
    let path = install(&new_exe)?;
    if daemon_was_running {
        std::process::Command::new(&path).args(["daemon", "--after-update"]).spawn()?;
    }
    let shown_path = path.display().to_string();
    println!("updated to {} at {}", release.version, shown_path.trim_start_matches(r"\\?\"));
    Ok(())
}

/// Stops the tray process if it runs this same exe; returns whether it did.
fn stop_own_daemon() -> bool {
    let Some(daemon) = crate::daemon::find_running() else {
        return false;
    };
    let Ok(current) = std::env::current_exe() else {
        return false;
    };
    if crate::daemon::process_path_of(daemon).as_deref() != Some(current.as_path()) {
        return false;
    }
    crate::daemon::send_command(daemon, "quit");
    for _ in 0..100 {
        if crate::daemon::find_running().is_none() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert!(is_newer("0.5.0", "0.4.1"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.4.1", "0.4.1"));
        assert!(!is_newer("0.4.0", "0.4.1"));
    }

    #[test]
    fn reads_tag_from_redirect() {
        let url = "https://github.com/codepawl/oh-snap/releases/tag/v0.5.0";
        assert_eq!(tag_from_release_url(url).as_deref(), Some("v0.5.0"));
        assert_eq!(tag_from_release_url("https://github.com/codepawl/oh-snap/releases"), None);
    }

    #[test]
    fn sha256_of_abc() {
        assert_eq!(
            sha256_hex(b"abc").unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
