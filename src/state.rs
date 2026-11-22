use std::{
    collections::HashMap,
    ffi::OsString,
    sync::Arc,
    time::{Duration, Instant},
};

/// How often the config file is checked for changes.
const CONFIG_CHECK: Duration = Duration::from_secs(1);
/// How long the "unlocked" pulse lasts.
const PULSE_TIME: Duration = Duration::from_millis(350);
/// How far the pulse grows outward (in pixels).
const PULSE_GROW: f64 = 16.0;
/// Window borders can be grabbed this far outside a window (screen pixels) ...
const BORDER_OUTSIDE: f64 = 10.0;
/// ... and this far inside it.
const BORDER_INSIDE: f64 = 5.0;

use crate::{
    arrange::{FreeSlots, Fullscreen, LayoutMenu},
    canvas::Canvas,
    cluster::{Clusters, Rect},
    config::Config,
    navigate::Overview,
    render::{OUTLINE_WIDTH, Outline, outline_bars},
    resize::{self, Edge, Follow},
    xwayland::XDisplay,
};
use smithay::{
    backend::input::InputTime,
    desktop::{PopupManager, Space, Window, WindowSurfaceType, layer_map_for_output},
    input::{
        Seat, SeatState,
        pointer::{CursorIcon, CursorImageStatus},
    },
    output::Output,
    reexports::{
        calloop::{
            EventLoop, Interest, LoopSignal, Mode, PostAction,
            generic::Generic,
            timer::{TimeoutAction, Timer},
        },
        wayland_server::{
            Display, DisplayHandle, backend::ClientData, protocol::wl_surface::WlSurface,
        },
    },
    utils::{Logical, Point, Rectangle, SERIAL_COUNTER},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        dmabuf::DmabufState,
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        shell::{
            wlr_layer::{Layer, WlrLayerShellState},
            xdg::XdgShellState,
        },
        shm::ShmState,
        socket::ListeningSocketSource,
        viewporter::ViewporterState,
    },
};

/// The complete compositor state.
pub struct InfinityDesk {
    pub start_time: std::time::Instant,
    pub socket_name: OsString,
    /// Where X11 programs show up (`None`: nowhere, xwayland-satellite is missing).
    pub xdisplay: Option<XDisplay>,
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
    /// Pointer image InfinityDesk shows itself (resize arrows at window borders); `Default`
    /// leaves it to the app under the pointer.
    pub cursor: CursorIcon,
    /// Where the pointer is on the screen. Kept apart from its canvas position, which changes
    /// whenever the view moves underneath it.
    pub pointer_screen: Point<f64, Logical>,
    /// The pointer image the app under the pointer asks for.
    pub client_cursor: CursorImageStatus,
    /// The screen, GPU and input devices when running standalone (`None`: nested).
    pub udev: Option<crate::udev::Udev>,
    /// The fullscreen window (Mod+F).
    pub fullscreen: Option<Fullscreen>,
    /// The open template menu (Mod+R).
    pub layout_menu: Option<LayoutMenu>,
    /// The open overview (Mod+Tab).
    pub overview: Option<Overview>,
    /// Slots of the last applied template that new windows fill.
    pub free_slots: Option<FreeSlots>,
    /// The window being worked in (it keeps this while a launcher has the keyboard). New windows
    /// dock next to it; clicking empty space clears it.
    pub active: Option<Window>,
    /// Windows in the order they were last active, the most recent first (Alt+Tab).
    pub recent: Vec<Window>,
    /// While Alt+Tab is held: the position in `recent` of the window shown now.
    pub switching: Option<usize>,
    /// The touchpad gesture in progress.
    pub gesture: Option<crate::input::Gesture>,

    // Smithay protocol state
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub layer_shell_state: WlrLayerShellState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<InfinityDesk>,
    pub data_device_state: DataDeviceState,
    /// Lets clients scale their buffers (wallpaper tools like awww need it, video players use it).
    pub viewporter_state: ViewporterState,
    /// Apps hand over GPU buffers directly (standalone only; offered by `udev`).
    pub dmabuf_state: DmabufState,
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
        let viewporter_state = ViewporterState::new::<Self>(&dh);

        let config = Config::load();
        let mut seat_state = SeatState::new();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "seat0");
        let k = &config.keyboard;
        let xkb = k.xkb.with_system_default();
        tracing::info!("Keyboard layout: {:?} {:?}", xkb.layout, xkb.variant);
        if let Err(err) = seat.add_keyboard(xkb.to_smithay(), k.repeat_delay, k.repeat_rate) {
            // E.g. a layout name that does not exist.
            tracing::error!("Keyboard layout from the config failed ({err:?}), using the default");
            (seat.add_keyboard(Default::default(), k.repeat_delay, k.repeat_rate))
                .expect("the default keyboard layout must work");
        }
        seat.add_pointer();

        let socket_name = Self::init_wayland_listener(display, event_loop);

        // Changes to the config file are picked up while running.
        let mut stamp = Config::stamp();
        let timer = Timer::from_duration(CONFIG_CHECK);
        (event_loop.handle())
            .insert_source(timer, move |_, _, state| {
                let now = Config::stamp();
                if now != stamp {
                    stamp = now;
                    state.reload_config();
                }
                TimeoutAction::ToDuration(CONFIG_CHECK)
            })
            .expect("failed to set up the config check");

        Self {
            start_time: std::time::Instant::now(),
            socket_name,
            xdisplay: None,
            display_handle: dh,
            loop_signal: event_loop.get_signal(),
            canvas: Canvas::new(config.canvas_size.into()),
            space: Space::default(),
            popups: PopupManager::default(),
            output: None,
            unplaced: Vec::new(),
            clusters: Clusters::default(),
            config,
            pulse: None,
            selected: Vec::new(),
            selection_box: None,
            resize_hint: Vec::new(),
            cursor: CursorIcon::Default,
            pointer_screen: (0.0, 0.0).into(),
            client_cursor: CursorImageStatus::default_named(),
            udev: None,
            fullscreen: None,
            layout_menu: None,
            overview: None,
            free_slots: None,
            active: None,
            recent: Vec::new(),
            switching: None,
            gesture: None,
            resizing: HashMap::new(),
            compositor_state,
            xdg_shell_state,
            layer_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            viewporter_state,
            dmabuf_state: DmabufState::new(),
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

    /// The config file changed: its settings take effect right away. With an error in it, the
    /// previous settings stay.
    pub fn reload_config(&mut self) {
        let Some(path) = Config::path() else {
            return;
        };
        match Config::read(&path) {
            Ok(config) => self.apply_config(config),
            Err(err) => tracing::error!("Error in the config, keeping the settings:\n{err}"),
        }
    }

    fn apply_config(&mut self, mut config: Config) {
        let keyboard = self.seat.get_keyboard().unwrap();
        let k = &mut config.keyboard;
        if k.xkb != self.config.keyboard.xkb
            && let Err(err) =
                keyboard.set_xkb_config(self, k.xkb.with_system_default().to_smithay())
        {
            tracing::error!(
                "Keyboard layout from the config failed ({err:?}), keeping the old one"
            );
            k.xkb = self.config.keyboard.xkb.clone();
        }
        keyboard.change_repeat_info(k.repeat_rate, k.repeat_delay);
        if config.canvas_size != self.config.canvas_size {
            self.canvas.set_size(config.canvas_size.into());
            self.fit_clusters_into_canvas();
            self.apply_viewport();
            self.fit_fullscreen();
        }
        if let Some(udev) = &mut self.udev
            && (config.touchpad != self.config.touchpad || config.mouse != self.config.mouse)
        {
            udev.set_up_devices(&config);
        }
        if let Some(udev) = &mut self.udev
            && (config.cursor_theme != self.config.cursor_theme
                || config.cursor_size != self.config.cursor_size)
        {
            udev.set_cursor_theme(&config);
        }
        self.config = config;
    }

    /// Moves clusters that lie (partly) outside the canvas back in, each as a whole.
    fn fit_clusters_into_canvas(&mut self) {
        for (geos, bbox) in self.all_clusters() {
            let shift = self.canvas.clamp_window_loc(bbox.loc, bbox.size) - bbox.loc;
            self.shift_windows(geos, shift);
        }
    }

    /// Every cluster: its windows with their places, and its bounding box.
    pub fn all_clusters(&self) -> Vec<(Vec<(Window, Rect)>, Rect)> {
        let mut done: Vec<Window> = Vec::new();
        let mut clusters = Vec::new();
        for window in self.space.elements() {
            if done.contains(window) {
                continue;
            }
            let members = self.clusters.members(window);
            let geos: Vec<(Window, Rect)> = (members.iter())
                .filter_map(|w| Some((w.clone(), self.space.element_geometry(w)?)))
                .collect();
            if let Some(bbox) = geos.iter().map(|(_, g)| *g).reduce(|a, b| a.merge(b)) {
                clusters.push((geos, bbox));
            }
            done.extend(members);
        }
        clusters
    }

    /// Moves windows from the places given by `shift` (also where they are about to be resized).
    pub fn shift_windows(&mut self, geos: Vec<(Window, Rect)>, shift: Point<i32, Logical>) {
        if shift == Point::default() {
            return;
        }
        for (w, geo) in geos {
            if let Some(r) = self.resizing.get_mut(&w) {
                r.rect.loc += shift;
            }
            self.space.map_element(w, geo.loc + shift, false);
        }
    }

    /// The screen is ready: the viewport starts in the middle of the canvas, leaving room in
    /// every direction.
    pub fn set_output(&mut self, output: Output) {
        let size = output.current_mode().unwrap().size.to_logical(1);
        self.output = Some(output);
        self.canvas.set_screen_size(size);
        let canvas = self.canvas.size().to_f64();
        (self.canvas).center_viewport_on((canvas.w / 2.0, canvas.h / 2.0).into());
        self.apply_viewport();
    }

    /// After a frame: tells apps they may draw their next one, and tidies up.
    pub fn send_frames(&mut self, output: &Output) {
        let time = self.start_time.elapsed();
        for window in self.space.elements() {
            window.send_frame(output, time, Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
        }
        for layer in layer_map_for_output(output).layers() {
            layer.send_frame(output, time, Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
        }
        self.space.refresh();
        self.popups.cleanup();
        let _ = self.display_handle.flush_clients();
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
        // Placed with the size it was asked for: some apps draw their own size first (alacritty).
        let asked = (window.toplevel().unwrap()).with_committed_state(|s| s.and_then(|s| s.size));
        let size = asked.unwrap_or(size);
        // A free template slot was reserved for it when it was configured the first time,
        // otherwise it docks next to the active window, or (with no room there) stands alone:
        // dialogs and windows that should not dock (window rules) in the middle of the screen,
        // other windows in its left half.
        let rules = self.rules(window.toplevel().unwrap());
        let slot = self.resizing.get(&window).map(|r| r.rect);
        let spot = (rules.dock).then(|| self.spot_next_to_active(&window, size));
        match slot.or(spot.flatten()) {
            Some(rect) => {
                self.space.map_element(window.clone(), rect.loc, false);
                if rect.size != size {
                    self.resize_to(&window, rect, None);
                }
                self.dock_into_slot(&window, rect);
                self.place_resized(&window);
                // Docked off the screen: the view follows. The pointer stays put on the screen.
                let screen = self.pointer_screen;
                self.canvas.reveal(rect, self.config.gaps);
                self.apply_viewport();
                self.pointer_moved(self.canvas.screen_to_canvas(screen), InputTime::now());
            }
            None => {
                let loc = match window.toplevel().unwrap().parent() {
                    None if rules.dock => self.free_slot().loc,
                    _ => self.canvas.centered_in_viewport(size),
                };
                self.space.map_element(window.clone(), loc, false);
            }
        }
        self.focus_window(&window);
        // Leaving fullscreen, it is back at the place it got above.
        if rules.open_fullscreen {
            self.start_fullscreen(&window);
        }
    }

    /// Raises the window with its cluster and gives it keyboard focus.
    pub fn focus_window(&mut self, window: &Window) {
        self.active = Some(window.clone());
        // While switching with Alt+Tab, the order is kept until the choice is made.
        if self.switching.is_none() {
            self.recent.retain(|w| w != window);
            self.recent.insert(0, window.clone());
        }
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
    fn outlines(&mut self) -> Vec<Outline> {
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
            let since_unlock = pressed.elapsed().checked_sub(self.config.hold_time);
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
        // New windows are placed by `place_if_pending` once they have content.
        if self.unplaced.contains(window) {
            return;
        }
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
        let toggle = self.plan_toggle(&window, edges, shift);
        self.resize_hint = toggle.windows.into_iter().zip(toggle.follow).collect();
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
    fn edge_highlights(&self) -> Vec<Rectangle<i32, Logical>> {
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
        self.layer_surface_under(pos, self.upper_layers())
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

    /// Layers drawn above the windows: bars (Top) are covered by a fullscreen window.
    pub fn upper_layers(&self) -> &'static [Layer] {
        match self.fullscreen {
            Some(_) => &[Layer::Overlay],
            None => &[Layer::Overlay, Layer::Top],
        }
    }

    /// Everything drawn as solid rectangles on top (screen coordinates, front to back): the
    /// template menu, blue frames and lit edges. A fullscreen window hides the frames and edges.
    pub fn bars(&mut self) -> Vec<(Rectangle<i32, Logical>, [f32; 4])> {
        let mut bars = self.menu_bars();
        if self.fullscreen.is_none() {
            let accent = self.config.accent;
            bars.extend(self.outlines().iter().flat_map(|o| outline_bars(o, accent)));
            bars.extend(self.edge_highlights().into_iter().map(|r| (r, accent)));
        }
        bars
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
