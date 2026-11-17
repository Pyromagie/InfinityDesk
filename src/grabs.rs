//! Pointer drags: moving window clusters or panning the viewport across the canvas.

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

use std::time::{Duration, Instant};

use crate::{
    InfinityDesk,
    cluster::{Rect, snap_delta, touching},
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
    delta += snap_delta(&moved, &others);
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

/// Shaking a window detaches it from its cluster: this many direction reversals ...
const SHAKE_REVERSALS: usize = 3;
/// ... each swinging back at least this far (in pixels) ...
const SHAKE_SWING: f64 = 40.0;
/// ... within this time.
const SHAKE_TIME: Duration = Duration::from_millis(600);

/// Detects shaking along one axis: the pointer quickly reverses direction several times.
#[derive(Default)]
struct Shake {
    /// Current direction (+1 / -1), 0 before the first swing.
    dir: f64,
    /// Furthest point reached in the current direction.
    extreme: Option<f64>,
    reversals: Vec<Instant>,
}

impl Shake {
    fn update(&mut self, pos: f64, now: Instant) -> bool {
        let extreme = *self.extreme.get_or_insert(pos);
        let moved = pos - extreme;
        if self.dir == 0.0 && moved.abs() >= SHAKE_SWING {
            self.dir = moved.signum();
            self.extreme = Some(pos);
        } else if moved * self.dir > 0.0 {
            self.extreme = Some(pos);
        } else if self.dir != 0.0 && -moved * self.dir >= SHAKE_SWING {
            self.dir = -self.dir;
            self.extreme = Some(pos);
            self.reversals.push(now);
        }
        self.reversals.retain(|t| now - *t <= SHAKE_TIME);
        self.reversals.len() >= SHAKE_REVERSALS
    }
}

/// A window being dragged along, with its starting position.
struct DraggedWindow {
    window: Window,
    initial_loc: Point<i32, Logical>,
    initial_geo: Rect,
}

enum DragKind {
    /// A cluster (or a single window) is dragged across the canvas.
    MoveCluster {
        /// The window the user grabbed.
        grabbed: Window,
        members: Vec<DraggedWindow>,
        /// Shake detection for the x and y axis.
        shake: [Shake; 2],
        /// Was the grabbed window shaken out of its cluster?
        detached: bool,
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
    /// Dragging a window moves its whole cluster.
    pub fn move_window(
        state: &InfinityDesk,
        start_data: GrabStartData<InfinityDesk>,
        window: Window,
    ) -> Self {
        let members = state
            .clusters
            .members(&window)
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
            kind: DragKind::MoveCluster {
                grabbed: window,
                members,
                shake: Default::default(),
                detached: false,
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
            DragKind::MoveCluster {
                grabbed,
                members,
                shake,
                detached,
            } => {
                let now = Instant::now();
                let shaken =
                    shake[0].update(event.location.x, now) | shake[1].update(event.location.y, now);
                if shaken && !*detached && members.len() > 1 {
                    *detached = true;
                    tracing::info!("Shake: window detached from its cluster");
                    // Put the other windows back where they were.
                    for m in members.iter().filter(|m| &m.window != grabbed) {
                        data.space
                            .map_element(m.window.clone(), m.initial_loc, false);
                    }
                    let space = &data.space;
                    data.clusters.remove(grabbed, |w| space.element_geometry(w));
                    members.retain(|m| &m.window == grabbed);
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
                let delta = screen_now - *start_screen;
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
        let DragKind::MoveCluster { members, .. } = &self.kind else {
            return;
        };
        let moved: Vec<(Window, Rect)> = members
            .iter()
            .filter_map(|m| Some((m.window.clone(), data.space.element_geometry(&m.window)?)))
            .collect();
        let others: Vec<(Window, Rect)> = others(data, members).collect();

        for (mw, mr) in &moved {
            for (ow, or) in &others {
                if touching(mr, or) {
                    tracing::info!("Docked");
                    data.clusters.join(mw, ow);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds positions to a detector, one every `step_ms`, and reports whether it fired.
    fn shake(positions: &[f64], step_ms: u64) -> bool {
        let start = Instant::now();
        let mut s = Shake::default();
        positions
            .iter()
            .enumerate()
            .any(|(i, &p)| s.update(p, start + Duration::from_millis(i as u64 * step_ms)))
    }

    #[test]
    fn quick_shake_detaches() {
        assert!(shake(&[0.0, 60.0, 0.0, 60.0, 0.0], 50));
    }

    #[test]
    fn steady_drag_does_not_detach() {
        let positions: Vec<f64> = (0..50).map(|i| i as f64 * 30.0).collect();
        assert!(!shake(&positions, 10));
    }

    #[test]
    fn small_wobble_does_not_detach() {
        assert!(!shake(&[0.0, 20.0, 0.0, 20.0, 0.0, 20.0, 0.0], 30));
    }

    #[test]
    fn slow_back_and_forth_does_not_detach() {
        assert!(!shake(&[0.0, 100.0, 0.0, 100.0, 0.0], 400));
    }
}
