use anyhow::Result;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size};
use log::LevelFilter;

mod app;
mod logging;

use app::application::App;

use crate::app::tabs;

fn set_terminal_title(title: &str) {
    print!("\x1B]0;{}\x07", title);
}

fn main() -> Result<()> {
    logging::init(LevelFilter::Trace)
        .map_err(|e| anyhow::anyhow!("failed to init logger: {e:?}"))?;
    enable_raw_mode()?;
    set_terminal_title("multiplexer");

    let (term_cols, term_rows) = size()?;
    let term_rows = term_rows.max(1);
    let term_cols = term_cols.max(1);

    let mut app = App {
        tabs: vec![tabs::Tab::new(term_rows - 2, term_cols - 4)],
        running: true,
        active_tab: 0,
        home: true,
    };
    ratatui::run(|terminal| app.run(terminal))?;

    disable_raw_mode()?;
    logging::dump();
    Ok(())
}
