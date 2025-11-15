# Configuration

Dezap merges defaults, TOML config files, and environment variables (prefixed with `DEZAP__`). The default config lives under `~/.config/dezap/config.toml` (if present).

Key sections:

- `listen`: default bind address and optional password for incoming peers.
- `peer`: default peers that the TUI will attempt to connect to on launch.
- `identity`: the local username shown to peers and stored in history logs.
- `paths`: directories for downloads, encrypted history, chat logs, and saved peers. Paths support `~` expansion.
- `limits`: global caps for message lengths, file size, and chunk size.
- `tls`: certificate/key overrides, insecure-local toggles, and server name for TLS validation.
- `ui`: color preferences and optional theme overrides.
- `discovery`: UDP discovery controls, including broadcast address and whether discovery runs automatically.
