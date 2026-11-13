//! Start at sign-in, the app bundle's place on disk, the system shortcuts and updates.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::pure;
use crate::desktop::platform::SetUp;

fn run(program: &str, arguments: &[String]) -> Result<String, String> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .map_err(|error| format!("could not run {program}: {error}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(format!("{program} failed: {reason}"))
}

fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| "no home folder".to_string())
}

fn running_bundle() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|error| format!("could not find the running program: {error}"))?;
    pure::bundle_of_executable(&executable)
        .ok_or_else(|| "oh-snap runs from oh-snap.app: unzip oh-snap-macos.zip and open the app inside".to_string())
}

fn is_applications_folder(bundle: &Path, home: &Path) -> bool {
    let folder = bundle.parent();
    folder == Some(Path::new("/Applications")) || folder == Some(home.join("Applications").as_path())
}

/// The graphical session of this user, as launchctl names it.
fn launchd_domain() -> String {
    format!("gui/{}", unsafe { libc::getuid() })
}

/// Tells the system to read the changed shortcut settings now.
fn apply_shortcut_changes() {
    let activate = "/System/Library/PrivateFrameworks/SystemAdministration.framework/Resources/activateSettings";
    let _ = run(activate, &["-u".to_string()]);
}

fn set_system_shortcuts(enabled: bool) -> Result<(), String> {
    for shortcut in pure::SYSTEM_SHORTCUTS {
        run("/usr/bin/defaults", &pure::shortcut_defaults_arguments(shortcut, enabled))?;
    }
    apply_shortcut_changes();
    Ok(())
}

/// Whether `install` has run: the sign-in agent is there.
pub fn is_installed() -> bool {
    home().is_ok_and(|home| pure::launch_agent_path(&home).exists())
}

pub fn install() -> Result<String, String> {
    install_bundle().map(|(_, report)| report.join("\n"))
}

/// `install` from the app's setup card. When the app ran from somewhere else, such as the disk
/// image, the copy in Applications must take over: it starts once this process has quit, so
/// the two never fight over the hotkeys and the socket.
pub fn set_up() -> Result<SetUp, String> {
    let running = running_bundle()?;
    let (installed, _) = install_bundle()?;
    if installed == running {
        return Ok(SetUp::Done);
    }
    let script = format!(
        "while /bin/kill -0 {process} 2>/dev/null; do /bin/sleep 0.2; done; /bin/launchctl kickstart {domain}/{label}",
        process = std::process::id(),
        domain = launchd_domain(),
        label = pure::BUNDLE_ID,
    );
    Command::new("/bin/sh")
        .args(["-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not start the installed app: {error}"))?;
    Ok(SetUp::Restart)
}

/// Puts the app in place, starts it at sign-in and takes the shortcuts. Returns where the app
/// now is and what was done.
fn install_bundle() -> Result<(PathBuf, Vec<String>), String> {
    let home = home()?;
    let mut report = Vec::new();

    let bundle = running_bundle()?;
    let installed = if is_applications_folder(&bundle, &home) {
        bundle.clone()
    } else {
        let folder = home.join("Applications");
        std::fs::create_dir_all(&folder).map_err(|error| format!("could not make {}: {error}", folder.display()))?;
        let target = folder.join(pure::APP_NAME);
        let _ = std::fs::remove_dir_all(&target);
        run("/usr/bin/ditto", &[bundle.to_string_lossy().to_string(), target.to_string_lossy().to_string()])?;
        report.push(format!("copied the app to {}", target.display()));
        target
    };

    let agent = pure::launch_agent_path(&home);
    if let Some(folder) = agent.parent() {
        std::fs::create_dir_all(folder).map_err(|error| format!("could not make {}: {error}", folder.display()))?;
    }
    let plist = pure::launch_agent_plist(&pure::executable_in_bundle(&installed));
    std::fs::write(&agent, plist).map_err(|error| format!("could not write {}: {error}", agent.display()))?;
    let domain = launchd_domain();
    // Reload so a changed plist is read; the first bootout fails when nothing was loaded.
    let _ = run("/bin/launchctl", &["bootout".into(), format!("{domain}/{}", pure::BUNDLE_ID)]);
    run("/bin/launchctl", &["bootstrap".into(), domain, agent.to_string_lossy().to_string()])?;
    report.push(format!("starts at sign-in ({})", agent.display()));

    set_system_shortcuts(false)?;
    report.push("turned off the system screenshot shortcuts (Shift+Cmd+3, 4 and 5, and the clipboard ones)".into());
    Ok((installed, report))
}

pub fn uninstall() -> Result<String, String> {
    let home = home()?;
    let mut report = Vec::new();

    let _ = run("/bin/launchctl", &["bootout".into(), format!("{}/{}", launchd_domain(), pure::BUNDLE_ID)]);
    let agent = pure::launch_agent_path(&home);
    if agent.exists() {
        std::fs::remove_file(&agent).map_err(|error| format!("could not remove {}: {error}", agent.display()))?;
        report.push("no longer starts at sign-in".to_string());
    }

    set_system_shortcuts(true)?;
    report.push("turned the system screenshot shortcuts back on".to_string());

    let copy = home.join("Applications").join(pure::APP_NAME);
    if copy.exists() {
        std::fs::remove_dir_all(&copy).map_err(|error| format!("could not remove {}: {error}", copy.display()))?;
        report.push(format!("removed {}", copy.display()));
    }
    Ok(report.join("\n"))
}

/// Replaces the installed bundle with the one inside the downloaded zip. The old bundle
/// stays until the new one is in place, and comes back if the swap fails.
pub fn install_update(archive: &[u8]) -> Result<(), String> {
    let installed = running_bundle()?;
    let staging = pure::staging_dir(&installed);
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|error| format!("could not make {}: {error}", staging.display()))?;
    let result = swap_in(archive, &installed, &staging);
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn swap_in(archive: &[u8], installed: &Path, staging: &Path) -> Result<(), String> {
    let zip = staging.join(pure::RELEASE_ASSET);
    std::fs::write(&zip, archive).map_err(|error| format!("could not save the update: {error}"))?;
    let unpacked = staging.join("unpacked");
    let extract = ["-x".to_string(), "-k".to_string(), zip.to_string_lossy().to_string(), unpacked.to_string_lossy().to_string()];
    run("/usr/bin/ditto", &extract)?;
    let fresh = unpacked.join(pure::APP_NAME);
    if !pure::executable_in_bundle(&fresh).exists() {
        return Err("the update does not contain oh-snap.app".into());
    }
    let verify = ["--verify".to_string(), "--deep".to_string(), fresh.to_string_lossy().to_string()];
    run("/usr/bin/codesign", &verify).map_err(|error| format!("the new app is not signed properly: {error}"))?;

    let old = pure::old_bundle_path(installed);
    let _ = std::fs::remove_dir_all(&old);
    std::fs::rename(installed, &old).map_err(|error| format!("could not move the running app aside: {error}"))?;
    if let Err(error) = std::fs::rename(&fresh, installed) {
        // Put the working app back before reporting.
        let _ = std::fs::rename(&old, installed);
        return Err(format!("could not put the new app in place: {error}"));
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

pub fn open(path: &Path) {
    let _ = Command::new("/usr/bin/open").arg(path).spawn();
}

pub fn reveal(path: &Path) {
    let _ = Command::new("/usr/bin/open").arg("-R").arg(path).spawn();
}
