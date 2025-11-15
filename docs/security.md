# Security & Persistence

- **TLS**: QUIC communications rely on TLS 1.3. Development mode issues self-signed certificates on the fly while production setups can load PEM files via config.
- **End-to-end encryption**: After establishing QUIC, peers exchange X25519 public keys and derive a ChaCha20-Poly1305 key. All text messages are encrypted before being sent.
- **Password protection**: Listening mode can require a password. If a peer provides the wrong password, the connection is denied immediately.
- **Encrypted history**: Chat history is compressed with gzip, encrypted with ChaCha20-Poly1305, and stored per-peer in the configured history directory. The key material is persisted in `history.key`.
- **Saved peers**: Peer metadata is stored in `peers.json`, sorted for deterministic display, and refreshed on each successful handshake.
- **File transfer**: Files are compressed before transmission; recipients must explicitly accept and choose a save path. Transfers provide live progress updates and resume only once the counterpart approves.
