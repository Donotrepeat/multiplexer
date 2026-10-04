# TODO — Implementation Plan

Credits: derived from the vt100/ratatui bridge plan in the old `PLAN.md` (now deleted), a code read-through of the implementation, and `vt100` 0.16 / `portable-pty` 0.9 API verification.

**Tooling state (verified 2026-10-04):** `cargo test` — 54/54 green. `cargo clippy --all-targets` — clean. CI (`.github/workflows/ci.yml`) — `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, all green.

**History:** the previous round (B4 tab-bar width budgeting, E2–E7 and E9/E10: the explicit update pass, the pane event channel, snapshot caching, the writer type, the reaper/teardown, poisoned-lock logging, and the small cleanups) is fully resolved and pruned. E8 (splitting `pane.rs` into `session.rs` / `render.rs` / `keys.rs`) landed earlier in `1ef8fd4`.

Known trade-off from E7: teardown signals the child with portable-pty's cloned `ChildKiller` (SIGHUP on Unix). A child that ignores it, or a grandchild that keeps the pty slave open, is detached after a bounded grace period instead of blocking the UI thread.

## Verification

After every change:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Manual checklist:

- **B:** tab bar shows titles and highlights the active tab; with more tabs than fit, every entry stays visible and truncates; terminal shrunk to a tiny size doesn't panic.
- **E:** `exit` in a pane shows `[exited]`; resize propagates to background tabs; background output/titles wake the loop without that pane being active; no `wait()` on the UI thread; quitting with a busy pane doesn't stall and leaves no zombies.

## Future work (deliberately out of scope)

- Mouse support: click-to-focus pane, wheel for scrollback.
- Copy/selection: needs mouse or keyboard selection first; OSC 52 as a terminal-agnostic fallback.
- Sync the active pane's title to the host terminal's window title (OSC title already parsed per-pane).
- Kitty keyboard protocol for lossless key round-tripping.
- Configurable keybindings (a TOML/ron config).
