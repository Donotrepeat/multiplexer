use anyhow::{Error, Result};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size};
use portable_pty::{native_pty_system, CommandBuilder, PtySize, PtySystem};
use ratatui::{
    style::{Color, Modifier, Style, Stylize},
    symbols::border,
    text::{Line, Span, Text},
    widgets::{Block, Paragraph},
    DefaultTerminal, Frame,
};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use vt100;

#[derive(Default)]
struct App {
    vpty: Arc<Mutex<vt100::Parser>>,
    pty_writer: Arc<Mutex<Option<Box<dyn Write + Send>>>>,
    running: bool,
}

impl App {
    /// runs the application's main loop until the user quits
    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while self.running {
            terminal.draw(|frame| self.draw(frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    fn handle_events(&mut self) -> Result<()> {
        todo!()
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
    // Use the native pty implementation for the system
    let pty_system = native_pty_system();

    // Create a new pty
    let mut pair = pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        // Not all systems support pixel_width, pixel_height,
        // but it is good practice to set it to something
        // that matches the size of the selected font.  That
        // is more complex than can be shown here in this
        // brief example though!
        pixel_width: 0,
        pixel_height: 0,
    })?;

    // Spawn a shell into the pty
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
    let cmd = CommandBuilder::new(shell);
    let mut child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);
    let pty_writer = Arc::new(Mutex::new(pair.master.take_writer()?));
    let vpty = Arc::new(Mutex::new(vt100::Parser::default()));
    let vpt_clone = Arc::clone(&vpty);
    // let mut ptty_writer =  Arc::new(Mutex::new(pty_system::))
    // create a reader
    //
    let mut reader = pair.master.try_clone_reader()?;
    // spawn a task to read the output of the created shell
    let reader_thread = thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF, shell exited
                Ok(n) => {
                    vpt_clone.lock().unwrap().process(&buf[..n]);
                    let screen = {
                        let guard_screen = vpty.lock().unwrap();
                        guard_screen.screen().clone()
                    };
                }
                Err(_) => break,
            }
        }
    });
    ratatui::run(|terminal| App::default().run(terminal))?;
    // Read and parse output from the pty with reader

    // Wait for the shell to exit.
    child.wait()?;

    disable_raw_mode()?;

    // These threads are blocked on blocking reads (stdin/PTY), so we don't
    // strictly join them — the process exit will clean them up. For a real
    // implementation you'd want a cleaner shutdown signal.
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
        let fg = style.fg.unwrap();
        style = style.fg(style.bg.unwrap());
        style = style.bg(fg);
    }

    style
}

fn vterm_to_ratatui(screen: &vt100::Screen) -> Text<'static> {
    let size = screen.size();
    let mut lines = Vec::new();
    let (rows, cols) = size;
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
