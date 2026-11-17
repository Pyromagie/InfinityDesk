//! Keyboard and pointer input handling.

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent, KeyboardKeyEvent,
        PointerAxisEvent, PointerButtonEvent,
    },
    input::{
        keyboard::FilterResult,
        pointer::{AxisFrame, ButtonEvent, Focus, GrabStartData, MotionEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::SERIAL_COUNTER,
};

use crate::{InfinityDesk, grabs::DragGrab};

/// Left mouse button (linux/input-event-codes.h).
const BTN_LEFT: u32 = 0x110;

impl InfinityDesk {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time(&event);
                self.seat.get_keyboard().unwrap().input::<(), _>(
                    self,
                    event.key_code(),
                    event.state(),
                    serial,
                    time,
                    |_, _, _| FilterResult::Forward,
                );
            }

            InputEvent::PointerMotionAbsolute { event, .. } => {
                let Some(output) = self.space.outputs().next() else { return };
                let output_geo = self.space.output_geometry(output).unwrap();
                let pos = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();

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
                if button_state == ButtonState::Pressed && !pointer.is_grabbed() {
                    let location = pointer.current_location();
                    let clicked = self.space.element_under(location).map(|(w, _)| w.clone());
                    let alt = keyboard.modifier_state().alt;
                    let start_data = GrabStartData {
                        focus: None,
                        button,
                        location,
                    };

                    if let Some(window) = clicked {
                        self.raise_cluster(&window);
                        keyboard.set_focus(self, Some(window.toplevel().unwrap().wl_surface().clone()), serial);
                        if alt && button == BTN_LEFT {
                            let grab = DragGrab::move_window(self, start_data, window);
                            pointer.set_grab(self, grab, serial, Focus::Clear);
                        }
                    } else {
                        self.space.elements().for_each(|w| {
                            w.set_activated(false);
                        });
                        keyboard.set_focus(self, Option::<WlSurface>::None, serial);
                        if button == BTN_LEFT {
                            let grab = DragGrab::pan(self, start_data);
                            pointer.set_grab(self, grab, serial, Focus::Clear);
                        }
                    }
                    self.space.elements().for_each(|w| {
                        w.toplevel().unwrap().send_pending_configure();
                    });
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
