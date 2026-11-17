//! Keyboard and pointer input handling.

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        InputTime, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    },
    input::{
        keyboard::{FilterResult, Keysym},
        pointer::{AxisFrame, ButtonEvent, Focus, GrabStartData, MotionEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, SERIAL_COUNTER},
};

use crate::{InfinityDesk, config::Action, grabs::DragGrab, layout, resize::Edge};

/// Mouse buttons (linux/input-event-codes.h).
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
/// Zoom change per scrolled pixel (a mouse wheel notch is 15 px, about 14 %).
const ZOOM_SPEED: f64 = 0.01;

/// What a key press does.
enum KeyAction {
    /// A shortcut from the config.
    Bind(Action),
    /// In the template menu: a template by its number (0-based), or `None` to close the menu.
    Template(Option<usize>),
    /// In the template menu: other keys do nothing.
    Nothing,
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
                    // Shortcuts from the config (see `config::Bind`), matched by the key without
                    // Shift, so "Mod+Shift+Q" is the Q key.
                    |state, mods, keysym| {
                        if !pressed {
                            return FilterResult::Forward;
                        }
                        let sym = keysym.modified_sym();
                        let raw = keysym.raw_syms().first().copied();
                        let bound = (state.config.binds.iter())
                            .find(|b| raw.is_some_and(|raw| b.keys.matches(raw, mods)))
                            .map(|b| b.action.clone());
                        let digit = sym.raw().wrapping_sub(Keysym::_1.raw()) as usize;
                        match (state.layout_menu.is_some(), bound) {
                            // The template menu takes all keys while it is open; its own shortcut
                            // closes it again.
                            (true, Some(Action::LayoutMenu)) => {
                                FilterResult::Intercept(KeyAction::Bind(Action::LayoutMenu))
                            }
                            (true, _) if sym == Keysym::Escape => {
                                FilterResult::Intercept(KeyAction::Template(None))
                            }
                            (true, _) if digit < layout::TEMPLATES.len() => {
                                FilterResult::Intercept(KeyAction::Template(Some(digit)))
                            }
                            (true, _) => FilterResult::Intercept(KeyAction::Nothing),
                            (false, Some(action)) => {
                                FilterResult::Intercept(KeyAction::Bind(action))
                            }
                            (false, None) => FilterResult::Forward,
                        }
                    },
                );
                // Most actions move the view or windows; the pointer stays put on the screen.
                let pointer = self.seat.get_pointer().unwrap();
                let screen = self.canvas.canvas_to_screen(pointer.current_location());
                match action {
                    Some(KeyAction::Bind(action)) => self.run(action),
                    Some(KeyAction::Template(Some(t))) => self.choose_template(t, None),
                    Some(KeyAction::Template(None)) => self.layout_menu = None,
                    Some(KeyAction::Nothing) | None => return,
                }
                self.pointer_moved(self.canvas.screen_to_canvas(screen), event.time());
            }

            InputEvent::PointerMotionAbsolute { event, .. } => {
                let viewport = self.canvas.viewport();
                let pos = event.position_transformed(viewport.size.to_i32_round()) + viewport.loc;
                self.pointer_moved(pos, event.time());
            }

            InputEvent::PointerButton { event, .. } => {
                let pointer = self.seat.get_pointer().unwrap();
                let keyboard = self.seat.get_keyboard().unwrap();
                let serial = SERIAL_COUNTER.next_serial();
                let button_state = event.state();

                let button = event.button_code();

                // Clicking a window raises and focuses it (with its cluster); clicking empty space unfocuses.
                // Provisional controls until bindings become configurable:
                //   Alt + left-drag on a window          -> move the window's whole cluster
                //   Alt + left, hold still, then drag  -> pull just this window out of its cluster
                //                                         (or all marked windows, if it is marked)
                //   Alt + right-drag on a window         -> resize it at the nearest edges; neighbors
                //                                         in the cluster follow (Shift: they move)
                //   Alt + Shift + click on a window      -> mark / unmark it
                //   Alt + Shift + drag on empty canvas   -> mark windows with a selection box
                //   left-drag on a window border         -> resize it (Shift: the cluster moves)
                //   left-drag on empty canvas   -> pan the viewport
                let location = pointer.current_location();
                // The template menu: a click on a slot chooses it, a click outside closes the menu.
                // Only presses are taken, so a drag still running gets its release.
                if self.layout_menu.is_some() && button_state == ButtonState::Pressed {
                    let screen = self.canvas.screen_size();
                    let on_screen = self.canvas.canvas_to_screen(location);
                    match layout::menu_hit(screen, on_screen) {
                        Some((t, s)) => self.choose_template(t, Some(s)),
                        None if !layout::menu_panel(screen).to_f64().contains(on_screen) => {
                            self.layout_menu = None;
                        }
                        None => {}
                    }
                    let canvas = self.canvas.screen_to_canvas(on_screen);
                    self.pointer_moved(canvas, event.time());
                    return;
                }
                // Launchers and bars on top of everything just get the click.
                let on_layer = (self.layer_surface_under(location, self.upper_layers())).is_some();
                // In the overview, a click on a window chooses it (and is not passed on).
                let clicked = self.space.element_under(location).map(|(w, _)| w.clone());
                let plain = !keyboard.modifier_state().alt && button == BTN_LEFT;
                if let Some(window) = (clicked.as_ref())
                    .filter(|_| self.overview.is_some() && plain && !on_layer)
                    .filter(|_| button_state == ButtonState::Pressed && !pointer.is_grabbed())
                {
                    let on_screen = self.canvas.canvas_to_screen(location);
                    self.choose_in_overview(window);
                    self.pointer_moved(self.canvas.screen_to_canvas(on_screen), event.time());
                    return;
                }
                // A fullscreen window just gets the click, nothing on the canvas can be grabbed.
                let fullscreen = self.fullscreen.is_some();
                if button_state == ButtonState::Pressed
                    && !pointer.is_grabbed()
                    && !on_layer
                    && !fullscreen
                {
                    let mods = keyboard.modifier_state();
                    let (alt, select) = (mods.alt, mods.alt && mods.shift && button == BTN_LEFT);
                    let start_data = GrabStartData {
                        focus: None,
                        button,
                        location,
                    };

                    // A window border (also a bit outside the window) is grabbed to resize it.
                    let border = (!alt && button == BTN_LEFT)
                        .then(|| self.border_under(location))
                        .flatten();
                    if let Some((window, edges)) = border {
                        self.focus_window(&window);
                        let grab = DragGrab::resize(self, start_data, window, edges, mods.shift);
                        pointer.set_grab(self, grab, serial, Focus::Clear);
                    } else if let Some(window) = clicked {
                        self.focus_window(&window);
                        if select {
                            self.toggle_selected(window);
                        } else if alt && button == BTN_LEFT {
                            let grab = DragGrab::move_window(self, start_data, window);
                            pointer.set_grab(self, grab, serial, Focus::Clear);
                        } else if alt && button == BTN_RIGHT {
                            // Resize at the edges nearest to the pointer (like a corner).
                            let geo = self.space.element_geometry(&window).unwrap();
                            let edge =
                                |p: f64, start: i32, len: i32| match p < (start + len / 2) as f64 {
                                    true => Edge::Start,
                                    false => Edge::End,
                                };
                            let edges = [
                                edge(location.x, geo.loc.x, geo.size.w),
                                edge(location.y, geo.loc.y, geo.size.h),
                            ];
                            let grab =
                                DragGrab::resize(self, start_data, window, edges, mods.shift);
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
                        self.active = None;
                        if select {
                            let grab = DragGrab::select(self, start_data);
                            pointer.set_grab(self, grab, serial, Focus::Clear);
                        } else if button == BTN_LEFT {
                            self.selected.clear();
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

            // Alt + scroll zooms around the pointer (not while a window is fullscreen).
            InputEvent::PointerAxis { event, .. }
                if self.seat.get_keyboard().unwrap().modifier_state().alt
                    && !self.seat.get_pointer().unwrap().is_grabbed()
                    && self.fullscreen.is_none() =>
            {
                let scroll = event.amount(Axis::Vertical).unwrap_or_else(|| {
                    event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.0
                });
                let pointer = self.seat.get_pointer().unwrap();
                let screen = self.canvas.canvas_to_screen(pointer.current_location());
                self.canvas.zoom_at(screen, (-scroll * ZOOM_SPEED).exp());
                self.apply_viewport();
                // The canvas moved under the pointer.
                self.pointer_moved(self.canvas.screen_to_canvas(screen), event.time());
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

    /// Runs a shortcut's action. Window actions work on the window with the keyboard focus,
    /// navigation starts from the active window (also while a launcher has the keyboard).
    fn run(&mut self, action: Action) {
        let keyboard = self.seat.get_keyboard().unwrap();
        let focused = (keyboard.current_focus()).and_then(|s| self.window_for_surface(&s));
        match action {
            Action::Spawn(command) => crate::spawn(&command, &self.socket_name),
            Action::CloseWindow => {
                if let Some(window) = focused {
                    window.toplevel().unwrap().send_close();
                }
            }
            Action::Fullscreen => {
                if let Some(window) = focused {
                    self.toggle_fullscreen(&window);
                }
            }
            Action::LayoutMenu => self.toggle_layout_menu(focused),
            // Moving around ends fullscreen first.
            action => {
                self.end_fullscreen();
                match action {
                    Action::Focus(dir) => self.focus_in(dir),
                    Action::Swap(dir) => self.swap_in(dir),
                    Action::Resize(dir) => self.resize_in(dir),
                    Action::Pan(dir) => self.pan(dir),
                    Action::ZoomIn => self.zoom_by(true),
                    Action::ZoomOut => self.zoom_by(false),
                    Action::Center => self.center_cluster(),
                    Action::Overview => self.toggle_overview(),
                    _ => {}
                }
            }
        }
    }

    /// Moves the pointer to a canvas position and updates what is under it.
    pub fn pointer_moved(&mut self, pos: Point<f64, Logical>, time: InputTime) {
        let pointer = self.seat.get_pointer().unwrap();
        let under = self.surface_under(pos);
        let serial = SERIAL_COUNTER.next_serial();
        let motion = MotionEvent {
            location: pos,
            serial,
            time,
        };
        pointer.motion(self, under, &motion);
        pointer.frame(self);

        let screen = self.canvas.canvas_to_screen(pos);
        if let Some(menu) = &mut self.layout_menu {
            menu.hover = layout::menu_hit(self.canvas.screen_size(), screen);
        }

        // Show which edges a click would grab (a running drag keeps its own).
        if !pointer.is_grabbed() {
            let mods = self.seat.get_keyboard().unwrap().modifier_state();
            let on_layer = (self.layer_surface_under(pos, self.upper_layers())).is_some();
            let free = self.layout_menu.is_none() && self.fullscreen.is_none();
            let hint = (!mods.alt && !on_layer && free)
                .then(|| self.border_under(pos))
                .flatten();
            self.set_resize_hint(hint, mods.shift);
        }
    }
}
