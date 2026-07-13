# Plan: Bridge vt100 Screen into ratatui Widget

## Status
- **Current code**: Single `src/main.rs` (148 lines). Does not compile (`term_buffer` undefined on line 56).
- **Goal**: Display PTY shell output (parsed through `vt100::Parser`) inside a ratatui widget, with bidirectional I/O: keyboard enters the PTY, and PTY output renders through ratatui.

## Problem

Two independent systems both writing to stdout with no bridge:

| System | Output target | Where it runs |
|--------|---------------|---------------|
| ratatui | stdout via crossterm backend | Main thread, `ratatui::run()` blocks |
| vt100 reader thread | stdout via `screen.contents_formatted()` + `write_all` | Background thread |

Issues:
1. `ratatui::run()` is blocking → lines 77+ (PTY/vt100 setup) are dead code
2. Even if reached, both systems race writing to the same stdout
3. `term_buffer` reference is undefined
4. `handle_events()` is `todo!()`

---

## Design

### High-level architecture

```
┌─────────────────────────────────────────────────────┐
│                   main()                             │
│                                                      │
│  ┌──────────────────────────────┐                    │
│  │   PTY setup (portable-pty)   │                    │
│  │   Reader thread spawning     │                    │
│  │   Arc<Mutex<vt100::Parser>>  │                    │
│  └──────────┬───────────────────┘                    │
│             │                                        │
│  ┌──────────▼───────────────────┐                    │
│  │   ratatui::run(main_loop)    │                    │
│  │                              │                    │
│  │  ┌────────────────────────┐  │                    │
│  │  │   terminal.draw()      │  │                    │
│  │  │   → read vpty          │  │                    │
│  │  │   → convert to Text    │  │                    │
│  │  │   → render Paragraph   │  │                    │
│  │  └────────────────────────┘  │                    │
│  │                              │                    │
│  │  ┌────────────────────────┐  │                    │
│  │  │   event::read()        │  │                    │
│  │  │   → Ctrl+Q → exit      │  │                    │
│  │  │   → other → PTY writer │  │                    │
│  │  └────────────────────────┘  │                    │
│  └──────────────────────────────┘                    │
└─────────────────────────────────────────────────────┘
```

### Step 1 — Restructure `main()` execution order

**Current** (broken):
```
enable_raw_mode()
ratatui::run()  // blocks forever, code below never runs
pty_setup()
reader_thread → vpty.process() + stdout.write_all()
writer_thread  → stdin → pty
```

**New**:
```
enable_raw_mode()
pty_setup()
let vpty = Arc::new(Mutex::new(vt100::Parser::default()))
let pty_writer = Arc::new(Mutex::new(...))
reader_thread → vpty.process() only (no stdout writes)
// Writer thread is REMOVED — event handling is done via crossterm
// Only join reader thread after ratatui::run() returns
ratatui::run(main_loop)
// After main_loop returns, clean up
```

Rationale: The reader thread must be running before `ratatui::run()` so that PTY output is being parsed from the start. The stdin→PTY forwarding moves into the event handler.

### Step 2 — Shared `vt100::Parser`

```rust
let vpty = Arc::new(Mutex::new(vt100::Parser::default()));
let vpty_clone = Arc::clone(&vpty);
```

- **Reader thread**: holds `vpty_clone`. Calls `vpty_clone.lock().unwrap().process(&buf[..n])`.
- **Main thread (draw)**: holds `vpty`. Calls `vpty.lock().unwrap().screen()` to get the screen state (or clones the screen).

**Important**: `vt100::Parser::screen()` returns a reference to the inner `Screen`. To avoid holding the lock during the entire conversion to ratatui Text, we clone the Parser's screen under the lock and release before doing the conversion.

```rust
let screen = {
    let parser = vpty.lock().unwrap();
    // parser.screen() returns &Screen which borrows the MutexGuard
    // So we clone it to drop the lock
    parser.screen().clone()  // vt100::Screen implements Clone
};
```

### Step 3 — Shared PTY Writer

```rust
let pty_writer = Arc::new(Mutex::new(pair.master.take_writer()?));
// Clone passed into App
```

The writer is an `Arc<Mutex<portable_pty::master::MasterPtyWriter>>`. `handle_events` locks it to forward key bytes.

### Step 4 — Remove direct stdout writes from reader thread

Current:
```rust
loop {
    match reader.read(&mut buf) {
        Ok(n) => {
            vpty.process(&buf[..n]);
            let screen = vpty.screen();
            let _ = stdout.write_all(&screen.contents_formatted());  // REMOVE
            let _ = stdout.flush();                                  // REMOVE
        }
        ...
    }
}
```

New:
```rust
loop {
    match reader.read(&mut buf) {
        Ok(n) => vpty.process(&buf[..n]),
        ...
    }
}
```

### Step 5 — Convert vt100::Screen to ratatui Text (new function)

```rust
fn vterm_to_ratatui(screen: &vt100::Screen) -> Text<'static> {
    let size = screen.size();
    // Build an empty fill row (spaces with default style) for the non-occupied area
    // Then iterate each row, then each column within that row
}
```

**vt100 → ratatui Color mapping**:

| `vt100::Color` | `ratatui::style::Color` |
|----------------|------------------------|
| `Color::Default` | `Color::Reset` |
| `Color::Idx(n)` where n < 16 | `Color::Indexed(n)` — crossterm/ratatui use the same xterm-256color palette for idx 0-15 |
| `Color::Idx(n)` where n >= 16 | `Color::Indexed(n)` |
| `Color::Rgb(r, g, b)` | `Color::Rgb(r, g, b)` |
| `Color::Ansicolor(c)` (8-color) | `Color::Indexed(c as u8)` — vt100's `Ansicolor` enum maps 0-7 |

**vt100 → ratatui Style mapping**:

Cell attributes from `vt100::attrs::Attrs`:
| Attr flag | ratatui `Style` method |
|-----------|----------------------|
| `bold` | `.bold()` |
| `italic` | `.italic()` |
| `underline` | `.underline()` |
| `fgcolor` | `.fg(color)` |
| `bgcolor` | `.bg(color)` |
| `inverse` | swap fg/bg |
| `strike` | `.crossed_out()` |

**Row-building algorithm**:
```
for each row_index in 0..screen.rows():
    row_cells = screen.row(row_index)  // returns &[Cell]
    spans: Vec<Span> = []
    for each cell in row_cells:
        char = cell.contents()  // string, may be multi-byte (wide chars)
        attrs = cell.attrs()
        span_style = convert_attrs_to_style(attrs)
        spans.push(Span::styled(char, span_style))
    lines.push(Line::from(spans))
// remaining rows below screen contents: fill with empty spaces
for remaining rows:
    lines.push(Line::from(vec![Span::raw(" ".repeat(screen.cols()))]))
Text::from(lines)
```

**Empty rows handling**: The vt100 `Screen` may have fewer rows than the ratatui rendering area. Fill the gap with empty `Line`s so the Paragraph spans the full widget area.

### Step 6 — Update `App` struct and `draw` method

```rust
struct App {
    vpty: Arc<Mutex<vt100::Parser>>,
    pty_writer: Arc<Mutex<portable_pty::master::MasterPtyWriter>>,
    running: bool,
}
```

`draw` method:
```rust
fn draw(&self, frame: &mut Frame) {
    let screen = self.vpty.lock().unwrap().screen().clone();
    let text = vterm_to_ratatui(&screen);

    let block = Block::bordered()
        .title(" multiplexer ".bold())
        .border_set(border::THICK);
    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, frame.area());
}
```

- Removed the `Widget for &App` impl — rendering is done directly in `App::draw()` via `Paragraph`.
- The block title changes from "Counter App Tutorial" to "multiplexer".

### Step 7 — Event handling (forward to PTY + local quit)

```rust
fn handle_events(&mut self) -> Result<()> {
    if crossterm::event::poll(std::time::Duration::from_millis(100))? {
        match crossterm::event::read()? {
            Event::Key(KeyEvent { code: KeyCode::Char('q'), modifiers: KeyModifiers::CONTROL, .. }) => {
                self.running = false;
            }
            // Resize events
            Event::Resize(cols, rows) => {
                let mut parser = self.vpty.lock().unwrap();
                parser.set_size(rows, cols);
                // Also resize the PTY if we have access to the pair
                // (This needs the pty pair handle — see note below)
            }
            // Forward everything else to the PTY
            Event::Key(key_event) => {
                // Convert KeyEvent to bytes and write to PTY
                let mut writer = self.pty_writer.lock().unwrap();
                // This is the tricky part — see below
            }
            _ => {}
        }
    }
    Ok(())
}
```

**Key event → byte conversion**:

This is the hardest part. The terminal emulator (crossterm) gives us decoded `KeyEvent`s. We need to encode them back into the byte sequences a shell expects.

For most **printable characters** (letters, numbers, symbols), the byte(s) are just the UTF-8 encoding of the character. That's straightforward.

For **special keys** (Enter, Backspace, Tab, Escape, arrows, function keys), we need to emit the correct escape sequences:
- `Enter` → `b'\n'` or `b"\r\n"` (typically `b'\r'`)
- `Backspace` → `b'\x7f'` (DEL) or `b'\x08'` (BS)
- `Tab` → `b'\t'`
- `Escape` → `b'\x1b'`
- `ArrowLeft` → `b"\x1b[D"`
- `ArrowRight` → `b"\x1b[C"`
- `ArrowUp` → `b"\x1b[A"`
- `ArrowDown` → `b"\x1b[B"`
- `Home` → `b"\x1b[H"` or `b"\x1b[1~"`
- `End` → `b"\x1b[F"` or `b"\x1b[4~"`
- `Delete` → `b"\x1b[3~"`
- `PageUp` → `b"\x1b[5~"`
- `PageDown` → `b"\x1b[6~"`
- `F(n)` → `b"\x1b[NN~"` (where NN = 10+n for F1-F4, etc.)

For **Ctrl+letter** combinations:
- Convert to the corresponding control character (e.g., `Ctrl+A` → `b'\x01'`, `Ctrl+C` → `b'\x03'`)

For **Alt+key** combinations:
- Prefix with ESC (`b"\x1b"`) followed by the key byte

We'll write a helper function `key_event_to_bytes(key_event: KeyEvent) -> Vec<u8>`.

**Edge case — Ctrl+C and Ctrl+Z**: These are typically used by the shell (SIGINT, SIGTSTP). Since we're forwarding raw bytes, the shell will handle them naturally through the PTY.

**Edge case — Ctrl+Q**: Reserved for quitting the multiplexer. Check modifier first, do not forward.

### Step 8 — Handle terminal resize

When `Event::Resize(cols, rows)` fires:
1. Call `parser.set_size(rows, cols)` on the vt100 parser (so the virtual screen matches the new size)
2. Call `pair.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })` so the child process (shell) is notified of the size change

This requires the `PtyPair` handle to be accessible. Options:
- Store it in an `Arc<Mutex<Option<PtyPair>>>` and share
- Or, more simply, don't resize the PTY pair for now (the shell will only see 24x80)
- Better: add `pty_pair: Option<portable_pty::PtyPair>` to `App` (only the main thread accesses it)

We'll add `pty_pair` to `App` for resize support.

### Step 9 — Remove `writer_thread` (stdin forwarding)

Current code has a separate writer thread that reads from stdin and writes to the PTY. This conflicts with crossterm's raw mode and event handling. Remove it entirely — keyboard forwarding is done in `App::handle_events()`.

### Step 10 — Clean up and error handling

- After `ratatui::run()` returns, join the reader thread (it will exit when the PTY is closed)
- Wait for the child process
- Disable raw mode

---

## vt100 API Reference

Key vt100 types and methods we need:

| Type/method | Purpose |
|-------------|---------|
| `vt100::Parser::default()` | Create parser (80x24 default) |
| `parser.process(bytes)` | Feed raw PTY bytes |
| `parser.screen()` | Get `&Screen` (borrows the parser) |
| `parser.set_size(rows, cols)` | Resize the virtual terminal |
| `Screen::size()` | Returns `(rows, cols)` |
| `Screen::row(i)` | Returns `&[Cell]` for row i |
| `Screen::rows()` | Returns number of rows |
| `Screen::cols()` | Returns number of columns |
| `Cell::contents()` | Returns `&str` (the character(s) at this cell, handles wide chars) |
| `Cell::attrs()` | Returns `Attrs` |
| `Attrs::fgcolor()` | Returns `Option<Color>` |
| `Attrs::bgcolor()` | Returns `Option<Color>` |
| `Attrs::bold()` | Returns `bool` |
| `Attrs::italic()` | Returns `bool` |
| `Attrs::underline()` | Returns `bool` |
| `Attrs::inverse()` | Returns `bool` |
| `Attrs::strike()` | Returns `bool` |
| `Color::Default` | Default fg/bg |
| `Color::Idx(u8)` | Indexed color (0-255) |
| `Color::Rgb(u8, u8, u8)` | True color |

---

## Files to modify

| File | Change |
|------|--------|
| `src/main.rs` | Full rewrite (~250 lines) |
| `Cargo.toml` | No changes needed (all deps present) |

---

## Verification

```bash
cargo build
```

The application should:
1. Open a PTY with the user's shell
2. Display the shell prompt inside a ratatui `Paragraph` with a bordered block titled "multiplexer"
3. Accept keyboard input — most keys forwarded to the shell, `Ctrl+Q` exits
4. Handle terminal resize
5. Render with correct colors, bold, italic, underline from the PTY output

---

## Future work (not in scope)

- Cursor rendering as inverted cell
- Scrollback buffer for browsing history
- Split panes / multiple terminals
- Mouse support
- Clipboard integration
