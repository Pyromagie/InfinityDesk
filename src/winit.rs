//! Backend für den genesteten Betrieb: InfinityDesk läuft als Fenster in der aktuellen Session.

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

use crate::{InfinityDesk, render::desk_elements};

/// Hintergrundfarbe der Arbeitsfläche (RGBA).
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
    output.change_current_state(Some(mode), Some(Transform::Flipped180), None, Some((0, 0).into()));
    output.set_preferred(mode);
    // Start: Viewport in der Mitte des Canvas, damit man in alle Richtungen Platz hat.
    state.output = Some(output.clone());
    state.canvas.set_viewport_size(mode.size.to_logical(1));
    let canvas_size = state.canvas.size().to_f64();
    state
        .canvas
        .center_viewport_on((canvas_size.w / 2.0, canvas_size.h / 2.0).into());
    state.apply_viewport();

    let mut damage_tracker = OutputDamageTracker::from_output(&output);

    event_loop.handle().insert_source(winit, move |event, _, state| match event {
        WinitEvent::Resized { size, .. } => {
            output.change_current_state(Some(Mode { size, refresh: 60_000 }), None, None, None);
            state.canvas.set_viewport_size(size.to_logical(1));
            state.apply_viewport();
        }
        WinitEvent::Input(event) => state.process_input_event(event),
        WinitEvent::Redraw => {
            let size = backend.window_size();
            let damage = Rectangle::from_size(size);

            {
                let (renderer, mut framebuffer) = backend.bind().unwrap();
                let elements = desk_elements(renderer, &output, &state.space, &state.canvas, &mut state.grid);
                if let Err(err) = damage_tracker.render_output(renderer, &mut framebuffer, 0, &elements, BACKGROUND) {
                    tracing::error!("Rendern fehlgeschlagen: {err:?}");
                }
            }
            backend.submit(Some(&[damage])).unwrap();

            // Den Programmen sagen, dass sie das nächste Bild zeichnen dürfen.
            state.space.elements().for_each(|window| {
                window.send_frame(&output, state.start_time.elapsed(), Some(Duration::ZERO), |_, _| {
                    Some(output.clone())
                })
            });

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
