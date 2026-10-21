use crate::app::command;
use crate::app::config::Config;
use crate::app::events::{PaneEvent, PaneId};
use crate::app::pane::Pane;
use crate::app::tabs::Tab;
use crate::app::util::initialize_pane_size;
use crate::app::workspace::{SpawnContext, Workspace, list_sessions, next_in_cycle};
use anyhow::Result;
use crossterm::event::Event;
use crossterm::terminal::size;
use ratatui::DefaultTerminal;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

mod commands;
mod render;

pub struct App {
    pub tabs: Vec<Tab>,
    pub running: bool,
    pub active_tab: usize,
    /// Panes send `PaneEvent`s here from their background threads.
    events_tx: Sender<PaneEvent>,
    events_rx: Receiver<PaneEvent>,
    next_pane_id: u64,
    /// Whether the next loop iteration needs to redraw.
    dirty: bool,
    last_size: (u16, u16),
    config: Config,
    /// The session this run saves to on exit and on session switches.
    session_name: String,
    show_pop_up: bool,
}

impl App {
    pub fn new(session_name: &str) -> Result<Self> {
        let config = Config::load()?;

        let (events_tx, events_rx) = mpsc::channel();
        let (term_cols, term_rows) = size()?;
        let term_rows = term_rows.max(1);
        let term_cols = term_cols.max(1);
        let (rows, cols) = initialize_pane_size(term_rows, term_cols);

        let mut app = App {
            tabs: Vec::new(),
            running: true,
            active_tab: 0,
            events_tx,
            events_rx,
            next_pane_id: 0,
            dirty: true,
            last_size: (term_cols, term_rows),
            config,
            session_name: session_name.to_string(),
            show_pop_up: false,
        };
        // Restore the saved workspace if one exists; a workspace that fails
        // to load (corrupt file, unreadable) must not stop startup.
        let workspace = match Workspace::load(session_name) {
            Ok(workspace) => workspace,
            Err(err) => {
                log::warn!("failed to load workspace '{session_name}': {err}");
                Workspace::default()
            }
        };
        if !workspace.tabs.is_empty() {
            let mut spawn = app.spawn_context(rows, cols);
            workspace.apply(&mut app, &mut spawn)?;
        }
        if app.tabs.is_empty() {
            let tab = app.fresh_tab(rows, cols)?;
            app.tabs.push(tab);
        }
        Ok(app)
    }

    /// Persist the current layout under the current session name. Save
    /// failures are logged, never fatal: this runs on exit and before
    /// session switches, where crashing would be far worse than losing a
    /// save.
    pub fn save_workspace(&self) {
        let workspace = Workspace::from_app(self);
        if let Err(err) = Workspace::save(&self.session_name, &workspace) {
            log::warn!("failed to save workspace '{}': {err}", self.session_name);
        }
    }

    /// Replace the current layout with the session stored under `name`.
    /// Switching to a different session saves the current one first, so
    /// cycling never loses work; reloading the *current* name skips that
    /// save, making it a revert to the on-disk state. A failed or missing
    /// file keeps the current layout; a session that applies badly (or is
    /// empty) falls back to a single fresh tab rather than leaving the app
    /// with nothing to display.
    fn switch_session(&mut self, name: &str) {
        let workspace = match Workspace::load(name) {
            Ok(workspace) => workspace,
            Err(err) => {
                log::warn!("failed to load session '{name}': {err}");
                return;
            }
        };
        if name != self.session_name {
            self.save_workspace();
        }

        let (rows, cols) = initialize_pane_size(self.last_size.1, self.last_size.0);
        let mut spawn = self.spawn_context(rows, cols);
        // Dropping the old tabs tears down their panes' PTYs (bounded joins,
        // see PtySession::drop) before the restored ones take their place.
        self.tabs.clear();
        if let Err(err) = workspace.apply(self, &mut spawn) {
            log::warn!("failed to restore session '{name}': {err}");
        }
        if self.tabs.is_empty() {
            match self.fresh_tab(rows, cols) {
                Ok(tab) => self.tabs.push(tab),
                Err(err) => {
                    log::error!("failed to spawn a fallback tab: {err}");
                    self.running = false;
                    return;
                }
            }
        }
        self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        self.session_name = name.to_string();
        self.dirty = true;
    }

    /// Switch to the session `delta` steps away in the sorted list on disk.
    fn cycle_session(&mut self, delta: isize) {
        let Some(name) = next_in_cycle(&list_sessions(), &self.session_name, delta) else {
            return;
        };
        self.switch_session(&name);
    }

    /// A single default tab, used when a restored session leaves the app
    /// with nothing to display.
    fn fresh_tab(&mut self, rows: u16, cols: u16) -> Result<Tab> {
        let id = self.next_pane_id();
        let tx = self.events_tx.clone();
        Tab::new(rows, cols, id, tx, &self.config.terminal)
    }

    /// runs the application's main loop until the user quits
    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while self.running {
            self.pump()?;
            if !self.running {
                break;
            }
            self.update_all(terminal)?;
            if self.dirty {
                terminal.draw(|frame| self.draw(frame))?;
                self.dirty = false;
            }
        }
        Ok(())
    }

    fn next_pane_id(&mut self) -> PaneId {
        let id = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        id
    }

    /// Sync the id counter after `Workspace::apply` spawned panes through a
    /// [`SpawnContext`], so later panes never reuse a restored pane's id.
    pub fn set_next_pane_id(&mut self, next: u64) {
        self.next_pane_id = next;
    }

    /// Everything `Workspace::apply` needs to spawn panes: the pane size,
    /// the event channel, the next free pane id, and the base terminal
    /// config saved overrides are layered onto.
    fn spawn_context(&self, rows: u16, cols: u16) -> SpawnContext {
        SpawnContext::new(
            rows,
            cols,
            self.events_tx.clone(),
            self.next_pane_id,
            &self.config.terminal,
        )
    }

    /// Wait briefly for a pane event, then drain everything pending: pane
    /// events first, then keys and terminal resizes. Output from background
    /// tabs wakes the loop exactly like output from the active one.
    fn pump(&mut self) -> Result<()> {
        match self.events_rx.recv_timeout(Duration::from_millis(16)) {
            Ok(event) => self.handle_pane_event(event),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
        }
        while let Ok(event) = self.events_rx.try_recv() {
            self.handle_pane_event(event);
        }

        while crossterm::event::poll(Duration::ZERO)? {
            match crossterm::event::read()? {
                Event::Key(key) => {
                    let is_alternate = self.active_pane().in_alternated_state();
                    self.execute(command::resolve(
                        key,
                        is_alternate,
                        &self.config.keybindings,
                    ))?;
                }
                Event::Resize(_, _) => self.dirty = true,
                _ => {}
            }
        }
        Ok(())
    }

    fn handle_pane_event(&mut self, event: PaneEvent) {
        match event {
            PaneEvent::Output(id) => {
                if let Some(pane) = self.find_pane_mut(id) {
                    pane.ack_output();
                }
                self.dirty = true;
            }
            PaneEvent::Exited(id) => {
                if let Some(pane) = self.find_pane_mut(id) {
                    pane.mark_exited();
                }
                self.dirty = true;
            }
            PaneEvent::Title(id, title) => {
                if let Some(pane) = self.find_pane_mut(id) {
                    pane.set_title(title);
                }
                self.dirty = true;
            }
        }
    }

    fn find_pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.tabs
            .iter_mut()
            .flat_map(|tab| tab.panes.iter_mut())
            .find(|pane| pane.id() == id)
    }

    /// Push the current layout into every pane of every tab, not just the
    /// visible one, so background shells see resizes and title changes
    /// immediately. Drawing stays read-only.
    fn update_all(&mut self, terminal: &DefaultTerminal) -> Result<()> {
        let size = terminal.size()?;
        if self.last_size != (size.width, size.height) {
            self.last_size = (size.width, size.height);
            self.dirty = true;
        }
        let (_, content_area) =
            render::split_areas(ratatui::layout::Rect::new(0, 0, size.width, size.height));
        for tab in &mut self.tabs {
            tab.update(content_area);
        }
        Ok(())
    }

    pub(super) fn get_tab(&self) -> &Tab {
        self.tabs.get(self.active_tab).unwrap()
    }
    pub(super) fn get_mut_tab(&mut self) -> &mut Tab {
        self.tabs.get_mut(self.active_tab).unwrap()
    }
    pub(super) fn active_pane(&self) -> &Pane {
        let tab = self.get_tab();
        &tab.panes[tab.active]
    }
    pub(super) fn active_pane_mut(&mut self) -> &mut Pane {
        let tab = self.get_mut_tab();
        &mut tab.panes[tab.active]
    }
}
