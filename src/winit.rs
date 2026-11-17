//! Nested backend: InfinityDesk runs as a window inside the current session.

use std::time::Duration;

use smithay::{
    backend::{
        renderer::damage::OutputDamageTracker,
        winit::{self, WinitEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::calloop::EventLoop,
    utils::{Rectangle, Transform},
};

use crate::{
    InfinityDesk,
    render::{compile_grid_shader, desk_elements},
};
use smithay::desktop::layer_map_for_output;

/// Canvas background color (RGBA).
const BACKGROUND: [f32; 4] = [0.1, 0.1, 0.12, 1.0];

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
    // Start with the viewport in the middle of the canvas, leaving room in every direction.
    state.output = Some(output.clone());
    state.canvas.set_screen_size(mode.size.to_logical(1));
    let canvas_size = state.canvas.size().to_f64();
    state
        .canvas
        .center_viewport_on((canvas_size.w / 2.0, canvas_size.h / 2.0).into());
    state.apply_viewport();

    let mut damage_tracker = OutputDamageTracker::from_output(&output);
    let grid_shader = compile_grid_shader(backend.renderer());

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
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw => {
                let size = backend.window_size();
                let damage = Rectangle::from_size(size);

                {
                    let outlines = state.outlines();
                    let (renderer, mut framebuffer) = backend.bind().unwrap();
                    let elements = desk_elements(
                        renderer,
                        &output,
                        &state.space,
                        &state.canvas,
                        &grid_shader,
                        &outlines,
                    );
                    if let Err(err) = damage_tracker.render_output(
                        renderer,
                        &mut framebuffer,
                        0,
                        &elements,
                        BACKGROUND,
                    ) {
                        tracing::error!("rendering failed: {err:?}");
                    }
                }
                backend.submit(Some(&[damage])).unwrap();

                // Tell clients they may draw their next frame.
                state.space.elements().for_each(|window| {
                    window.send_frame(
                        &output,
                        state.start_time.elapsed(),
                        Some(Duration::ZERO),
                        |_, _| Some(output.clone()),
                    )
                });

                for layer in layer_map_for_output(&output).layers() {
                    layer.send_frame(
                        &output,
                        state.start_time.elapsed(),
                        Some(Duration::ZERO),
                        |_, _| Some(output.clone()),
                    );
                }

                state.space.refresh();
                state.popups.cleanup();
                let _ = state.display_handle.flush_clients();

                backend.window().request_redraw();
            }
            WinitEvent::CloseRequested => state.loop_signal.stop(),
            _ => (),
        })?;

    Ok(())
}
