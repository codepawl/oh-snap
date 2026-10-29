# Writing an oh-snap add-on

An add-on is a folder in `%APPDATA%\oh-snap\addons`. Run `oh-snap addons folder` to print the path. The folder holds an `addon.toml` and whatever the command needs.

## addon.toml

```toml
name = "Copy text"
description = "Reads the text in the snip with Windows OCR and copies it"
command = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File ocr.ps1 \"{file}\""
result = "text"
```

| Key | Meaning |
| --- | --- |
| `name` | The title in the editor's add-ons menu. Required |
| `description` | One line under the title |
| `command` | What runs. Required |
| `result` | What oh-snap does with the output. The default is `message` |

Values are TOML basic strings: double quotes, with `\"` and `\\` inside.

## The command

oh-snap replaces these words in `command` before it runs the command:

| Word | Value |
| --- | --- |
| `{file}` | The snip as a PNG, with every edit drawn in |
| `{out}` | Where an `image` add-on must write its PNG |
| `{dir}` | The add-on's own folder |

Put quotes around `{file}`, `{out}` and `{dir}`, because paths can contain spaces. The command runs through `cmd.exe` in the add-on's folder, without a window. oh-snap stops it after 60 seconds.

## The result

| `result` | What the add-on does | What oh-snap does |
| --- | --- | --- |
| `text` | Prints text to standard output | Copies the text to the clipboard |
| `image` | Writes a PNG to `{out}` | Shows that PNG in the editor in place of the snip |
| `url` | Prints an `http` or `https` link as the first line | Opens the link in the browser |
| `message` | Prints one line | Shows the line in the editor's status bar |
| `none` | Anything | Nothing |

If the command exits with a code other than 0, the editor shows the first line of its error output.

## Try it

```text
oh-snap addons add C:\path\to\my-addon
oh-snap addons run my-addon some-snip.png
```

`addons run` prints what the add-on returned and how long it took. Use it while you write an add-on. The add-ons that ship with oh-snap are in [`addons/`](../addons) as examples.
