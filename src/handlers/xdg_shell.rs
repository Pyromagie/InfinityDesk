use smithay::{
    desktop::{PopupKind, PopupManager, Space, Window, find_popup_root_surface, get_popup_toplevel_coords},
    input::{
        Seat,
        pointer::{Focus, GrabStartData},
    },
    reexports::wayland_server::{
        Resource,
        protocol::{wl_seat, wl_surface::WlSurface},
    },
    utils::Serial,
    wayland::{
        compositor::with_states,
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState, XdgToplevelSurfaceData,
        },
    },
};

use crate::{InfinityDesk, grabs::DragGrab};

/// Checks that a client move request belongs to a real, currently pressed pointer button.
fn check_grab(seat: &Seat<InfinityDesk>, surface: &WlSurface, serial: Serial) -> Option<GrabStartData<InfinityDesk>> {
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
        let window = Window::new_wayland_window(surface);
        // The size is only known after the first commit. Until then, park it roughly in the middle;
        // `place_if_pending` centers it precisely afterwards.
        // TODO: when a cluster is focused, dock the new window to that cluster instead (Plan.md).
        let loc = self.canvas.viewport_center().to_i32_round();
        self.space.map_element(window.clone(), loc, true);
        self.unplaced.push(window);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.unplaced.retain(|w| w.toplevel().unwrap() != &surface);
        // Leave the cluster; the rest may split into several clusters.
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            let space = &self.space;
            self.clusters.remove(&window, |w| space.element_geometry(w));
        }
    }

    /// The client asks to be moved (e.g. dragging its own title bar).
    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let seat = Seat::from_resource(&seat).unwrap();
        let Some(start_data) = check_grab(&seat, surface.wl_surface(), serial) else {
            return;
        };
        let Some(window) = self.window_for_surface(surface.wl_surface()) else {
            return;
        };
        let grab = DragGrab::move_window(self, start_data, window);
        seat.get_pointer().unwrap().set_grab(self, grab, serial, Focus::Clear);
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        // TODO: popup grabs (e.g. menus that close when clicking elsewhere)
    }
}

/// Must be called on every commit: sends the initial configure to new windows/popups.
pub fn handle_commit(popups: &mut PopupManager, space: &Space<Window>, surface: &WlSurface) {
    if let Some(window) = space
        .elements()
        .find(|w| w.toplevel().unwrap().wl_surface() == surface)
    {
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
            window.toplevel().unwrap().send_configure();
        }
    }

    popups.commit(surface);
    if let Some(PopupKind::Xdg(ref xdg)) = popups.find_popup(surface)
        && !xdg.is_initial_configure_sent()
    {
        xdg.send_configure().expect("initial popup configure failed");
    }
}

impl InfinityDesk {
    /// Keeps popups (menus) from extending beyond the screen.
    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some(window) = self
            .space
            .elements()
            .find(|w| w.toplevel().unwrap().wl_surface() == &root)
        else {
            return;
        };
        let Some(output) = self.space.outputs().next() else {
            return;
        };

        let output_geo = self.space.output_geometry(output).unwrap();
        let window_geo = self.space.element_geometry(window).unwrap();

        let mut target = output_geo;
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= window_geo.loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}
