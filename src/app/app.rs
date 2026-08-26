use crate::app::pane;
use crate::app::tabs::Tab;
use anyhow::{Ok, Result};
use crossterm::event::{KeyCode, KeyModifiers};
use pane::Pane;
use portable_pty::PtySize;
use ratatui::{layout::Rect, DefaultTerminal, Frame};

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
            self.handle_events(timeout);
            if !self.home {
                let num_panes = self.get_tab().panes.len();
                let active = self.get_tab().active;
                self.get_mut_tab().panes[active].scroll_to_input(num_panes, active);
            }
            terminal.draw(|frame| self.draw(frame))?;
        }
        Ok(())
    }

    fn handle_events(&mut self, timeout: std::time::Duration) -> Result<()> {
        if crossterm::event::poll(timeout)? {
            match crossterm::event::read()? {
                crossterm::event::Event::Key(key) => {
                    if key.code == crossterm::event::KeyCode::Char('w')
                        && key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
                    {
                        self.running = false;
                    } else if key.code == crossterm::event::KeyCode::Char('c')
                        && key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
                    {
                        let (term_cols, term_rows) = size()?;
                        let term_rows = term_rows.max(1);
                        let term_cols = term_cols.max(1);

                        self.tabs.push(Tab::new(term_rows - 2, term_cols - 4));

                        self.active_tab += 1;

                        return Ok(());
                    } else if key.code == KeyCode::Char('e')
                        && key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
                    {
                        let tab_count = self.tabs.len() - 1;
                        if self.active_tab == tab_count {
                            self.active_tab = 0;
                        } else {
                            self.active_tab += 1;
                        }
                    } else if key.code == KeyCode::Char('q')
                        && key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
                    {
                        let tab_count = self.tabs.len() - 1;
                        if self.active_tab == 0 {
                            self.active_tab = tab_count;
                        } else {
                            self.active_tab -= 1;
                        }
                    } else if key.code == KeyCode::Char('j')
                        && key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
                    {
                        let tab = self.get_mut_tab();
                        tab.grid = tab.grid.next();
                        tab.reshape();
                    } else if key.code == KeyCode::Char('n')
                        && key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
                    {
                        if self.get_tab().active == (self.get_tab().panes.len() - 1) {
                            self.get_mut_tab().active = 0;
                        } else {
                            self.get_mut_tab().active += 1;
                        }
                    } else if key.code == KeyCode::Char('t')
                        && key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
                    {
                        let pane_count = self.get_tab().panes.len();
                        let size = self.get_tab().panes[self.get_tab().active]
                            .pty_master
                            .get_size()
                            .unwrap();
                        let new_rows = size.rows / (pane_count + 1) as u16 - 1;
                        let new_cols = size.cols.saturating_sub(2) - 2;
                        if new_cols < 2 {
                            // Too narrow to split — ignore or flash a message
                            return Ok(());
                        }
                        let new_pane = Pane::new(new_rows, new_cols)?;
                        self.get_tab().panes[self.get_tab().active]
                            .pty_master
                            .resize(PtySize {
                                rows: new_rows,
                                cols: new_cols,
                                pixel_width: 0,
                                pixel_height: 0,
                            });
                        self.get_tab().panes[self.get_tab().active]
                            .vpty
                            .lock()
                            .unwrap()
                            .screen_mut()
                            .set_size(new_rows, new_cols);
                        self.tabs
                            .get_mut(self.active_tab)
                            .unwrap()
                            .panes
                            .push(new_pane);
                        self.get_mut_tab().active = self.get_tab().panes.len() - 1;
                    } else {
                        // Handle all key events for the active pane
                        let mut handled = false;

                        // Handle scroll keys first - use active index before any borrows
                        if key.code == KeyCode::Home {
                            // Handle Home for scrolling to top
                            let active = self.get_tab().active;
                            self.get_mut_tab().panes[active].scroll_to_top();
                            handled = true;
                            self.home = true;
                        } else if key.code == KeyCode::End {
                            // Handle End for scrolling to bottom
                            let active = self.get_tab().active;
                            self.get_mut_tab().panes[active].scroll_to_bottom();
                            handled = true;

                            self.home = false;
                        } else if key.code == KeyCode::PageUp {
                            // Handle PageUp for scrolling up
                            let active = self.get_tab().active;
                            let visible = self.get_tab().panes[active].visible_lines();
                            log::debug!("visible {visible}");
                            self.get_mut_tab().panes[active].scroll_up(1);
                            handled = true;
                            self.home = false;
                        } else if key.code == KeyCode::PageDown {
                            // Handle PageDown for scrolling down
                            let active = self.get_tab().active;
                            let visible = self.get_tab().panes[active].visible_lines();
                            self.get_mut_tab().panes[active].scroll_down(visible);
                            handled = true;
                            if self.get_tab().panes[self.get_tab().active].at_bottom() {
                                self.home = true;
                            } else {
                                self.home = false;
                            }
                        } else {
                            // Handle normal character input
                            let active = self.get_tab().active;
                            // Get mutable reference and write to the pane
                            if let Some(active_pane) = self.get_mut_tab().panes.get_mut(active) {
                                if let Some(ref mut w) = *active_pane.pty_writer.lock().unwrap() {
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
                                        KeyCode::Char(c)
                                            if key.modifiers.contains(KeyModifiers::CONTROL) =>
                                        {
                                            w.write_all(&[c as u8 - b'a' + 1])?;
                                        }
                                        KeyCode::Char(c) => {
                                            w.write_all(c.to_string().as_bytes())?
                                        }
                                        _ => {}
                                    };
                                }
                            }
                        }

                        // If we handled a scroll key, return early to prevent other writes
                        if handled {
                            return Ok(());
                        }
                    }
                }
                crossterm::event::Event::Resize(cols, rows) => {
                    self.get_tab().panes[self.get_tab().active]
                        .pty_master
                        .resize(PtySize {
                            rows,
                            cols,
                            pixel_width: 0,
                            pixel_height: 0,
                        })?;
                    self.get_tab().panes[self.get_tab().active]
                        .vpty
                        .lock()
                        .unwrap()
                        .screen_mut()
                        .set_size(rows, cols);
                    // Reset scroll position on resize to maintain relative position
                    let active = self.get_tab().active;
                    let scroll_offset = self.get_tab().panes[active].get_scroll_offset();
                    self.get_mut_tab().panes[active].set_scroll_offset(scroll_offset);
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn get_tab(&self) -> &Tab {
        self.tabs.get(self.active_tab).unwrap()
    }
    fn get_mut_tab(&mut self) -> &mut Tab {
        self.tabs.get_mut(self.active_tab).unwrap()
    }
    fn draw(&self, frame: &mut Frame) {
        self.tabs[self.active_tab].draw_tab(frame);
    }
}
