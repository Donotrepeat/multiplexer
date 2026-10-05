use crate::app::command::{self, Command};
use crate::app::config::Config;
use crate::app::events::{PaneEvent, PaneId};
use crate::app::pane::Pane;
use crate::app::tabs::Tab;
use crate::app::util::initialize_pane_size;
use anyhow::Result;
use arboard::Clipboard;
use crossterm::event::{Event, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::{DefaultTerminal, Frame};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;
use unicode_width::UnicodeWidthChar;

use crossterm::terminal::size;

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
}

impl App {
    pub fn new() -> Result<Self> {
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
        };
        let id = app.next_pane_id();
        let tx = app.events_tx.clone();
        app.tabs
            .push(Tab::new(rows, cols, id, tx, &app.config.terminal)?);
        Ok(app)
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
        let (_, content_area) = split_areas(Rect::new(0, 0, size.width, size.height));
        for tab in &mut self.tabs {
            tab.update(content_area);
        }
        Ok(())
    }

    fn execute(&mut self, command: Command) -> Result<()> {
        match command {
            Command::Quit => self.running = false,
            Command::NewTab => {
                let (term_cols, term_rows) = size()?;
                let term_rows = term_rows.max(1);
                let term_cols = term_cols.max(1);
                let (rows, cols) = initialize_pane_size(term_rows, term_cols);
                let id = self.next_pane_id();
                let tx = self.events_tx.clone();

                self.tabs
                    .push(Tab::new(rows, cols, id, tx, &self.config.terminal)?);

                self.active_tab = self.tabs.len() - 1;
            }
            Command::NextTab => {
                let tab_count = self.tabs.len() - 1;
                if self.active_tab == tab_count {
                    self.active_tab = 0;
                } else {
                    self.active_tab += 1;
                }
            }
            Command::PrevTab => {
                let tab_count = self.tabs.len() - 1;
                if self.active_tab == 0 {
                    self.active_tab = tab_count;
                } else {
                    self.active_tab -= 1;
                }
            }
            Command::CycleGrid => {
                let tab = self.get_mut_tab();
                tab.grid = tab.grid.next();
            }
            Command::DeletePane => {
                let tab = self.get_mut_tab();
                tab.del_pane();
                if tab.panes.is_empty() {
                    self.tabs.remove(self.active_tab);
                    let tab_count = self.tabs.len().saturating_sub(1);
                    if self.active_tab == 0 {
                        self.active_tab = tab_count;
                    } else {
                        self.active_tab -= 1;
                    }
                }
                if self.tabs.is_empty() {
                    self.running = false;
                }
            }
            Command::NextPane => {
                if self.get_tab().active == (self.get_tab().panes.len() - 1) {
                    self.get_mut_tab().active = 0;
                } else {
                    self.get_mut_tab().active += 1;
                }
            }
            Command::Paste => {
                match Clipboard::new().and_then(|mut clipboard| clipboard.get_text()) {
                    Ok(text) => self.active_pane_mut().paste(&text)?,
                    Err(err) => log::warn!("paste failed: {err}"),
                }
            }
            Command::NewPane => {
                let (rows, cols) = self.active_pane().size();
                let new_rows = (rows / (self.get_tab().panes.len() as u16 + 1)).max(2);
                let new_cols = cols.max(2);
                let id = self.next_pane_id();
                let tx = self.events_tx.clone();
                let new_pane = Pane::new(new_rows, new_cols, id, tx, &self.config.terminal)?;
                let tab = self.get_mut_tab();
                tab.panes.push(new_pane);
                tab.active = tab.panes.len() - 1;
            }
            Command::ScrollToTop => {
                self.active_pane_mut().scroll_to_top();
            }
            Command::ScrollToBottom => {
                self.active_pane_mut().scroll_to_bottom();
            }
            Command::ScrollPageUp => {
                let visible = self.active_pane().visible_lines();
                self.active_pane_mut().scroll_up(visible);
            }
            Command::ScrollPageDown => {
                let visible = self.active_pane().visible_lines();
                self.active_pane_mut().scroll_down(visible);
            }
            Command::SendKey(key) => self.send_key(key)?,
        }
        self.dirty = true;
        Ok(())
    }

    fn send_key(&mut self, key: KeyEvent) -> Result<()> {
        let active = self.get_tab().active;
        if let Some(active_pane) = self.get_mut_tab().panes.get_mut(active) {
            active_pane.write_key(key)?;
        }
        Ok(())
    }

    fn get_tab(&self) -> &Tab {
        self.tabs.get(self.active_tab).unwrap()
    }
    fn get_mut_tab(&mut self) -> &mut Tab {
        self.tabs.get_mut(self.active_tab).unwrap()
    }
    fn active_pane(&self) -> &Pane {
        let tab = self.get_tab();
        &tab.panes[tab.active]
    }
    fn active_pane_mut(&mut self) -> &mut Pane {
        let tab = self.get_mut_tab();
        &mut tab.panes[tab.active]
    }
    fn draw(&mut self, frame: &mut Frame) {
        let (bar_area, content_area) = split_areas(frame.area());
        self.draw_bar(frame, bar_area);

        self.tabs[self.active_tab].draw_tab(frame, content_area);
    }

    fn draw_bar(&self, frame: &mut Frame, area: Rect) {
        let budgets = label_budgets(area.width as usize, self.tabs.len());
        let spans: Vec<Span> = self
            .tabs
            .iter()
            .zip(budgets)
            .enumerate()
            .map(|(i, (tab, budget))| {
                let label = format!("{}:{}", i + 1, tab.panes[tab.active].title);
                let style = if i == self.active_tab {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                Span::styled(fit_label(&label, budget), style)
            })
            .collect();

        frame.render_widget(Line::from(spans), area);
    }
}

/// Split the screen into the one-row tab bar and the content area below it.
fn split_areas(area: Rect) -> (Rect, Rect) {
    let areas = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
    (areas[0], areas[1])
}

/// Split the bar's width evenly across `n` tabs. Earlier tabs absorb the
/// remainder, so every column of the bar is assigned to exactly one tab.
fn label_budgets(width: usize, n: usize) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    let base = width / n;
    let extra = width % n;
    (0..n).map(|i| base + usize::from(i < extra)).collect()
}

/// Truncate `s` to `budget` display columns, padding it with spaces so each
/// tab occupies exactly its slot. Wide characters are never split: a glyph
/// that would cross the boundary is dropped and the slot padded instead.
/// Control characters (an OSC title can carry them) are dropped rather than
/// rendered into the bar.
fn fit_label(s: &str, budget: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        if c.is_control() {
            continue;
        }
        let width = c.width().unwrap_or(0);
        if used + width > budget {
            break;
        }
        used += width;
        out.push(c);
    }
    out.extend(std::iter::repeat_n(' ', budget - used));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn budgets_cover_the_full_width() {
        assert_eq!(label_budgets(10, 3), vec![4, 3, 3]);
        assert_eq!(label_budgets(9, 3), vec![3, 3, 3]);
        assert_eq!(label_budgets(0, 3), vec![0, 0, 0]);
        assert_eq!(label_budgets(10, 0), Vec::<usize>::new());
    }

    #[test]
    fn fit_label_truncates_and_pads_by_display_width() {
        assert_eq!(fit_label("1:abcdef", 4), "1:ab");
        assert_eq!(fit_label("1:ab", 6), "1:ab  ");
        assert_eq!(fit_label("1:日本語", 6), "1:日本");
        assert_eq!(fit_label("1:日本語", 5), "1:日 ");
        // A control character in an OSC title is dropped, not counted as a
        // zero-width glyph that would render as garbage in the bar.
        assert_eq!(fit_label("1:a\u{b}b", 5), "1:ab ");
    }

    #[test]
    fn narrow_bar_keeps_every_tab_visible() {
        let labels = ["1:this title is far too long", "2:a", "3:b", "4:c", "5:d"];
        let fitted: Vec<String> = label_budgets(40, labels.len())
            .into_iter()
            .zip(labels)
            .map(|(budget, label)| fit_label(label, budget))
            .collect();

        assert_eq!(fitted.len(), 5);
        assert!(fitted[0].starts_with("1:this"));
        assert!(fitted.iter().all(|label| !label.trim().is_empty()));
        assert_eq!(fitted.iter().map(|label| label.width()).sum::<usize>(), 40);
    }
}
