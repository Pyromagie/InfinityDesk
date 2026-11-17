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

/// Prüft, ob ein Verschiebe-Wunsch zu einem echten, gerade gedrückten Mausklick gehört.
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

    /// Ein Programm hat ein neues Fenster geöffnet.
    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        tracing::info!("Neues Fenster");
        let window = Window::new_wayland_window(surface);
        // Die Größe kennen wir erst beim ersten Inhalt. Bis dahin grob in die Mitte legen,
        // danach zentriert `place_if_pending` es genau.
        // TODO: bei fokussiertem Cluster stattdessen an das Cluster andocken (Plan.md).
        let loc = self.canvas.viewport_center().to_i32_round();
        self.space.map_element(window.clone(), loc, true);
        self.unplaced.push(window);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.unplaced.retain(|w| w.toplevel().unwrap() != &surface);
        // Aus dem Cluster nehmen; der Rest teilt sich ggf. in mehrere Cluster auf.
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            let space = &self.space;
            self.clusters.remove(&window, |w| space.element_geometry(w));
        }
    }

    /// Das Programm möchte verschoben werden (z. B. Ziehen an der eigenen Titelleiste).
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
        // TODO: Popup-Grabs (z. B. Menüs, die sich bei Klick daneben schließen)
    }
}

/// Muss bei jedem Commit aufgerufen werden: schickt die erste Konfiguration an neue Fenster/Popups.
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
        xdg.send_configure().expect("erste Popup-Konfiguration fehlgeschlagen");
    }
}

impl InfinityDesk {
    /// Sorgt dafür, dass Popups (Menüs) nicht aus dem Bildschirm ragen.
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
