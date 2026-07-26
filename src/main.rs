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

mod app;

use app::app::App;
use app::pane::Pane;

fn main() -> Result<()> {
    enable_raw_mode()?;

    let (term_rows, term_cols) = size()?;
    let term_rows = term_rows.max(1);
    let term_cols = term_cols.max(1);

    // Use the native pty implementation for the system
    let mut app = App {
        panes: vec![Pane::new(term_rows, term_cols)],
        running: true,
        active: 0,
    };
    ratatui::run(|terminal| app.run(terminal))?;

    // Wait for the shell to exit.

    disable_raw_mode()?;
    Ok(())
}
