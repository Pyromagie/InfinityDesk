//! Pointer drags: moving or resizing windows and clusters, selecting, panning the viewport.

use std::time::Instant;

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
    cluster::{Rect, snap_delta, touching},
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
    let threshold = (data.config.snap_threshold / data.canvas.zoom()).round() as i32;
    delta += snap_delta(&moved, &others, threshold, data.config.gaps);
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

/// Toggling the size of a window between half and full (see `resize::toggle`).
#[derive(Clone)]
pub struct Toggle {
    /// The window's cluster: where its windows are and how small they may get.
    pub windows: Vec<Window>,
    pub rects: Vec<Rect>,
    pub min: Vec<Size<i32, Logical>>,
    /// How each window follows (empty if the size cannot change).
    pub follow: Vec<[Follow; 2]>,
    /// The movement of the moving edges that toggles them.
    pub delta: Point<i32, Logical>,
    /// Per axis 1 if the edges move with the pointer, -1 against it (the other edge moves).
    pub sign: [i32; 2],
}

impl Toggle {
    /// The movement of the moving edges for a pointer movement.
    pub fn edges_moved(&self, pointer: Point<i32, Logical>) -> Point<i32, Logical> {
        (pointer.x * self.sign[0], pointer.y * self.sign[1]).into()
    }
}

impl InfinityDesk {
    /// Plans toggling the size of `window` at `edges` (with `shift`: the rest of the cluster
    /// beyond the edges moves instead of the neighbors changing size).
    pub fn plan_toggle(&self, window: &Window, edges: [Edge; 2], shift: bool) -> Toggle {
        let (windows, rects) = self.cluster_rects(window);
        let dragged = windows.iter().position(|w| w == window).unwrap_or(0);
        let min: Vec<_> = windows.iter().map(min_size).collect();
        let screen = self.canvas.screen_size();
        let gap = self.config.gaps;
        let (follow, delta, sign) =
            resize::toggle(&rects, &min, dragged, edges, shift, screen, gap).unwrap_or_default();
        Toggle {
            windows,
            rects,
            min,
            follow,
            delta,
            sign,
        }
    }

    /// Resizes the windows as planned in `toggle`, with the dragged edges moved by `delta`.
    /// They follow at once: a drag follows the pointer, a toggle glides in steps (`ResizeGlide`).
    pub fn apply_toggle(&mut self, toggle: &Toggle, delta: Point<i32, Logical>) {
        let new = resize::apply(&toggle.rects, &toggle.follow, &toggle.min, delta);
        for ((window, f), rect) in toggle.windows.iter().zip(&toggle.follow).zip(new) {
            if changes_size(f) {
                let anchor_end = f.map(|f| f == Follow::Start);
                self.resizing
                    .insert(window.clone(), Resizing { rect, anchor_end });
                let toplevel = window.toplevel().unwrap();
                toplevel.with_pending_state(|s| s.size = Some(rect.size));
                toplevel.send_pending_configure();
                self.place_resized(window, false);
            } else if f.contains(&Follow::Shift) {
                self.space.map_element(window.clone(), rect.loc, false);
            }
        }
    }

    /// Lets the dragged edges of `toggle` glide from `from` to `to` (or jump, without
    /// animations): the windows get a new size every frame, like while dragging.
    pub fn glide_resize(
        &mut self,
        toggle: Toggle,
        from: Point<i32, Logical>,
        to: Point<i32, Logical>,
    ) {
        self.finish_resize_glide();
        if from == to || self.config.animation.is_zero() {
            self.apply_toggle(&toggle, to);
            return;
        }
        let start = Instant::now();
        self.resize_glide = Some(ResizeGlide {
            toggle,
            from,
            to,
            start,
        });
    }

    /// A gliding resize ends at once: before something else moves or resizes windows, which the
    /// next frame of the glide would undo.
    pub fn finish_resize_glide(&mut self) {
        if let Some(glide) = self.resize_glide.take() {
            self.apply_toggle(&glide.toggle, glide.to);
        }
    }

    /// One frame of a gliding resize, slowing down towards the end like the other animations.
    pub fn step_resize_glide(&mut self) {
        let Some(glide) = self.resize_glide.take() else {
            return;
        };
        let t = glide.start.elapsed().as_secs_f64() / self.config.animation.as_secs_f64();
        let eased = 1.0 - (1.0 - t.min(1.0)).powi(3);
        let way = (glide.to - glide.from).to_f64().upscale(eased);
        self.apply_toggle(&glide.toggle, glide.from + way.to_i32_round());
        if t < 1.0 {
            self.resize_glide = Some(glide);
        }
    }
}

/// A window's size gliding to half or full, its neighbors with it (see `glide_resize`).
pub struct ResizeGlide {
    pub toggle: Toggle,
    from: Point<i32, Logical>,
    to: Point<i32, Logical>,
    start: Instant,
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
    /// A window's size changes between half and full; its cluster neighbors follow. The dragged
    /// edges follow the pointer (by the second field) and glide to a size when let go.
    Resize(Toggle, Point<i32, Logical>),
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
        state.finish_resize_glide();
        let cluster = state.clusters.members(&window);
        // Moving ends any pending resize placement or glide of these windows.
        state.resizing.retain(|w, _| !cluster.contains(w));
        state.window_glides.retain(|w, _| !cluster.contains(w));
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

    /// Resizes `window` at `edges`, following the pointer between half and full; with `shift`,
    /// the cluster beyond the edge moves instead of the neighbors changing size.
    pub fn resize(
        state: &mut InfinityDesk,
        start_data: GrabStartData<InfinityDesk>,
        window: Window,
        edges: [Edge; 2],
        shift: bool,
    ) -> Self {
        state.finish_resize_glide();
        let toggle = state.plan_toggle(&window, edges, shift);
        set_resizing(&toggle.windows, &toggle.follow, true);
        state.resize_hint = (toggle.windows.iter().cloned())
            .zip(toggle.follow.iter().copied())
            .collect();
        state.set_cursor_for(edges);
        Self {
            start_data,
            kind: DragKind::Resize(toggle, Point::default()),
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
                    if time.elapsed() < data.config.hold_time {
                        data.pulse = None;
                    } else {
                        tracing::info!("{} window(s) pulled out of their cluster", group.len());
                        members.retain(|m| group.contains(&m.window));
                        data.clusters.detach(group);
                        for w in group.iter() {
                            data.emit("cluster.undock", &[w], None);
                        }
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
            DragKind::Resize(toggle, followed) => {
                let moved = toggle.edges_moved(pointer_delta.to_i32_round());
                *followed = resize::following(toggle.delta, moved);
                data.apply_toggle(toggle, *followed);
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
        if let DragKind::Resize(toggle, followed) = &self.kind {
            set_resizing(&toggle.windows, &toggle.follow, false);
            data.set_resize_hint(None, false);
            // On to the closer size: the other one once past half of the way.
            let to = resize::toggled(toggle.delta, *followed);
            data.glide_resize(toggle.clone(), *followed, to);
        }
        if let DragKind::Pan { .. } = self.kind {
            data.snap_view();
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
                data.glide_window(&m.window, m.initial_loc + delta);
            }
        }
        let moved: Vec<(Window, Rect)> = members
            .iter()
            .filter_map(|m| Some((m.window.clone(), data.space.element_geometry(&m.window)?)))
            .collect();
        let others: Vec<(Window, Rect)> = others(data, members).collect();

        for (mw, mr) in &moved {
            for (ow, or) in &others {
                if touching(mr, or, data.config.gaps) {
                    tracing::info!("Docked");
                    data.clusters.join(mw, ow);
                    data.emit("cluster.dock", &[mw, ow], None);
                }
            }
        }
    }
}
