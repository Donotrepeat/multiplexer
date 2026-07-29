use crate::app::pane;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use pane::Pane;
use portable_pty::PtySize;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style, Stylize},
    symbols::border,
    text::{Line, Span, Text},
    widgets::{Block, Paragraph},
    DefaultTerminal, Frame,
};
use std::io::Write;
use std::sync::atomic::Ordering;

pub struct App {
    pub panes: Vec<pane::Pane>,
    pub running: bool,
    pub active: usize,
}
impl App {
    /// runs the application's main loop until the user quits
    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while self.running {
            terminal.draw(|frame| self.draw(frame))?;
            let any_changed = self
                .panes
                .iter()
                .any(|p| p.screen_changed.swap(false, Ordering::Relaxed));
            let timeout = if any_changed {
                std::time::Duration::ZERO
            } else {
                std::time::Duration::from_millis(16)
            };
            self.handle_events(timeout);
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
                    }
                    if key.code == KeyCode::Char('t')
                        && key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
                    {
                        let pane_count = self.panes.len();
                        let size = self.panes[self.active].pty_master.get_size().unwrap();
                        let new_cols = size.cols / (pane_count + 1) as u16;
                        let new_rows = size.rows.saturating_sub(2) - 2;
                        if new_cols < 2 {
                            // Too narrow to split — ignore or flash a message
                            return Ok(());
                        }
                        let new_pane = Pane::new(new_rows, new_cols)?;
                        self.panes.push(new_pane);
                        self.active = self.panes.len() - 1;
                    }
                    if let Some(ref mut w) = *self.panes[self.active].pty_writer.lock().unwrap() {
                        let _ = match key.code {
                            KeyCode::Enter => w.write_all(b"\r"),
                            KeyCode::Tab => w.write_all(b"\t"),
                            KeyCode::Backspace => w.write_all(b"\x7f"),
                            KeyCode::Esc => w.write_all(b"\x1b"),
                            KeyCode::Up => w.write_all(b"\x1b[A"),
                            KeyCode::Down => w.write_all(b"\x1b[B"),
                            KeyCode::Right => w.write_all(b"\x1b[C"),
                            KeyCode::Left => w.write_all(b"\x1b[D"),
                            KeyCode::Home => w.write_all(b"\x1b[H"),
                            KeyCode::End => w.write_all(b"\x1b[F"),
                            KeyCode::Delete => w.write_all(b"\x1b[3~"),
                            KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                w.write_all(&[c as u8 - b'a' + 1])
                            }
                            KeyCode::Char(c) => w.write_all(c.to_string().as_bytes()),
                            _ => Ok(()),
                        };
                    }
                }
                crossterm::event::Event::Resize(cols, rows) => {
                    self.panes[self.active].pty_master.resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    })?;
                    self.panes[self.active]
                        .vpty
                        .lock()
                        .unwrap()
                        .screen_mut()
                        .set_size(rows, cols);
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        let total_panes = self.panes.len();

        match total_panes {
            1 => {
                // Single pane - fill entire screen (current behavior)
                if let Some(pane) = self.panes.first() {
                    render_pane(pane, frame, area);
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

                render_pane(&self.panes[0], frame, top_area);
                render_pane(&self.panes[1], frame, bottom_area);
            }
            3.. => {
                // Three or more - create a basic grid
                // Calculate rows/cols and render each pane in its position
            }
            _ => {}
        }
    }
}
fn render_pane(pane: &Pane, frame: &mut Frame, area: Rect) {
    let screen = pane.vpty.lock().unwrap().screen().clone();
    let text = vterm_to_ratatui(&screen);
    let paragraph = Paragraph::new(text).block(Block::bordered());
    frame.render_widget(paragraph, area);
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

fn vterm_to_ratatui(screen: &vt100::Screen) -> Text<'static> {
    let size = screen.size();
    let (rows, cols) = size;
    let mut lines = Vec::with_capacity(rows as usize);
    // Build an empty fill row (spaces with default style) for the non-occupied area
    // Then iterate each row, then each column within that row
    for row in 0..rows {
        let mut spans = vec![];
        let mut col: u16 = 0;
        while col < cols {
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
