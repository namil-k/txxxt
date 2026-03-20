<p align="center">
  <h1 align="center">txxxt</h1>
  <p align="center">
    ASCII webcam viewer & video call in your terminal
    <br />
    <a href="https://txxxt.me">txxxt.me</a> · <a href="https://github.com/namil-k/txxxt/releases">Releases</a>
  </p>
</p>

<p align="center">
  <a href="https://github.com/namil-k/txxxt/releases/latest"><img src="https://img.shields.io/github/v/release/namil-k/txxxt?style=flat-square" alt="Latest Release"></a>
  <a href="https://github.com/namil-k/txxxt/blob/main/LICENSE"><img src="https://img.shields.io/github/license/namil-k/txxxt?style=flat-square" alt="License"></a>
  <a href="https://github.com/namil-k/txxxt/stargazers"><img src="https://img.shields.io/github/stars/namil-k/txxxt?style=flat-square" alt="Stars"></a>
</p>

---

Turn your webcam into real-time ASCII art. Call your friends — no account, just a terminal.

## Quick Start

```bash
# Install
curl -fsSL https://raw.githubusercontent.com/namil-k/txxxt/main/install.sh | bash

# Run
txxxt
```

## Features

- **ASCII webcam viewer** — 6 visual styles (blocks, dots, outline, letters, digits, standard)
- **Video call** — relay-based, works across any network
- **Audio** — mic + speaker with echo cancellation
- **Room codes** — press `r`, share a 6-char code, done
- **Shareable links** — `txxxt.me/CODE` landing page for easy joining
- **PIP layout** — FaceTime-style with movable, resizable picture-in-picture
- **Auto-update** — checks for updates on startup
- **Cross-platform** — macOS (ARM64), Linux (x86_64)

## Video Call

**Create a room:**

Press `r` in the app. A link like `txxxt.me/ABC123` is copied to your clipboard. Share it with a friend.

**Join a room:**

```bash
txxxt ABC123
```

Or press `c` in the app and type the code.

## Keybindings

### General

| Key | Action |
|-----|--------|
| `v` | Switch visual style |
| `f` | Settings (color, bg removal, mirror, brightness) |
| `,` | Preferences (save folder) |
| `y` | Save snapshot |
| `q` | Quit / hang up |

### Call

| Key | Action |
|-----|--------|
| `r` | Create relay room |
| `c` | Connect (room code) |
| `m` | Mute / unmute mic |
| `h` | Hide / show camera |
| `p` | Move PIP (cycles corners) |
| `+` / `-` | Resize PIP |

## CLI

```bash
txxxt                  # open webcam viewer
txxxt ABC123           # join room directly
txxxt update           # update to latest version
```

## Build from Source

```bash
git clone https://github.com/namil-k/txxxt.git
cd txxxt
cargo build --release
```

## Requirements

- A terminal with Unicode support
- A webcam
- macOS (ARM64) or Linux (x86_64)

## License

MIT
