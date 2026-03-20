# txxxt

ASCII webcam viewer & video call in your terminal.

Turn your webcam into real-time ASCII art. Call your friends over the network — no server, no account, just a terminal.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/kimnam1/txxxt/main/install.sh | sh
```

Or build from source:

```bash
cargo install --git https://github.com/kimnam1/txxxt
```

## Usage

```bash
txxxt
```

That's it. Your webcam opens as ASCII art in the terminal.

### Video call

From inside the app:

- Press `l` to listen — your address is copied to clipboard, send it to a friend
- Press `c` to call — paste the address your friend sent you

Or from the command line:

```bash
txxxt listen              # wait for incoming call
txxxt call 192.168.1.5:7878   # connect to a friend
```

## Keybindings

| Key | Action |
|-----|--------|
| `v` | Switch visual style (standard, letters, dots, digits, blocks, outline) |
| `f` | Settings (color, bg removal, mirror, brightness) |
| `,` | Preferences (save folder) |
| `y` | Save snapshot to file |
| `c` | Connect to a peer |
| `l` | Listen for incoming call |
| `q` | Quit (or hang up during a call) |

## Requirements

- A terminal with Unicode support
- A webcam
- macOS, Linux, or Windows

## License

MIT
