//! Without a screen (`--headless`), for tests: made-up outputs (1920×1080, or the size from the
//! config), `headless` and, with `--headless 2`, also `headless-2` and so on. Nothing is drawn,
//! and programs get their frame callbacks 60 times a second as on a real screen. Tests look and
//! steer through `infinitydesk msg` (see `tests/headless.rs`).

use std::time::Duration;

use smithay::{
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::calloop::{
        EventLoop,
        timer::{TimeoutAction, Timer},
    },
    utils::Transform,
};

use crate::InfinityDesk;

pub fn init_headless(
    event_loop: &mut EventLoop<InfinityDesk>,
    state: &mut InfinityDesk,
    count: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    for i in 1..=count {
        let name = match i {
            1 => "headless".to_string(),
            _ => format!("headless-{i}"),
        };
        add_output(state, name);
    }

    // What a frame does, without drawing it.
    let frame = Duration::from_micros(16_667);
    event_loop
        .handle()
        .insert_source(Timer::immediate(), move |_, _, state| {
            state.step_animations();
            let outputs: Vec<Output> = state.monitor.outputs().cloned().collect();
            for output in &outputs {
                state.send_frames(output);
            }
            TimeoutAction::ToDuration(frame)
        })?;
    Ok(())
}

fn add_output(state: &mut InfinityDesk, name: String) {
    // The size is fixed from the start, so tests know it; the scale can change.
    let settings = state.config.output(&name);
    let size = (settings.mode).map_or((1920, 1080), |m| (m.width, m.height));
    let mode = Mode {
        size: size.into(),
        refresh: 60_000,
    };
    let output = Output::new(
        name,
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "InfinityDesk".into(),
            model: "Headless".into(),
            serial_number: "Unknown".into(),
        },
    );
    let _global = output.create_global::<InfinityDesk>(&state.display_handle);
    output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        Some(Scale::Fractional(settings.scale)),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.add_output(output);
}
