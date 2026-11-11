//! When a capture may run. Two things hold it back: the snip delay from the settings, and our
//! own notification card. The card is hidden first, and the compositor needs a moment to
//! draw the screen without it, or the card ends up in the picture.
//!
//! The card is not excluded from capture instead: on macOS 15.4 and later ScreenCaptureKit,
//! which oh-snap captures with, is reported to still capture windows whose `sharingType` is
//! `.none` (developer.apple.com/forums/thread/792152), and X11 and Wayland have no such flag.

use std::time::Duration;

/// Long enough for the window server or compositor to drop a hidden window from the screen.
pub const CARD_SETTLE: Duration = Duration::from_millis(200);

/// How long to wait before capturing, or `None` to capture now.
///
/// `delay` is the snip delay (zero for tools that have none). `waited` is true when this
/// call comes back from an earlier wait, which already covered the delay; without it the
/// delay would be asked for again on every return and the capture would never happen.
pub fn wait_before_capture(delay: Duration, card_on_screen: bool, waited: bool) -> Option<Duration> {
    let delay = if waited { Duration::ZERO } else { delay };
    let wait = if card_on_screen { delay.max(CARD_SETTLE) } else { delay };
    (!wait.is_zero()).then_some(wait)
}

#[cfg(test)]
mod tests {
    use super::*;

    const THREE_SECONDS: Duration = Duration::from_secs(3);

    #[test]
    fn no_delay_and_no_card_captures_at_once() {
        assert_eq!(wait_before_capture(Duration::ZERO, false, false), None);
    }

    #[test]
    fn a_delayed_snip_captures_when_the_wait_comes_back() {
        // The app's loop: ask, wait what it says, ask again with `waited`.
        let mut waited = false;
        let mut waits = Vec::new();
        while let Some(wait) = wait_before_capture(THREE_SECONDS, false, waited) {
            waits.push(wait);
            waited = true;
            assert!(waits.len() < 5, "the delay keeps asking for itself");
        }
        assert_eq!(waits, vec![THREE_SECONDS]);
    }

    #[test]
    fn a_card_on_screen_is_given_time_to_leave() {
        assert_eq!(wait_before_capture(Duration::ZERO, true, false), Some(CARD_SETTLE));
        // A card that came up during the delay still gets its time.
        assert_eq!(wait_before_capture(THREE_SECONDS, true, true), Some(CARD_SETTLE));
    }

    #[test]
    fn a_long_delay_covers_the_card() {
        assert_eq!(wait_before_capture(THREE_SECONDS, true, false), Some(THREE_SECONDS));
    }
}
