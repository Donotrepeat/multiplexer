# TODO — Implementation Plan

Credits: derived from the vt100/ratatui bridge plan in the old `PLAN.md` (now deleted), a code read-through of the current implementation, and `vt100` 0.16 / `portable-pty` 0.9 API verification.

Each item lists: what's wrong → why it's a problem → where → suggested fix → acceptance criteria. Items are grouped into milestones ranked by priority; within a milestone, land them in order.

**Tooling state (verified 2026-09-13):** `cargo test` — 35/35 green. `cargo clippy --all-targets` — clean. CI (`.github/workflows/ci.yml`) — `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, all green.

**History:** the previous TODO (code-quality findings) was fully resolved and pruned in `190a0e6`. This plan is the next round: correctness fixes, tab bar UI, and clipboard + polish. Future-work items that are deliberately deferred are listed at the bottom.

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

## Future work (deliberately out of scope)

- Mouse support: click-to-focus pane, wheel for scrollback.
- Copy/selection: needs mouse or keyboard selection first; OSC 52 as a terminal-agnostic fallback.
- Sync the active pane's title to the host terminal's window title (OSC title already parsed per-pane).
- Kitty keyboard protocol for lossless key round-tripping.
- Configurable keybindings (a TOML/ron config).
