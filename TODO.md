# TODO — Code Quality Findings

Credits: Standards axis derived from the Fowler smell baseline; tooling differences from `cargo build`, `cargo clippy --all-targets`, and `cargo test`.

Each item lists: what's wrong → why it's a problem → where → suggested fix. Findings are ranked roughly by priority.

**Tooling state (re-verified 2026-09-05):** `cargo test` — 14/14 green. `cargo clippy --all-targets` — **4 warnings**: the `upper_case_acronyms` on the `Grid` variants (cleared by the rename in #16).

**History:** original-review items #1–#8 were fixed and pruned in the first cleanup. Second-review items #8 (unused `vte`, dead `src/lib.rs`), #12 (`app::app` → `application`), #13 (collapsible `if`) and #14 (giant dispatch → `command` module) were fixed and pruned since; remaining items keep their second-review numbers.

---

## Correctness bugs (hard problems — fix first)
---

## Tooling & hygiene

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

