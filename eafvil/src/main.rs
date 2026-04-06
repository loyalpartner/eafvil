#![allow(irrefutable_let_patterns)]

mod grabs;
mod handlers;
mod input;
mod state;
mod winit;

use smithay::reexports::wayland_server::Display;
pub use state::EafvilState;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logging();

    let mut event_loop: smithay::reexports::calloop::EventLoop<EafvilState> =
        smithay::reexports::calloop::EventLoop::try_new()?;

    let display: Display<EafvilState> = Display::new()?;

    let mut state = EafvilState::new(&mut event_loop, display);

    // Open a Wayland/X11 window for our nested compositor
    crate::winit::init_winit(&mut event_loop, &mut state)?;

    spawn_emacs(&mut state);

    event_loop.run(None, &mut state, move |_| {})?;

    // Clean up Emacs child process
    if let Some(mut child) = state.emacs_child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }

    Ok(())
}

fn spawn_emacs(state: &mut EafvilState) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let socket_name = state.socket_name.to_str().unwrap_or("").to_string();

    let command = match args.first().map(|s| s.as_str()) {
        Some("--no-spawn") => {
            tracing::info!("--no-spawn: waiting for external Emacs connection");
            return;
        }
        Some("--emacs-command") => match args.get(1) {
            Some(cmd) => cmd.as_str(),
            None => {
                tracing::error!("--emacs-command requires a command argument");
                return;
            }
        },
        _ => "emacs",
    };

    tracing::info!(
        "Spawning Emacs: {} (WAYLAND_DISPLAY={})",
        command,
        socket_name
    );
    match std::process::Command::new(command)
        .env("WAYLAND_DISPLAY", &socket_name)
        .spawn()
    {
        Ok(child) => state.emacs_child = Some(child),
        Err(e) => tracing::error!("Failed to spawn '{}': {}", command, e),
    }
}

fn init_logging() {
    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }
}
