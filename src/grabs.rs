//! Pointer drags: moving window clusters or panning the viewport across the canvas.

use std::time::{Duration, Instant};

use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
        GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab,
        PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point},
};

use crate::{
    InfinityDesk,
    cluster::{Rect, SNAP_THRESHOLD, grid_size, snap_delta, touching},
};

/// How far the cluster moves: pointer movement, kept inside the canvas, plus snapping.
fn cluster_delta(
    data: &InfinityDesk,
    members: &[DraggedWindow],
    pointer_delta: Point<i32, Logical>,
) -> Point<i32, Logical> {
    let initial: Vec<Rect> = members.iter().map(|m| m.initial_geo).collect();
    let Some(bbox) = initial.iter().copied().reduce(|a, b| a.merge(b)) else {
        return pointer_delta;
    };

    // The whole cluster stays inside the canvas.
    let target = bbox.loc + pointer_delta;
    let clamped = data.canvas.clamp_window_loc(target, bbox.size);
    let mut delta = pointer_delta + (clamped - target);

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
pub const HOLD_TIME: Duration = Duration::from_millis(150);
/// Pointer movement (in screen pixels) that still counts as holding still.
const HOLD_TOLERANCE: f64 = 6.0;

/// A window being dragged along, with its starting position.
struct DraggedWindow {
    window: Window,
    initial_loc: Point<i32, Logical>,
    initial_geo: Rect,
}

enum DragKind {
    /// Windows are dragged across the canvas: a whole cluster, or one window pulled out of it.
    MoveWindows {
        grabbed: Window,
        members: Vec<DraggedWindow>,
        /// Press time, while it is still undecided whether the whole cluster or just
        /// the grabbed window moves.
        pressed: Option<Instant>,
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
    /// Dragging a window moves its whole cluster; holding still first pulls just this window out.
    pub fn move_window(
        state: &mut InfinityDesk,
        start_data: GrabStartData<InfinityDesk>,
        window: Window,
    ) -> Self {
        let cluster = state.clusters.members(&window);
        let pressed = (cluster.len() > 1).then(Instant::now);
        // Pulses once the window is unlocked (see `InfinityDesk::pulse`).
        state.pulse = pressed.map(|time| (window.clone(), time));
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
        Self {
            start_data,
            kind: DragKind::MoveWindows {
                grabbed: window,
                members,
                pressed,
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
                grabbed,
                members,
                pressed,
            } => {
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
                        tracing::info!("Window pulled out of its cluster");
                        members.retain(|m| &m.window == grabbed);
                        let space = &data.space;
                        data.clusters.remove(grabbed, |w| space.element_geometry(w));
                    }
                }

                let delta = cluster_delta(data, members, pointer_delta.to_i32_round());
                for m in members.iter() {
                    data.space
                        .map_element(m.window.clone(), m.initial_loc + delta, false);
                }
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
        let DragKind::MoveWindows {
            members, pressed, ..
        } = &self.kind
        else {
            return;
        };
        // Released without moving: no pulse anymore.
        if pressed.is_some() {
            data.pulse = None;
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
            // A single docked window takes its neighbors' height/width, so the cluster is a grid.
            if moved.len() == 1 && size != mr.size {
                let toplevel = mw.toplevel().unwrap();
                toplevel.with_pending_state(|s| s.size = Some(size));
                toplevel.send_pending_configure();
            }
        }
    }
}
