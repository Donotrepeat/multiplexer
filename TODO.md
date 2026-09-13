# TODO — Code Quality Findings

Credits: Standards axis derived from the Fowler smell baseline; tooling differences from `cargo build`, `cargo clippy --all-targets`, and `cargo test`.

Each item lists: what's wrong → why it's a problem → where → suggested fix. Findings are ranked roughly by priority.

**Tooling state (re-verified 2026-09-13):** `cargo test` — 32/32 green. `cargo clippy --all-targets` — clean (the `upper_case_acronyms` warnings on the `Grid` variants were cleared by the rename in #16).

**History:** original-review items #1–#8 were fixed and pruned in the first cleanup. Second-review items #8 (unused `vte`, dead `src/lib.rs`), #12 (`app::app` → `application`), #13 (collapsible `if`), #14 (giant dispatch → `command` module) and #15 (key encoding moved onto `Pane`) were fixed and pruned since; remaining items keep their second-review numbers.

---

## Correctness bugs (hard problems — fix first)
---

## Tooling & hygiene

---

## Clippy / compiler warnings (tooling-enforced)

> None remaining — the `upper_case_acronyms` warnings on the `Grid` variants were cleared by the rename in #16.

---

## Smell findings (Fowler baseline — judgement calls)

### 20. [SMELL] Divergent Change — Pane's shared-state wiring (residual)
- **Where:** `src/app/pane.rs`
- **What:** The title trio is now bundled into `SharedTitle` (the Data Clump prong is fixed), but `Pane` still hand-wires four parallel shared-state fields — `vpty`, `pty_writer`, `screen_changed`, `title_shared` — created, cloned, and kept coherent between Pane, its reader thread, and `MuxCallbacks`.
- **Why it's a problem:** `Pane` mixes several unrelated concerns — terminal emulation (`vpty`), PTY I/O plumbing (`pty_writer`, `pty_master`), thread signalling (`screen_changed`), scrolling state, and rendering. Each of those changes for different reasons, but they're all glued into one struct; the `Arc<Mutex<>>` wiring is easy to get wrong (deadlock, missed `swap(false)`).
- **Fix:** Consolidate the remaining shared channel objects so their lifecycle is created/consumed in one place rather than four parallel fields.


