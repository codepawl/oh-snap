<p align="center"><img src="assets/oh-snap.png" width="96" alt="oh-snap logo"></p>

# oh-snap

A fast screen snipping tool for Windows, written in Rust. It replaces Snipping Tool for PrtSc, Win+Shift+S and `ms-screenclip:` links. The program is one file of about 1.3 MB.

![The oh-snap overlay over a desktop](docs/overlay.png)

## Install

Run this line in PowerShell. You do not need admin rights.

```powershell
irm https://raw.githubusercontent.com/codepawl/oh-snap/main/install.ps1 | iex
```

The installer puts oh-snap in `%LOCALAPPDATA%\Programs\oh-snap`, adds it to your PATH and the Start menu, and starts it at sign-in. Run the same line again to update by hand. oh-snap also updates itself, see [Updates](#commands). To give the shortcuts back to Snipping Tool, run `oh-snap uninstall`.

You can also download `oh-snap.exe` from [GitHub Releases](https://github.com/codepawl/oh-snap/releases) and run `oh-snap install`, or build it with Cargo:

```text
cargo install --git https://github.com/codepawl/oh-snap
oh-snap install
```

oh-snap runs on Windows 10 and 11. macOS and Linux versions are coming.

## Use

| Action | Result |
| --- | --- |
| PrtSc or Win+Shift+S | Opens the overlay |
| Drag | Snips a region |
| Click | Snips the window under the pointer. In `screen` mode, it snips the monitor |
| `r` `w` `s` | Switches to region, window or screen mode |
| Arrow keys | Move the pointer 1 px. Hold Shift to move 10 px |
| Enter | Snips what is highlighted, or the whole monitor |
| Esc or right click | Cancels |

While you drag, bits of the screen break off the pointer like mined blocks and fall away.

After you pick an area, oh-snap asks if you want to keep it. Press Enter to save, `e` to edit first, `r` to take it again, or Esc to cancel. If you want speed, tick "don't ask again" and every snip saves at once.

![The dialog that asks to keep the snip](docs/confirm.png)

## Settings

Open the settings from the tray menu or with `oh-snap settings`. A change saves at once.

![The settings window](docs/settings.png)

| Group | Settings |
| --- | --- |
| Capture | Default mode, delay of 3, 5 or 10 seconds, ask before saving, snap to windows, loupe, crosshair |
| Effects | Drag particles: while dragging, at the start and end of the drag, or off. Shutter sound |
| Shortcuts | PrtSc, Win+Shift+S, Win+Shift+R |
| After capture | Copy to clipboard, save to folder, folder, PNG or JPG, result card and how long it stays, recent snips, open the editor |
| Recording | Frame rate of 30 or 60, system sound, microphone |
| System | Start at sign-in, updates: install on their own, only tell you, or off. `check now` looks at once |

The settings live in `%APPDATA%\oh-snap\settings.toml`.

A snip plays a short shutter sound. It goes to the clipboard as a bitmap and as PNG, and to `Pictures\Screenshots`, the folder that Windows and Photos use. A card in the corner shows the snip and the file name. The ring in its corner empties until the card closes, and stops while the pointer is on the card. Click the card to open the image. Click `edit` to draw on it, or `folder` to show the file in Explorer. Right-click the image on the card or in the "keep this snip?" dialog to copy it, save a copy somewhere else, or edit it.

oh-snap saves without asking. Turn on "ask before saving" to get a "keep this snip?" dialog first. The snip is on the clipboard as soon as you release the mouse, also with the dialog on, which only decides about the file.

![The card that confirms a snip](docs/card.png)

## Edit

The editor opens from the card, from `oh-snap edit FILE`, or from "Edit with oh-snap" when you right-click a PNG in Explorer.

![The editor with an arrow, a box, a highlight, text and a hidden area](docs/editor.png)

| Key | Tool |
| --- | --- |
| `a` | Arrow |
| `b` | Box |
| `p` | Pen |
| `m` | Highlighter |
| `t` | Text. Click, type, then press Enter. Shift+Enter starts a new line |
| `h` | Hide: pixelates the area, for passwords and names |
| `c` | Crop |
| `1` to `5` | Colour |

Ctrl+Z undoes and Ctrl+Y redoes. Ctrl+C copies the edited snip and Ctrl+S saves it over its file. If you close the window with unsaved edits, oh-snap asks first.

## Add-ons

An add-on is a folder with an `addon.toml` and any program. oh-snap gives it the snip as a PNG. The add-on gives back text for the clipboard, a new image, a link to open, or a message. Run one from the `add-ons` menu in the editor.

Two add-ons come with oh-snap:

- **Copy text** reads the text in the snip with the OCR engine that ships with Windows. Nothing is downloaded.
- **Copy as data URI** copies the snip as a `data:image/png;base64,` link for HTML, CSS or Markdown.

To write your own, see [docs/addons.md](docs/addons.md).

## Compared with Snipping Tool

Measured with `oh-snap bench compare 6` on Windows 11 build 26300, two 1920x1080 monitors, an RTX 3060 and Snipping Tool 11.2608.41.0. Both tools start from a command: oh-snap from `oh-snap capture`, Snipping Tool from `ms-screenclip:`. "Screen dims" is the time until the screen image itself gets darker.

| | oh-snap | Snipping Tool |
| --- | --- | --- |
| Overlay window shows (median) | 20 ms | 366 ms |
| Screen dims (median) | 51 ms | 387 ms |
| First snip after start | 81 ms | 1033 ms |
| Memory while in use, working set | 84 MB in 1 process | 392 MB in 2 processes |
| Memory while waiting, working set | 0.5 MB | not running |
| Size on disk | 1.3 MB | 586 MB |

The speed numbers are from version 0.10.2, while other programs used about half of the CPU and most of the GPU. The memory while in use and the size of Snipping Tool are older measurements. Run the same test on your computer with `oh-snap bench compare`. It opens and closes both overlays a few times.

## Commands

```text
oh-snap                      start in the tray
oh-snap install              install for this user and take the shortcuts
oh-snap uninstall            give the shortcuts back to Snipping Tool
oh-snap capture [region|window|screen] [--rect X,Y,W,H] [--out FILE] [--no-clipboard] [--no-save]
oh-snap capture last         snip the last area again (needs the tray process)
oh-snap edit FILE            open a PNG in the editor
oh-snap addons [list]        show the add-ons
oh-snap addons add FOLDER    copy an add-on folder in
oh-snap addons remove NAME   delete an add-on
oh-snap addons run NAME FILE run an add-on on a PNG and print its result
oh-snap addons folder        print where add-ons live
oh-snap bench [ROUNDS]       measure capture and overlay speed
oh-snap bench compare [N]    race Snipping Tool
oh-snap preview DIR          draw the overlay, the card and the editor into PNG files
oh-snap logo DIR             write the logo as SVG, PNG and ICO
oh-snap settings             open the settings
oh-snap update               install the latest release from GitHub
oh-snap text FILE            print the QR code or the text in a PNG
oh-snap record --rect X,Y,W,H [--seconds N] [--fps N] [--no-sound] [--microphone] [--out FILE]
oh-snap gif FILE             make a GIF from a recording
oh-snap quit                 stop the tray process
```

Press `D` on the overlay to draw before you keep the snip. Pick an area, a window or a screen, then use the bar next to it: arrow, box, pen, marker, text and hide, in five colours. Ctrl+Z undoes a step. Enter keeps the snip with your drawing, and Esc drops it. The drawing tools are the same as in the editor.

Press Win+Shift+R, or `V` on the overlay, to record video. Pick an area, a window or a screen. After a 3 second countdown, a small bar shows the time with pause and stop, and a thin frame marks the area. Windows keeps both out of the video. Press Win+Shift+R again or click the tray icon to stop. The MP4 goes to `Videos\Screen Recordings`, with the pointer and the sound that your computer plays. Turn on `Microphone` in the settings to add your voice. The card can open the file, show it in Explorer, or make a GIF next to it. You can drag the card's image into another app.

Press `T` on the overlay to read text instead of keeping an image. Pick an area, a window or a screen, and the text goes to the clipboard. A QR code goes to the clipboard the same way, and when it holds a web link, a click on the card opens it. It uses the OCR engine in Windows, so it needs no download and takes about 100 ms.

Press `C` on the overlay to copy the colour under the pointer as a hex value. The card also shows it as RGB and HSL.

Press Shift+PrtSc to snip the last area again without the overlay, for before and after shots.

The tray menu lists your last 10 snips under `recent snips`. Picking one puts it back on the clipboard and shows its card. For this, oh-snap keeps the last 20 snips in `%LOCALAPPDATA%\oh-snap\history`, also when saving to a folder is off. Set `Recent snips` to `off` in the settings to keep none and delete those copies, or pick `clear` to delete them once. `oh-snap uninstall` deletes them too.

Drag the image on the card into a chat, a browser or Explorer to drop the file there.

Right-click a snip and pick `pin to screen` to keep it on top of every window while you work. Drag the pin to move it, scroll to zoom, and Ctrl+scroll to make it see-through. Ctrl+C copies it, and Esc or a double-click closes it.

oh-snap updates itself from GitHub Releases. The tray process looks for a new version 10 seconds after it starts and then every 6 hours. It downloads the new `oh-snap.exe`, checks it against the release's `SHA256SUMS.txt`, runs it once to see that it starts, and swaps it in between snips, never in the middle of one. If a check fails, the working version stays. A card then shows the new version. Set `Updates` to `notify` to get only the card, or to `off`.

Other apps and scripts can start a snip with `oh-snap capture` or an `oh-snap://capture/window` link.

## Build

```text
cargo build --release
```

On the `x86_64-pc-windows-gnu` toolchain, `windows-rs` needs a `dlltool` that works. The one that Rust ships needs `as.exe`, which is not included. Run `rustup component add llvm-tools`, then put a copy of `llvm-ar.exe` named `dlltool.exe` on PATH. The MSVC toolchain does not need this step.

## How it works

- `capture.rs` keeps one DXGI Desktop Duplication for each monitor. It keeps the latest frame in video memory, so a snip reads back only once.
- `overlay.rs` draws the frozen desktop with Direct2D at 1 unit for 1 physical pixel. The process is per-monitor DPI aware, so mixed-DPI setups line up.
- `daemon.rs` uses a low-level keyboard hook to get Win+Shift+S and PrtSc before Explorer does. Win+PrtSc and Alt+PrtSc stay with Windows.
- `card.rs` shows the result without taking the focus. `sound.rs` makes the shutter sound in code, so no audio file ships.
- `editor.rs` and `annotate.rs` keep the edits as shapes in image pixels. The window and the export use the same drawing code.
- `addons.rs` runs each add-on as its own process without a window, and stops it after 60 seconds.
- `updater.rs` finds the latest tag from where `releases/latest` redirects, so it does not use the GitHub API and its limit of 60 calls an hour. HTTPS goes through WinHTTP and SHA-256 through CNG, both part of Windows.
- `recorder.rs` takes frames from the same capture device, draws the pointer with GDI, encodes H.264 and AAC with Media Foundation, and gets the system sound from WASAPI loopback. All of these ship with Windows, so the program stays small. `gif_export.rs` decodes the MP4 with Media Foundation and keeps only the pixels that change.
- `install.rs` writes only to `HKCU` and keeps the old PrtSc setting, so `uninstall` can restore it.

## Known limits

- The tray process keeps its capture device open so the overlay is fast. Thirty seconds after the last snip it gives its memory back to Windows: Task Manager shows about 0.3 MB while it waits. The first snip after that takes about 20 ms longer.
- The release file is not code-signed yet. Smart App Control or SmartScreen can block it on some computers.
- Windows does not let a normal process get keys while an elevated window has focus. In that case, use the tray icon.
- Protected video (DRM) shows as black, and the UAC prompt cannot be captured.
- Rotated monitors and HDR monitors are not tested yet.

## Next

- Add-ons that run after every snip, not only from the editor.
- Trimming for recordings.
- macOS and Linux versions, which are coming.

## License

MIT
