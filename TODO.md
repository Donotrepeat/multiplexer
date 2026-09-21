# TODO — Implementation Plan

Credits: derived from the vt100/ratatui bridge plan in the old `PLAN.md` (now deleted), a code read-through of the current implementation, and `vt100` 0.16 / `portable-pty` 0.9 API verification.

Each item lists: what's wrong → why it's a problem → where → suggested fix → acceptance criteria. Items are grouped into milestones ranked by priority; within a milestone, land them in order.

**Tooling state (verified 2026-09-13):** `cargo test` — 35/35 green. `cargo clippy --all-targets` — clean. CI (`.github/workflows/ci.yml`) — `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, all green.

**History:** the previous TODO (code-quality findings) was fully resolved and pruned in `190a0e6`. This plan is the next round: correctness fixes, tab bar UI, and clipboard + polish. Future-work items that are deliberately deferred are listed at the bottom.

---

## Milestone A — Correctness bugs & wiring

### A5. Key encoding ignores modifiers and common keys

- **What:** `key_to_bytes` handles Enter/Tab/Backspace/Esc/arrows/Delete and plain/Ctrl/Alt chars, but: Home, End, PageUp, PageDown, Insert, and F1-F12 have no encodings, and modifiers on navigation keys are dropped (Ctrl+Left sends plain `\x1b[D`).
- **Why:** apps that bind these keys (word-jump in shells/vim, `less` paging once A4 lands) misbehave. Also, Alt on CSI keys currently uses the "meta sends escape" prefix (`\x1b\x1b[A`); xterm's modifier-parameter form (`\x1b[1;3A`) is what apps actually parse.
- **Where:** `src/app/pane.rs:356-399` (`key_to_bytes`, `unmodified_key_to_bytes`).
- **Suggested fix:**
  - Add unmodified encodings: Home → `\x1b[H`, End → `\x1b[F`, PageUp → `\x1b[5~`, PageDown → `\x1b[6~`, Insert → `\x1b[2~`, F1-F4 → `\x1bOP`…`\x1bOS`, F5-F12 → `\x1b[15~`, `\x1b[17~`…`\x1b[24~` (13-14 unused), Shift+Tab → `\x1b[Z`.
  - Modifier parameter `m = 1 + shift + 2·alt + 4·ctrl`: arrows/Home/End → `\x1b[1;{m}{A..H}`, `~`-keys → `\x1b[{n};{m}~`. Replace the ESC-prefix for Alt on CSI keys with this (keep ESC-prefix for plain chars — "meta sends escape" is what shells expect).
  - Keep the Alt+Ctrl composition: both fold into `m`.
- **Acceptance + tests:** table-driven tests covering every new key × {none, Shift, Ctrl, Alt, Ctrl+Shift}; update `alt_keys_are_esc_prefixed` expectations for CSI keys. Manual: Ctrl+arrows word-jump in the shell, Shift+arrows select in `vim`.

---

## Milestone B — Tab bar UI

### B1. Tabs are invisible

- **What:** `App` supports multiple tabs (NewTab/NextTab/PrevTab), but `draw` renders only the active tab — there is no tab bar, so users can't see how many tabs exist or which is active.
- **Why:** invisible state is undiscoverable state; `NewTab` also reserves horizontal space (`term_cols - 4`) for a bar that never renders, shrinking every initial pane for no reason.
- **Where:** `src/app/application.rs:150-152` (`App::draw`), `src/app/tabs.rs:139` (`Tab::draw_tab`).
- **Suggested fix:**
  - In `App::draw`: `Layout::vertical([Length(1), Min(0)])` → bar area + content area.
  - Render the bar from per-tab titles: a tab's title is its active pane's title (already synced from OSC 0/1/2). Numbered entries (`1:title  2:title …`), active tab highlighted (bold + cyan, matching the pane title style); truncate long titles so `N` tabs always fit one row.
  - `draw_tab(&mut self, frame, area: Rect)` takes the content rect instead of `frame.area()`.
- **Acceptance:** with 3 tabs, all three titles are visible, the active one is highlighted; adding/switching tabs updates the bar; pane content no longer overlaps the bar.

### B2. Duplicated, lossy initial-size math

- **What:** `main.rs` and `Command::NewTab` both compute `Tab::new(term_rows - 2, term_cols - 4)`; on a tiny terminal `term_rows - 2` / `term_cols - 4` underflow and panic, and `-4` has no justification.
- **Why:** duplicated logic drifts (B1 changes the right answer again); underflow turns a small window into a crash.
- **Where:** `src/main.rs:36-40`, `src/app/application.rs:47-55`.
- **Suggested fix:** one helper (e.g. `fn initial_pane_size() -> (u16, u16)`) using `saturating_sub`: reserve 2 rows for pane borders + 1 for the new tab bar, 2 cols for borders. Both call sites use it. Exactness doesn't matter — `draw_tab` resizes every pane to its real rect each frame — but the first spawn should be close to avoid a resize storm.
- **Acceptance:** resizing the terminal to a few rows/cols and pressing Alt+C no longer panics.

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

## Future work (deliberately out of scope)

- Mouse support: click-to-focus pane, wheel for scrollback.
- Copy/selection: needs mouse or keyboard selection first; OSC 52 as a terminal-agnostic fallback.
- Sync the active pane's title to the host terminal's window title (OSC title already parsed per-pane).
- Kitty keyboard protocol for lossless key round-tripping.
- Configurable keybindings (a TOML/ron config).
