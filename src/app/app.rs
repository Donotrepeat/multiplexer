use crate::app::pane;
use crate::app::tabs::Tab;
use anyhow::{Ok, Result};
use crossterm::event::{KeyCode, KeyModifiers};
use pane::Pane;
use portable_pty::PtySize;
use ratatui::prelude::Position;
use ratatui::{
    layout::{Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Paragraph},
    DefaultTerminal, Frame,
};

use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size};
use std::io::Write;
use std::sync::atomic::Ordering;

//TODO add tabs for other groups on panes
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
                .tabs
                .get(self.active_tab)
                .unwrap()
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
                let num_panes = self.tabs.get(self.active_tab).unwrap().panes.len();
                for (idx, pane) in self
                    .tabs
                    .get_mut(self.active_tab)
                    .unwrap()
                    .panes
                    .iter_mut()
                    .enumerate()
                {
                    pane.scroll_to_input(num_panes, idx);
                }
            }
            terminal.draw(|frame| self.draw(frame))?;
        }
        Ok(())
    }

    fn handle_events(&mut self, timeout: std::time::Duration) -> Result<()> {
        if crossterm::event::poll(timeout)? {
            match crossterm::event::read()? {
                crossterm::event::Event::Key(key) => {
                    if key.code == crossterm::event::KeyCode::Char('q')
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

                        self.active_tab = 1;

                        return Ok(());
                    } else if key.code == KeyCode::Char('n')
                        && key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
                    {
                        if self.tabs.get(self.active_tab).unwrap().active
                            == (self.tabs.get(self.active_tab).unwrap().panes.len() - 1)
                        {
                            self.tabs.get_mut(self.active_tab).unwrap().active -= 1;
                        } else {
                            self.tabs.get_mut(self.active_tab).unwrap().active += 1;
                        }
                        //TOOD add keybinding for creating a tab
                        //And switching between tabs
                    } else if key.code == KeyCode::Char('t')
                        && key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
                    {
                        let pane_count = self.tabs.get(self.active_tab).unwrap().panes.len();
                        let size = self.tabs.get(self.active_tab).unwrap().panes
                            [self.tabs.get(self.active_tab).unwrap().active]
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
                        self.tabs.get(self.active_tab).unwrap().panes
                            [self.tabs.get(self.active_tab).unwrap().active]
                            .pty_master
                            .resize(PtySize {
                                rows: new_rows,
                                cols: new_cols,
                                pixel_width: 0,
                                pixel_height: 0,
                            });
                        self.tabs.get(self.active_tab).unwrap().panes
                            [self.tabs.get(self.active_tab).unwrap().active]
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
                        self.tabs.get_mut(self.active_tab).unwrap().active =
                            self.tabs.get(self.active_tab).unwrap().panes.len() - 1;
                        //TODO add keybinding to change horizontally to vertical
                    } else {
                        // Handle all key events for the active pane
                        let mut handled = false;

                        // Handle scroll keys first - use active index before any borrows
                        if key.code == KeyCode::Home {
                            // Handle Home for scrolling to top
                            let active = self.tabs.get(self.active_tab).unwrap().active;
                            self.tabs.get_mut(self.active_tab).unwrap().panes[active]
                                .scroll_to_top();
                            handled = true;
                            self.home = true;
                        } else if key.code == KeyCode::End {
                            // Handle End for scrolling to bottom
                            let active = self.tabs.get(self.active_tab).unwrap().active;
                            self.tabs.get_mut(self.active_tab).unwrap().panes[active]
                                .scroll_to_bottom();
                            handled = true;

                            self.home = false;
                        } else if key.code == KeyCode::PageUp {
                            // Handle PageUp for scrolling up
                            let active = self.tabs.get(self.active_tab).unwrap().active;
                            let visible = self.tabs.get(self.active_tab).unwrap().panes[active]
                                .visible_lines();
                            log::debug!("visible {visible}");
                            self.tabs.get_mut(self.active_tab).unwrap().panes[active].scroll_up(1);
                            handled = true;
                            self.home = false;
                        } else if key.code == KeyCode::PageDown {
                            // Handle PageDown for scrolling down
                            let active = self.tabs.get(self.active_tab).unwrap().active;
                            let visible = self.tabs.get(self.active_tab).unwrap().panes[active]
                                .visible_lines();
                            self.tabs.get_mut(self.active_tab).unwrap().panes[active]
                                .scroll_down(visible);
                            handled = true;
                            if self.tabs.get(self.active_tab).unwrap().panes
                                [self.tabs.get(self.active_tab).unwrap().active]
                                .at_bottom()
                            {
                                self.home = true;
                            } else {
                                self.home = false;
                            }
                        } else {
                            // Handle normal character input
                            let active = self.tabs.get(self.active_tab).unwrap().active;
                            // Get mutable reference and write to the pane
                            if let Some(active_pane) = self
                                .tabs
                                .get_mut(self.active_tab)
                                .unwrap()
                                .panes
                                .get_mut(active)
                            {
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
                    self.tabs.get(self.active_tab).unwrap().panes
                        [self.tabs.get(self.active_tab).unwrap().active]
                        .pty_master
                        .resize(PtySize {
                            rows,
                            cols,
                            pixel_width: 0,
                            pixel_height: 0,
                        })?;
                    self.tabs.get(self.active_tab).unwrap().panes
                        [self.tabs.get(self.active_tab).unwrap().active]
                        .vpty
                        .lock()
                        .unwrap()
                        .screen_mut()
                        .set_size(rows, cols);
                    // Reset scroll position on resize to maintain relative position
                    let active = self.tabs.get(self.active_tab).unwrap().active;
                    let scroll_offset =
                        self.tabs.get(self.active_tab).unwrap().panes[active].get_scroll_offset();
                    self.tabs.get_mut(self.active_tab).unwrap().panes[active]
                        .set_scroll_offset(scroll_offset);
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        let total_panes = self.tabs.get(self.active_tab).unwrap().panes.len();

        match total_panes {
            1 => {
                // Single pane - fill entire screen (current behavior)
                if let Some(pane) = self
                    .tabs
                    .get(self.active_tab)
                    .unwrap()
                    .panes
                    .get(self.tabs.get(self.active_tab).unwrap().active)
                {
                    render_pane(pane, frame, area, true);
                }
            }
            2 => {
                // Two panes - split horizontally
                let chunk_size = area.height / 2;
                let top_area = Rect::new(area.x, area.y, area.width, chunk_size);
                let bottom_area = Rect::new(
                    area.x,
                    area.y + chunk_size,
                    area.width,
                    area.height - chunk_size,
                );

                render_pane(
                    &self.tabs.get(self.active_tab).unwrap().panes[0],
                    frame,
                    top_area,
                    self.tabs.get(self.active_tab).unwrap().active == 0,
                );
                render_pane(
                    &self.tabs.get(self.active_tab).unwrap().panes[1],
                    frame,
                    bottom_area,
                    self.tabs.get(self.active_tab).unwrap().active == 1,
                );
            }
            3.. => {
                //TODO add multi panes and grid structure
                // Three or more - create a basic grid
                // Calculate rows/cols and render each pane in its position
            }
            _ => {}
        }
    }
}
fn render_pane(pane: &Pane, frame: &mut Frame, area: Rect, is_active: bool) {
    let parser = pane.vpty.lock().unwrap();
    let screen = parser.screen();
    let (visible_rows, _cols) = screen.size();
    let text = vterm_to_ratatui(screen, pane.scroll_offset, visible_rows as usize);
    frame.render_widget(Paragraph::new(text).block(Block::bordered()), area);

    if is_active {
        let (row, col) = screen.cursor_position();
        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });
        if row as usize >= pane.scroll_offset {
            let x = (inner.x + col).min(inner.right() - 1);
            let y = (inner.y + row - pane.scroll_offset as u16).min(inner.bottom() - 1);
            frame.set_cursor_position(Position::new(x, y));
        }
    }
}
fn build_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default();

    // fg color
    style = style.fg(match cell.fgcolor() {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(n) => Color::Indexed(n),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    });

    // bg color
    style = style.bg(match cell.bgcolor() {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(n) => Color::Indexed(n),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    });

    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.dim() {
        style = style.add_modifier(Modifier::DIM);
    }
    if cell.inverse() {
        // swap fg and bg
        let fg = style.fg.unwrap_or(Color::Reset);
        style = style.fg(style.bg.unwrap_or(Color::Reset));
        style = style.bg(fg);
    }

    style
}

fn vterm_to_ratatui(
    screen: &vt100::Screen,
    scroll_offset: usize,
    visible_rows: usize,
) -> Text<'static> {
    let size = screen.size();
    let (_rows, cols) = size;
    let mut lines = Vec::with_capacity(visible_rows);
    // Build an empty fill row (spaces with default style) for the non-occupied area
    // Then iterate each row, then each column within that row
    for row in 0..visible_rows as u16 {
        let mut spans = vec![];
        let mut col: u16 = 0;
        while col < cols {
            // Adjust row index based on scroll position
            match screen.cell(row, col) {
                Some(cell) if !cell.is_wide_continuation() => {
                    let style = build_style(cell);
                    let content = if cell.has_contents() {
                        cell.contents().to_string()
                    } else {
                        " ".to_string()
                    };
                    spans.push(Span::styled(content, style));
                    if cell.is_wide() {
                        col += 1;
                    }
                }
                _ => spans.push(Span::raw(" ")),
            }
            col += 1;
        }
        lines.push(Line::from(spans));
    }
    Text::from(lines)
}
