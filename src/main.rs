//! InfinityDesk – a Wayland compositor with a huge, bounded canvas to place windows on.
//!
//! Runs "nested" as a regular window when started inside a session (e.g. niri), otherwise
//! standalone on the screen (from a console or a login manager). With `--headless` it runs
//! without a screen, for tests.

mod arrange;
mod canvas;
mod cluster;
mod config;
mod cursor;
mod focus;
mod grabs;
mod handlers;
mod headless;
mod idle;
mod input;
mod ipc;
mod layout;
mod monitor;
mod navigate;
mod render;
mod resize;
mod script;
mod state;
mod test_input;
mod udev;
mod winit;
mod xwayland;

use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
pub use state::InfinityDesk;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // `infinitydesk msg ...`: a command to the running InfinityDesk (see `ipc.rs`).
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "msg") {
        std::process::exit(ipc::send(&args[1..]));
    }
    init_logging();

    let mut event_loop: EventLoop<InfinityDesk> = EventLoop::try_new()?;
    let display: Display<InfinityDesk> = Display::new()?;
    // Inside another session: a window there. Otherwise: the screen is ours. (Mod is Alt or
    // Super by default, see `Config::mod_key`.)
    let headless = args.iter().any(|a| a == "--headless");
    let nested = !headless
        && (std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some());
    config::set_nested(nested);
    let mut state = InfinityDesk::new(&mut event_loop, display);

    if headless {
        // `--headless 2`: that many monitors.
        let count = args.windows(2).find(|a| a[0] == "--headless");
        let count = count.and_then(|a| a[1].parse().ok()).unwrap_or(1);
        headless::init_headless(&mut event_loop, &mut state, count)?;
    } else if nested {
        winit::init_winit(&mut event_loop, &mut state)?;
    } else {
        udev::init_udev(&mut event_loop, &mut state)?;
    }

    tracing::info!("InfinityDesk running on {:?}", state.socket_name);
    if !headless {
        state.xdisplay = xwayland::XDisplay::start(&state.socket_name);
    }
    let ipc_socket = ipc::socket_path(&state.socket_name);
    match &ipc_socket {
        Some(path) => {
            if let Err(err) = ipc::listen(&event_loop.handle(), path) {
                tracing::warn!("No commands from outside, the socket {path:?} failed: {err}");
            }
        }
        None => tracing::warn!("No commands from outside: XDG_RUNTIME_DIR is not set"),
    }

    // The desk starts empty; `-c <program>` starts one inside, then the config's own.
    let command = (args.windows(2))
        .find(|a| a[0] == "-c" || a[0] == "--command")
        .map(|a| a[1].clone());
    for command in command.iter().chain(&state.config.startup) {
        spawn(command, &state);
    }
    state.emit("desk.start", &[], None);

    // Between two turns of the loop, scripts get what happened, and programs what was sent to them.
    event_loop.run(None, &mut state, |state| {
        state.dispatch_events();
        state.update_idle_inhibit();
        let _ = state.display_handle.flush_clients();
    })?;
    if let Some(path) = ipc_socket {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,infinitydesk=info"));
    // To stderr, like the programs started inside (the session script puts it into a file).
    (tracing_subscriber::fmt().with_env_filter(filter))
        .with_writer(std::io::stderr)
        .init();
}

/// Starts a program inside InfinityDesk: it connects to our socket, not to the host compositor.
/// The command line goes through `sh -c`, so arguments, quotes and `~` work. The environment
/// variables from the config come along.
pub fn spawn(command: &str, state: &InfinityDesk) {
    let mut cmd = std::process::Command::new("sh");
    cmd.args(["-c", command])
        .envs(state.config.env.iter().map(|(name, value)| (name, value)))
        .env("WAYLAND_DISPLAY", &state.socket_name);
    match ipc::socket_path(&state.socket_name) {
        Some(path) => cmd.env("INFINITYDESK_SOCKET", path),
        None => cmd.env_remove("INFINITYDESK_SOCKET"),
    };
    // X11 programs too: on our X display, never on the host's.
    match &state.xdisplay {
        Some(x) => cmd.env("DISPLAY", &x.name),
        None => cmd.env_remove("DISPLAY"),
    };
    match cmd.spawn() {
        // Reap the child when it exits, so it does not linger as a zombie.
        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
        Err(err) => tracing::warn!("Failed to spawn {command}: {err}"),
    }
}
