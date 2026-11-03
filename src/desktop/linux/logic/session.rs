/// Which display server the user is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Session {
    X11,
    Wayland,
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.is_empty())
}

/// Wayland wins when a compositor socket is set, unless the login says x11 outright.
pub fn detect(wayland_display: Option<&str>, session_type: Option<&str>, display: Option<&str>) -> Option<Session> {
    let session_type = non_empty(session_type).map(str::to_ascii_lowercase);
    match session_type.as_deref() {
        Some("wayland") => return Some(Session::Wayland),
        Some("x11") if non_empty(display).is_some() => return Some(Session::X11),
        _ => {}
    }
    if non_empty(wayland_display).is_some() {
        Some(Session::Wayland)
    } else if non_empty(display).is_some() {
        Some(Session::X11)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_the_session() {
        assert_eq!(detect(Some("wayland-0"), Some("wayland"), Some(":0")), Some(Session::Wayland));
        assert_eq!(detect(None, Some("x11"), Some(":0")), Some(Session::X11));
        assert_eq!(detect(Some("wayland-0"), None, Some(":0")), Some(Session::Wayland));
        assert_eq!(detect(None, None, Some(":1")), Some(Session::X11));
        assert_eq!(detect(Some(""), Some("tty"), Some("")), None);
    }
}
