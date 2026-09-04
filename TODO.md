# TODO — Code Quality Findings

Credits: Standards axis derived from the Fowler smell baseline; tooling differences from `cargo build`, `cargo clippy`, and `cargo test --no-run`.

Each item lists: what's wrong → why it's a problem → where → suggested fix. Findings are ranked roughly by priority.

---

## Build & tooling defects (hard problems — fix first)

## Clippy / compiler warnings (tooling-enforced)

> These are auto-detected and cheap to fix. Not smells, but they should be zeroed out: `cargo clippy` currently emits 14 warnings.


### 5. [WARN] Collapsible `if` blocks
- **Where:** `src/app/app.rs:144-148` and `:153`, `src/app/pane.rs:67`, `:151`
- **Why wrong:** Nested `if let`/`if` that could fold into a concatenated guard. Several also just assign a bool literal (`if cond { true } else { false }`).
- **Fix:** Collapse the conditions; replace `if cond { true } else { false }` with `cond`.

### 6. [WARN] `match` used for a single pattern (use `if let`)
- **Where:** `src/app/app.rs:44`
- **Why wrong:** Easier to misread as a multi-arm dispatch when only one arm is meaningful.
- **Fix:** Prefer `if let`.

### 7. [WARN] Redundant field names in struct init
- **Where:** `src/app/pane.rs:138` — `vpty: vpty`
- **Why wrong:** Verbal noise; the shorter `vpty` shorthand is the idiom.
- **Fix:** `Pane { vpty, … }`.

### 8. [WARN] Dead code — method never used
- **Where:** `src/app/pane.rs:211` — `at_top()` (plus `cargo build` reports unused code).
- **Why wrong:** Unreachable public API that's maintained for nothing; a sign of Speculative Generality.
- **Fix:** Either wire it into the scroll logic or delete it.

### 9. [WARN] `module has the same name as its containing module`
- **Where:** `src/app/mod.rs:1`
- **Why wrong:** `src/app/app.rs` creates `app::app`, which clashes/reads awkwardly with the `app` module → the confusing double `crate::app::app::App` path used across the code.
- **Fix:** Rename `app.rs` → e.g. `application.rs` or `ui.rs` and update `mod.rs`.

---

## Smell findings (Fowler baseline — judgement calls)

### 10. [SMELL] Repeated Switches — `handle_events` is one giant dispatch
- **Where:** `src/app/app.rs:42-188`
- **What:** A ~140-line `if`/`else if` cascade. Eight arms repeat the identical guard — `key.code == KeyCode::Char('X') && key.modifiers.contains(KeyModifiers::ALT)` — then a *second* nested cascade for scroll keys, then a *third* `match` that maps `KeyCode` to raw escape-sequence bytes.
- **Why it's a problem:**
  - **Rigidity:** Adding one new hotkey means writing another near-copy of the same guard; the failure mode is copy-paste bugs.
  - **Mysterious control flow:** Reading it is a scan for which arm fires and in what order — the borrow juggling (repeated `self.get_tab()` / `self.get_mut_tab()` with sentinel `active` copies) exists *because* the cascade holds `self` across many paths.
  - **Divergent reasons to change:** quitting, tab ops, layout cycling, pane deletion, scrolling and character-input all live in the same method.
- **Fix:** 
  - Dispatch on `(key.code, key.modifiers)` via a `match` or a keybinding table mapping `(KeyCode, KeyModifiers) -> Command`.
  - Give `Pane` a `send_key`/`scroll` interface so `App` orchestrates commands rather than performing low-level byte writes (see #11).

### 11. [SMELL] Feature Envy — `App` reaches into `Pane`'s writer to send bytes
- **Where:** `src/app/app.rs:153-154`
- **What:** `App::handle_events` locks `active_pane.pty_writer`, then hand-maps every `KeyCode` to escape bytes (`Enter -> b"\r"`, `Up -> b"\x1b[A"`, `Ctrl+c -> c as u8 - b'a' + 1`, …).
- **Why it's a problem:**
  - **Wrong home:** the `KeyCode`→bytes knowledge is intrinsic to a *pane/terminal*, not to the app shell that routes keys. `Pane` owns the writer; `Pane` should be the only thing that knows how to translate keys into bytes for its PTY.
  - **Encapsulation leak:** `App` depends on the mutable internals of `Pane` (`pty_writer` is `pub`), coupling the two modules and duplicating terminal-protocol knowledge where a second consumer would have to re-derive it.
  - **Complicates borrows:** the `active`/`get_mut_tab()` dance in the same method is a direct consequence of doing the write here instead of in a `&mut self` method on `Pane`.
- **Fix:** Add `Pane::send_key(&mut self, code: KeyCode, modifiers: KeyModifiers)` (holding the escape map) and call that from `App`.

### 12. [SMELL] Mysterious Name — typo'd `Grid` variants
- **Where:** `src/app/tabs.rs:7-10`
- **What:** `Grid::HORIZONTALE`, `Grid::SQUIRE`, `Grid::GOLDER`.
- **Why it's a problem:** Names are the primary UI. `SQUIRE` and `GOLDER` are clearly misspellings of SQUARE and GOLDEN; `HORIZONTALE` mixes English/French. Anyone reading `draw_tab`'s `match self.grid` gets actively misled about which layout each arm produces, and typos propagate to error messages/UI labels if used.
- **Fix:** Rename to `Horizontal`, `Vertical`, `Square`, `Golden` (also resolves the clippy uppercase-acronym warnings #9-adjacent).

### 13. [SMELL] Duplicated Code — horizontal/vertical rect layout
- **Where:** `src/app/tabs.rs:40-51` (`horizontal_rects`) vs `:82-93` (`vertical_rects`)
- **What:** Both split an area into `n` equal chunks plus distribute a remainder, differing only in axis (height/y vs width/x). Same `chunk`, `remainder`, loop, `chunk + u16::from(i < remainder)` shape.
- **Why it's a problem:** Two copies of the same algorithm means a fix to the remainder-distribution (a classic off-by-one) has to be applied twice and can drift apart. The axis is the *only* difference, which is exactly the sort of thing a parameter captures.
- **Fix:** One `split_along_axis(area, n, vertical: bool)` (or two thin wrappers calling a shared core). Borderline due to short bodies, but cheap and removes a real drift risk.

### 14. [SMELL] Primitive Obsession — `golden_rects` works in tuples
- **Where:** `src/app/tabs.rs:97-117`
- **What:** `golden_rects` uses `Vec<(u16,u16,u16,u16)>` for regions, converting to `Rect` only at the end, while every sibling layout method manipulates `Rect` directly.
- **Why it's a problem:** Inconsistent with its siblings; tuple indices (`.0`, `.1`, `.2`, `.3`) are unreadable and error-prone compared to `.x/.y/.width/.height`, and the code couldn't use `Rect` helper methods (`saturating_sub`, `inner`) inside the loop. Tuples carry no meaning about what each element is.
- **Fix:** Work with `Rect` and modify copies, or use named fields; convert at the boundary only.

### 15. [SMELL] Primitive Obsession / magic literal — scrollback `1200` in three places
- **Where:** `src/app/pane.rs:115` (parser init), `:202` (`scroll_to_bottom`), `:239` (`at_bottom`)
- **What:** The scrollback buffer size `1200` is hardcoded three times with no shared name.
- **Why it's a problem:** Three literal touchpoints that must stay in lockstep. If the parser's scrollback depth changes in one place but not another, `at_bottom()` and `scroll_to_bottom()` silently disagree about what "bottom" means, producing off-by-one scroll behaviour that's hard to trace.
- **Fix:** `const SCROLLBACK_SIZE: usize = 1200;` and reference it everywhere.

### 16. [SMELL] Data Clump / Divergent Change — Pane's shared-state wiring
- **Where:** `src/app/pane.rs:75-88`
- **What:** Five `Arc<Mutex<…>>`/`Arc<AtomicBool>` fields (`vpty`, `pty_writer`, `screen_changed`, and the `title`/`title_shared`/`title_changed` trio) that must be created, cloned, and kept coherent between Pane, its reader thread, and `MuxCallbacks`.
- **Why it's a problem:**
  - **Data Clump (title trio):** `title` (the Pane copy), `title_shared` (the shared source) and `title_changed` (the dirty flag) always travel and co-change together; they clearly want to be one `SharedTitle` type so the coupling is explicit and re-used in one place.
  - **Divergent Change:** `Pane` mixes several unrelated concerns — terminal emulation (`vpty`), PTY I/O plumbing (`pty_writer`, `pty_master`), thread signalling (`screen_changed`), scrolling state, and rendering. Each of those changes for different reasons, but they're all glued into one struct by hand-wiring shared state. The risk is that a change to one concern (e.g. the title channel) destabilises render or input paths, and the `Arc<Mutex<>>` wiring is easy to get wrong (deadlock, missed `swap(false)`).
- **Fix:** Bundle the title triple into a small struct; consider consolidating the shared channel objects so their lifecycle is created/consumed in one place rather than five parallel fields.

### 17. [SMELL] Mysterious Name — `scroll_to_input`
- **Where:** `src/app/pane.rs:187-194`
- **What:** The name suggests "go to where I am typing", but the body computes `cursor_row - visible_lines` — scolling so the cursor line sits at the *top* of the viewport.
- **Why it's a problem:** The name promises convenience-seeking behaviour; the implementation actually pins the cursor to the top edge. A future reader (or the call site in `run()`) will misread intent and it couples oddly with `home`. The vagueness also obscures that it writes directly to `self.scroll_offset` rather than going through `set_scroll_offset`.
- **Fix:** Rename to something accurate, e.g. `scroll_cursor_to_top` / `align_cursor_to_viewport_top`, and route through `set_scroll_offset` for consistency.

### 18. [SMELL] Speculative Generality — `App.home` flag
- **Where:** `src/app/app.rs:16`, set in `main.rs:31`, toggled in `app.rs:122/129/137/145-148`, read only in `run()` (`:33`).
- **What:** `home: bool` is initialised in `main`, flipped by several scroll-key branches, and consumed in exactly one place to decide whether to call `scroll_to_input`.
- **Why it's a problem:** The flag's meaning is implicit and only coherent if every write site stays consistent; it's scattered across the event handler as a side effect. Its value (a scroll-mode toggle) is really property of the Pane's scroll state, not of the whole App. It smells like generality the caller doesn't need — a second consumer would immediately need the invariant documented.
- **Fix:** Fold the "is scrolled home / at input" state into Pane's scroll model (it already knows offset vs cursor), or compute the needed behaviour from scratch each `run()` iteration instead of carrying a bool.
