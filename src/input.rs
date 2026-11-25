//! Keyboard and pointer input handling.

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, GestureBeginEvent,
        GestureEndEvent, GesturePinchUpdateEvent, GestureSwipeUpdateEvent, InputBackend,
        InputEvent, InputTime, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
        PointerMotionEvent,
    },
    input::{
        keyboard::{FilterResult, Keysym, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, Focus, GrabStartData, MotionEvent},
    },
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::shell::wlr_layer::Layer,
};

use crate::{
    InfinityDesk,
    config::{Action, Key},
    grabs::DragGrab,
    layout,
    resize::Edge,
};

/// Mouse buttons (linux/input-event-codes.h).
const BTN_LEFT: u32 = 0x110;
/// Zoom change per scrolled pixel (a mouse wheel notch is 15 px, about 14 %), times the
/// `zoom_speed` from the config.
const ZOOM_SPEED: f64 = 0.01;
/// How far (screen pixels) four fingers swipe up or down to open or close the overview.
const OVERVIEW_SWIPE: f64 = 100.0;

/// A touchpad gesture in progress.
pub enum Gesture {
    /// Three fingers: the view follows them.
    Pan,
    /// Four fingers, with how far they went up (negative) or down so far.
    Overview(f64),
    /// Pinch, with its last scale: zooms around the pointer.
    Zoom(f64),
}

/// What a key press does.
enum KeyAction {
    /// A shortcut from the config.
    Bind(Action),
    /// In the template menu: a template by its number (0-based), or `None` to close the menu.
    Template(Option<usize>),
    /// In the template menu: other keys do nothing.
    Nothing,
    /// Ctrl+Alt+F1..F12: switch to another console (standalone).
    SwitchVt(i32),
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
                        // Alt+Tab ends when its modifiers are let go.
                        if state.switching.is_some() && !(mods.alt || mods.ctrl || mods.logo) {
                            state.end_switching();
                        }
                        if !pressed {
                            return FilterResult::Forward;
                        }
                        let sym = keysym.modified_sym();
                        let raw = keysym.raw_syms().first().copied();
                        let bound = raw.and_then(|raw| state.config.bound(Key::Sym(raw), mods));
                        let bound = bound.cloned();
                        let digit = sym.raw().wrapping_sub(Keysym::_1.raw()) as usize;
                        let vt = sym.raw().wrapping_sub(Keysym::XF86_Switch_VT_1.raw());
                        if vt < 12 {
                            return FilterResult::Intercept(KeyAction::SwitchVt(vt as i32 + 1));
                        }
                        // No other shortcuts while the screen is locked.
                        if state.lock.is_some() {
                            return FilterResult::Forward;
                        }
                        match (state.monitor.layout_menu.is_some(), bound) {
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
                match action {
                    Some(KeyAction::Bind(action)) => self.run(action),
                    Some(KeyAction::Template(Some(t))) => self.choose_template(t, None),
                    Some(KeyAction::Template(None)) => self.monitor.layout_menu = None,
                    Some(KeyAction::SwitchVt(vt)) => {
                        if let Some(udev) = &mut self.udev {
                            udev.switch_vt(vt);
                        }
                        return;
                    }
                    Some(KeyAction::Nothing) | None => return,
                }
                // Most actions move the view or windows; the pointer stays put on the screen.
                let screen = self.pointer_screen;
                self.pointer_moved(self.monitor.canvas.screen_to_canvas(screen), event.time());
            }

            // Mice and touchpads (standalone) move the pointer by an amount.
            InputEvent::PointerMotion { event, .. } => {
                self.move_pointer(event.delta(), event.time());
            }

            InputEvent::PointerMotionAbsolute { event, .. } => {
                let viewport = self.monitor.canvas.viewport();
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
                //   Mod + left-drag on a window          -> move the window's whole cluster
                //   Mod + left, hold still, then drag  -> pull just this window out of its cluster
                //                                         (or all marked windows, if it is marked)
                //   Mod + right-drag on a window         -> resize it at the nearest edges; neighbors
                //                                         in the cluster follow (Shift: they move)
                //   Mod + Shift + click on a window      -> mark / unmark it
                //   Mod + Shift + drag on empty canvas   -> mark windows with a selection box
                //   left-drag on a window border         -> resize it (Shift: the cluster moves)
                //   left-drag on empty canvas   -> pan the viewport
                // Where the pointer is on the canvas now (the view may have moved since the last
                // pointer motion, e.g. by panning).
                let location = self.monitor.canvas.screen_to_canvas(self.pointer_screen);
                if location != pointer.current_location() {
                    self.pointer_moved(location, event.time());
                }
                // While the screen is locked, the lock just gets the click.
                let locked = self.lock.is_some();
                // The template menu: a click on a slot chooses it, a click outside closes the menu.
                // Only presses are taken, so a drag still running gets its release.
                if self.monitor.layout_menu.is_some()
                    && button_state == ButtonState::Pressed
                    && !locked
                {
                    let screen = self.monitor.canvas.screen_size();
                    let on_screen = self.monitor.canvas.canvas_to_screen(location);
                    match layout::menu_hit(screen, on_screen) {
                        Some((t, s)) => self.choose_template(t, Some(s)),
                        None if !layout::menu_panel(screen).to_f64().contains(on_screen) => {
                            self.monitor.layout_menu = None;
                        }
                        None => {}
                    }
                    let canvas = self.monitor.canvas.screen_to_canvas(on_screen);
                    self.pointer_moved(canvas, event.time());
                    return;
                }
                // Launchers and bars on top of everything just get the click.
                let upper = self.upper_layers();
                let on_layer = (self.layer_surface_under(location, upper)).is_some();
                if button_state == ButtonState::Pressed && !pointer.is_grabbed() && !locked {
                    let windows_under = self.space.element_under(location).is_some();
                    self.click_layer(
                        location,
                        match on_layer || windows_under {
                            true => upper,
                            false => &[Layer::Bottom, Layer::Background],
                        },
                    );
                }
                // In the overview, a click on a window chooses it (and is not passed on).
                let clicked = self.space.element_under(location).map(|(w, _)| w.clone());
                let plain =
                    !(self.config.mod_key()).held(&keyboard.modifier_state()) && button == BTN_LEFT;
                if let Some(window) = (clicked.as_ref())
                    .filter(|_| self.monitor.overview.is_some() && plain && !on_layer && !locked)
                    .filter(|_| button_state == ButtonState::Pressed && !pointer.is_grabbed())
                {
                    let on_screen = self.monitor.canvas.canvas_to_screen(location);
                    self.choose_in_overview(window);
                    self.pointer_moved(
                        self.monitor.canvas.screen_to_canvas(on_screen),
                        event.time(),
                    );
                    return;
                }
                // A fullscreen window just gets the click, nothing on the canvas can be grabbed.
                let fullscreen = self.monitor.fullscreen.is_some();
                if button_state == ButtonState::Pressed
                    && !pointer.is_grabbed()
                    && !on_layer
                    && !fullscreen
                    && !locked
                {
                    let mods = keyboard.modifier_state();
                    let alt = self.config.mod_key().held(&mods);
                    let bound = self.mouse_bind(Key::Button(button), &mods, clicked.is_none());
                    let start_data = GrabStartData {
                        focus: None,
                        button,
                        location,
                    };

                    // A window border (also a bit outside the window) is grabbed to resize it.
                    let borders = match !alt && button == BTN_LEFT {
                        true => self.borders_under(location),
                        false => Vec::new(),
                    };
                    if let Some((window, _)) = borders.first() {
                        self.focus_window(&window.clone());
                        let grab = DragGrab::resize(self, start_data, borders, mods.shift);
                        pointer.set_grab(self, grab, serial, Focus::Clear);
                    } else if let Some(window) = clicked {
                        self.focus_window(&window);
                        if bound == Some(Action::DragSelect) {
                            self.toggle_selected(window);
                        } else if bound == Some(Action::DragMove) {
                            let grab = DragGrab::move_window(self, start_data, window);
                            pointer.set_grab(self, grab, serial, Focus::Clear);
                        } else if bound == Some(Action::DragResize) {
                            // Anywhere in the window: the pulling direction decides which edges
                            // move (see `choose_resize`), the nearest corner first.
                            let geo = self.space.element_geometry(&window).unwrap();
                            let near = |p: f64, start: i32, len: i32| p < (start + len / 2) as f64;
                            let near = [
                                near(location.x, geo.loc.x, geo.size.w),
                                near(location.y, geo.loc.y, geo.size.h),
                            ];
                            let edge = |start: bool| if start { Edge::Start } else { Edge::End };
                            let borders =
                                [(true, true), (false, true), (true, false), (false, false)]
                                    .map(|(x, y)| {
                                        (window.clone(), [edge(near[0] == x), edge(near[1] == y)])
                                    })
                                    .to_vec();
                            let grab = DragGrab::resize(self, start_data, borders, mods.shift);
                            pointer.set_grab(self, grab, serial, Focus::Clear);
                        }
                    } else {
                        self.space.elements().for_each(|w| {
                            w.set_activated(false);
                        });
                        self.space.elements().for_each(|w| {
                            w.toplevel().unwrap().send_pending_configure();
                        });
                        if self.active.take().is_some() {
                            self.emit("window.focus", &[], None);
                        }
                        self.refocus();
                        if bound == Some(Action::DragSelect) {
                            let grab = DragGrab::select(self, start_data);
                            pointer.set_grab(self, grab, serial, Focus::Clear);
                        } else if bound == Some(Action::DragMove) {
                            self.selected.clear();
                            let grab = DragGrab::pan(self, start_data);
                            pointer.set_grab(self, grab, serial, Focus::Clear);
                        }
                    }
                }

                let view = self.monitor.canvas.viewport_loc();
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
                // The view snapped when a pan ended: the canvas moved under the pointer.
                if self.monitor.canvas.viewport_loc() != view {
                    let screen = self.pointer_screen;
                    self.pointer_moved(self.monitor.canvas.screen_to_canvas(screen), event.time());
                }
            }

            InputEvent::GestureSwipeBegin { event, .. } => match event.fingers() {
                3 => self.gesture_begins(Gesture::Pan),
                4 => self.gesture_begins(Gesture::Overview(0.0)),
                _ => self.gesture = None,
            },
            InputEvent::GestureSwipeUpdate { event, .. } => match &mut self.gesture {
                // The view follows the fingers: content moves with them.
                Some(Gesture::Pan) => self.pan_screen(event.delta().upscale(-1.0), event.time()),
                Some(Gesture::Overview(dy)) => *dy += event.delta_y(),
                _ => {}
            },
            InputEvent::GestureSwipeEnd { event, .. } => {
                match self.gesture.take() {
                    Some(Gesture::Overview(dy))
                        if !event.cancelled()
                            && (dy < -OVERVIEW_SWIPE && self.monitor.overview.is_none()
                                || dy > OVERVIEW_SWIPE && self.monitor.overview.is_some()) =>
                    {
                        self.toggle_overview()
                    }
                    Some(Gesture::Pan) => self.snap_view(),
                    _ => return,
                }
                let screen = self.pointer_screen;
                self.pointer_moved(self.monitor.canvas.screen_to_canvas(screen), event.time());
            }
            InputEvent::GesturePinchBegin { .. } => self.gesture_begins(Gesture::Zoom(1.0)),
            InputEvent::GesturePinchUpdate { event, .. } => {
                if let Some(Gesture::Zoom(last)) = &mut self.gesture {
                    let factor = event.scale() / *last;
                    *last = event.scale();
                    let screen = self.pointer_screen;
                    self.monitor.canvas.zoom_at(screen, factor);
                    self.apply_viewport();
                    self.pointer_moved(self.monitor.canvas.screen_to_canvas(screen), event.time());
                }
            }
            InputEvent::GesturePinchEnd { .. } => self.gesture = None,
            // Mod + scroll (by default) zooms around the pointer (not while a window is
            // fullscreen).
            InputEvent::PointerAxis { event, .. }
                if (self.mouse_bind(
                    Key::Scroll,
                    &self.seat.get_keyboard().unwrap().modifier_state(),
                    self.on_empty_canvas(),
                ))
                .is_some()
                    && self.lock.is_none()
                    && !self.seat.get_pointer().unwrap().is_grabbed()
                    && self.monitor.fullscreen.is_none() =>
            {
                let scroll = event.amount(Axis::Vertical).unwrap_or_else(|| {
                    event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.0
                });
                let screen = self.pointer_screen;
                let speed = ZOOM_SPEED * self.config.zoom_speed;
                self.monitor.canvas.zoom_at(screen, (-scroll * speed).exp());
                self.apply_viewport();
                // The canvas moved under the pointer.
                self.pointer_moved(self.monitor.canvas.screen_to_canvas(screen), event.time());
            }

            // Scrolling on empty canvas moves the view.
            InputEvent::PointerAxis { event, .. } if self.on_empty_canvas() => {
                let factor = match event.source() {
                    // A notch counts as 15 px.
                    AxisSource::Wheel => self.config.wheel_pan / 15.0,
                    _ => 1.0,
                };
                let amount = |axis| scroll_amount(&event, axis) * factor;
                let delta = Point::from((amount(Axis::Horizontal), amount(Axis::Vertical)));
                self.pan_screen(delta, event.time());
                // Fingers lifted from the touchpad: the view snaps into the grid if almost there.
                if event.source() == AxisSource::Finger && delta == Point::default() {
                    self.snap_view();
                    let screen = self.pointer_screen;
                    self.pointer_moved(self.monitor.canvas.screen_to_canvas(screen), event.time());
                }
            }
            InputEvent::PointerAxis { event, .. } => {
                let source = event.source();
                let amount = |axis| scroll_amount(&event, axis);

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
    pub fn run(&mut self, action: Action) {
        self.finish_resize_glide();
        let keyboard = self.seat.get_keyboard().unwrap();
        let focused = (keyboard.current_focus()).and_then(|s| self.window_for_surface(&s));
        match action {
            Action::Spawn(command) => crate::spawn(&command, self),
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
            Action::Quit => self.loop_signal.stop(),
            Action::Lua(f) => self.call_lua(&f, mlua::MultiValue::new(), "key binding"),
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
                    Action::RecentWindows => self.switch_recent(),
                    _ => {}
                }
            }
        }
    }

    /// What a mouse button or scrolling is bound to in the config. Bindings without a modifier
    /// only act on empty canvas: over a window, the program gets the click.
    fn mouse_bind(&self, key: Key, mods: &ModifiersState, empty: bool) -> Option<Action> {
        let any_mod = mods.logo || mods.alt || mods.ctrl || mods.shift;
        let action = self.config.bound(key, mods)?;
        (any_mod || empty).then(|| action.clone())
    }

    /// The pointer is over nothing but the canvas (no window, no bar), and nothing else is going on.
    fn on_empty_canvas(&self) -> bool {
        let pointer = self.seat.get_pointer().unwrap();
        let pos = pointer.current_location();
        !pointer.is_grabbed()
            && self.lock.is_none()
            && self.monitor.fullscreen.is_none()
            && self.space.element_under(pos).is_none()
            && self.layer_surface_under(pos, self.upper_layers()).is_none()
    }

    /// Moves the view by `delta` screen pixels; the pointer stays put on the screen.
    fn pan_screen(&mut self, delta: Point<f64, Logical>, time: InputTime) {
        let screen = self.pointer_screen;
        let loc = self.monitor.canvas.viewport_loc() + delta.downscale(self.monitor.canvas.zoom());
        self.monitor.canvas.set_viewport_loc(loc);
        self.apply_viewport();
        self.pointer_moved(self.monitor.canvas.screen_to_canvas(screen), time);
    }

    /// Touchpad gestures: they work everywhere, also over windows (apps get none of them).
    /// Not while a drag or fullscreen is going on.
    fn gesture_begins(&mut self, gesture: Gesture) {
        let busy = self.seat.get_pointer().unwrap().is_grabbed()
            || self.monitor.fullscreen.is_some()
            || self.lock.is_some();
        self.gesture = (!busy).then_some(gesture);
    }

    /// Moves the pointer to a canvas position and updates what is under it.
    pub fn pointer_moved(&mut self, pos: Point<f64, Logical>, time: InputTime) {
        self.pointer_screen = self.monitor.canvas.canvas_to_screen(pos);
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

        let screen = self.monitor.canvas.canvas_to_screen(pos);
        let size = self.monitor.canvas.screen_size();
        if let Some(menu) = &mut self.monitor.layout_menu {
            menu.hover = layout::menu_hit(size, screen);
        }

        // Show which edges a click would grab (a running drag keeps its own).
        if !pointer.is_grabbed() {
            let mods = self.seat.get_keyboard().unwrap().modifier_state();
            let on_layer = (self.layer_surface_under(pos, self.upper_layers())).is_some();
            let free = self.monitor.layout_menu.is_none()
                && self.monitor.fullscreen.is_none()
                && self.lock.is_none();
            let hint = match !self.config.mod_key().held(&mods) && !on_layer && free {
                true => self.borders_under(pos),
                false => Vec::new(),
            };
            self.set_resize_hint(hint, mods.shift);
        }
    }
}

/// How far a scroll event goes along `axis`, in pixels (a wheel notch counts as 15).
fn scroll_amount<I: InputBackend>(event: &impl PointerAxisEvent<I>, axis: Axis) -> f64 {
    (event.amount(axis)).unwrap_or_else(|| event.amount_v120(axis).unwrap_or(0.0) * 15.0 / 120.0)
}
