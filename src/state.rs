use std::{
    collections::HashMap,
    ffi::OsString,
    sync::Arc,
    time::{Duration, Instant},
};

/// How long the "unlocked" pulse lasts.
const PULSE_TIME: Duration = Duration::from_millis(350);
/// How far the pulse grows outward (in pixels).
const PULSE_GROW: f64 = 16.0;
/// Window borders can be grabbed this far outside a window (screen pixels) ...
const BORDER_OUTSIDE: f64 = 10.0;
/// ... and this far inside it.
const BORDER_INSIDE: f64 = 5.0;

use crate::{
    canvas::{Canvas, DEFAULT_CANVAS_HEIGHT, DEFAULT_CANVAS_WIDTH},
    cluster::Clusters,
    config::Config,
    grabs::HOLD_TIME,
    render::{OUTLINE_WIDTH, Outline},
    resize::{self, Edge, Follow},
};
use smithay::{
    desktop::{PopupManager, Space, Window, WindowSurfaceType, layer_map_for_output},
    input::{Seat, SeatState, pointer::CursorIcon},
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
    pub config: Config,
    /// Window(s) held to pull them out of their cluster, with the press time. They pulse when unlocked.
    pub pulse: Option<(Vec<Window>, Instant)>,
    /// Marked windows (Alt+Shift); they are pulled out of their cluster together.
    pub selected: Vec<Window>,
    /// Windows whose size is being changed, with their target rectangle (see `place_resized`).
    pub resizing: HashMap<Window, Resizing>,
    /// The selection box being dragged open, in canvas coordinates.
    pub selection_box: Option<Rectangle<i32, Logical>>,
    /// Window edges that move when resizing at the border under the pointer (or while resizing);
    /// they light up.
    pub resize_hint: Vec<(Window, [Follow; 2])>,
    /// Pointer image shown by the host (resize arrows at window borders).
    pub cursor: CursorIcon,

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
            config: Config::load(),
            pulse: None,
            selected: Vec::new(),
            selection_box: None,
            resize_hint: Vec::new(),
            cursor: CursorIcon::Default,
            resizing: HashMap::new(),
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
        (self.space.elements().chain(&self.unplaced))
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
        // The pulse grows outward around the held window(s) and fades once they are unlocked.
        if let Some((windows, pressed)) = &self.pulse {
            let since_unlock = pressed.elapsed().checked_sub(HOLD_TIME);
            let t = since_unlock.map(|d| d.as_secs_f64() / PULSE_TIME.as_secs_f64());
            let geos = windows
                .iter()
                .filter_map(|w| self.space.element_geometry(w));
            match (t, geos.reduce(|a, b| a.merge(b))) {
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

    /// Places a window that is being resized. Clients take a moment to change their size, so the
    /// edge that does not move (e.g. the right one while dragging the left) is kept in place,
    /// whatever size the window currently has. Called while dragging and on every commit.
    pub fn place_resized(&mut self, window: &Window) {
        let Some(Resizing { rect, anchor_end }) = self.resizing.get(window) else {
            return;
        };
        let size = window.geometry().size;
        let pos =
            |start: i32, len: i32, now: i32, end: bool| if end { start + len - now } else { start };
        let x = pos(rect.loc.x, rect.size.w, size.w, anchor_end[0]);
        let y = pos(rect.loc.y, rect.size.h, size.h, anchor_end[1]);
        // Done once the client has answered the requested size (even if it kept a larger minimum).
        let acked = window
            .toplevel()
            .unwrap()
            .with_committed_state(|s| s.and_then(|s| s.size));
        if acked == Some(rect.size) || size == rect.size {
            self.resizing.remove(window);
        }
        self.space.map_element(window.clone(), (x, y), false);
    }

    /// The window whose border is under a canvas point, and which of its edges are grabbed there.
    /// The topmost window wins; a point deeper inside a window belongs to its content.
    pub fn border_under(&self, pos: Point<f64, Logical>) -> Option<(Window, [Edge; 2])> {
        let zoom = self.canvas.zoom();
        for window in self.space.elements().rev() {
            let Some(geo) = self.space.element_geometry(window) else {
                continue;
            };
            let edges =
                resize::border_edges(&geo, pos, BORDER_INSIDE / zoom, BORDER_OUTSIDE / zoom);
            if let Some(edges) = edges {
                return Some((window.clone(), edges));
            }
            if geo.to_f64().contains(pos) {
                return None;
            }
        }
        None
    }

    /// Lights up the edges that move when `window` is resized at `edges` (`None`: nothing),
    /// and shows matching resize arrows.
    pub fn set_resize_hint(&mut self, hint: Option<(Window, [Edge; 2])>, shift: bool) {
        let Some((window, edges)) = hint else {
            self.resize_hint.clear();
            self.cursor = CursorIcon::Default;
            return;
        };
        let (windows, rects): (Vec<Window>, Vec<_>) = (self.clusters.members(&window))
            .into_iter()
            .filter_map(|w| Some((w.clone(), self.space.element_geometry(&w)?)))
            .unzip();
        let dragged = windows.iter().position(|w| w == &window).unwrap_or(0);
        let follow = resize::plan(&rects, dragged, edges, shift);
        self.resize_hint = windows.into_iter().zip(follow).collect();
        self.set_cursor_for(edges);
    }

    /// Resize arrows matching the grabbed edges.
    pub fn set_cursor_for(&mut self, edges: [Edge; 2]) {
        self.cursor = match edges {
            [Edge::None, Edge::None] => CursorIcon::Default,
            [Edge::None, _] => CursorIcon::NsResize,
            [_, Edge::None] => CursorIcon::EwResize,
            [x, y] if x == y => CursorIcon::NwseResize,
            _ => CursorIcon::NeswResize,
        };
    }

    /// Screen rectangles of the edges in `resize_hint`.
    pub fn edge_highlights(&self) -> Vec<Rectangle<i32, Logical>> {
        let half = OUTLINE_WIDTH / 2;
        let mut bars = Vec::new();
        for (window, follow) in &self.resize_hint {
            let Some(geo) = self.space.element_geometry(window) else {
                continue;
            };
            let r = self.screen_rect(geo);
            let (left, top) = (r.loc.x, r.loc.y);
            let (right, bottom) = (left + r.size.w, top + r.size.h);
            let [x, y] = follow.map(|f| match f {
                Follow::Start => Some(false),
                Follow::End => Some(true),
                _ => None,
            });
            if let Some(end) = x {
                let at = if end { right } else { left };
                bars.push(Rectangle::new(
                    (at - half, top).into(),
                    (OUTLINE_WIDTH, r.size.h).into(),
                ));
            }
            if let Some(end) = y {
                let at = if end { bottom } else { top };
                bars.push(Rectangle::new(
                    (left, at - half).into(),
                    (r.size.w, OUTLINE_WIDTH).into(),
                ));
            }
        }
        bars
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

/// Target of a window being resized.
pub struct Resizing {
    pub rect: Rectangle<i32, Logical>,
    /// Per axis: the right/bottom edge stays in place (the left/top one is dragged).
    pub anchor_end: [bool; 2],
}
