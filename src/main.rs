//! InfinityDesk – ein Wayland-Compositor mit einer großen, begrenzten Arbeitsfläche.
//!
//! Aktuell läuft er "genestet": als normales Fenster in der laufenden Session (z. B. niri).

mod handlers;
mod input;
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

    // Programme, die wir starten, sollen sich mit InfinityDesk verbinden statt mit niri.
    unsafe { std::env::set_var("WAYLAND_DISPLAY", &state.socket_name) };
    tracing::info!("InfinityDesk läuft auf {:?}", state.socket_name);

    spawn_client();

    event_loop.run(None, &mut state, |_| {})?;
    Ok(())
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,infinitydesk=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Startet ein Testprogramm im Compositor: `-c <programm>`, sonst ein Terminal.
fn spawn_client() {
    let mut args = std::env::args().skip(1);
    let command = match (args.next().as_deref(), args.next()) {
        (Some("-c" | "--command"), Some(cmd)) => cmd,
        _ => "alacritty".to_string(),
    };
    if let Err(err) = std::process::Command::new(&command).spawn() {
        tracing::warn!("Konnte {command} nicht starten: {err}");
    }
}
