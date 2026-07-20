use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use ratatui::{
    style::{Color, Modifier, Style, Stylize},
    symbols::border,
    text::{Line, Span, Text},
    widgets::{Block, Paragraph},
    DefaultTerminal, Frame,
};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

struct App {
    vpty: Arc<Mutex<vt100::Parser>>,
    pty_writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    pty_master: Box<dyn MasterPty>,
    running: bool,
    screen_changed: Arc<AtomicBool>,
}

impl App {
    /// runs the application's main loop until the user quits
    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while self.running {
            terminal.draw(|frame| self.draw(frame))?;
            let timeout = if self.screen_changed.swap(false, Ordering::Relaxed) {
                std::time::Duration::ZERO
            } else {
                std::time::Duration::from_millis(5)
            };
            self.handle_events(timeout)?;
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
                    if let Some(ref mut w) = *self.pty_writer.lock().unwrap() {
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
                            KeyCode::Char(c)
                                if key.modifiers.contains(KeyModifiers::CONTROL) =>
                            {
                                w.write_all(&[c as u8 - b'a' + 1])
                            }
                            KeyCode::Char(c) => w.write_all(c.to_string().as_bytes()),
                            _ => Ok(()),
                        };
                    }
                }
                crossterm::event::Event::Resize(cols, rows) => {
                    self.pty_master.resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    })?;
                    self.vpty
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
}

impl App {
    fn draw(&self, frame: &mut Frame) {
        let screen = self.vpty.lock().unwrap().screen().clone();
        let text = vterm_to_ratatui(&screen);

        let block = Block::bordered()
            .title(" multiplexer ".bold())
            .border_set(border::THICK);
        let paragraph = Paragraph::new(text).block(block);
        frame.render_widget(paragraph, frame.area());
    }
}

fn main() -> Result<()> {
    enable_raw_mode()?;

    let (term_rows, term_cols) = size()?;
    let term_rows = term_rows.max(1);
    let term_cols = term_cols.max(1);

    // Use the native pty implementation for the system
    let pty_system = native_pty_system();

    // Create a new pty
    let pair = pty_system.openpty(PtySize {
        rows: term_rows,
        cols: term_cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    // Spawn a shell into the pty
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
    let cmd = CommandBuilder::new(shell);
    let mut child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);
    let pty_writer = Arc::new(Mutex::new(Some(pair.master.take_writer()?)));
    let vpty = Arc::new(Mutex::new(vt100::Parser::new(
        term_rows,
        term_cols,
        12,
    )));
    let vpt_clone = Arc::clone(&vpty);

    let screen_changed = Arc::new(AtomicBool::new(true));
    let sc_clone = Arc::clone(&screen_changed);

    let mut reader = pair.master.try_clone_reader()?;
    let reader_thread = thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    vpt_clone.lock().unwrap().process(&buf[..n]);
                    sc_clone.store(true, Ordering::Relaxed);
                }
                Err(_) => break,
            }
        }
    });

    let mut app = App {
        vpty,
        pty_writer,
        pty_master: pair.master,
        running: true,
        screen_changed,
    };
    ratatui::run(|terminal| app.run(terminal))?;

    // Wait for the shell to exit.
    child.wait()?;

    disable_raw_mode()?;

    drop(reader_thread);

    Ok(())
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
        for mut col in 0..cols {
            match screen.cell(row, col) {
                Some(cell) if !cell.is_wide_continuation() => {
                    let style = build_style(cell);
                    spans.push(Span::styled(cell.contents().to_string(), style));
                    if cell.is_wide() {
                        col += 1;
                    } // skip continuation
                }
                _ => spans.push(Span::raw(" ")),
            }
        }
        lines.push(Line::from(spans));
    }
    Text::from(lines)
}
