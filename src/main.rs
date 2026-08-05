use anyhow::Result;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size};

mod app;

use app::app::App;
use app::pane::Pane;

fn main() -> Result<()> {
    enable_raw_mode()?;

    let (term_cols, term_rows) = size()?;
    println!("the base terminal is of size {term_rows}, {term_cols}");
    let term_rows = term_rows.max(1);
    let term_cols = term_cols.max(1);

    let mut app = App {
        panes: vec![Pane::new(term_rows, term_cols - 4).unwrap()],
        running: true,
        active: 0,
    };
    ratatui::run(|terminal| app.run(terminal))?;

    disable_raw_mode()?;
    Ok(())
}
