//! The right-click menu on a snip shown in a window: copy it, save a copy somewhere
//! else, open it in the editor or pin it on top of everything.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FileSaveDialog, IFileSaveDialog, IShellItem, SHCreateItemFromParsingName,
    SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, MF_STRING, SetForegroundWindow, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TrackPopupMenu,
};
use windows::core::{HSTRING, w};

use crate::output::{self, Image};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageAction {
    Copy,
    SaveAs,
    Edit,
    Pin,
    /// Closes the window the menu belongs to, such as a pin.
    Close,
}

impl ImageAction {
    fn label(self) -> &'static str {
        match self {
            ImageAction::Copy => "copy image",
            ImageAction::SaveAs => "save as…",
            ImageAction::Edit => "edit",
            ImageAction::Pin => "pin to screen",
            ImageAction::Close => "close",
        }
    }
}

/// What a snip on a card or in the dialog offers.
pub const SNIP_ACTIONS: [ImageAction; 4] = [ImageAction::Copy, ImageAction::SaveAs, ImageAction::Edit, ImageAction::Pin];

/// Shows the menu at the cursor and returns what was picked.
pub fn show(owner: HWND, actions: &[ImageAction]) -> Option<ImageAction> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        for (index, action) in actions.iter().enumerate() {
            let _ = AppendMenuW(menu, MF_STRING, index + 1, &HSTRING::from(action.label()));
        }
        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        // The menu only closes on an outside click when its owner is foreground.
        let _ = SetForegroundWindow(owner);
        let chosen = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, cursor.x, cursor.y, None, owner, None);
        let _ = DestroyMenu(menu);
        let index = (chosen.0 as usize).checked_sub(1)?;
        actions.get(index).copied()
    }
}

/// Puts the snip on the clipboard, logging a failure instead of showing it.
pub fn copy(owner: HWND, image: &Image) {
    let copied = output::encode_png(image).map_err(|error| error.to_string()).and_then(|png_bytes| {
        output::copy_to_clipboard(Some(owner), image, &png_bytes).map_err(|error| error.message())
    });
    if let Err(error) = copied {
        crate::log::write(&format!("copy from menu failed: {error}"));
    }
}

/// Asks where to save a copy and writes it as PNG or JPG, following the chosen file type.
/// Returns the file it wrote, or nothing when the user closed the dialog.
pub fn save_as(owner: HWND, image: &Image, folder: &Path) -> Option<PathBuf> {
    let path = ask_save_path(owner, folder)?;
    let is_jpeg = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg"));
    let bytes = if is_jpeg { output::encode_jpeg(image) } else { output::encode_png(image) };
    match bytes.and_then(|bytes| output::write_png_to(&bytes, &path)) {
        Ok(()) => Some(path),
        Err(error) => {
            crate::log::write(&format!("save as {} failed: {error}", path.display()));
            None
        }
    }
}

fn ask_save_path(owner: HWND, folder: &Path) -> Option<PathBuf> {
    let file_types = [
        COMDLG_FILTERSPEC {
            pszName: w!("PNG image"),
            pszSpec: w!("*.png"),
        },
        COMDLG_FILTERSPEC {
            pszName: w!("JPG image"),
            pszSpec: w!("*.jpg;*.jpeg"),
        },
    ];
    unsafe {
        let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let options = dialog.GetOptions().ok()?;
        dialog.SetOptions(options | FOS_FORCEFILESYSTEM | FOS_OVERWRITEPROMPT).ok()?;
        dialog.SetFileTypes(&file_types).ok()?;
        // With a default extension the dialog adds .png or .jpg from the chosen type.
        dialog.SetDefaultExtension(w!("png")).ok()?;
        dialog.SetFileName(&HSTRING::from(output::screenshot_file_stem())).ok()?;
        if let Ok(start_folder) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(folder.as_os_str()), None) {
            let _ = dialog.SetFolder(&start_folder);
        }
        dialog.Show(Some(owner)).ok()?;
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        path.map(PathBuf::from)
    }
}
