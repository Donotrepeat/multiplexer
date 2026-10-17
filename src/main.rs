use anyhow::Result;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use log::LevelFilter;

mod app;
mod logging;

use app::application::App;
use app::workspace::{DEFAULT_WORKSPACE, valid_session_name};

fn set_terminal_title(title: &str) {
    print!("\x1B]0;{}\x07", title);
}

/// The session to start on: `--session <name>` (or `--session=<name>`),
/// defaulting to the auto-saved default session. Deliberately hand-rolled;
/// `clap` is deferred until there are more flags.
fn parse_session_arg() -> Result<String> {
    let mut session = DEFAULT_WORKSPACE.to_string();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--session" {
            let Some(name) = args.next() else {
                anyhow::bail!("--session requires a name");
            };
            session = name;
        } else if let Some(name) = arg.strip_prefix("--session=") {
            session = name.to_string();
        } else {
            anyhow::bail!("unknown argument: {arg}\nusage: multiplexer [--session <name>]");
        }
    }
    if !valid_session_name(&session) {
        anyhow::bail!("invalid session name: {session}");
    }
    Ok(session)
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
    let session_name = parse_session_arg()?;
    enable_raw_mode()?;
    set_terminal_title("multiplexer");

    let mut app = App::new(&session_name)?;
    ratatui::run(|terminal| app.run(terminal))?;

    // Persist the workspace on clean exit only, never in the panic hook.
    app.save_workspace();

    disable_raw_mode()?;
    logging::dump();
    Ok(())
}
