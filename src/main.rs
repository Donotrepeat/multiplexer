use anyhow::Result;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use log::LevelFilter;

mod app;
mod logging;

use app::application::App;

fn set_terminal_title(title: &str) {
    print!("\x1B]0;{}\x07", title);
}

fn main() -> Result<()> {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::terminal::disable_raw_mode(); // leave raw mode
        let _ = crossterm::execute!(
            std::io::stdout(), // leave alternate screen
            crossterm::terminal::LeaveAlternateScreen
        );
        default_hook(info); // still print the panic
    }));

    logging::init(LevelFilter::Trace)
        .map_err(|e| anyhow::anyhow!("failed to init logger: {e:?}"))?;
    enable_raw_mode()?;
    set_terminal_title("multiplexer");

    let mut app = App::new()?;
    ratatui::run(|terminal| app.run(terminal))?;

    disable_raw_mode()?;
    logging::dump();
    Ok(())
}
