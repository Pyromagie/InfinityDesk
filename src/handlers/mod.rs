//! Implementierungen der Wayland-Protokolle, die Smithay von uns verlangt.

mod compositor;
mod xdg_shell;

use smithay::{
    input::{
        Seat, SeatHandler, SeatState,
        dnd::{DnDGrab, DndGrabHandler, GrabType, Source},
        pointer::{CursorImageStatus, Focus},
    },
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    utils::Serial,
    wayland::{
        output::OutputHandler,
        pointer_constraints::PointerConstraintsHandler,
        selection::{
            SelectionHandler,
            data_device::{DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler, set_data_device_focus},
        },
    },
};

use crate::InfinityDesk;

// --- Seat (Tastatur + Maus) ---

impl SeatHandler for InfinityDesk {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {}

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let dh = &self.display_handle;
        let client = focused.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client);
    }
}

impl PointerConstraintsHandler for InfinityDesk {}

// --- Zwischenablage und Drag & Drop ---

impl SelectionHandler for InfinityDesk {
    type SelectionUserData = ();
}

impl DataDeviceHandler for InfinityDesk {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.data_device_state
    }
}

impl DndGrabHandler for InfinityDesk {}

impl WaylandDndGrabHandler for InfinityDesk {
    fn dnd_requested<S: Source>(
        &mut self,
        source: S,
        _icon: Option<WlSurface>,
        seat: Seat<Self>,
        serial: Serial,
        type_: GrabType,
    ) {
        match type_ {
            GrabType::Pointer => {
                let ptr = seat.get_pointer().unwrap();
                let start_data = ptr.grab_start_data().unwrap();
                let grab = DnDGrab::new_pointer(&self.display_handle, start_data, source, seat);
                ptr.set_grab(self, grab, serial, Focus::Keep);
            }
            GrabType::Touch => source.cancel(),
        }
    }
}

// --- Ausgaben (Monitore) ---

impl OutputHandler for InfinityDesk {}

smithay::delegate_dispatch2!(InfinityDesk);
