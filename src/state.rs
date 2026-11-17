use std::{ffi::OsString, sync::Arc};

use crate::{
    cluster::Clusters,
    canvas::{Canvas, DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH},
    render::Grid,
};
use smithay::{
    desktop::{PopupManager, Space, Window, WindowSurfaceType},
    input::{Seat, SeatState},
    output::Output,
    reexports::{
        calloop::{EventLoop, Interest, LoopSignal, Mode, PostAction, generic::Generic},
        wayland_server::{
            Display, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{Logical, Point},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        shell::xdg::XdgShellState,
        shm::ShmState,
        socket::ListeningSocketSource,
    },
};

/// Der gesamte Zustand des Compositors.
pub struct InfinityDesk {
    pub start_time: std::time::Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,

    /// Die Arbeitsfläche mit Viewport.
    pub canvas: Canvas,
    /// Alle Fenster und Ausgaben in Canvas-Koordinaten. Die Ausgabe sitzt dort, wo der Viewport ist.
    pub space: Space<Window>,
    pub popups: PopupManager,
    /// Der Bildschirm (im genesteten Betrieb: das winit-Fenster).
    pub output: Option<Output>,
    /// Fenster, die noch keinen Inhalt hatten und deshalb noch nicht platziert sind.
    pub unplaced: Vec<Window>,
    /// Welche Fenster zu welchem Cluster gehören.
    pub clusters: Clusters<Window>,
    pub grid: Grid,

    // Smithay-Protokollzustände
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<InfinityDesk>,
    pub data_device_state: DataDeviceState,
    pub seat: Seat<Self>,
}

impl InfinityDesk {
    pub fn new(event_loop: &mut EventLoop<Self>, display: Display<Self>) -> Self {
        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let data_device_state = DataDeviceState::new::<Self>(&dh);

        let mut seat_state = SeatState::new();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "seat0");
        seat.add_keyboard(Default::default(), 200, 25).unwrap();
        seat.add_pointer();

        let socket_name = Self::init_wayland_listener(display, event_loop);

        Self {
            start_time: std::time::Instant::now(),
            socket_name,
            display_handle: dh,
            loop_signal: event_loop.get_signal(),
            canvas: Canvas::new((DEFAULT_CANVAS_WIDTH, DEFAULT_CANVAS_HEIGHT).into()),
            space: Space::default(),
            popups: PopupManager::default(),
            output: None,
            unplaced: Vec::new(),
            clusters: Clusters::default(),
            grid: Grid::default(),
            compositor_state,
            xdg_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            seat,
        }
    }

    /// Öffnet den Wayland-Socket, über den sich Programme verbinden.
    fn init_wayland_listener(display: Display<Self>, event_loop: &mut EventLoop<Self>) -> OsString {
        let listening_socket = ListeningSocketSource::new_auto().unwrap();
        let socket_name = listening_socket.socket_name().to_os_string();
        let handle = event_loop.handle();

        handle
            .insert_source(listening_socket, |client_stream, _, state| {
                state
                    .display_handle
                    .insert_client(client_stream, Arc::new(ClientState::default()))
                    .unwrap();
            })
            .expect("Wayland-Socket konnte nicht registriert werden");

        handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    // Safety: das Display wird nie gedroppt, solange die Quelle lebt.
                    unsafe {
                        display.get_mut().dispatch_clients(state).unwrap();
                    }
                    Ok(PostAction::Continue)
                },
            )
            .unwrap();

        socket_name
    }

    /// Überträgt die Viewport-Position des Canvas auf die Ausgabe im Space.
    pub fn apply_viewport(&mut self) {
        if let Some(output) = &self.output {
            self.space.map_output(output, self.canvas.viewport_loc().to_i32_round());
        }
    }

    /// Platziert ein neues Fenster, sobald seine Größe bekannt ist (beim ersten Inhalt).
    pub fn place_if_pending(&mut self, surface: &WlSurface) {
        let Some(idx) = self
            .unplaced
            .iter()
            .position(|w| w.toplevel().unwrap().wl_surface() == surface)
        else {
            return;
        };
        let size = self.unplaced[idx].geometry().size;
        if size.w <= 0 || size.h <= 0 {
            return;
        }
        let window = self.unplaced.remove(idx);
        let loc = self.canvas.centered_in_viewport(size);
        tracing::info!(
            "Fenster platziert bei {:?} (Größe {:?}, Viewport {:?})",
            loc,
            size,
            self.canvas.viewport()
        );
        self.space.map_element(window, loc, true);
    }

    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<Window> {
        self.space
            .elements()
            .find(|w| w.toplevel().unwrap().wl_surface() == surface)
            .cloned()
    }

    /// Holt ein Fenster samt seinem ganzen Cluster nach vorne.
    pub fn raise_cluster(&mut self, window: &Window) {
        for w in self.clusters.members(window) {
            if &w != window {
                self.space.raise_element(&w, false);
            }
        }
        self.space.raise_element(window, true);
    }

    /// Findet die Oberfläche (Fenster/Popup) unter einem Punkt.
    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space.element_under(pos).and_then(|(window, location)| {
            window
                .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                .map(|(s, p)| (s, (p + location).to_f64()))
        })
    }
}

/// Daten pro verbundenem Programm.
#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}
