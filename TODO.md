# TODO — Implementation Plan

Credits: derived from the vt100/ratatui bridge plan in the old `PLAN.md` (now deleted), a code read-through of the current implementation, and `vt100` 0.16 / `portable-pty` 0.9 API verification.

Each item lists: what's wrong → why it's a problem → where → suggested fix → acceptance criteria. Items are grouped into milestones ranked by priority; within a milestone, land them in order.

**Tooling state (verified 2026-09-13):** `cargo test` — 35/35 green. `cargo clippy --all-targets` — clean. CI (`.github/workflows/ci.yml`) — `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, all green.

**History:** the previous TODO (code-quality findings) was fully resolved and pruned in `190a0e6`. This plan is the next round: correctness fixes, tab bar UI, clipboard + polish, and concurrency/architecture (Milestone E, added 2026-09-26). Future-work items that are deliberately deferred are listed at the bottom.

---

## Milestone B — Tab bar UI

### B2. Duplicated, lossy initial-size math

- **What:** `main.rs` and `Command::NewTab` both compute `Tab::new(term_rows - 2, term_cols - 4)`; on a tiny terminal `term_rows - 2` / `term_cols - 4` underflow and panic, and `-4` has no justification.
- **Why:** duplicated logic drifts (B1 changes the right answer again); underflow turns a small window into a crash.
- **Where:** `src/main.rs:36-40`, `src/app/application.rs:47-55`.
- **Suggested fix:** one helper (e.g. `fn initial_pane_size() -> (u16, u16)`) using `saturating_sub`: reserve 2 rows for pane borders + 1 for the new tab bar, 2 cols for borders. Both call sites use it. Exactness doesn't matter — `draw_tab` resizes every pane to its real rect each frame — but the first spawn should be close to avoid a resize storm.
- **Acceptance:** resizing the terminal to a few rows/cols and pressing Alt+C no longer panics.

### B3. Background tab titles in the bar go stale

- **What:** `draw_bar` renders `tab.panes[tab.active].title` for every tab, but `pane.title` is only refreshed by `sync_title()`, which runs inside `draw_tab` — and `App::draw` only draws the active tab.
- **Why:** a background tab's process can change its title (shell prompt hook, vim OSC 0/2) while the bar keeps showing the title from when that tab was last active. The fresh value sits in the `SharedTitle` mutex already; it just never propagates to `pane.title` until the tab is drawn.
- **Where:** `src/app/application.rs` (`draw`/`draw_bar`, commit `b3e5f80`), `src/app/pane.rs` (`sync_title`), `src/app/tabs.rs` (`draw_tab`).
- **Suggested fix:** sync titles for all tabs in `App::draw` before rendering the bar (cheap — a changed-flag check per pane), or have `draw_bar` read the `SharedTitle` directly instead of the pane copy.
- **Acceptance:** with two tabs, trigger a title change in the background tab (e.g. `cd` in a prompt-hook shell); the bar updates without switching to that tab.

### B4. Tab bar clips instead of fitting all tabs

- **What:** `draw_bar` renders one long `Line` of `N:title  ` spans and lets ratatui clip it at the terminal width.
- **Why:** with enough tabs (or long titles) the later tabs are cut off the right edge — the user can't see that they exist. This is the failure mode B1 called out; the acceptance ("truncate long titles so `N` tabs always fit one row") is only half-met (no crash, but whole tabs disappear).
- **Where:** `src/app/application.rs` (`draw_bar`, commit `b3e5f80`).
- **Suggested fix:** budget `area.width` across tabs — give each entry an equal share (minus its `N:` prefix and padding) and truncate each title to fit, so per-tab truncation replaces whole-tab loss.
- **Acceptance:** with 5 tabs and one very long pane title on a narrow terminal, all five entries remain visible, each truncated.

---

## Milestone C — Clipboard & polish

### C1. No paste support

- **What:** there is no way to paste system-clipboard text into a pane.
- **Why:** the single most-missed terminal feature; retyping long commands into a multiplexer is misery.
- **Where:** new — `Cargo.toml`, `src/app/command.rs` (`ALT_BINDINGS`), `src/app/application.rs` (`execute`), `src/app/pane.rs` (write path).
- **Suggested fix:**
  - Add `arboard` (3.x) as a dependency; it's the standard Rust cross-platform clipboard (no system dev libraries needed on Linux).
  - Bind `Alt+V` → `Command::Paste` (fits the existing Alt-letter scheme; `v` is free).
  - `execute`: read `Clipboard::new()?.get_text()?`, forward to the active pane.
  - Bracketed paste: `vt100::Screen::bracketed_paste()` already tracks `\x1b[?2004h/l` — when the pane's shell enabled the mode, wrap the payload in `\x1b[200~` … `\x1b[201~` and normalize newlines to `\r`, so multi-line pastes edit instead of execute in zsh/bash. Unwrapped otherwise.
- **Acceptance + tests:** unit tests for the wrapping decision (mode on/off) and newline normalization; manual: paste a multi-line command at a zsh prompt → it appears as editable text, not instant execution.

### C2. Stray Python files from an unrelated project

- **What:** `src/models/user.py`, `src/utils/auth.py`, `src/utils/database.py`, `tests/test_auth.py` were committed in `96a6ca5` ("started todo"); they are a Flask-style user/auth scaffold with no relation to this Rust project.
- **Why:** they confuse every code search and make the repo look like it hosts two projects; nothing in the crate references them.
- **Where:** the four files above.
- **Suggested fix:** `git rm -r src/models src/utils tests/test_auth.py` (the `tests/` directory then disappears; Rust tests live inline under `src/`). Optionally align CI's clippy invocation with the local baseline (`cargo clippy --all-targets -- -D warnings`) in `.github/workflows/ci.yml`.
- **Acceptance:** `git grep -l "\.py"` returns nothing; `cargo test` still green.

### C3. Outdated `PLAN.md`

- **What:** `PLAN.md` documents the original single-file, non-compiling design (steps 1-10); every step shipped long ago and the architecture it describes no longer exists.
- **Why:** it misleads any agent or human doing repo orientation into fixing problems that are already fixed.
- **Where:** `PLAN.md`.
- **Suggested fix:** `git rm PLAN.md`. This file becomes the single living planning document.
- **Acceptance:** gone from the repo root; this TODO is the planning source of truth.

---

## Milestone D — Key encoding & robustness (code review 2026-09-24)

### D1. Shift+Tab is swallowed (`BackTab` never encoded)

- **What:** `csi_key_to_bytes` encodes Shift+Tab as `KeyCode::Tab` + `SHIFT` → `\x1b[Z`, but on Unix crossterm delivers a real Shift+Tab as `KeyCode::BackTab` (crossterm itself parses the terminal's `\x1b[Z`, see crossterm 0.29 `parse.rs`). `BackTab` matches no arm and falls through to `_ => Vec::new()`.
- **Why:** `write_key` sends zero bytes to the pane: pressing Shift+Tab in any TUI form with field navigation does nothing. The `Tab`+`SHIFT` arm only fires under the kitty keyboard protocol, which isn't enabled. No test covers `BackTab`, which is why the suite stays green.
- **Where:** `src/app/pane.rs` (`csi_key_to_bytes`, commit `f7089f1`).
- **Suggested fix:** add `KeyCode::BackTab => Some(b"\x1b[Z".to_vec())` to `csi_key_to_bytes`; add a `BackTab` test case.
- **Acceptance:** unit test asserting `BackTab` encodes to `\x1b[Z`; manual: Shift+Tab moves to the previous field in a TUI form inside a pane.

### D2. `in_alternate_screen` breaks the poisoned-lock pattern

- **What:** the new helper locks the parser with `.unwrap()` while every other `vpty` lock in `pane.rs` uses `.unwrap_or_else(|e| e.into_inner())`.
- **Why:** it's on the keypress hot path (`handle_events` calls it for every key); if the reader thread ever panics while holding the parser lock, the UI thread now panics on every subsequent keypress instead of degrading the way the rest of the file is written to.
- **Where:** `src/app/pane.rs` (`PtySession::in_alternate_screen`, commit `7166290`).
- **Suggested fix:** use the same `unwrap_or_else(|e| e.into_inner())` pattern.
- **Acceptance:** no bare `.lock().unwrap()` on `vpty` remains in `pane.rs`.

### D3. `Command::DeletePane` underflows when the last tab dies (pre-existing)

- **What:** after removing the last tab, `let tab_count = self.tabs.len() - 1;` computes `0 - 1` on `usize`.
- **Why:** debug builds panic (Alt+R on a single-pane, single-tab app); release builds wrap `active_tab` to `usize::MAX` and are saved only by the `running = false` check that follows. `NextTab`/`PrevTab` share the `len() - 1` pattern but can't reach it while running.
- **Where:** `src/app/application.rs` (`Command::DeletePane`).
- **Suggested fix:** guard the decrement with `saturating_sub(1)`, or restructure: remove the tab, then clamp `active_tab` behind an `is_empty` check.
- **Acceptance:** Alt+R on the last pane of the last tab quits cleanly in a debug build; `cargo test` green.

---

## Milestone E — Concurrency & architecture

Ranked in landing order: E1 unblocks every exit feature; E2/E3 are correctness + safety; E4 is the structural core the rest hang off; E5–E10 build on it.

### E1. `exited` flag is inverted — panes never appear dead

- **What:** `read_loop` stores `false` into the `exited` atomic on EOF and on read error, so `is_not_alive()` is always `false` and a pane whose child died keeps looking alive.
- **Why:** every exit feature is dead code — the `[exited]` title prefix in `sync_title` can never fire, and the `write_bytes` early-return guard never triggers, so writing to a dead PTY tries (and errors) instead of no-op'ing. This is also the *only* signal of child exit, so nothing downstream can ever react to a pane dying.
- **Where:** `src/app/pane.rs` (`read_loop` lines 112 and 122; `is_not_alive` 212-214; `sync_title` 294-305).
- **Suggested fix:** store `true` on EOF/error. Second half: `sync_title` only runs when a *new* OSC title arrives, so a dying process that emits no title leaves `take_title()` `None` and `[exited]` still never shows — give exit its own "changed" signal (or fold it into the E4 event stream).
- **Acceptance:** `exit` in a pane sets `[exited]` without an intervening OSC title; the existing `read_loop_eof_leaves_flag_clear` test is updated to assert the corrected behaviour.

### E2. Resize & title sync happen only inside `draw`

- **What:** `draw_tab` is the only caller of `pane.resize(...)` and `pane.sync_title()`, and `App::draw` draws only the active tab.
- **Why:** a render pass is performing model mutation, so background tabs never get resized on a terminal resize (their shell sees a stale size / no SIGWINCH until you switch to them) and their titles go stale (see B3).
- **Where:** `src/app/tabs.rs` (`draw_tab`, 150-174), `src/app/application.rs` (`draw`, 154-162).
- **Suggested fix:** move resize + sync into an explicit update pass (e.g. `App::update_all()`) that walks every tab and every pane, called on each loop iteration or on resize events; `draw_tab` becomes read-only. Land together with B3.
- **Acceptance:** resize the terminal while a background tab is running a pager; switching to it shows the correct size immediately.

### E3. Undocumented parser→writer lock order (deadlock hazard)

- **What:** `MuxCallbacks::unhandled_csi` locks the PTY writer while already holding the parser mutex (it runs inside `process()`). This is safe only because no path locks parser-after-writer.
- **Why:** an invisible, undocumented invariant that is one accidental `writer.lock()` + `vpty.lock()` away from a deadlock.
- **Where:** `src/app/pane.rs` (`unhandled_csi`, 90-98; `read_loop` calls `process` at 116-118).
- **Suggested fix:** buffer the reply bytes locally inside `unhandled_csi` and flush to the writer *after* `process()` returns and the parser lock is released; at minimum document the rule ("the parser lock may acquire the writer lock; never the reverse").
- **Acceptance:** no write to the writer occurs while the parser lock is held; a comment states the lock-order rule.

### E4. Replace the 16ms poll with an event channel

- **What:** `App::run` polls all panes of the *active tab only* every iteration; `screen_changed`/`exited`/title are bare atomics with no wakeup power, and output from background tabs doesn't even shorten the poll.
- **Why:** ~16ms output latency; background-tab changes are invisible to the loop; three ad-hoc flags where one typed message would do.
- **Where:** `src/app/application.rs` (`run`, 21-37; `handle_events`, 39-47), `src/app/pane.rs` (atomics).
- **Suggested fix:** give the app one `mpsc` channel; each reader thread gets a `Sender` and emits `PaneEvent::Output | Exited | Title(..)` instead of flipping flags. Loop does `recv_timeout(~16ms)` + a zero-timeout `poll` for keys; draw only when something changed. Use a bounded/`sync_channel` with a "coalesce old Output" strategy (Output is a boolean "dirty", not a count). This folds E1's exit signal and B3/E2's title propagation into one mechanism.
- **Acceptance:** a background tab emitting output wakes the loop (observable via faster bar/title update); output no longer gated on the active tab.

### E5. `screen()` clones the whole vt100 screen every frame

- **What:** `render_pane` calls `session.screen()`, which clones the full `vt100::Screen` (`SCROLLBACK_SIZE` = 1200 rows) under the lock every frame, for every pane.
- **Why:** real O(pane-count × 1200 rows) cost at 60fps; grows with panes and starves the reader thread's lock.
- **Where:** `src/app/pane.rs` (`screen`, 267-273; `render_pane`, 362-385).
- **Suggested fix:** clone only when `take_screen_changed()` was set, caching the snapshot between dirty frames. (Deeper: double-buffer the snapshot on the reader thread and publish via E4 — defer until the channel lands.)
- **Acceptance:** idle frames do no `Screen` clone; rendering still correct.

### E6. `Option<Box<dyn Write + Send>>` is never `None`

- **What:** the writer is `Arc<Mutex<Option<Box<dyn Write + Send>>>>`, but nothing ever sets it to `None`, so every `if let Some(w) = ...as_mut()` branch is dead.
- **Why:** the `Option` + `dyn Write` exist only to inject `TestWriter` in tests, yet leak into production hot paths (`write_bytes`, `unhandled_csi`).
- **Where:** `src/app/pane.rs` (46-49, 131, 197-205).
- **Suggested fix:** a `PtyWriter: Write + Send` trait or generic, or at minimum drop the `Option` and keep `Box<dyn Write + Send>`; the test shim lives behind the trait bound.
- **Acceptance:** `write_bytes` has no `Option` unwrap; tests still inject a writer.

### E7. Children reaped and threads detached on the UI thread

- **What:** `Drop for PtySession` does `kill()` + `wait()` synchronously on whatever thread drops it (the UI thread), and the reader thread's `JoinHandle` is discarded at spawn.
- **Why:** `wait()` can stall a frame (uninterruptible sleep); the reader thread outlives the session and holds `Arc`s (parser, flags) until the master fd closes.
- **Where:** `src/app/pane.rs` (spawn, 177-178; `Drop`, 388-393).
- **Suggested fix:** a small reaper that `wait()`s exited children and emits `PaneEvent::Exited` (E4), and store the reader `JoinHandle` so shutdown can join deterministically.
- **Acceptance:** teardown is explicit; no `wait()` on the UI thread; no detached thread.

### E8. `pane.rs` bundles four concerns (~1000 lines)

- **What:** PTY lifecycle, the vt100↔ratatui bridge, key→byte encoding, and `MuxCallbacks` all in one file with almost no shared state.
- **Why:** makes the lock-order invariant (E3) hard to see and each piece hard to test in isolation.
- **Where:** `src/app/pane.rs`.
- **Suggested fix:** split into `session.rs` / `render.rs` / `keys.rs` (or `keys/` + `term/`).
- **Acceptance:** modules compile; `cargo test` green with tests co-located per module.

### E9. Poisoned locks recovered silently

- **What:** every `.lock().unwrap_or_else(|e| e.into_inner())` continues as if nothing happened.
- **Why:** right default (the UI shouldn't hang because one pane panicked), but the panic is invisible — a reader-thread crash is only noticed as a frozen pane.
- **Where:** throughout `src/app/pane.rs`.
- **Suggested fix:** log the poison (via the buffered logger) before recovering, so crashes are observable in the dump.
- **Acceptance:** a poisoned parser lock produces a log line, not silence.

### E10. Small cleanups

- **What:** (a) `sync_title` returns a `bool` every caller ignores (`tabs.rs:170`); (b) `draw_bar` reads the pane's `String` copy while a `SharedTitle` already holds the canonical value (`application.rs:170`); (c) `App::run` calls `terminal.draw` unconditionally every loop even on idle frames (`application.rs:34`).
- **Why:** dead return value; two title stores; redundant redraw.
- **Where:** as above.
- **Suggested fix:** drop the return value (or use it to dirty only the bar); make the bar read `SharedTitle` (or the E4 event); draw only when changed.
- **Acceptance:** single source of truth for titles; no unconditional draw.

---

## Verification

After every item, and once more per milestone:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Manual checklist per milestone:

- **A:** PageUp pages; Alt+R on a single-pane tab doesn't crash; `exit` in a pane → `[exited]` title, no zombies after quit; `less` receives PageUp/PageDown; Ctrl+arrows word-jump in the shell.
- **B:** tab bar shows titles and highlights the active tab; Alt+C/Alt+E/Alt+Q cycle visibly; terminal shrunk to tiny size doesn't panic.
- **C:** Alt+V pastes; multi-line paste at a zsh prompt is editable, not executed; repo contains no Python files; `PLAN.md` gone.
- **D:** Shift+Tab works inside a pane's TUI form; Alt+R on the last pane of the last tab quits cleanly in a debug build.
- **E:** `exit` in a pane shows `[exited]`; resize propagates to background tabs; background output wakes the loop without that pane being active; no `wait()` on the UI thread.

## Future work (deliberately out of scope)

- Mouse support: click-to-focus pane, wheel for scrollback.
- Copy/selection: needs mouse or keyboard selection first; OSC 52 as a terminal-agnostic fallback.
- Sync the active pane's title to the host terminal's window title (OSC title already parsed per-pane).
- Kitty keyboard protocol for lossless key round-tripping.
- Configurable keybindings (a TOML/ron config).
