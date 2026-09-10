# TODO — Code Quality Findings

Credits: Standards axis derived from the Fowler smell baseline; tooling differences from `cargo build`, `cargo clippy --all-targets`, and `cargo test`.

Each item lists: what's wrong → why it's a problem → where → suggested fix. Findings are ranked roughly by priority.

**Tooling state (re-verified 2026-09-05):** `cargo test` — 14/14 green. `cargo clippy --all-targets` — **4 warnings**: the `upper_case_acronyms` on the `Grid` variants (cleared by the rename in #16).

**History:** original-review items #1–#8 were fixed and pruned in the first cleanup. Second-review items #8 (unused `vte`, dead `src/lib.rs`), #12 (`app::app` → `application`), #13 (collapsible `if`) and #14 (giant dispatch → `command` module) were fixed and pruned since; remaining items keep their second-review numbers.

---

## Correctness bugs (hard problems — fix first)

### 3. [BUG] Ctrl+letter encoding can panic (byte underflow)
- **Where:** `src/app/application.rs:147-149` (`send_key`)
- **What:** `w.write_all(&[c as u8 - b'a' + 1])` assumes `c` is a lowercase `a..=z`. For uppercase (e.g. Ctrl+Shift+key, which some terminals report as `Char('C')`) or non-letters, `c as u8 - b'a'` underflows: panic in debug builds, garbage byte in release.
- **Why it's a problem:** A reachable panic from ordinary keyboard input; also silently wrong for the other control ranges (Ctrl+@, Ctrl+[, Ctrl+], Ctrl+_, …).
- **Fix:** Guard `matches!(c, 'a'..='z' | 'A'..='Z')` (lowercasing first) and handle the remaining control ranges explicitly — or use a key-to-bytes helper that already knows the mapping (this belongs in `Pane`, see #15).


---

## Tooling & hygiene


### 9. [HYGIENE] Panic hygiene: unwraps on fallible I/O + no terminal-restoring panic hook
- **Where:** `src/app/tabs.rs:34` (`Pane::new(..).unwrap()`), `src/app/pane.rs:229` (`resize(..).unwrap()`), `src/app/application.rs:98` (`get_size().unwrap()`), and `lock().unwrap()` throughout (`application.rs:135`, `pane.rs:129`, `:162`, `:170`, `:238`)
- **What:** `Tab::new` can't propagate `Pane::new`'s error, so it unwraps. A panic in the reader thread while holding the vpty mutex poisons it → the next `lock().unwrap()` in `render_pane` takes down the main thread too. No panic hook restores the terminal, so any crash leaves raw mode on and the user's shell unusable.
- **Why it's a problem:** One dropped PTY or panicking thread crashes the whole app *and* trashes the user's terminal state.
- **Fix:** Make `Tab::new` return `Result`; replace unwraps with `?`/logged failures; install a panic hook that restores the terminal (`disable_raw_mode` + leave alternate screen) before exiting.

### 10. [PERF] vpty mutex held across the whole render conversion
- **Where:** `src/app/pane.rs:237-251` — `render_pane` keeps the parser lock while `vterm_to_ratatui` walks every cell
- **What:** PLAN.md "Step 2" prescribed cloning the screen under the lock and converting outside it; instead the guard is held through the entire span-building loop (every cell × every pane, up to 60 fps).
- **Why it's a problem:** The reader thread stalls on every frame's render — PTY ingestion gains latency exactly when output is flowing.
- **Fix:** Clone the screen under the lock, convert outside it. Also worth coalescing runs of identical style into one `Span` instead of one span per cell.

### 11. [RISK] Layout math has zero test coverage; no CI
- **Where:** `src/app/tabs.rs:40-119` (`horizontal_rects`, `grid_rects`, `vertical_rects`, `golden_rects`)
- **What:** The most algorithmic, off-by-one-prone code in the crate (remainder distribution, saturating splits, `div_ceil` balancing) has no tests; existing tests cover only `MuxCallbacks` replies and the logger. No CI workflow in the repo.
- **Why it's a problem:** Refactors like #17/#18 are exactly when this code breaks silently.
- **Fix:** Unit tests asserting exact `Rect` outputs for n = 1..8 at a fixed area (including non-divisible sizes) plus a golden-ratio case; minimal CI running `cargo fmt --check && cargo clippy -- -D warnings && cargo test`.

---

## Clippy / compiler warnings (tooling-enforced)

> 4 remaining — the `upper_case_acronyms` warnings on the `Grid` variants, cleared by the rename in #16.

---

## Smell findings (Fowler baseline — judgement calls)

### 15. [SMELL] Feature Envy — `App` reaches into `Pane`'s writer to send bytes
- **Where:** `src/app/application.rs:132-155` (`send_key`)
- **What:** `App::send_key` locks `active_pane.pty_writer`, then hand-maps every `KeyCode` to escape bytes (`Enter -> b"\r"`, `Up -> b"\x1b[A"`, `Ctrl+c -> c as u8 - b'a' + 1`, …).
- **Why it's a problem:**
  - **Wrong home:** the `KeyCode`→bytes knowledge is intrinsic to a *pane/terminal*, not to the app shell that routes keys. `Pane` owns the writer; `Pane` should be the only thing that knows how to translate keys into bytes for its PTY.
  - **Encapsulation leak:** `App` depends on the mutable internals of `Pane` (`pty_writer` is `pub`), coupling the two modules and duplicating terminal-protocol knowledge where a second consumer would have to re-derive it.
  - **Complicates borrows:** the `active`/`get_mut_tab()` dance in the same method is a direct consequence of doing the write here instead of in a `&mut self` method on `Pane`.
- **Fix:** Add `Pane::send_key(&mut self, code: KeyCode, modifiers: KeyModifiers)` (holding the escape map) and call that from `App`. This is also the natural landing spot for the encoding guards from #3/#7.

### 20. [SMELL] Data Clump / Divergent Change — Pane's shared-state wiring
- **Where:** `src/app/pane.rs:75-88`
- **What:** Five `Arc<Mutex<…>>`/`Arc<AtomicBool>` fields (`vpty`, `pty_writer`, `screen_changed`, and the `title`/`title_shared`/`title_changed` trio) that must be created, cloned, and kept coherent between Pane, its reader thread, and `MuxCallbacks`.
- **Why it's a problem:**
  - **Data Clump (title trio):** `title` (the Pane copy), `title_shared` (the shared source) and `title_changed` (the dirty flag) always travel and co-change together; they clearly want to be one `SharedTitle` type so the coupling is explicit and re-used in one place.
  - **Divergent Change:** `Pane` mixes several unrelated concerns — terminal emulation (`vpty`), PTY I/O plumbing (`pty_writer`, `pty_master`), thread signalling (`screen_changed`), scrolling state, and rendering. Each of those changes for different reasons, but they're all glued into one struct by hand-wiring shared state. The risk is that a change to one concern (e.g. the title channel) destabilises render or input paths, and the `Arc<Mutex<>>` wiring is easy to get wrong (deadlock, missed `swap(false)`).
- **Fix:** Bundle the title triple into a small struct; consider consolidating the shared channel objects so their lifecycle is created/consumed in one place rather than five parallel fields.

