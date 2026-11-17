use smithay::{
    desktop::{
        PopupKeyboardGrab, PopupKind, PopupPointerGrab, PopupUngrabStrategy, Window,
        find_popup_root_surface, get_popup_toplevel_coords,
    },
    input::{
        Seat,
        pointer::{Focus, GrabStartData},
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Resource,
            protocol::{wl_output::WlOutput, wl_seat, wl_surface::WlSurface},
        },
    },
    utils::Serial,
    wayland::{
        compositor::with_states,
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            XdgToplevelSurfaceData,
        },
    },
};

use crate::{InfinityDesk, grabs::DragGrab, resize::Edge, state::Resizing};

/// Checks that a client move request belongs to a real, currently pressed pointer button.
fn check_grab(
    seat: &Seat<InfinityDesk>,
    surface: &WlSurface,
    serial: Serial,
) -> Option<GrabStartData<InfinityDesk>> {
    let pointer = seat.get_pointer()?;
    if !pointer.has_grab(serial) {
        return None;
    }
    let start_data = pointer.grab_start_data()?;
    let (focus, _) = start_data.focus.as_ref()?;
    if !focus.id().same_client_as(&surface.id()) {
        return None;
    }
    Some(start_data)
}

impl XdgShellHandler for InfinityDesk {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    /// A client opened a new window.
    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        tracing::info!("New window");
        // A new window (also a dialog) should be seen, so fullscreen ends.
        self.end_fullscreen();
        // The size is only known after the first commit; `place_if_pending` maps it then.
        // TODO: when a cluster is focused, dock the new window to that cluster instead (Plan.md).
        self.unplaced.push(Window::new_wayland_window(surface));
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        // Leave the cluster; the rest may split into several clusters.
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            self.selected.retain(|w| w != &window);
            self.resizing.remove(&window);
            self.resize_hint.retain(|(w, _)| w != &window);
            self.clusters.remove(&window);
            if self.fullscreen.as_ref().is_some_and(|f| f.window == window) {
                self.end_fullscreen();
            }
            if self
                .layout_menu
                .as_ref()
                .is_some_and(|m| m.window == window)
            {
                self.layout_menu = None;
            }
            self.forget_slot_anchor(&window);
        }
        self.unplaced.retain(|w| w.toplevel().unwrap() != &surface);
    }

    /// The client asks to fill the screen (e.g. a video player): same as Alt+F.
    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        let window = self.window_for_surface(surface.wl_surface());
        let already = self.fullscreen.as_ref().map(|f| &f.window) == window.as_ref();
        match window {
            Some(window) if !already && self.space.element_geometry(&window).is_some() => {
                self.end_fullscreen();
                self.start_fullscreen(&window);
            }
            // Not placed yet or already fullscreen: the client still expects an answer.
            _ => {
                surface.send_configure();
            }
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if (self.fullscreen.as_ref()).is_some_and(|f| f.window.toplevel() == Some(&surface)) {
            self.end_fullscreen();
        }
    }

    /// The client asks to be moved (e.g. dragging its own title bar).
    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        use xdg_toplevel::ResizeEdge as E;
        let seat = Seat::from_resource(&seat).unwrap();
        let Some(start_data) = check_grab(&seat, surface.wl_surface(), serial) else {
            return;
        };
        let Some(window) = self.window_for_surface(surface.wl_surface()) else {
            return;
        };
        if self.fullscreen.is_some() {
            return;
        }
        let x = match edges {
            E::Left | E::TopLeft | E::BottomLeft => Edge::Start,
            E::Right | E::TopRight | E::BottomRight => Edge::End,
            _ => Edge::None,
        };
        let y = match edges {
            E::Top | E::TopLeft | E::TopRight => Edge::Start,
            E::Bottom | E::BottomLeft | E::BottomRight => Edge::End,
            _ => Edge::None,
        };
        let shift = self.seat.get_keyboard().unwrap().modifier_state().shift;
        let grab = DragGrab::resize(self, start_data, window, [x, y], shift);
        seat.get_pointer()
            .unwrap()
            .set_grab(self, grab, serial, Focus::Clear);
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let seat = Seat::from_resource(&seat).unwrap();
        let Some(start_data) = check_grab(&seat, surface.wl_surface(), serial) else {
            return;
        };
        let Some(window) = self.window_for_surface(surface.wl_surface()) else {
            return;
        };
        if self.fullscreen.is_some() {
            return;
        }
        let grab = DragGrab::move_window(self, start_data, window);
        seat.get_pointer()
            .unwrap()
            .set_grab(self, grab, serial, Focus::Clear);
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    /// Menus: keyboard and pointer go to the popup until it closes; a click elsewhere closes it.
    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let Some(seat) = Seat::<Self>::from_resource(&seat) else {
            return;
        };
        let kind = PopupKind::Xdg(surface);
        let Ok(root) = find_popup_root_surface(&kind) else {
            return;
        };
        let Ok(mut grab) = self.popups.grab_popup(root, kind, &seat, serial) else {
            return;
        };
        // Only if the client's request belongs to the input that is currently going on.
        let keyboard = seat.get_keyboard().unwrap();
        let pointer = seat.get_pointer().unwrap();
        let previous = grab.previous_serial().unwrap_or(serial);
        if (keyboard.is_grabbed() && !keyboard.has_grab(serial) && !keyboard.has_grab(previous))
            || (pointer.is_grabbed() && !pointer.has_grab(serial) && !pointer.has_grab(previous))
        {
            grab.ungrab(PopupUngrabStrategy::All);
            return;
        }
        keyboard.set_focus(self, grab.current_grab(), serial);
        keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
    }
}

impl InfinityDesk {
    /// Must be called on every commit: sends the initial configure to new windows/popups.
    pub fn handle_xdg_commit(&mut self, surface: &WlSurface) {
        if let Some(window) = self.window_for_surface(surface) {
            let initial_configure_sent = with_states(surface, |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .initial_configure_sent
            });
            if !initial_configure_sent {
                // A free slot of the last template: the window gets its size right away and is
                // placed there (`place_if_pending`).
                if let Some(rect) = self.take_free_slot() {
                    let anchor_end = [false; 2];
                    let toplevel = window.toplevel().unwrap();
                    toplevel.with_pending_state(|s| s.size = Some(rect.size));
                    self.resizing
                        .insert(window.clone(), Resizing { rect, anchor_end });
                }
                window.toplevel().unwrap().send_configure();
            }
        }

        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(ref xdg)) = self.popups.find_popup(surface)
            && !xdg.is_initial_configure_sent()
        {
            xdg.send_configure()
                .expect("initial popup configure failed");
        }
    }

    /// Keeps popups (menus) from extending beyond the screen.
    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some(window_geo) = self
            .window_for_surface(&root)
            .and_then(|w| self.space.element_geometry(&w))
        else {
            return;
        };

        let mut target = self.canvas.viewport().to_i32_round();
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= window_geo.loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}
