//! InfinityDesk – a Wayland compositor with a huge, bounded canvas to place windows on.
//!
//! Runs "nested" as a regular window when started inside a session (e.g. niri), otherwise
//! standalone on the screen (from a console or a login manager).

mod arrange;
mod canvas;
mod cluster;
mod config;
mod cursor;
mod grabs;
mod handlers;
mod input;
mod layout;
mod navigate;
mod render;
mod resize;
mod state;
mod udev;
mod winit;

use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
pub use state::InfinityDesk;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logging();

    let mut event_loop: EventLoop<InfinityDesk> = EventLoop::try_new()?;
    let display: Display<InfinityDesk> = Display::new()?;
    // Inside another session: a window there. Otherwise: the screen is ours, and Mod is Super.
    let nested =
        std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some();
    config::set_mod_super(!nested);
    let mut state = InfinityDesk::new(&mut event_loop, display);

    if nested {
        winit::init_winit(&mut event_loop, &mut state)?;
    } else {
        udev::init_udev(&mut event_loop, &mut state)?;
    }

    tracing::info!("InfinityDesk running on {:?}", state.socket_name);

    // Start a program inside: `-c <program>`, otherwise a terminal.
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        (Some("-c" | "--command"), Some(cmd)) => spawn(&cmd, &state.socket_name),
        _ => spawn("alacritty", &state.socket_name),
    }

    event_loop.run(None, &mut state, |_| {})?;
    Ok(())
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,infinitydesk=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Starts a program inside InfinityDesk: it connects to our socket, not to the host compositor.
/// The command line goes through `sh -c`, so arguments, quotes and `~` work.
pub fn spawn(command: &str, socket: &std::ffi::OsStr) {
    match (std::process::Command::new("sh"))
        .args(["-c", command])
        .env("WAYLAND_DISPLAY", socket)
        .spawn()
    {
        // Reap the child when it exits, so it does not linger as a zombie.
        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
        Err(err) => tracing::warn!("Failed to spawn {command}: {err}"),
    }
}
