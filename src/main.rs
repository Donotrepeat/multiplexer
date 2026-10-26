use anyhow::Result;
use log::LevelFilter;

mod app;
mod logging;

use app::application::App;
use app::host::{Host, LocalHost};
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
        match &arg[..] {
            "--session" => {
                let Some(name) = args.next() else {
                    anyhow::bail!("--session requires a name");
                };
                session = name;
            }
            "--server" => anyhow::bail!("not yet implemented"),
            "--attach" => anyhow::bail!("not yet implemented"),
            _ => anyhow::bail!("unkown argument"),
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
    // LocalHost enters raw mode / the alternate screen and restores the
    // terminal on drop, so setup and teardown live in one place.
    let mut host = LocalHost::new()?;
    set_terminal_title("multiplexer");

    let mut app = App::new(&session_name, host.size())?;
    app.run(&mut host)?;

    // Persist the workspace on clean exit only, never in the panic hook.
    app.save_workspace();

    logging::dump();
    Ok(())
}
