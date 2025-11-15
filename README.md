# dezap

Dezap is a secure, LAN-only peer-to-peer messenger with a retro-styled terminal interface and headless CLI. It uses QUIC for all traffic, compresses files before transport, and stores encrypted chat history alongside saved peer metadata for long-term auditability.

## Highlights

- End-to-end encrypted chat over QUIC with ChaCha20-Poly1305 wrapping and live status updates.
- Interactive TUI built on `ratatui` with ASCII art header, configurable accent colors, chat browsing, clipboard copy, and file autocompletion.
- Dual-mode CLI (`tui`, `listen`, `send`, `send-file`) plus persistent config, discovery filtering, and logging hooks.
- File transfers compress before sending, offer dialogs on the recipient, and stream progress via `ServiceEvent::FileTransfer`.
- History files are gzip-compressed, encrypted, and stored per peer; saved peer metadata is maintained in `peers.json`.

## Getting Started

```bash
cargo build
cargo run      # launches the TUI (same as `cargo run -- tui`)
```

The TUI default keybindings:

| Binding        | Description                                  |
|----------------|----------------------------------------------|
| `Enter`        | Send message / confirm dialogs                |
| `Ctrl+K`       | Connect to peer                              |
| `Ctrl+L`       | Start listener                               |
| `Ctrl+F`       | Send file (with Tab-based autocomplete)       |
| `Ctrl+D`       | Discover peers                               |
| `Ctrl+G`       | Browse chat history (`↑`/`↓`, `c` copies)     |
| `Ctrl+X`       | Disconnect current peer                      |
| `Esc`          | Close dialog (or decline incoming file)       |
| `Tab`          | Toggle help or autocomplete (contextual)      |

Incoming file offers pre-fill the download directory; edit the path, press `Enter` to save, or `Esc` to decline. Queued offers display status messages in the help window.

## CLI Usage

```
dezap tui [--bind <addr>] [--connect <peer>]
dezap listen --bind <addr> [--password <secret>]
dezap send --to <peer> --text "hello"
dezap send-file --to <peer> --path ./archive.zip
```

Use `--config` to point to a custom `config.toml`, `-v/--verbose` to change logging, and `--disable-discovery` when broadcasts are not allowed.

## Configuration

Dezap merges defaults, `$XDG_CONFIG_HOME/dezap/config.toml`, CLI overrides, and `DEZAP__` environment variables. `docs/configuration.md` describes all sections (`listen`, `identity`, `paths`, `limits`, `tls`, `ui`, `discovery`).

## Documentation

- `docs/overview.md`: high-level goals and runtime flow.
- `docs/architecture.md`: crate layout and service responsibilities.
- `docs/network.md`: QUIC framing, control messages, and encryption.
- `docs/tui.md`: interface layout, navigation, and shortcuts.
- `docs/cli.md`: CLI verbs and flags.
- `docs/configuration.md`: config structure.
- `docs/security.md`: TLS, encryption, persistence, and file transfer details.

## Testing

```bash
cargo test
```

The repo includes unit tests for the protocol and TUI state; integration/network tests are marked `ignore` due to socket permissions.

## Licensing & Contributions

Contributions are welcome. Please follow the code style already established, keep encrypted artifacts out of git (`history.key`, TLS material), and document new behaviors that affect the TUI or networking in `docs/`.

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
| `Ctrl+P`       | Focus discovered peers (Enter to connect) |
| `Ctrl+S`       | Focus saved peers (Enter to connect) |
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
