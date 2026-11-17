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

use crate::{
    InfinityDesk,
    cluster::{Rect, snap_delta, touching},
    input::BTN_RIGHT,
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

/// A window pulled out of its cluster stretches until the pointer is this far away (in pixels) ...
const TEAR_DISTANCE: f64 = 120.0;
/// ... and only follows the pointer by this fraction until then, like a rubber band.
const STRETCH: f64 = 0.25;

/// A window being dragged along, with its starting position.
struct DraggedWindow {
    window: Window,
    initial_loc: Point<i32, Logical>,
    initial_geo: Rect,
}

enum DragKind {
    /// Windows are dragged across the canvas: a whole cluster, or one window pulled out of it.
    MoveWindows {
        members: Vec<DraggedWindow>,
        /// The single dragged window still sticks to its cluster and only stretches.
        sticking: bool,
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
    /// Right-drag moves the window's whole cluster; any other drag pulls the window out of it.
    pub fn move_window(
        state: &InfinityDesk,
        start_data: GrabStartData<InfinityDesk>,
        window: Window,
    ) -> Self {
        let cluster = state.clusters.members(&window);
        let whole_cluster = start_data.button == BTN_RIGHT;
        let sticking = !whole_cluster && cluster.len() > 1;
        let windows = if whole_cluster { cluster } else { vec![window] };
        let members = windows
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
            kind: DragKind::MoveWindows { members, sticking },
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
            DragKind::MoveWindows { members, sticking } => {
                if *sticking {
                    let distance = pointer_delta.x.hypot(pointer_delta.y);
                    let m = &members[0];
                    if distance < TEAR_DISTANCE {
                        let stretch = pointer_delta.upscale(STRETCH).to_i32_round();
                        data.space
                            .map_element(m.window.clone(), m.initial_loc + stretch, false);
                        return;
                    }
                    *sticking = false;
                    tracing::info!("Window torn out of its cluster");
                    let space = &data.space;
                    data.clusters
                        .remove(&m.window, |w| space.element_geometry(w));
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
        let DragKind::MoveWindows { members, sticking } = &self.kind else {
            return;
        };
        // Released before tearing it out: the window snaps back into its cluster.
        if *sticking {
            let m = &members[0];
            data.space
                .map_element(m.window.clone(), m.initial_loc, false);
            return;
        }
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
