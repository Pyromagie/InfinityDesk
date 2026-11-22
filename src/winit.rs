//! Nested backend: InfinityDesk runs as a window inside the current session.

use smithay::{
    backend::{
        renderer::damage::OutputDamageTracker,
        winit::{self, WinitEvent},
    },
    input::pointer::{CursorIcon, CursorImageStatus},
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::calloop::EventLoop,
    utils::Transform,
};

use crate::{
    InfinityDesk,
    render::{Grid, bar_elements, desk_elements},
};

pub fn init_winit(
    event_loop: &mut EventLoop<InfinityDesk>,
    state: &mut InfinityDesk,
) -> Result<(), Box<dyn std::error::Error>> {
    let (mut backend, winit) = winit::init()?;

    let mode = Mode {
        size: backend.window_size(),
        refresh: 60_000,
    };

    let output = Output::new(
        "winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "InfinityDesk".into(),
            model: "Winit".into(),
            serial_number: "Unknown".into(),
        },
    );
    let _global = output.create_global::<InfinityDesk>(&state.display_handle);
    // Nested, only the scale from the config counts: the host decides size and orientation.
    let scale = state.config.output(&output.name()).scale;
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        Some(Scale::Fractional(scale)),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.set_output(output.clone());

    let mut damage_tracker = OutputDamageTracker::from_output(&output);
    let mut grid = Grid::new(backend.renderer());
    let mut bar_buffers = Vec::new();
    // The back buffer has no age before the first frame (asking EGL then logs an error).
    let mut first_frame = true;
    // Pointer image currently shown by the host.
    let mut cursor = CursorIcon::Default;

    event_loop
        .handle()
        .insert_source(winit, move |event, _, state| match event {
            WinitEvent::Resized { size, .. } => {
                output.change_current_state(
                    Some(Mode {
                        size,
                        refresh: 60_000,
                    }),
                    None,
                    None,
                    None,
                );
                state.output_changed();
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw => {
                // Only redraw what changed since this back buffer was last used.
                let age = match first_frame {
                    true => 0,
                    false => backend.buffer_age().unwrap_or(0),
                };
                first_frame = false;
                state.step_animations();
                let bars = state.bars();
                let offsets = state.window_offsets();
                // Ours (resize arrows), else what the app asks for (as far as the host can show it).
                let wanted = match &state.client_cursor {
                    CursorImageStatus::Named(icon) if state.cursor == CursorIcon::Default => *icon,
                    _ => state.cursor,
                };
                if wanted != cursor {
                    cursor = wanted;
                    backend.window().set_cursor(cursor.into());
                }
                let damage = match backend.bind() {
                    Ok((renderer, mut framebuffer)) => {
                        let scale = output.current_scale().fractional_scale();
                        let mut elements = bar_elements(&mut bar_buffers, &bars, scale);
                        elements.extend(desk_elements(
                            renderer,
                            &output,
                            &state.space,
                            &state.canvas,
                            &mut grid,
                            &state.config,
                            state.upper_layers(),
                            &offsets,
                        ));
                        damage_tracker
                            .render_output(
                                renderer,
                                &mut framebuffer,
                                age,
                                &elements,
                                state.config.background,
                            )
                            .map(|result| result.damage.cloned())
                            .unwrap_or_else(|err| {
                                tracing::error!("rendering failed: {err:?}");
                                None
                            })
                    }
                    Err(err) => {
                        tracing::error!("binding the back buffer failed: {err:?}");
                        None
                    }
                };
                // Nothing changed: nothing to show.
                if let Some(damage) = damage
                    && let Err(err) = backend.submit(Some(&damage))
                {
                    tracing::warn!("submitting the frame failed: {err:?}");
                }

                state.send_frames(&output);
                backend.window().request_redraw();
            }
            WinitEvent::CloseRequested => state.loop_signal.stop(),
            _ => (),
        })?;

    Ok(())
}
