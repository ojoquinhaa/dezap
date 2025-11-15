# Architecture

## Crate Layout

- `src/main.rs` glues together CLI parsing and TUI runtime startup.
- `src/cli.rs` declares CLI verbs (`tui`, `listen`, `send`, `send-file`) with `clap`, wiring flags such as `--config` or `--verbose` into configuration loading.
- `src/config.rs` merges defaults, config files, and env vars, expanding paths under `~/.config/dezap` and establishing directories for downloads, history, and saved peers.
- `src/logging.rs` centralizes `tracing` subscriber setup.
- `src/net.rs` contains the QUIC/TLS bootstrap logic, discovery helpers, and TLS certificate material handling.
- `src/protocol.rs` defines the typed wire protocol (plaintext/cipherframe, control messages, file offers, metadata).
- `src/service.rs` runs the long-lived carrier: `DezapService` accepts commands, maintains state, orchestrates QUIC connections, encrypts chat via ChaCha20-Poly1305, and manages compressed file transfers with persistence hooks.
- `src/tui/` owns the terminal experience, including event handling, layout, widgets, and sharing state with the service layer.

## Runtime Flow

1. CLI/TUI code builds an `AppConfig`, configures logging, and starts `DezapService`. Default mode is TUI; non-interactive commands simply dispatch commands to the service runtime.
2. Commands (`Listen`, `Connect`, `SendText`, `SendFile`, `Discover`, `AcceptFile`, `DeclineFile`) are forwarded to the service via async channels. Events (`Connected`, `MessageReceived`, `FileOffer`, etc.) travel back on the event channel.
3. The runtime handles QUIC connections via `quinn`. Upon connection, it sends/receives handshake messages to derive a shared ChaCha key and establishes `ConnectionMeta` for symmetric encryption.
4. File transfers compress files to temporary storage, send a `FileOffer`, await a `FileAccept`, stream compressed chunks, then the recipient decompresses them and plants the final artifact where they asked.

