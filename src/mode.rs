//! What a snip picks, and what the overlay does with the pick. Shared by every OS.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Region,
    Window,
    Screen,
    Freeform,
}

impl Mode {
    pub fn parse(text: &str) -> Option<Mode> {
        match text.to_ascii_lowercase().as_str() {
            "region" | "rect" | "rectangle" => Some(Mode::Region),
            "window" => Some(Mode::Window),
            "screen" | "fullscreen" | "monitor" => Some(Mode::Screen),
            "freeform" | "free" => Some(Mode::Freeform),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Region => "region",
            Mode::Window => "window",
            Mode::Screen => "screen",
            Mode::Freeform => "freeform",
        }
    }
}

/// What the overlay does with the pick. Text works with every mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Snip,
    Text,
    Color,
    Record,
    Draw,
}
