//! Wayland protocol handlers required by Smithay.

mod compositor;
mod layer_shell;
mod xdg_shell;

pub use xdg_shell::app_id_and_title;

use smithay::{
    backend::{allocator::dmabuf::Dmabuf, renderer::ImportDma},
    input::{
        Seat, SeatHandler, SeatState,
        dnd::{DnDGrab, DndGrabHandler, GrabType, Source},
        pointer::{CursorImageStatus, Focus},
    },
    reexports::{
        wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode,
        wayland_server::{Resource, protocol::wl_surface::WlSurface},
    },
    utils::Serial,
    wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
    wayland::{
        compositor::with_states,
        fractional_scale::{FractionalScaleHandler, with_fractional_scale},
        output::OutputHandler,
        pointer_constraints::PointerConstraintsHandler,
        selection::{
            SelectionHandler,
            data_device::{
                DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler, set_data_device_focus,
            },
        },
        shell::xdg::{ToplevelSurface, decoration::XdgDecorationHandler},
    },
};

use crate::InfinityDesk;

// --- Seat (keyboard + pointer) ---

impl SeatHandler for InfinityDesk {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.client_cursor = image;
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let dh = &self.display_handle;
        let client = focused.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client);
    }
}

impl PointerConstraintsHandler for InfinityDesk {}

// --- Clipboard and drag & drop ---

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

// --- GPU buffers from apps (standalone) ---

impl DmabufHandler for InfinityDesk {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    /// A buffer is only accepted if the GPU can use it.
    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        let renderer = self.udev.as_mut().map(|u| &mut u.renderer);
        if renderer.is_some_and(|r| r.import_dmabuf(&dmabuf, None).is_ok()) {
            let _ = notifier.successful::<Self>();
        } else {
            notifier.failed();
        }
    }
}

// --- Titlebars ---

/// With `titlebars = false` (the default) programs that support it leave out their own titlebar
/// and InfinityDesk draws none, so windows in a cluster fit together. Otherwise each program
/// decides.
impl XdgDecorationHandler for InfinityDesk {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        self.set_decoration(&toplevel, None);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: Mode) {
        self.set_decoration(&toplevel, Some(mode));
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.set_decoration(&toplevel, None);
    }

    fn decoration_destroyed(&mut self, _toplevel: ToplevelSurface) {}
}

impl InfinityDesk {
    /// `ServerSide` means: no titlebar from the program (InfinityDesk draws none either).
    fn set_decoration(&mut self, toplevel: &ToplevelSurface, wanted: Option<Mode>) {
        let mode = match self.config.titlebars {
            false => Mode::ServerSide,
            true => wanted.unwrap_or(Mode::ClientSide),
        };
        toplevel.with_pending_state(|s| s.decoration_mode = Some(mode));
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }
}

// --- Outputs (monitors) ---

impl OutputHandler for InfinityDesk {}

/// An app asks for the screen's scale for one of its surfaces (a window on another monitor
/// gets that one's with its first frame).
impl FractionalScaleHandler for InfinityDesk {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        if let Some(output) = &self.monitor.output {
            let scale = output.current_scale().fractional_scale();
            with_states(&surface, |states| {
                with_fractional_scale(states, |f| f.set_preferred_scale(scale))
            });
        }
    }
}

smithay::delegate_dispatch2!(InfinityDesk);
