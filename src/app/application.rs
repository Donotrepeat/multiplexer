use crate::app::command::{self, Command};
use crate::app::pane;
use crate::app::tabs::Tab;
use anyhow::{Ok, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use pane::Pane;
use ratatui::{DefaultTerminal, Frame};

use crossterm::terminal::size;
use std::io::Write;
use std::sync::atomic::Ordering;

pub struct App {
    pub tabs: Vec<Tab>,
    pub running: bool,
    pub active_tab: usize,
    pub home: bool,
}
impl App {
    /// runs the application's main loop until the user quits
    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while self.running {
            let any_changed = self
                .get_tab()
                .panes
                .iter()
                .any(|p| p.screen_changed.swap(false, Ordering::Relaxed));
            let timeout = if any_changed {
                std::time::Duration::ZERO
            } else {
                std::time::Duration::from_millis(16)
            };
            self.handle_events(timeout)?;
            if !self.home {
                let active = self.get_tab().active;
                self.get_mut_tab().panes[active].scroll_to_input();
            }
            terminal.draw(|frame| self.draw(frame))?;
        }
        Ok(())
    }

    fn handle_events(&mut self, timeout: std::time::Duration) -> Result<()> {
        if crossterm::event::poll(timeout)?
            && let crossterm::event::Event::Key(key) = crossterm::event::read()?
        {
            self.execute(command::resolve(key))?;
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

                self.tabs.push(Tab::new(term_rows - 2, term_cols - 4));

                self.active_tab += 1;
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
            }
            Command::NextPane => {
                if self.get_tab().active == (self.get_tab().panes.len() - 1) {
                    self.get_mut_tab().active = 0;
                } else {
                    self.get_mut_tab().active += 1;
                }
            }
            Command::NewPane => {
                let (rows, cols) = {
                    let tab = self.get_tab();
                    let size = tab.panes[tab.active].pty_master.get_size().unwrap();
                    (size.rows, size.cols)
                };
                let new_rows = (rows / (self.get_tab().panes.len() as u16 + 1)).max(2);
                let new_cols = cols.max(2);
                let new_pane = Pane::new(new_rows, new_cols)?;
                let tab = self.get_mut_tab();
                tab.panes.push(new_pane);
                tab.active = tab.panes.len() - 1;
            }
            Command::ScrollToTop => {
                self.active_pane_mut().scroll_to_top();
                self.home = true;
            }
            Command::ScrollToBottom => {
                self.active_pane_mut().scroll_to_bottom();
                self.home = false;
            }
            Command::ScrollPageUp => {
                let visible = self.active_pane().visible_lines();
                log::debug!("visible {visible}");
                self.active_pane_mut().scroll_up(1);
                self.home = false;
            }
            Command::ScrollPageDown => {
                let visible = self.active_pane().visible_lines();
                self.active_pane_mut().scroll_down(visible);
                self.home = self.active_pane().at_bottom();
            }
            Command::SendKey(key) => self.send_key(key)?,
        }
        Ok(())
    }

    fn send_key(&mut self, key: KeyEvent) -> Result<()> {
        let active = self.get_tab().active;
        if let Some(active_pane) = self.get_mut_tab().panes.get_mut(active)
            && let Some(ref mut w) = *active_pane.pty_writer.lock().unwrap()
        {
            match key.code {
                KeyCode::Enter => w.write_all(b"\r")?,
                KeyCode::Tab => w.write_all(b"\t")?,
                KeyCode::Backspace => w.write_all(b"\x7f")?,
                KeyCode::Esc => w.write_all(b"\x1b")?,
                KeyCode::Up => w.write_all(b"\x1b[A")?,
                KeyCode::Down => w.write_all(b"\x1b[B")?,
                KeyCode::Right => w.write_all(b"\x1b[C")?,
                KeyCode::Left => w.write_all(b"\x1b[D")?,
                KeyCode::Delete => w.write_all(b"\x1b[3~")?,
                KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    w.write_all(&[c as u8 - b'a' + 1])?;
                }
                KeyCode::Char(c) => w.write_all(c.to_string().as_bytes())?,
                _ => {}
            };
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
        self.tabs[self.active_tab].draw_tab(frame);
    }
}
