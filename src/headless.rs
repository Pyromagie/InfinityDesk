//! Without a screen (`--headless`), for tests: one made-up 1920×1080 output, nothing is drawn,
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
) -> Result<(), Box<dyn std::error::Error>> {
    let mode = Mode {
        size: (1920, 1080).into(),
        refresh: 60_000,
    };
    let output = Output::new(
        "headless".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "InfinityDesk".into(),
            model: "Headless".into(),
            serial_number: "Unknown".into(),
        },
    );
    let _global = output.create_global::<InfinityDesk>(&state.display_handle);
    // The size is fixed, so tests know it; the scale comes from the config.
    let scale = state.config.output(&output.name()).scale;
    output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        Some(Scale::Fractional(scale)),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.set_output(output.clone());

    // What a frame does, without drawing it.
    let frame = Duration::from_micros(16_667);
    event_loop
        .handle()
        .insert_source(Timer::immediate(), move |_, _, state| {
            state.step_animations();
            state.send_frames(&output);
            TimeoutAction::ToDuration(frame)
        })?;
    Ok(())
}
