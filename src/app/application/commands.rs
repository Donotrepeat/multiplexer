use crate::app::application::App;
use crate::app::command::Command;
use anyhow::Result;
use arboard::Clipboard;
use crossterm::event::KeyEvent;
use crossterm::terminal::size;

impl App {
    pub(super) fn execute(&mut self, command: Command) -> Result<()> {
        match command {
            Command::Quit => self.running = false,
            Command::NewTab => {
                let (term_cols, term_rows) = size()?;
                let term_rows = term_rows.max(1);
                let term_cols = term_cols.max(1);
                let (rows, cols) = crate::app::util::initialize_pane_size(term_rows, term_cols);
                let id = self.next_pane_id();
                let tx = self.events_tx.clone();

                self.tabs.push(crate::app::tabs::Tab::new(
                    rows,
                    cols,
                    id,
                    tx,
                    &self.config.terminal,
                )?);

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
                let new_pane =
                    crate::app::pane::Pane::new(new_rows, new_cols, id, tx, &self.config.terminal)?;
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
            Command::SaveSession(name) => {
                if let Some(name) = name {
                    self.session_name = name;
                }
                self.save_workspace();
            }
            Command::LoadSession(name) => {
                let name = name.unwrap_or_else(|| self.session_name.clone());
                self.switch_session(&name);
            }
            Command::NextSession => self.cycle_session(1),
            Command::PrevSession => self.cycle_session(-1),
            Command::SendKey(key) => self.send_key(key)?,
            Command::ShowBindings => self.show_pop_up = !self.show_pop_up,
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
}
