# ULTRA relay (friends & chat)

A small Rust server that ULTRAs sign in to with their ULTRA ID. Friends and chat go by
**@handle**. Messages are end-to-end encrypted by ULTRA itself (X25519 between the two ID keys,
XChaCha20-Poly1305), so the relay only stores and passes on ciphertext, and only between
friends. Each message is deleted as soon as the recipient's ULTRA confirms it arrived.

- **Sign-in:** challenge/response with the ULTRA ID's ed25519 key, no passwords. The first
  sign-in with a handle claims it for that key.
- **Limits:** 20 frames/s per connection, 24 KB per message, 2,000 waiting messages per person.
- **Data:** `ultra.db` (SQLite): users, friends, pending requests, undelivered messages.

## Running

```bash
cargo build --release
ULTRA_SERVER_DIR=/path/for/data ULTRA_SERVER_PORT=7777 ./target/release/ultra-server
```

## On the owner's Raspberry Pi

- Code and data live in `~/ultra-server` (data in `~/ultra-server/data`).
- It runs as a user service: `systemctl --user status|restart ultra-server`. It's enabled at
  boot (lingering is on).
- To update: copy `Cargo.toml`, `Cargo.lock` and `src/` over, run `~/.cargo/bin/cargo build
  --release` on the Pi, then `systemctl --user restart ultra-server`.
- ULTRA's default server is `ws://oliverbarnet.local:7777/ws` (setting `chatserver`). That
  only works on the home network. Reaching it from anywhere needs TLS and a public address,
  for example a Cloudflare Tunnel to `wss://…`, plus `wss://` support in `social.rs`.
