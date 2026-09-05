# TODO — Code Quality Findings

Credits: Standards axis derived from the Fowler smell baseline; tooling differences from `cargo build`, `cargo clippy --all-targets`, and `cargo test`.

Each item lists: what's wrong → why it's a problem → where → suggested fix. Findings are ranked roughly by priority.

**Tooling state (re-verified 2026-09-05):** `cargo test` — 9/9 green. `cargo clippy --all-targets` — **6 warnings**: `module_inception` (#12), `collapsible_if` (#13), and four `upper_case_acronyms` on the `Grid` variants (cleared by the rename in #16).

**History:** former #1–#4 and #6–#8 were verified fixed and pruned. Former #5 is partially fixed — one collapsible `if` remains (#13). Former #9–#18 live on as #12, #14–#22. New in this revision: correctness bugs #1–#7 and tooling/hygiene findings #8–#11.

---

## Correctness bugs (hard problems — fix first)

### 1. [BUG] Scroll direction is inverted
- **Where:** `src/app/pane.rs:197-199` (`scroll_to_top`), `:202-204` (`scroll_to_bottom`), `:234-236` (`at_bottom`); wired to Home/End at `src/app/app.rs:116-128`
- **What:** vt100's scrollback offset counts from the *bottom*: `set_scrollback(0)` shows the live screen, and larger values reach further back into history (vt100 0.16.2 `Screen::set_scrollback` docs). The code assumes the opposite: `scroll_to_top()` sets offset `0` — jumping to the **bottom** (live view) — while `scroll_to_bottom()` sets `1200`, jumping to the **top** (deepest history). `at_bottom()` (`offset >= 1200`) is therefore true exactly when the view is at the *top*.
- **Why it's a problem:** Home and End do the opposite of their names, and `at_bottom()` feeds the `App.home` flag (#22), so follow-cursor mode is armed by the wrong key.
- **Fix:** Swap the semantics (`scroll_to_top` → max scrollback, `scroll_to_bottom` → `0`), or better: derive both from the parser's clamped scrollback instead of the hardcoded `1200` (see #2 and #19).

### 2. [BUG] `at_bottom()` can never be true while history is short
- **Where:** `src/app/pane.rs:234-236`
- **What:** `at_bottom()` tests `get_scroll_offset() >= 1200`, but vt100 *clamps* the scrollback offset to the actual scrollback size. Until 1200 lines of history exist, `scrollback()` returns less than 1200, so `at_bottom()` stays `false` even with the live screen in view.
- **Why it's a problem:** After pressing End on a fresh shell, `App.home` never flips back to "at input", so `run()` (`app.rs:33-36`) calls `scroll_to_input()` every frame — the user is pinned out of the bottom view.
- **Fix:** Test the real boundary (once #1 fixes the direction, live view ⇔ `scrollback() == 0`), not a magic number.

### 3. [BUG] Ctrl+letter encoding can panic (byte underflow)
- **Where:** `src/app/app.rs:161-165`
- **What:** `w.write_all(&[c as u8 - b'a' + 1])` assumes `c` is a lowercase `a..=z`. For uppercase (e.g. Ctrl+Shift+key, which some terminals report as `Char('C')`) or non-letters, `c as u8 - b'a'` underflows: panic in debug builds, garbage byte in release.
- **Why it's a problem:** A reachable panic from ordinary keyboard input; also silently wrong for the other control ranges (Ctrl+@, Ctrl+[, Ctrl+], Ctrl+_, …).
- **Fix:** Guard `matches!(c, 'a'..='z' | 'A'..='Z')` (lowercasing first) and handle the remaining control ranges explicitly — or use a key-to-bytes helper that already knows the mapping (this belongs in `Pane`, see #15).

### 4. [BUG] New tab activates the wrong tab
- **Where:** `src/app/app.rs:56-58`
- **What:** Alt+C pushes the new tab at the *end* of `self.tabs` but activates it with `self.active_tab += 1`. With tabs [A,B,C] and A active, this lands on B, not the new tab D.
- **Why it's a problem:** The tab is created but not shown — the keybinding looks broken.
- **Fix:** `self.active_tab = self.tabs.len() - 1;` after the push.

### 5. [BUG] Deleting the last pane panics
- **Where:** `src/app/tabs.rs:148-157` (`del_pane`), consumed by `src/app/app.rs:34-35` and `:92`
- **What:** With one pane left, `del_pane` computes `self.active - 1` with `active == 0` → usize underflow (debug panic). Even past that, it empties `panes`, and the next loop iteration hits `panes[active]` (`app.rs:35`) and `panes.len() - 1` (`app.rs:92`) on an empty vec.
- **Why it's a problem:** Alt+R on a single-pane tab is an ordinary user action that crashes the app.
- **Fix:** Guard in `del_pane` (`len() <= 1` → refuse, or close the tab). Closing the tab then needs App-level handling for the last remaining tab.

### 6. [BUG] Scroll state lives in three inconsistent coordinate systems
- **Where:** `src/app/pane.rs:161-166` (`set_scroll_offset` — parser scrollback), `:188-195` (`scroll_to_input` — writes the `scroll_offset` *field only*, computed from a screen-relative cursor row), `:253-263` (`render_pane` — renders cells via the parser's scrollback but positions the cursor using the field)
- **What:** The parser's scrollback (what the user sees) and `pane.scroll_offset` (what the cursor math uses) are updated by different code paths that don't agree: `scroll_to_input` never touches the parser; `set_scroll_offset` never updates the field's cursor-relative meaning.
- **Why it's a problem:** After any manual scroll plus `scroll_to_input`, the cursor is drawn at a row that doesn't match the scrolled content — two sources of truth drift apart. This is the root cause that #21/#22 orbit around.
- **Fix:** One source of truth: make the parser's scrollback the only scroll state, route all writes through a single method, and compute cursor position in the same coordinate system as `screen.cell()`.

### 7. [BUG] Alt+unhandled keys type a bare letter into the shell
- **Where:** `src/app/app.rs:166` — the fall-through `KeyCode::Char(c) => w.write_all(c.to_string().as_bytes())`
- **What:** Alt combos not claimed by the multiplexer (anything but w/c/e/q/j/r/n/t) fall into normal input handling, which sends the character without the `ESC` prefix. Alt+X becomes a literal `x` in the child shell.
- **Why it's a problem:** Contradicts PLAN.md §7 ("Alt+key → prefix with ESC"): programs with Alt bindings never see them, and stray letters appear at the prompt.
- **Fix:** In the fall-through arm, if `key.modifiers.contains(KeyModifiers::ALT)`, write `b"\x1b"` before the char bytes — or deliberately swallow unhandled Alt combos. Pick one, on purpose.

---

## Tooling & hygiene


### 9. [HYGIENE] Panic hygiene: unwraps on fallible I/O + no terminal-restoring panic hook
- **Where:** `src/app/tabs.rs:34` (`Pane::new(..).unwrap()`), `src/app/pane.rs:229` (`resize(..).unwrap()`), `src/app/app.rs:102` (`get_size().unwrap()`), and `lock().unwrap()` throughout (`app.rs:149`, `pane.rs:129`, `:162`, `:170`, `:238`)
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

> 6 remaining. Auto-detected and cheap to fix; should be zeroed out.
### 13. [WARN] Collapsible `if` in `handle_events`
- **Where:** `src/app/app.rs:43-44`
- **Why wrong:** `if poll(..)? { if let Event::Key(key) = read()? { … } }` can fold into one guard chain: `if poll(..)? && let Event::Key(key) = read()?` (let-chains are already used elsewhere in this codebase).
- **Fix:** Collapse it. (Remnant of the old #5 — the other collapsible ifs were fixed.)

> The four `upper_case_acronyms` warnings on the `Grid` variants are cleared by the rename in #16.

---

## Smell findings (Fowler baseline — judgement calls)

### 14. [SMELL] Repeated Switches — `handle_events` is one giant dispatch
- **Where:** `src/app/app.rs:42-180`
- **What:** A ~140-line `if`/`else if` cascade. Eight arms repeat the identical guard — `key.code == KeyCode::Char('X') && key.modifiers.contains(KeyModifiers::ALT)` (`:45-99`) — then a *second* nested cascade for scroll keys (`:116-143`), then a *third* `match` that maps `KeyCode` to raw escape-sequence bytes (`:151-168`).
- **Why it's a problem:**
  - **Rigidity:** Adding one new hotkey means writing another near-copy of the same guard; the failure mode is copy-paste bugs (see #4 for one that already slipped through).
  - **Mysterious control flow:** Reading it is a scan for which arm fires and in what order — the borrow juggling (repeated `self.get_tab()` / `self.get_mut_tab()` with sentinel `active` copies) exists *because* the cascade holds `self` across many paths.
  - **Divergent reasons to change:** quitting, tab ops, layout cycling, pane deletion, scrolling and character-input all live in the same method.
- **Fix:**
  - Dispatch on `(key.code, key.modifiers)` via a `match` or a keybinding table mapping `(KeyCode, KeyModifiers) -> Command`.
  - Give `Pane` a `send_key`/`scroll` interface so `App` orchestrates commands rather than performing low-level byte writes (see #15).

### 15. [SMELL] Feature Envy — `App` reaches into `Pane`'s writer to send bytes
- **Where:** `src/app/app.rs:148-169`
- **What:** `App::handle_events` locks `active_pane.pty_writer`, then hand-maps every `KeyCode` to escape bytes (`Enter -> b"\r"`, `Up -> b"\x1b[A"`, `Ctrl+c -> c as u8 - b'a' + 1`, …).
- **Why it's a problem:**
  - **Wrong home:** the `KeyCode`→bytes knowledge is intrinsic to a *pane/terminal*, not to the app shell that routes keys. `Pane` owns the writer; `Pane` should be the only thing that knows how to translate keys into bytes for its PTY.
  - **Encapsulation leak:** `App` depends on the mutable internals of `Pane` (`pty_writer` is `pub`), coupling the two modules and duplicating terminal-protocol knowledge where a second consumer would have to re-derive it.
  - **Complicates borrows:** the `active`/`get_mut_tab()` dance in the same method is a direct consequence of doing the write here instead of in a `&mut self` method on `Pane`.
- **Fix:** Add `Pane::send_key(&mut self, code: KeyCode, modifiers: KeyModifiers)` (holding the escape map) and call that from `App`. This is also the natural landing spot for the encoding guards from #3/#7.

### 16. [SMELL] Mysterious Name — typo'd `Grid` variants
- **Where:** `src/app/tabs.rs:7-10`
- **What:** `Grid::HORIZONTALE`, `Grid::SQUIRE`, `Grid::GOLDER`.
- **Why it's a problem:** Names are the primary UI. `SQUIRE` and `GOLDER` are clearly misspellings of SQUARE and GOLDEN; `HORIZONTALE` mixes English/French. Anyone reading `draw_tab`'s `match self.grid` gets actively misled about which layout each arm produces, and typos propagate to error messages/UI labels if used.
- **Fix:** Rename to `Horizontal`, `Vertical`, `Square`, `Golden`. This also clears the four `upper_case_acronyms` clippy warnings.

### 17. [SMELL] Duplicated Code — horizontal/vertical rect layout
- **Where:** `src/app/tabs.rs:40-51` (`horizontal_rects`) vs `:82-93` (`vertical_rects`)
- **What:** Both split an area into `n` equal chunks plus distribute a remainder, differing only in axis (height/y vs width/x). Same `chunk`, `remainder`, loop, `chunk + u16::from(i < remainder)` shape.
- **Why it's a problem:** Two copies of the same algorithm means a fix to the remainder-distribution (a classic off-by-one) has to be applied twice and can drift apart. The axis is the *only* difference, which is exactly the sort of thing a parameter captures.
- **Fix:** One `split_along_axis(area, n, vertical: bool)` (or two thin wrappers calling a shared core). Borderline due to short bodies, but cheap and removes a real drift risk — do it alongside the tests from #11.

### 18. [SMELL] Primitive Obsession — `golden_rects` works in tuples
- **Where:** `src/app/tabs.rs:95-119`
- **What:** `golden_rects` uses `Vec<(u16,u16,u16,u16)>` for regions, converting to `Rect` only at the end, while every sibling layout method manipulates `Rect` directly. (Bonus naming oddity in the same function: the loop variable `_k` is underscore-prefixed *and* used as an index.)
- **Why it's a problem:** Inconsistent with its siblings; tuple indices (`.0`, `.1`, `.2`, `.3`) are unreadable and error-prone compared to `.x/.y/.width/.height`, and the code couldn't use `Rect` helper methods (`saturating_sub`, `inner`) inside the loop. Tuples carry no meaning about what each element is.
- **Fix:** Work with `Rect` and modify copies, or use named fields; convert at the boundary only.

### 19. [SMELL] Primitive Obsession / magic literal — scrollback `1200` in four places
- **Where:** `src/app/pane.rs:115` (parser init), `:203` (`scroll_to_bottom`), `:235` (`at_bottom`) — plus the test helper at `:364`
- **What:** The scrollback buffer size `1200` is hardcoded four times with no shared name.
- **Why it's a problem:** Four literal touchpoints that must stay in lockstep. If the parser's scrollback depth changes in one place but not another, `at_bottom()` and `scroll_to_bottom()` silently disagree about what "bottom" means. This literal is load-bearing for bugs #1/#2 — fixing those without naming it would keep the trap armed.
- **Fix:** `const SCROLLBACK_SIZE: usize = 1200;` referenced everywhere.

### 20. [SMELL] Data Clump / Divergent Change — Pane's shared-state wiring
- **Where:** `src/app/pane.rs:75-88`
- **What:** Five `Arc<Mutex<…>>`/`Arc<AtomicBool>` fields (`vpty`, `pty_writer`, `screen_changed`, and the `title`/`title_shared`/`title_changed` trio) that must be created, cloned, and kept coherent between Pane, its reader thread, and `MuxCallbacks`.
- **Why it's a problem:**
  - **Data Clump (title trio):** `title` (the Pane copy), `title_shared` (the shared source) and `title_changed` (the dirty flag) always travel and co-change together; they clearly want to be one `SharedTitle` type so the coupling is explicit and re-used in one place.
  - **Divergent Change:** `Pane` mixes several unrelated concerns — terminal emulation (`vpty`), PTY I/O plumbing (`pty_writer`, `pty_master`), thread signalling (`screen_changed`), scrolling state, and rendering. Each of those changes for different reasons, but they're all glued into one struct by hand-wiring shared state. The risk is that a change to one concern (e.g. the title channel) destabilises render or input paths, and the `Arc<Mutex<>>` wiring is easy to get wrong (deadlock, missed `swap(false)`).
- **Fix:** Bundle the title triple into a small struct; consider consolidating the shared channel objects so their lifecycle is created/consumed in one place rather than five parallel fields.

### 21. [SMELL] Mysterious Name — `scroll_to_input`
- **Where:** `src/app/pane.rs:188-195`
- **What:** The name suggests "go to where I am typing", but the body computes `cursor_row - visible_lines` — scrolling so the cursor line sits at the *top* of the viewport.
- **Why it's a problem:** The name promises convenience-seeking behaviour; the implementation actually pins the cursor to the top edge. A future reader (or the call site in `run()`) will misread intent and it couples oddly with `home`. The vagueness also obscures that it writes directly to `self.scroll_offset` rather than going through `set_scroll_offset` — the inconsistency behind bug #6.
- **Fix:** Rename to something accurate, e.g. `scroll_cursor_to_top` / `align_cursor_to_viewport_top`, and route through `set_scroll_offset` (or whatever single write path #6 establishes).

### 22. [SMELL] Speculative Generality — `App.home` flag
- **Where:** `src/app/app.rs:16`, set in `main.rs:30`, toggled in `app.rs:121/128/136/143`, read only in `run()` (`:33`)
- **What:** `home: bool` is initialised in `main`, flipped by several scroll-key branches, and consumed in exactly one place to decide whether to call `scroll_to_input`.
- **Why it's a problem:** The flag's meaning is implicit and only coherent if every write site stays consistent; it's scattered across the event handler as a side effect. Its value (a scroll-mode toggle) is really a property of the Pane's scroll state, not of the whole App. It smells like generality the caller doesn't need — a second consumer would immediately need the invariant documented. Bugs #1/#2 currently make every write site semantically wrong anyway.
- **Fix:** Fold the "is scrolled home / at input" state into Pane's scroll model (it already knows offset vs cursor — see #6), or compute the needed behaviour from scratch each `run()` iteration instead of carrying a bool.
