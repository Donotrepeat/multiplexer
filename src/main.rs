use anyhow::Result;
use log::LevelFilter;

mod app;
mod logging;

use app::application::App;
use app::host::{Host, LocalHost};
use app::workspace::{valid_session_name, DEFAULT_WORKSPACE};

use crate::app::workspace::session;
enum Mode {
    Local { session: String },
    Server { session: String },
}
fn set_terminal_title(title: &str) {
    print!("\x1B]0;{}\x07", title);
}

/// The session to start on: `--session <name>` (or `--session=<name>`),
/// defaulting to the auto-saved default session. Deliberately hand-rolled;
/// `clap` is deferred until there are more flags.
fn parse_session_arg() -> Result<Mode> {
    let mut session = DEFAULT_WORKSPACE.to_string();
    let mut server = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match &arg[..] {
            "--session" => {
                let Some(name) = args.next() else {
                    anyhow::bail!("--session requires a name");
                };
                session = name;
            }
            "--server" => server = true,
            "--attach" => anyhow::bail!("not yet implemented"),
            _ => anyhow::bail!("unkown argument"),
        }
    }

    if !valid_session_name(&session) {
        anyhow::bail!("invalid session name: {session}");
    }
    if server {
        Ok(Mode::Server { session })
    } else {
        Ok(Mode::Local { session })
    }
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

    match parse_session_arg()? {
        Mode::Local { session } => run_local(&session),
        Mode::Server { session } => run_server(&session),
    }?;
    // LocalHost enters raw mode / the alternate screen and restores the
    // terminal on drop, so setup and teardown live in one place.
    Ok(())
}

fn run_local(session: &str) -> Result<()> {
    // panic hook moves here — it only makes sense when there is a
    // terminal to restore
    let mut host = LocalHost::new()?;
    set_terminal_title("multiplexer");
    let mut app = App::new(session, host.size())?;
    app.run(&mut host)?;
    app.save_workspace();
    logging::dump();
    Ok(())
}

fn run_server(session: &str) -> Result<()> {
    // Tasks 2–5 land here: ServerHost::new(session), App::new(...),
    // app.run(&mut host), save_workspace on clean exit — no raw mode,
    // no alt screen, no panic hook (nothing to restore).
    anyhow::bail!("server mode is not implemented yet")
}
