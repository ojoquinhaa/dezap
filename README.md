# dezap

Dezap is a retro-flavored, LAN-first peer-to-peer messenger and file transfer tool. It couples a colorful TUI with a CLI so you can chat or automate workflows entirely over QUIC.

## Features

- QUIC transport (via `quinn`) with multiplexed streams for chat and file data
- Auto-generated self-signed TLS for local development with optional hardened mode
- Retro terminal UI powered by `ratatui` + `crossterm`
- Peer discovery over UDP broadcast (toggleable)
- Scriptable CLI for one-off sends or running listeners
- Structured logging with `tracing`
- Configurable limits, download directory, themes, and TLS paths

## Building

```bash
cargo build
```

Run the default TUI:

```bash
cargo run
# or explicitly
cargo run -- tui
```

## TUI Walkthrough

1. Start dezap with `cargo run` (or `dezap tui` once installed).
2. Press `Ctrl+L` to set a listen address (defaults to `0.0.0.0:5000`).
3. On another machine, press `Ctrl+K` and enter the listener's address to connect.
4. Type messages and hit `Enter` to send.
5. Hit `Ctrl+F` to open the file prompt, type a path, and press `Enter` to start a transfer.

### Keybindings

| Binding        | Action                               |
|----------------|--------------------------------------|
| `Enter`        | Submit message / form                |
| `Ctrl+K`       | Connect to a peer                    |
| `Ctrl+L`       | Start/stop listener                  |
| `Ctrl+F`       | Send a file                          |
| `Ctrl+D`       | Trigger peer discovery               |
| `Tab`          | Toggle help overlay                  |
| `Esc`          | Close dialogs                        |
| `Ctrl+C` / `q` | Quit and close connections           |

## CLI

```
dezap listen --bind 0.0.0.0:5000           # headless listener
dezap send --to 192.168.0.10:5000 --text "hello"
dezap send-file --to 192.168.0.10:5000 --path ./archive.tar.gz
dezap tui --bind 0.0.0.0:5000 --connect 192.168.0.42:5000
```

Use `-v/--verbose` to raise logging verbosity and `--config` to point at another TOML file.

## Configuration

Dezap loads configuration from:

1. Built-in defaults
2. `$XDG_CONFIG_HOME/dezap/config.toml` (or `%APPDATA%` on Windows)
3. `--config <path>`
4. Environment variables with the `DEZAP__` prefix

Example `config.toml`:

```toml
[listen]
bind_addr = "0.0.0.0:5000"

[peer]
default_peer = "192.168.0.42:5000"

[paths]
download_dir = "~/dezap/downloads"
chat_log = "~/dezap/chat.log"

[limits]
max_message_bytes = 16384
max_file_bytes = 1073741824
chunk_size_bytes = 65536

[tls]
cert_path = "./certs/cert.pem"
key_path = "./certs/key.pem"
insecure_local = true

[logging]
level = "info"

[ui]
accent = "cyan"

[discovery]
enabled = true
port = 54095
response_ttl_ms = 2000
```

## Known Limitations / Future Ideas

- Resumeable file transfers are stubbed but not yet implemented.
- Discovery uses simple UDP broadcast and may not work across all segments.
- CLI listener currently runs until Ctrl+C; optional timers or daemonization are future work.
- Certificate pinning per peer is not yet exposed in the UI.

Contributions are welcome!
