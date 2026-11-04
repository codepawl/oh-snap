# Contributing to oh-snap

Thanks for helping. Bug reports, small fixes and tests on macOS and Linux help the most right now.

## Report a bug

Open an issue with the bug report form. Include these:

- The oh-snap version: `oh-snap --version`.
- Your system: Windows build, macOS version, or Linux distribution with X11 or Wayland.
- What you pressed, what you expected and what happened.
- The last lines of the log, if there are any:
  - Windows: `%LOCALAPPDATA%\oh-snap\oh-snap.log`
  - macOS: `~/Library/Application Support/oh-snap/oh-snap.log`
  - Linux: `~/.local/share/oh-snap/oh-snap.log`

The log has no screen content. Remove file paths from it if you do not want to share them.

## Send a change

1. Open an issue first for a large change, so we can agree on the shape before you write it.
2. Fork the repository and make a branch from `main`.
3. Run `cargo build --release` and `cargo test --release`.
4. Open a pull request that says what changed and how you checked it.

CI builds and tests every pull request on Windows, macOS and Linux, and checks that a Linux capture works under Xvfb.

## Where the code is

- `src/` holds the Windows app. It uses Windows APIs directly: DXGI, Direct2D, Media Foundation.
- `src/desktop/` holds the macOS and Linux app. The drawing code in `src/desktop/ui/` is shared and also runs in the Windows tests.
- `src/desktop/platform.rs` is the line between that app and the system. `src/desktop/macos/` and `src/desktop/linux/` implement it.
- All three systems share the code for settings, history and PNG output.

## Style

- Use full, descriptive names. Do not abbreviate.
- Keep functions small, and put each statement on its own line.
- Write a comment only where the reason is not obvious from the code.
- Keep the binary small. Ask in an issue before you add a large dependency.

By contributing, you agree that your work is licensed under the MIT License of this repository.
