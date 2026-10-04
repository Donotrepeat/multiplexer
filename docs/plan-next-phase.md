# Next Phase Plan: Config + Sessions

Derived from `TODO.md` and a read-through of the current implementation (2026-10-04). The codebase is a single-context Rust terminal multiplexer using `ratatui`, `crossterm`, `portable-pty`, and `vt100`.

## Goals

1. **Config file** — a user-editable config that overrides default keybindings and supplies terminal defaults (shell, working directory, environment).
2. **Sessions** — persist and restore the layout of tabs/panes across restarts, so the multiplexer reopens where the user left off.

## Non-goals

- Mouse/selection (already in `TODO.md` future work).
- Per-pane scrollback persistence (too much data; restore starts fresh shells).
- Process/TTY state restoration (not feasible with `portable-pty`).
- Remote/multiplexed sessions across machines.

## Dependencies to add

- `toml = "0.8"`
- `serde = { version = "1", features = ["derive"] }`
- `dirs = "5"` (or `directories = "5"` for typed project dirs)

`clap` is deliberately deferred; the first milestone uses no CLI flags.

---

## 1. Config module

### Location

`$XDG_CONFIG_HOME/multiplexer/config.toml`, falling back to `~/.config/multiplexer/config.toml`. The file is optional; missing file == defaults.

### Example file

```toml
[keybindings]
alt-c = "new_tab"
alt-e = "next_tab"
alt-q = "prev_tab"
alt-j = "cycle_grid"
alt-r = "delete_pane"
alt-n = "next_pane"
alt-t = "new_pane"
alt-v = "paste"
alt-w = "quit"
home = "scroll_to_top"
end = "scroll_to_bottom"
pageup = "scroll_page_up"
pagedown = "scroll_page_down"

[terminal]
shell = "/bin/zsh"
working_dir = "/home/wouter/projects"
```

### Proposed seam

New module `src/app/config.rs`. It is the **only** place that knows about TOML, file paths, and default merging.

```rust
pub struct Config {
    pub keybindings: KeyMap,
    pub terminal: TerminalConfig,
}

impl Config {
    /// Load from the XDG config path; return defaults if the file is missing.
    pub fn load() -> Result<Self>;
}

pub struct KeyMap {
    /// Resolved bindings layered on top of built-in defaults.
    /// User bindings win; unrecognized keys/commands are logged and ignored.
    bindings: HashMap<KeySpec, Command>,
}

impl KeyMap {
    pub fn resolve(&self, key: KeyEvent, alternate: bool) -> Command;
}

pub struct TerminalConfig {
    pub shell: Option<PathBuf>,
    pub working_dir: Option<PathBuf>,
    pub env: HashMap<String, String>,
}

impl TerminalConfig {
    pub fn shell(&self) -> PathBuf;
    pub fn working_dir(&self) -> PathBuf;
    pub fn env(&self) -> Vec<(String, String)>;
}
```

### Wiring changes

- `command::resolve(key, alternate)` becomes `command::resolve(key, alternate, &config.keybindings)`.
- `App::new()` loads `Config::load()` once and stores it.
- `PtySession::spawn` gains parameters for shell, working directory, and environment, sourced from `Config::terminal`.

### Key representation

`KeySpec` is an internal serde-friendly type. Serial form is lowercase: `alt-c`, `ctrl-shift-t`, `home`, `f1`, etc. Parsed with clear error messages. The parser lives inside `config.rs` so no other module sees string keys.

---

## 2. Sessions (workspace persistence)

### Terminology

- User-facing: **session** (a saved workspace of tabs/panes).
- Code module: `src/app/workspace.rs` to avoid collision with the existing `pane::session::PtySession`.

### Location

`$XDG_STATE_HOME/multiplexer/sessions/` (or `XDG_DATA_HOME` if state dir is unavailable), falling back to `~/.local/state/multiplexer/sessions/`.

### Example file (`default.toml`)

```toml
name = "default"
active_tab = 0

[[tabs]]
grid = "horizontal"
active_pane = 0

[[tabs.panes]]
shell = "/bin/zsh"
working_dir = "/home/wouter/projects/multiplexer"

[[tabs.panes]]
shell = "/bin/zsh"
working_dir = "/home/wouter/projects/multiplexer"
```

### Proposed seam

```rust
pub struct Workspace {
    pub active_tab: usize,
    pub tabs: Vec<TabLayout>,
}

pub struct TabLayout {
    pub grid: Grid,
    pub active_pane: usize,
    pub panes: Vec<PaneLayout>,
}

pub struct PaneLayout {
    pub shell: Option<PathBuf>,
    pub working_dir: Option<PathBuf>,
}

impl Workspace {
    pub fn load(name: &str) -> Result<Option<Self>>;
    pub fn save(name: &str, workspace: &Self) -> Result<()>;

    /// Capture the current app state. Cheap; does not touch PTY internals.
    pub fn from_app(app: &App) -> Self;

    /// Apply a workspace to an empty app, spawning panes as configured.
    pub fn apply(self, app: &mut App, config: &Config) -> Result<()>;
}
```

### Lifecycle

- On startup, `App::new()` loads the "default" workspace if it exists. If not, it creates a single default tab as today.
- On clean exit, the app saves the current workspace back to "default".
- Explicit save/load keybindings and named sessions come in milestone 3.

### Wiring changes

- `App` needs access to `Config` and the active workspace name.
- `App::new()` signature becomes roughly `App::new(config: Config, workspace: Option<Workspace>)`.
- The `Drop`/`disable_raw_mode` cleanup path must still run even if workspace save fails; save errors are logged, not fatal.

---

## 3. Milestones

### M1: Config file

1. Add `toml`, `serde`, `dirs` to `Cargo.toml`.
2. Create `src/app/config.rs` with `Config`, `KeyMap`, `TerminalConfig`.
3. Replace hardcoded `ALT_BINDINGS` / `SCROLL_BINDINGS` in `command.rs` with `KeyMap::resolve`.
4. Thread `&Config` through `App` to `command::resolve`.
5. Use `TerminalConfig` in `PtySession::spawn`.
6. Tests: parse example config, override one binding, unknown key/command ignored, missing file defaults.

Acceptance:
- `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` green.
- A config file with `alt-x = "new_tab"` makes `Alt+X` open a tab.
- A config file with `terminal.shell = "/bin/bash"` makes new panes use bash.

### M2: Auto-save/restore default workspace

1. Create `src/app/workspace.rs` with `Workspace`, `TabLayout`, `PaneLayout`.
2. Add serde support for `Grid`.
3. On startup, try to load `default` workspace; fall back to single default tab.
4. On exit, capture current state and save to `default`.
5. Tests: round-trip a workspace through TOML; restore spawns the expected number of panes.

Acceptance:
- Open multiplexer, create two tabs with panes, quit, reopen: layout restores.
- Deleting/restoring panes does not corrupt the saved file.

### M3: Named sessions + in-app commands

1. Add commands `SaveSession`, `LoadSession`, `NextSession`/`PrevSession` (or a picker later).
2. Bind them in config (no hardcoded keys).
3. Simple CLI arg parsing without `clap`: `multiplexer --session <name>`.
4. Tests: save to `foo`, load `foo`, list sessions.

Acceptance:
- `multiplexer --session work` restores the `work` session.
- In-app keybinding saves current layout to a named session.

---

## 4. Open decisions

1. **Config path name**: `~/.config/multiplexer/config.toml` vs a dotfile in `$HOME`. The XDG path is recommended; confirm if you want legacy support.
2. **Session storage path**: `~/.local/state/multiplexer/sessions/` is preferred for state. Acceptable?
3. **Working directory capture**: capture each pane's actual `cwd` (requires `/proc/<pid>/cwd` or `lsof` on Unix) vs just using the configured `terminal.working_dir`. Actual `cwd` is more useful but OS-specific. For M2, use `terminal.working_dir`; M3 can add real cwd detection.
4. **Default auto-save on exit**: should quitting always overwrite `default`, or only when the user has opted in? Recommended: always auto-save `default`; explicit named saves are opt-in.
5. **Existing `PtySession` name**: keep it or rename to `Pty` to free the word "session" for the user-facing feature? Renaming is a small refactor; it reduces confusion but adds diff noise.

---

## 5. Risks

- **Keybinding parse errors**: must not crash startup. Log and ignore invalid entries.
- **Workspace save on panic**: the panic hook disables raw mode first; workspace save should happen only on normal exit, not in the panic hook.
- **Restore with missing shell**: if a saved shell path no longer exists, fall back to `Config::terminal.shell()` then `$SHELL` then `/bin/bash`.
- **Concurrency**: workspace save reads `App` state while background threads own panes. Only serializable metadata is captured; no PTY locks needed.

---

## 6. Suggested first commit

A minimal slice that lands M1 config loading + one override, without touching sessions. This keeps reviews small and proves the seam before adding persistence.
