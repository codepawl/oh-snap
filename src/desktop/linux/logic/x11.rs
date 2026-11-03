//! X11 numbers and texts that need no connection.

pub const SHIFT: u16 = 1;
pub const LOCK: u16 = 2;
pub const CONTROL: u16 = 4;
pub const NUM_LOCK: u16 = 16;
pub const KEYSYM_PRINT: u32 = 0xff61;

/// Every combination of the lock keys, so a grab still fires with Caps Lock or Num Lock on.
pub fn with_lock_variants(modifiers: u16) -> [u16; 4] {
    [modifiers, modifiers | LOCK, modifiers | NUM_LOCK, modifiers | LOCK | NUM_LOCK]
}

pub fn without_locks(state: u16) -> u16 {
    state & !(LOCK | NUM_LOCK)
}

/// The keycode that produces `keysym` in any column, from a GetKeyboardMapping reply.
pub fn keycode_for_keysym(min_keycode: u8, keysyms_per_keycode: u8, keysyms: &[u32], target: u32) -> Option<u8> {
    let width = keysyms_per_keycode as usize;
    if width == 0 {
        return None;
    }
    keysyms
        .chunks(width)
        .position(|row| row.contains(&target))
        .and_then(|row| u8::try_from(row + min_keycode as usize).ok())
}

/// A window's outer rectangle: its inner one grown by _NET_FRAME_EXTENTS (left, right, top, bottom).
pub fn with_frame_extents(x: i32, y: i32, width: u32, height: u32, extents: [u32; 4]) -> (i32, i32, u32, u32) {
    let [left, right, top, bottom] = extents;
    (x - left as i32, y - top as i32, width + left + right, height + top + bottom)
}

/// `Xft.dpi: 144` in the RESOURCE_MANAGER text to a scale factor against 96 dpi, never below 1.
pub fn scale_from_resources(resources: &str) -> f32 {
    let dpi = resources
        .lines()
        .find_map(|line| line.strip_prefix("Xft.dpi:"))
        .and_then(|value| value.trim().parse::<f32>().ok());
    match dpi {
        Some(dpi) if dpi > 96.0 => ((dpi / 96.0) * 4.0).round() / 4.0,
        _ => 1.0,
    }
}

/// Turns the unused byte of every 32-bit BGRX pixel into an opaque alpha.
pub fn make_opaque(bgrx: &mut [u8]) {
    for pixel in bgrx.chunks_exact_mut(4) {
        pixel[3] = 255;
    }
}

/// What the clipboard owner offers.
pub enum Payload {
    Text(String),
    Png(Vec<u8>),
}

pub fn offered_targets(payload: &Payload) -> &'static [&'static str] {
    match payload {
        Payload::Text(_) => &["UTF8_STRING", "STRING", "text/plain;charset=utf-8", "text/plain"],
        Payload::Png(_) => &["image/png"],
    }
}

pub fn bytes_for<'a>(payload: &'a Payload, target: &str) -> Option<&'a [u8]> {
    if !offered_targets(payload).contains(&target) {
        return None;
    }
    match payload {
        Payload::Text(text) => Some(text.as_bytes()),
        Payload::Png(bytes) => Some(bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_variants_cover_caps_and_num_lock() {
        assert_eq!(with_lock_variants(SHIFT), [1, 3, 17, 19]);
        assert_eq!(without_locks(19), 1);
    }

    #[test]
    fn finds_keycodes() {
        // keycodes 8, 9, 10 with two columns each
        let keysyms = [1, 2, 0xff61, 0, 5, 6];
        assert_eq!(keycode_for_keysym(8, 2, &keysyms, KEYSYM_PRINT), Some(9));
        assert_eq!(keycode_for_keysym(8, 2, &keysyms, 99), None);
        assert_eq!(keycode_for_keysym(8, 0, &keysyms, 1), None);
    }

    #[test]
    fn grows_by_frame_extents() {
        assert_eq!(with_frame_extents(10, 40, 200, 100, [1, 1, 30, 1]), (9, 10, 202, 131));
    }

    #[test]
    fn reads_xft_dpi() {
        assert_eq!(scale_from_resources("*font: x\nXft.dpi:\t144\n"), 1.5);
        assert_eq!(scale_from_resources("Xft.dpi: 96"), 1.0);
        assert_eq!(scale_from_resources("nothing"), 1.0);
    }

    #[test]
    fn serves_only_offered_targets() {
        let text = Payload::Text("hi".into());
        assert_eq!(bytes_for(&text, "UTF8_STRING"), Some(&b"hi"[..]));
        assert_eq!(bytes_for(&text, "image/png"), None);
        let png = Payload::Png(vec![1, 2]);
        assert_eq!(bytes_for(&png, "image/png"), Some(&[1u8, 2][..]));
    }

    #[test]
    fn opaque_alpha() {
        let mut pixels = [1, 2, 3, 0, 4, 5, 6, 0];
        make_opaque(&mut pixels);
        assert_eq!(pixels, [1, 2, 3, 255, 4, 5, 6, 255]);
    }
}
