# Plan: Server Mode for Multiplexer

**Date:** 2026-10-21  
**Status:** draft — implementation not started  
**Decisions taken:**
1. One server process owns exactly one session (matches existing `--session` model).
2. All attached clients see a mirrored view of that session.
3. Remote access uses TCP with a token-challenge handshake (Option B).
4. `std::net` + threads; no async runtime.

This plan supersedes the "Remote/multiplexed sessions across machines" non-goal in `docs/plan-next-phase.md`.

---

## 1. Goal

A user can run:

```bash
multiplexer --server --session work
```

Then, from the same machine or a remote one, attach one or more clients:

```bash
multiplexer --attach 127.0.0.1:7345
multiplexer --attach /run/user/1000/multiplexer/work.sock   # local fast path
```

All attached clients see the same tabs, panes, and output. Any client can send keys. The server keeps the session alive when no client is connected.

---

## 2. Architecture

### 2.1 Core seam: `Host`

`App` currently mixes multiplexing logic with `crossterm`/`ratatui` calls. Introduce a `Host` adapter so the same `App` can run locally or inside the server.

```rust
pub trait Host {
    /// Input from the outside world: a key, a resize, or a disconnect.
    fn poll_event(&mut self, timeout: Duration) -> Result<Option<HostEvent>>;

    /// Draw one frame. The closure receives a ratatui `Frame`.
    fn draw<F: FnOnce(&mut Frame)>(&mut self, f: F) -> Result<()>;

    /// Size the layout should target.
    fn size(&self) -> (u16, u16);
}
```

`HostEvent`:

```rust
pub enum HostEvent {
    Key(KeyEvent),
    Resize(u16, u16),
    Detach, // graceful disconnect; server keeps running
}
```

Adapters:
- **`LocalHost`** — wraps `crossterm::event` and `ratatui::DefaultTerminal`. Preserves today's behavior.
- **`ServerHost`** — accepts network clients, merges their input into `HostEvent`, and broadcasts rendered frames back to every client.

This makes `App` a deep module: it keeps all command, tab, pane, and workspace logic; only the I/O adapter changes.

### 2.2 Server process model

```text
┌─────────────────────────────────────────────┐
│  Server process: multiplexer --server work   │
│                                              │
│  ┌─────┐  ┌─────┐        ┌─────┐            │
│  │ TCP │  │ UDS │   ->   │ App │  ->  PTYs  │
│  │listener│ │listener│   │     │            │
│  └──┬──┘  └──┬──┘        └──┬──┘            │
│     │        │              │               │
│     └────┬───┘              │               │
│          │                  │               │
│     ┌────┴──────────────────┘               │
│     │ ClientManager (one thread per client) │
│     └───────────────────────────────────────┘
└─────────────────────────────────────────────┘
```

- **One thread per accepted client.** It reads `ClientEvent`s, translates them to `HostEvent`s, and pushes them into a shared queue.
- **`ServerHost::poll_event` drains that queue** with a timeout, exactly like `LocalHost` drains `crossterm::event`.
- **`ServerHost::draw` serializes the frame** and broadcasts it to every connected client.
- The server owns the workspace file lock by virtue of being the process; it saves on clean exit as today.

### 2.3 Wire protocol

Length-delimited messages over a stream. One `ClientEvent` or `ServerEvent` per message.

```rust
// client -> server
#[derive(Serialize, Deserialize)]
pub enum ClientEvent {
    Key(KeyEvent),     // crossterm event, serialized
    Resize(u16, u16),  // terminal size of this client
    Detach,            // client is leaving cleanly
}

// server -> client
#[derive(Serialize, Deserialize)]
pub enum ServerEvent {
    Frame(FramePacket),   // full or incremental screen state
    Title(String),        // window/title-bar string
    Beep,
    Error(String),        // human-readable, e.g. auth failed
}
```

`FramePacket` (first cut):

```rust
#[derive(Serialize, Deserialize)]
pub struct FramePacket {
    pub rows: u16,
    pub cols: u16,
    pub tab_bar: Vec<TabLabel>,
    pub active_tab: usize,
    pub panes: Vec<PaneFrame>,
}

pub struct PaneFrame {
    pub id: u64,
    pub title: String,
    pub exited: bool,
    pub cursor: Option<(u16, u16)>,
    pub cells: Vec<Cell>,  // row-major; empty cells omitted for compression
}
```

Encoding: `serde_json` with a `u32` big-endian length prefix. Measure bandwidth after M3; switch to `bincode` or a diff if needed.

### 2.4 Authentication handshake

On connect, the server sends a random 32-byte nonce. The client must respond with:

```rust
struct AuthResponse {
    token: String, // from the server's token file
    nonce_signature: [u8; 32], // HMAC-SHA256(token, nonce)
}
```

The server stores the session token in:

```
$XDG_STATE_HOME/multiplexer/tokens/<session>.token
```

Only clients that can read that file (or are given the token out of band) can attach. For remote access, the user copies the token to the remote machine via SSH or a secret mechanism.

This avoids sending the raw token over the wire and lets the server rotate the nonce per connection.

---

## 3. Milestones

### M1 — Introduce the `Host` seam

**Files touched:** `src/app/application.rs`, new `src/host/mod.rs`, `src/host/local.rs`, `src/main.rs`.

**Work:**
1. Define `Host`, `HostEvent`, and `LocalHost`.
2. Refactor `App::run`, `pump`, `update_all`, and `draw` to operate on `&mut dyn Host` instead of `DefaultTerminal` and direct `crossterm` calls.
3. Keep `LocalHost` behavior identical to today's code.

**Acceptance:**
- `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` are green.
- Running `multiplexer` locally behaves exactly as before.

### M2 — Server process

**Files touched:** new `src/server/`, `src/host/server.rs`, `src/main.rs`, `Cargo.toml`.

**Work:**
1. Add CLI parsing: `multiplexer --server --session <name>` (hand-rolled, no `clap`).
2. `ServerHost`:
   - Listen on Unix domain socket: `$XDG_RUNTIME_DIR/multiplexer/<session>.sock` (Linux) or fallback to `$XDG_STATE_HOME/multiplexer/sockets/<session>.sock`.
   - Listen on TCP (`127.0.0.1:7345` by default, configurable later).
   - Accept clients, run handshake, spawn a reader thread per client.
   - Collect client input into a `mpsc` queue consumed by `poll_event`.
3. Frame serialization: convert the ratatui `Frame` content into `FramePacket` inside `ServerHost::draw` and broadcast it.
4. Server writes its token file on startup and removes the socket file on exit.
5. Persist workspace on clean exit as today.

**Acceptance:**
- `multiplexer --server --session test` starts and writes a socket + token file.
- A throwaway test client can connect, complete the handshake, send a key, and receive a `FramePacket`.
- Server keeps running when the test client disconnects.

### M3 — Client process

**Files touched:** new `src/client/`, `src/main.rs`.

**Work:**
1. Add CLI parsing: `multiplexer --attach <target>` where target is `host:port` or a socket path.
2. Client connection:
   - Complete the auth handshake.
   - Spawn two threads: one reads `ServerEvent`s, one reads `crossterm` input and sends `ClientEvent`s.
3. Client TUI:
   - Render `FramePacket` using ratatui (convert back to `Paragraph`/`Block` cells, or write a custom widget).
   - Handle `Resize` by sending `ClientEvent::Resize`.
   - Forward all non-command keys as `ClientEvent::Key`.
   - The client has no command logic; `Alt+C`, `Alt+T`, etc. are sent to the server and interpreted there.

**Acceptance:**
- Two local terminals can `multiplexer --attach <socket>` to the same server.
- Both show the same output; typing in either affects the shared session.
- Disconnecting one client does not disturb the other.

### M4 — Robustness and cleanup

**Work:**
1. Reap stale TCP/UDS sockets on startup.
2. Limit number of clients and idle timeouts.
3. Rotate token file on server restart.
4. Signal handling (`SIGTERM`, `SIGINT`) for graceful shutdown and workspace save.
5. Logging: server logs attaches/detaches/errors to the existing log dump.
6. Manual page / `--help` text.

**Acceptance:**
- `kill <server-pid>` saves the workspace and removes the socket.
- Rebooting the server rejects old tokens.
- All existing `cargo test` / clippy / fmt checks still pass.

---

## 4. Dependencies to add

- `serde_json` — for the wire protocol (already a transitive dep via `toml`, but pin explicitly).
- `hmac` + `sha2` — for token challenge signature.
- `rand` — for nonce generation (or use `std` if sufficient).

No async runtime. No `clap`.

---

## 5. Risks and mitigations

| Risk | Mitigation |
|------|------------|
| Multiple clients with different terminal sizes | M2/M3 use the first attached client's size, or a configured default. Document that mirrored sessions share one layout size. |
| Input from two clients interleaves | Acceptable for a mirrored session; document as shared input. |
| Frame serialization bandwidth | Start with JSON + full frame; measure and switch to binary/diff in M4 if needed. |
| Raw mode leaks on client crash | Client panic hook mirrors the existing main.rs hook to disable raw mode. |
| Server token file permissions | Create with `0o600`; rely on filesystem ACLs for remote access. |
| Crossterm `KeyEvent` serialization | `KeyEvent` derives `Serialize`/`Deserialize` in crossterm 0.29; verify and wrap in a newtype if necessary. |

---

## 6. First action

Implement **M1**: define `Host`/`HostEvent`/`LocalHost` and refactor `App::run` to use the adapter. Do not add networking yet. This isolates the I/O seam and gives a safe base for M2.
