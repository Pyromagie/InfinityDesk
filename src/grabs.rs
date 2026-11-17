//! Pointer drags: moving or resizing windows and clusters, selecting, panning the viewport.

use std::time::{Duration, Instant};

use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
        GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab,
        PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Rectangle, Size},
    wayland::{compositor::with_states, shell::xdg::SurfaceCachedState},
};

use crate::{
    InfinityDesk,
    cluster::{Rect, SNAP_THRESHOLD, grid_size, snap_delta, touching},
    resize::{self, Edge, Follow},
    state::Resizing,
};

/// How far the cluster moves: pointer movement, kept inside the canvas, plus snapping (if `snap`).
fn cluster_delta(
    data: &InfinityDesk,
    members: &[DraggedWindow],
    pointer_delta: Point<i32, Logical>,
    snap: bool,
) -> Point<i32, Logical> {
    let initial: Vec<Rect> = members.iter().map(|m| m.initial_geo).collect();
    let Some(bbox) = initial.iter().copied().reduce(|a, b| a.merge(b)) else {
        return pointer_delta;
    };

    // The whole cluster stays inside the canvas.
    let target = bbox.loc + pointer_delta;
    let clamped = data.canvas.clamp_window_loc(target, bbox.size);
    let mut delta = pointer_delta + (clamped - target);
    if !snap {
        return delta;
    }

    // Snap to windows outside the cluster.
    let moved: Vec<Rect> = initial
        .iter()
        .map(|r| Rect::new(r.loc + delta, r.size))
        .collect();
    let others: Vec<Rect> = others(data, members).map(|(_, r)| r).collect();
    let threshold = (SNAP_THRESHOLD / data.canvas.zoom()).round() as i32;
    delta += snap_delta(&moved, &others, threshold);
    delta
}

/// All windows outside the dragged cluster, with their geometry.
fn others<'a>(
    data: &'a InfinityDesk,
    members: &'a [DraggedWindow],
) -> impl Iterator<Item = (Window, Rect)> + 'a {
    data.space
        .elements()
        .filter(|w| !members.iter().any(|m| &m.window == *w))
        .filter_map(|w| Some((w.clone(), data.space.element_geometry(w)?)))
}

/// Holding still this long before dragging pulls the window out of its cluster.
pub const HOLD_TIME: Duration = Duration::from_millis(300);
/// Windows dragged faster than this (screen pixels per second) pass other windows without snapping.
const SNAP_MAX_SPEED: f64 = 1000.0;
/// Pointer movement (in screen pixels) that still counts as holding still.
const HOLD_TOLERANCE: f64 = 6.0;

/// A window being dragged along, with its starting position.
struct DraggedWindow {
    window: Window,
    initial_loc: Point<i32, Logical>,
    initial_geo: Rect,
}

/// Windows are never resized below this (or below what the client asks for).
const MIN_WINDOW_SIZE: i32 = 50;

/// Does a window change its size (and not just move) during a resize?
fn changes_size(follow: &[Follow; 2]) -> bool {
    follow
        .iter()
        .any(|f| matches!(f, Follow::Start | Follow::End))
}

/// Tells clients they are being resized interactively (some draw differently meanwhile).
fn set_resizing(windows: &[Window], follow: &[[Follow; 2]], on: bool) {
    for (window, f) in windows.iter().zip(follow) {
        if changes_size(f) {
            let toplevel = window.toplevel().unwrap();
            toplevel.with_pending_state(|s| match on {
                true => s.states.set(xdg_toplevel::State::Resizing),
                false => s.states.unset(xdg_toplevel::State::Resizing),
            });
            toplevel.send_pending_configure();
        }
    }
}

/// The smallest size a window may get.
fn min_size(window: &Window) -> Size<i32, Logical> {
    let surface = window.toplevel().unwrap().wl_surface();
    let min = with_states(surface, |states| {
        states
            .cached_state
            .get::<SurfaceCachedState>()
            .current()
            .min_size
    });
    (min.w.max(MIN_WINDOW_SIZE), min.h.max(MIN_WINDOW_SIZE)).into()
}

enum DragKind {
    /// Windows are dragged across the canvas: a whole cluster, or one window pulled out of it.
    MoveWindows {
        /// What comes out of the cluster when held: the grabbed window, or all marked windows
        /// of the cluster if the grabbed one is marked.
        group: Vec<Window>,
        members: Vec<DraggedWindow>,
        /// Press time, while it is still undecided whether the whole cluster or just
        /// the group moves.
        pressed: Option<Instant>,
        /// Last pointer position and time, and the pointer speed (screen pixels per second).
        last: (Point<f64, Logical>, Instant),
        speed: f64,
    },
    /// A window is resized; its cluster neighbors follow (see `resize`).
    Resize {
        windows: Vec<Window>,
        initial: Vec<Rect>,
        follow: Vec<[Follow; 2]>,
        min: Vec<Size<i32, Logical>>,
    },
    /// A selection box is dragged open (Alt+Shift on empty space); windows inside get marked.
    Select {
        /// Windows marked before, they stay marked.
        initial: Vec<Window>,
    },
    /// The canvas is moved underneath the screen.
    Pan {
        /// Pointer position on screen at the start.
        start_screen: Point<f64, Logical>,
        initial_viewport: Point<f64, Logical>,
    },
}

pub struct DragGrab {
    start_data: GrabStartData<InfinityDesk>,
    kind: DragKind,
}

impl DragGrab {
    /// Dragging a window moves its whole cluster; holding still first pulls its group out.
    pub fn move_window(
        state: &mut InfinityDesk,
        start_data: GrabStartData<InfinityDesk>,
        window: Window,
    ) -> Self {
        let cluster = state.clusters.members(&window);
        // Moving ends any pending resize placement of these windows.
        state.resizing.retain(|w, _| !cluster.contains(w));
        let pressed = (cluster.len() > 1).then(Instant::now);
        let group: Vec<Window> = if state.selected.contains(&window) {
            let marked = cluster.iter().filter(|w| state.selected.contains(w));
            marked.cloned().collect()
        } else {
            vec![window]
        };
        // Pulses around the group once it is unlocked (see `InfinityDesk::outlines`).
        state.pulse = pressed.map(|time| (group.clone(), time));
        let members = cluster
            .into_iter()
            .filter_map(|w| {
                Some(DraggedWindow {
                    initial_loc: state.space.element_location(&w)?,
                    initial_geo: state.space.element_geometry(&w)?,
                    window: w,
                })
            })
            .collect();
        let last = (start_data.location, Instant::now());
        Self {
            start_data,
            kind: DragKind::MoveWindows {
                last,
                group,
                members,
                pressed,
                speed: 0.0,
            },
        }
    }

    /// Resizes `window` at `edges`; with `shift`, the cluster beyond the edge moves instead of
    /// the neighbors shrinking.
    pub fn resize(
        state: &mut InfinityDesk,
        start_data: GrabStartData<InfinityDesk>,
        window: Window,
        edges: [Edge; 2],
        shift: bool,
    ) -> Self {
        let (windows, initial): (Vec<Window>, Vec<Rect>) = (state.clusters.members(&window))
            .into_iter()
            .filter_map(|w| Some((w.clone(), state.space.element_geometry(&w)?)))
            .unzip();
        let dragged = windows.iter().position(|w| w == &window).unwrap_or(0);
        let follow = resize::plan(&initial, dragged, edges, shift);
        set_resizing(&windows, &follow, true);
        let min = windows.iter().map(min_size).collect();
        Self {
            start_data,
            kind: DragKind::Resize {
                windows,
                initial,
                follow,
                min,
            },
        }
    }

    pub fn select(state: &InfinityDesk, start_data: GrabStartData<InfinityDesk>) -> Self {
        Self {
            start_data,
            kind: DragKind::Select {
                initial: state.selected.clone(),
            },
        }
    }

    pub fn pan(state: &InfinityDesk, start_data: GrabStartData<InfinityDesk>) -> Self {
        let start_screen = state.canvas.canvas_to_screen(start_data.location);
        Self {
            start_data,
            kind: DragKind::Pan {
                start_screen,
                initial_viewport: state.canvas.viewport_loc(),
            },
        }
    }
}

impl PointerGrab<InfinityDesk> for DragGrab {
    fn motion(
        &mut self,
        data: &mut InfinityDesk,
        handle: &mut PointerInnerHandle<'_, InfinityDesk>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        // No client receives pointer events while dragging.
        handle.motion(data, None, event);

        let pointer_delta = event.location - self.start_data.location;
        match &mut self.kind {
            DragKind::MoveWindows {
                group,
                members,
                pressed,
                last,
                speed,
            } => {
                // Smoothed pointer speed: windows passing by quickly do not snap.
                let (last_pos, last_time) = *last;
                let distance = (event.location - last_pos).upscale(data.canvas.zoom());
                let dt = last_time.elapsed().as_secs_f64().max(0.001);
                *speed = (*speed + distance.x.hypot(distance.y) / dt) / 2.0;
                *last = (event.location, Instant::now());

                // Decide on the first real movement: held still long enough -> just this window.
                if let Some(time) = *pressed {
                    let moved = pointer_delta.x.hypot(pointer_delta.y) * data.canvas.zoom();
                    if moved < HOLD_TOLERANCE {
                        return;
                    }
                    *pressed = None;
                    if time.elapsed() < HOLD_TIME {
                        data.pulse = None;
                    } else {
                        tracing::info!("{} window(s) pulled out of their cluster", group.len());
                        members.retain(|m| group.contains(&m.window));
                        let space = &data.space;
                        data.clusters.detach(group, |w| space.element_geometry(w));
                        data.selected.clear();
                    }
                }

                let snap = *speed < SNAP_MAX_SPEED;
                let delta = cluster_delta(data, members, pointer_delta.to_i32_round(), snap);
                for m in members.iter() {
                    data.space
                        .map_element(m.window.clone(), m.initial_loc + delta, false);
                }
            }
            DragKind::Resize {
                windows,
                initial,
                follow,
                min,
            } => {
                let new = resize::apply(initial, follow, min, pointer_delta.to_i32_round());
                for ((window, f), rect) in windows.iter().zip(follow.iter()).zip(new) {
                    if changes_size(f) {
                        let anchor_end = f.map(|f| f == Follow::Start);
                        data.resizing
                            .insert(window.clone(), Resizing { rect, anchor_end });
                        let toplevel = window.toplevel().unwrap();
                        toplevel.with_pending_state(|s| s.size = Some(rect.size));
                        toplevel.send_pending_configure();
                        data.place_resized(window);
                    } else if f.contains(&Follow::Shift) {
                        data.space.map_element(window.clone(), rect.loc, false);
                    }
                }
            }
            DragKind::Select { initial } => {
                let corners = [self.start_data.location, event.location];
                let selection_box = Rectangle::bounding_box(corners).to_i32_round();
                let inside = data.space.elements().filter(|w| {
                    let geo = data.space.element_geometry(w);
                    geo.is_some_and(|g| g.overlaps(selection_box)) && !initial.contains(w)
                });
                let mut selected = initial.clone();
                selected.extend(inside.cloned());
                data.selected = selected;
                data.selection_box = Some(selection_box);
            }
            DragKind::Pan {
                start_screen,
                initial_viewport,
            } => {
                // Work in screen coordinates, because the canvas moves under the pointer.
                let screen_now = data.canvas.canvas_to_screen(event.location);
                let delta = (screen_now - *start_screen).downscale(data.canvas.zoom());
                data.canvas.set_viewport_loc(*initial_viewport - delta);
                data.apply_viewport();
            }
        }
    }

    fn relative_motion(
        &mut self,
        data: &mut InfinityDesk,
        handle: &mut PointerInnerHandle<'_, InfinityDesk>,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut InfinityDesk,
        handle: &mut PointerInnerHandle<'_, InfinityDesk>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if !handle.current_pressed().contains(&self.start_data.button) {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(
        &mut self,
        data: &mut InfinityDesk,
        handle: &mut PointerInnerHandle<'_, InfinityDesk>,
        details: AxisFrame,
    ) {
        handle.axis(data, details)
    }

    fn frame(
        &mut self,
        data: &mut InfinityDesk,
        handle: &mut PointerInnerHandle<'_, InfinityDesk>,
    ) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut InfinityDesk,
        handle: &mut PointerInnerHandle<'_, InfinityDesk>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event)
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut InfinityDesk,
        handle: &mut PointerInnerHandle<'_, InfinityDesk>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event)
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut InfinityDesk,
        handle: &mut PointerInnerHandle<'_, InfinityDesk>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event)
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut InfinityDesk,
        handle: &mut PointerInnerHandle<'_, InfinityDesk>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event)
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut InfinityDesk,
        handle: &mut PointerInnerHandle<'_, InfinityDesk>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event)
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut InfinityDesk,
        handle: &mut PointerInnerHandle<'_, InfinityDesk>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event)
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut InfinityDesk,
        handle: &mut PointerInnerHandle<'_, InfinityDesk>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event)
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut InfinityDesk,
        handle: &mut PointerInnerHandle<'_, InfinityDesk>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event)
    }

    fn start_data(&self) -> &GrabStartData<InfinityDesk> {
        &self.start_data
    }

    /// Button released: if the cluster now touches other windows, they get docked.
    fn unset(&mut self, data: &mut InfinityDesk) {
        data.selection_box = None;
        if let DragKind::Resize {
            windows, follow, ..
        } = &self.kind
        {
            set_resizing(windows, follow, false);
        }
        let DragKind::MoveWindows {
            members, pressed, ..
        } = &self.kind
        else {
            return;
        };
        // Released without moving: no pulse anymore.
        if pressed.is_some() {
            data.pulse = None;
        } else if let Some(m) = members.first() {
            // Released while moving fast: snap now.
            let current = data
                .space
                .element_location(&m.window)
                .unwrap_or(m.initial_loc);
            let delta = cluster_delta(data, members, current - m.initial_loc, true);
            for m in members {
                data.space
                    .map_element(m.window.clone(), m.initial_loc + delta, false);
            }
        }
        let moved: Vec<(Window, Rect)> = members
            .iter()
            .filter_map(|m| Some((m.window.clone(), data.space.element_geometry(&m.window)?)))
            .collect();
        let others: Vec<(Window, Rect)> = others(data, members).collect();

        for (mw, mr) in &moved {
            let mut size = mr.size;
            for (ow, or) in &others {
                if touching(mr, or) {
                    tracing::info!("Docked");
                    data.clusters.join(mw, ow);
                    size = grid_size(&Rect::new(mr.loc, size), or);
                }
            }
            // Setting: a single docked window takes its neighbors' height/width (clean grid).
            if data.config.match_size && moved.len() == 1 && size != mr.size {
                let toplevel = mw.toplevel().unwrap();
                toplevel.with_pending_state(|s| s.size = Some(size));
                toplevel.send_pending_configure();
            }
        }
    }
}
