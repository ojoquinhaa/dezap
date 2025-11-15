# Networking & Protocol

## QUIC Transport

- `quinn` establishes QUIC connections with TLS 1.3; the server-side generates self-signed material when development mode is enabled, and prod setups allow loading cert/key via config.
- Local discovery happens via UDP broadcasts within the configured subnet; the service can auto-run discovery or respect CLI overrides.
- The network layer distinguishes between control and data streams: text/file/control data each go over their own unidirectional or bidirectional QUIC stream.

## Framing & Message Types

- `WireMessage` is the framed payload with variants for `Text`, `FileMeta`, `FileChunk`, `Ack`, `Control`, and encrypted `Ciphertext`.
- `ControlMessage` carries handshake info (`Hello`/`Denied`/`Info`) plus file orchestration (`FileOffer`, `FileAccept`, `FileReject`).
- File metadata tracks both compressed and original sizes so each peer can display progress and pre-approve downloads.
- Chat uses `bincode::serde` serialization over framed streams, while file chunks stream raw bytes with an explicit `last` flag.

## E2E Encryption

- After QUIC/TLS handshake, we perform a Diffie-Hellman exchange using `x25519-dalek` to derive a ChaCha20-Poly1305 key.
- Each text payload is encrypted with a fresh nonce before being wrapped in `WireMessage::Ciphertext`.
- File chunks are already compressed and transported inside QUIC; their confidentiality is secured by the QUIC/TLS channel and the optional handshake-level file acceptance.
