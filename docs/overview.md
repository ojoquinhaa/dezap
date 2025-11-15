# Dezap Overview

Dezap is a retro-inspired, LAN-only peer-to-peer messenger that pairs a colorful text-based UI with a headless CLI mode. It focuses on secure, low-latency communication via QUIC, layered with symmetric encryption, message framing, and a file-transfer orchestration that compresses before transmission and decrypts/decompresses on receipt.

## Design Goals

- **Retro yet modern TUI** with vibrant accent colors and manageable panels, while keeping the experience keyboard-first.
- **Fully asynchronous networking** using `tokio` and `quinn`, avoiding blocking operations so that chat, file I/O, and serialization run concurrently.
- **Configurable security** with TLS parameters, optional password gating, and in-application encrypted history + stored peer metadata.
- **Dual interface**: a TUI for interactive sessions and CLI subcommands for scripting/automation.
