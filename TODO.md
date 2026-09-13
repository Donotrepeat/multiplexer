# TODO — Code Quality Findings

Credits: Standards axis derived from the Fowler smell baseline; tooling differences from `cargo build`, `cargo clippy --all-targets`, and `cargo test`.

Each item lists: what's wrong → why it's a problem → where → suggested fix. Findings are ranked roughly by priority.

**Tooling state (re-verified 2026-09-13):** `cargo test` — 35/35 green. `cargo clippy --all-targets` — clean (the `upper_case_acronyms` warnings on the `Grid` variants were cleared by the rename in #16).

**History:** original-review items #1–#8 were fixed and pruned in the first cleanup. Second-review items #8 (unused `vte`, dead `src/lib.rs`), #12 (`app::app` → `application`), #13 (collapsible `if`), #14 (giant dispatch → `command` module), #15 (key encoding moved onto `Pane`) and #20 (title trio → `SharedTitle`, session channels + reader loop → `PtySession`; `Pane` reduced to the view) were fixed and pruned since; remaining items keep their second-review numbers.

---

## Correctness bugs (hard problems — fix first)
---

## Tooling & hygiene

---

## Clippy / compiler warnings (tooling-enforced)

> None remaining — the `upper_case_acronyms` warnings on the `Grid` variants were cleared by the rename in #16.

---

## Smell findings (Fowler baseline — judgement calls)
