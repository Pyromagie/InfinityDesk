use std::{
    ffi::OsString,
    sync::Arc,
    time::{Duration, Instant},
};

/// How long the "unlocked" pulse lasts.
const PULSE_TIME: Duration = Duration::from_millis(350);
/// How far the pulse grows outward (in pixels).
const PULSE_GROW: f64 = 16.0;

use crate::{
    canvas::{Canvas, DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH},
    cluster::Clusters,
    grabs::HOLD_TIME,
    render::{OUTLINE_WIDTH, Outline},
};
use smithay::{
    desktop::{PopupManager, Space, Window, WindowSurfaceType, layer_map_for_output},
    input::{Seat, SeatState},
    output::Output,
    reexports::{
        calloop::{EventLoop, Interest, LoopSignal, Mode, PostAction, generic::Generic},
        wayland_server::{
            Display, DisplayHandle, backend::ClientData, protocol::wl_surface::WlSurface,
        },
    },
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        shell::{
            wlr_layer::{Layer, WlrLayerShellState},
            xdg::XdgShellState,
        },
        shm::ShmState,
        socket::ListeningSocketSource,
    },
};

/// The complete compositor state.
pub struct InfinityDesk {
    pub start_time: std::time::Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,

    /// The canvas and its viewport.
    pub canvas: Canvas,
    /// All windows and outputs in canvas coordinates. The output sits where the viewport is.
    pub space: Space<Window>,
    pub popups: PopupManager,
    /// The screen (when nested: the winit window).
    pub output: Option<Output>,
    /// Windows that have not committed a buffer yet and are therefore not placed yet.
    pub unplaced: Vec<Window>,
    /// Which windows belong to which cluster.
    pub clusters: Clusters<Window>,
    /// A window held to pull it out of its cluster, with the press time. It pulses when unlocked.
    pub pulse: Option<(Window, Instant)>,
    /// Marked windows (Alt+Shift); they are pulled out of their cluster together.
    pub selected: Vec<Window>,
    /// The selection box being dragged open, in canvas coordinates.
    pub selection_box: Option<Rectangle<i32, Logical>>,

    // Smithay protocol state
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub layer_shell_state: WlrLayerShellState,
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
        let layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
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
            pulse: None,
            selected: Vec::new(),
            selection_box: None,
            compositor_state,
            xdg_shell_state,
            layer_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            seat,
        }
    }

    /// Opens the Wayland socket that clients connect to.
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
            .expect("failed to register the Wayland socket");

        handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    // Safety: the display is never dropped while the source is alive.
                    unsafe {
                        display.get_mut().dispatch_clients(state).unwrap();
                    }
                    Ok(PostAction::Continue)
                },
            )
            .unwrap();

        socket_name
    }

    /// Moves the output within the space to the canvas viewport position.
    pub fn apply_viewport(&mut self) {
        if let Some(output) = &self.output {
            self.space
                .map_output(output, self.canvas.viewport_loc().to_i32_round());
        }
    }

    /// Places a new window once its size is known (on its first buffer commit).
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
        self.space.map_element(window.clone(), loc, false);
        self.focus_window(&window);
    }

    /// Raises the window with its cluster and gives it keyboard focus.
    pub fn focus_window(&mut self, window: &Window) {
        self.raise_cluster(window);
        let surface = window.toplevel().unwrap().wl_surface().clone();
        let keyboard = self.seat.get_keyboard().unwrap();
        keyboard.set_focus(self, Some(surface), SERIAL_COUNTER.next_serial());
        // Tell all windows whether they are active now.
        self.space.elements().for_each(|w| {
            w.toplevel().unwrap().send_pending_configure();
        });
    }

    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<Window> {
        self.space
            .elements()
            .find(|w| w.toplevel().unwrap().wl_surface() == surface)
            .cloned()
    }

    /// Raises a window together with its whole cluster.
    pub fn raise_cluster(&mut self, window: &Window) {
        for w in self.clusters.members(window) {
            if &w != window {
                self.space.raise_element(&w, false);
            }
        }
        self.space.raise_element(window, true);
    }

    /// Blue frames to draw: the "unlocked" pulse, selected windows and the selection box.
    pub fn outlines(&mut self) -> Vec<Outline> {
        let mut outlines: Vec<Outline> = self
            .selected
            .iter()
            .filter_map(|w| self.space.element_geometry(w))
            .map(|geo| Outline {
                rect: self.screen_rect(geo),
                grow: 0,
                alpha: 1.0,
            })
            .collect();
        if let Some(rect) = self.selection_box {
            outlines.push(Outline {
                rect: self.screen_rect(rect),
                grow: -OUTLINE_WIDTH,
                alpha: 1.0,
            });
        }
        // The pulse grows outward and fades once a held window is unlocked.
        if let Some((window, pressed)) = self.pulse.clone() {
            let since_unlock = pressed.elapsed().checked_sub(HOLD_TIME);
            let t = since_unlock.map(|d| d.as_secs_f64() / PULSE_TIME.as_secs_f64());
            match (t, self.space.element_geometry(&window)) {
                (Some(t), Some(geo)) if t < 1.0 => outlines.push(Outline {
                    rect: self.screen_rect(geo),
                    grow: (t * PULSE_GROW) as i32,
                    alpha: (1.0 - t) as f32,
                }),
                (None, Some(_)) => {} // Not unlocked yet.
                _ => self.pulse = None,
            }
        }
        outlines
    }

    /// Converts a rectangle on the canvas into one on the screen.
    fn screen_rect(&self, rect: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
        let loc = self.canvas.canvas_to_screen(rect.loc.to_f64());
        let size = rect.size.to_f64().upscale(self.canvas.zoom());
        Rectangle::new(loc, size).to_i32_round()
    }

    /// Alt+Shift+click: marks a window, or unmarks it.
    pub fn toggle_selected(&mut self, window: Window) {
        match self.selected.iter().position(|w| w == &window) {
            Some(i) => drop(self.selected.remove(i)),
            None => self.selected.push(window),
        }
    }

    /// Finds the surface under a canvas point: launchers/bars on top, then windows, then wallpapers.
    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.layer_surface_under(pos, &[Layer::Overlay, Layer::Top])
            .or_else(|| {
                self.space
                    .element_under(pos)
                    .and_then(|(window, location)| {
                        window
                            .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                            .map(|(s, p)| (s, (p + location).to_f64()))
                    })
            })
            .or_else(|| self.layer_surface_under(pos, &[Layer::Bottom, Layer::Background]))
    }

    /// Layer surfaces are placed on the screen, so convert canvas <-> screen coordinates around the lookup.
    pub fn layer_surface_under(
        &self,
        pos: Point<f64, Logical>,
        layers: &[Layer],
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let map = layer_map_for_output(self.output.as_ref()?);
        let screen = self.canvas.canvas_to_screen(pos);
        layers.iter().find_map(|&l| {
            let layer = map.layer_under(l, screen)?;
            let loc = map.layer_geometry(layer)?.loc;
            layer
                .surface_under(screen - loc.to_f64(), WindowSurfaceType::ALL)
                // Unzoomed: the surface gets screen pixels, not canvas pixels.
                .map(|(s, p)| (s, pos - screen + (p + loc).to_f64()))
        })
    }
}

/// Per-client data.
#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {}
