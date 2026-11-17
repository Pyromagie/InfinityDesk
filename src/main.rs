//! InfinityDesk – a Wayland compositor with a huge, bounded canvas to place windows on.
//!
//! Currently runs "nested": as a regular window inside the running session (e.g. niri).

mod canvas;
mod cluster;
mod grabs;
mod handlers;
mod input;
mod render;
mod state;
mod winit;

use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
pub use state::InfinityDesk;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logging();

    let mut event_loop: EventLoop<InfinityDesk> = EventLoop::try_new()?;
    let display: Display<InfinityDesk> = Display::new()?;
    let mut state = InfinityDesk::new(&mut event_loop, display);

    winit::init_winit(&mut event_loop, &mut state)?;

    // Programs we spawn should connect to InfinityDesk, not to the host compositor.
    unsafe { std::env::set_var("WAYLAND_DISPLAY", &state.socket_name) };
    tracing::info!("InfinityDesk running on {:?}", state.socket_name);

    spawn_client();

    event_loop.run(None, &mut state, |_| {})?;
    Ok(())
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,infinitydesk=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Spawns a test client inside the compositor: `-c <program>`, otherwise a terminal.
fn spawn_client() {
    let mut args = std::env::args().skip(1);
    let command = match (args.next().as_deref(), args.next()) {
        (Some("-c" | "--command"), Some(cmd)) => cmd,
        _ => "alacritty".to_string(),
    };
    if let Err(err) = std::process::Command::new(&command).spawn() {
        tracing::warn!("Failed to spawn {command}: {err}");
    }
}
