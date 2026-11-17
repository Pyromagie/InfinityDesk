//! Keyboard and pointer input handling.

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    },
    input::{
        keyboard::{FilterResult, Keysym},
        pointer::{AxisFrame, ButtonEvent, Focus, GrabStartData, MotionEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::SERIAL_COUNTER,
    wayland::shell::wlr_layer::Layer,
};

use crate::{InfinityDesk, grabs::DragGrab};

/// Left mouse button (linux/input-event-codes.h).
const BTN_LEFT: u32 = 0x110;

enum Action {
    Spawn(&'static str),
    CloseWindow,
}

impl InfinityDesk {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let pressed = event.state() == KeyState::Pressed;
                let keyboard = self.seat.get_keyboard().unwrap();
                let action = keyboard.input(
                    self,
                    event.key_code(),
                    event.state(),
                    SERIAL_COUNTER.next_serial(),
                    event.time(),
                    // Provisional niri-like bindings. Mod is Alt while nested (niri does the same).
                    |_, mods, keysym| match keysym.modified_sym() {
                        _ if !pressed || !mods.alt => FilterResult::Forward,
                        Keysym::t => FilterResult::Intercept(Action::Spawn("alacritty")),
                        Keysym::d => FilterResult::Intercept(Action::Spawn("fuzzel")),
                        Keysym::q => FilterResult::Intercept(Action::CloseWindow),
                        _ => FilterResult::Forward,
                    },
                );
                match action {
                    Some(Action::Spawn(command)) => crate::spawn(command),
                    Some(Action::CloseWindow) => {
                        let focused = keyboard
                            .current_focus()
                            .and_then(|s| self.window_for_surface(&s));
                        if let Some(window) = focused {
                            window.toplevel().unwrap().send_close();
                        }
                    }
                    None => {}
                }
            }

            InputEvent::PointerMotionAbsolute { event, .. } => {
                let viewport = self.canvas.viewport();
                let pos = event.position_transformed(viewport.size.to_i32_round()) + viewport.loc;

                let serial = SERIAL_COUNTER.next_serial();
                let pointer = self.seat.get_pointer().unwrap();
                let under = self.surface_under(pos);
                pointer.motion(
                    self,
                    under,
                    &MotionEvent {
                        location: pos,
                        serial,
                        time: event.time(),
                    },
                );
                pointer.frame(self);
            }

            InputEvent::PointerButton { event, .. } => {
                let pointer = self.seat.get_pointer().unwrap();
                let keyboard = self.seat.get_keyboard().unwrap();
                let serial = SERIAL_COUNTER.next_serial();
                let button_state = event.state();

                let button = event.button_code();

                // Clicking a window raises and focuses it (with its cluster); clicking empty space unfocuses.
                // Provisional controls until bindings become configurable:
                //   Alt + left-drag on a window -> move the window's cluster
                //   left-drag on empty canvas   -> pan the viewport
                let location = pointer.current_location();
                // Launchers and bars on top of everything just get the click.
                let on_layer = self
                    .layer_surface_under(location, &[Layer::Overlay, Layer::Top])
                    .is_some();
                if button_state == ButtonState::Pressed && !pointer.is_grabbed() && !on_layer {
                    let clicked = self.space.element_under(location).map(|(w, _)| w.clone());
                    let alt = keyboard.modifier_state().alt;
                    let start_data = GrabStartData {
                        focus: None,
                        button,
                        location,
                    };

                    if let Some(window) = clicked {
                        self.focus_window(&window);
                        if alt && button == BTN_LEFT {
                            let grab = DragGrab::move_window(self, start_data, window);
                            pointer.set_grab(self, grab, serial, Focus::Clear);
                        }
                    } else {
                        self.space.elements().for_each(|w| {
                            w.set_activated(false);
                        });
                        self.space.elements().for_each(|w| {
                            w.toplevel().unwrap().send_pending_configure();
                        });
                        keyboard.set_focus(self, Option::<WlSurface>::None, serial);
                        if button == BTN_LEFT {
                            let grab = DragGrab::pan(self, start_data);
                            pointer.set_grab(self, grab, serial, Focus::Clear);
                        }
                    }
                }

                pointer.button(
                    self,
                    &ButtonEvent {
                        button,
                        state: button_state,
                        serial,
                        time: event.time(),
                    },
                );
                pointer.frame(self);
            }

            InputEvent::PointerAxis { event, .. } => {
                let source = event.source();
                let amount = |axis| {
                    event
                        .amount(axis)
                        .unwrap_or_else(|| event.amount_v120(axis).unwrap_or(0.0) * 15.0 / 120.)
                };

                let mut frame = AxisFrame::new(event.time()).source(source);
                for axis in [Axis::Horizontal, Axis::Vertical] {
                    let value = amount(axis);
                    if value != 0.0 {
                        frame = frame.value(axis, value);
                        if let Some(discrete) = event.amount_v120(axis) {
                            frame = frame.v120(axis, discrete as i32);
                        }
                    }
                    if source == AxisSource::Finger && event.amount(axis) == Some(0.0) {
                        frame = frame.stop(axis);
                    }
                }

                let pointer = self.seat.get_pointer().unwrap();
                pointer.axis(self, frame);
                pointer.frame(self);
            }

            _ => {}
        }
    }
}
