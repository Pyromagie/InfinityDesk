//! Nested backend: InfinityDesk runs as a window inside the current session.

use smithay::{
    backend::{
        renderer::damage::OutputDamageTracker,
        winit::{self, WinitEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::calloop::EventLoop,
    utils::Transform,
};

use crate::{
    InfinityDesk,
    render::{Grid, bar_elements, desk_elements},
};
use smithay::{
    desktop::layer_map_for_output,
    input::pointer::{CursorIcon, CursorImageStatus},
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
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        None,
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
                layer_map_for_output(&output).arrange();
                state.canvas.set_screen_size(size.to_logical(1));
                state.apply_viewport();
                state.fit_fullscreen();
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw => {
                // Only redraw what changed since this back buffer was last used.
                let age = match first_frame {
                    true => 0,
                    false => backend.buffer_age().unwrap_or(0),
                };
                first_frame = false;
                let bars = state.bars();
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
                        let mut elements = bar_elements(&mut bar_buffers, &bars);
                        elements.extend(desk_elements(
                            renderer,
                            &output,
                            &state.space,
                            &state.canvas,
                            &mut grid,
                            &state.config,
                            state.upper_layers(),
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
