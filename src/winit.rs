//! Nested backend: InfinityDesk runs as a window inside the current session.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use smithay::{
    backend::{
        renderer::damage::OutputDamageTracker,
        winit::{self, WinitEvent},
    },
    input::pointer::{CursorIcon, CursorImageStatus},
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::calloop::{
        EventLoop,
        timer::{TimeoutAction, Timer},
    },
    utils::Transform,
};

use crate::{
    InfinityDesk,
    render::{Scene, bar_elements, desk_elements, lock_elements},
};

/// How long to wait before looking again when nothing changed: one frame at 60 Hz.
const FRAME: Duration = Duration::from_micros(16_667);

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
    // Nested, only the scale from the config counts: the host decides size and orientation.
    let scale = state.config.output(&output.name()).scale;
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        Some(Scale::Fractional(scale)),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.add_output(output.clone());

    let mut damage_tracker = OutputDamageTracker::from_output(&output);
    let mut scene = Scene::new(backend.renderer());
    // Shared with the timer that looks again when nothing changed (see `Redraw`).
    let shared = Rc::new(RefCell::new(backend));
    let look_again_planned = Rc::new(Cell::new(false));
    let handle = event_loop.handle();
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
                state.output_changed(&output);
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw => {
                let mut backend = shared.borrow_mut();
                // Only redraw what changed since this back buffer was last used.
                let age = match first_frame {
                    true => 0,
                    false => backend.buffer_age().unwrap_or(0),
                };
                first_frame = false;
                state.step_animations();
                let bars = state.bars();
                let outlines = state.outlines();
                let offsets = state.window_offsets();
                if state.redraw_all {
                    state.redraw_all = false;
                    damage_tracker = OutputDamageTracker::from_output(&output);
                }
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
                        let mut elements;
                        if state.lock.is_some() {
                            elements = lock_elements(renderer, &output, state.lock_surface());
                        } else {
                            elements = bar_elements(&mut bar_buffers, &bars, scale);
                            elements.extend(desk_elements(
                                renderer,
                                &output,
                                &state.space,
                                &state.monitor.canvas,
                                &mut scene,
                                &state.config,
                                state.upper_layers(),
                                &offsets,
                                &outlines,
                                state.fullscreen_window(),
                            ));
                        }
                        damage_tracker
                            .render_output(
                                renderer,
                                &mut framebuffer,
                                age,
                                &elements,
                                state.background(),
                            )
                            .map(|result| result.damage.cloned())
                            .inspect_err(|err| tracing::error!("rendering failed: {err:?}"))
                            .ok()
                    }
                    Err(err) => {
                        tracing::error!("binding the back buffer failed: {err:?}");
                        None
                    }
                };
                let changed = matches!(damage, Some(Some(_)));
                let shown = match damage {
                    Some(Some(damage)) => backend
                        .submit(Some(&damage))
                        .inspect_err(|err| tracing::warn!("submitting the frame failed: {err:?}"))
                        .is_ok(),
                    // Nothing changed: nothing to show.
                    Some(None) => true,
                    None => false,
                };
                if shown {
                    state.lock_shown(&output);
                }
                state.send_frames(&output);
                if changed {
                    // Showing the frame waits for the host's next one, so this does not spin.
                    backend.window().request_redraw();
                } else if !look_again_planned.get() {
                    // Nothing changed: look again after about one frame instead of right away,
                    // like the standalone session. Otherwise the loop spins, and programs get
                    // more frame callbacks than frames (niri does the same).
                    look_again_planned.set(true);
                    let (shared, planned) = (shared.clone(), look_again_planned.clone());
                    let timer = Timer::from_duration(FRAME);
                    let inserted = handle.insert_source(timer, move |_, _, _| {
                        planned.set(false);
                        shared.borrow().window().request_redraw();
                        TimeoutAction::Drop
                    });
                    if inserted.is_err() {
                        look_again_planned.set(false);
                        backend.window().request_redraw();
                    }
                }
            }
            WinitEvent::CloseRequested => state.loop_signal.stop(),
            _ => (),
        })?;

    Ok(())
}
