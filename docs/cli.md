# CLI Reference

## Subcommands

- `dezap tui [--bind <addr>] [--connect <peer>]`: launches the TUI (default when no subcommand is provided).
- `dezap listen --bind <addr> [--password <password>]`: starts a headless listener. This command works well for embedded deployments or scripting.
- `dezap send --to <peer> --text "message"`: opens a temporary connection, sends the message, and tears down the session.
- `dezap send-file --to <peer> --path ./file.bin`: negotiates a file offer, streams the compressed payload, and exits.

## Common Flags

- `--config <path>`: overrides the automatic config discovery and uses the provided file.
- `--verbose` / `-v`: raises the logging level across both TUI and CLI contexts.
- `--disable-discovery`: disables LAN UDP discovery when not desired.
