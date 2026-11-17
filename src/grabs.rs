//! Maus-Ziehen: Fenster verschieben oder den Viewport über den Canvas bewegen (Pan).

use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
        GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point},
};

use std::time::{Duration, Instant};

use crate::{
    InfinityDesk,
    cluster::{Rect, SNAP_THRESHOLD, bounding_box, snap_delta, touching},
};

/// Wie weit das Cluster verschoben wird: Mausbewegung, im Canvas gehalten, plus Einrasten.
fn cluster_delta(data: &InfinityDesk, members: &[DraggedWindow], pointer_delta: Point<i32, Logical>) -> Point<i32, Logical> {
    let initial: Vec<Rect> = members.iter().map(|m| m.initial_geo).collect();
    let Some(bbox) = bounding_box(&initial) else {
        return pointer_delta;
    };

    // Das ganze Cluster bleibt im Canvas.
    let target = bbox.loc + pointer_delta;
    let clamped = data.canvas.clamp_window_loc(target, bbox.size);
    let mut delta = pointer_delta + (clamped - target);

    // An Fenstern außerhalb des Clusters einrasten.
    let moved: Vec<Rect> = initial
        .iter()
        .map(|r| Rect::new(r.loc + delta, r.size))
        .collect();
    let others: Vec<Rect> = data
        .space
        .elements()
        .filter(|w| !members.iter().any(|m| &m.window == *w))
        .filter_map(|w| data.space.element_geometry(w))
        .collect();
    delta += snap_delta(&moved, &others, SNAP_THRESHOLD);
    delta
}

/// Ein "Ruck" löst ein Fenster aus seinem Cluster: so weit (in Pixeln) ...
const RUCK_DISTANCE: f64 = 120.0;
/// ... innerhalb dieser Zeit nach Beginn des Ziehens.
const RUCK_TIME: Duration = Duration::from_millis(150);

/// Ein Fenster, das gerade mitgezogen wird, mit seiner Ausgangslage.
pub struct DraggedWindow {
    window: Window,
    initial_loc: Point<i32, Logical>,
    initial_geo: Rect,
}

pub enum DragKind {
    /// Ein Cluster (oder ein einzelnes Fenster) wird über den Canvas gezogen.
    MoveCluster {
        /// Das Fenster, an dem gezogen wird.
        grabbed: Window,
        members: Vec<DraggedWindow>,
        started: Instant,
        /// Wurde das gezogene Fenster per Ruck aus seinem Cluster gelöst?
        detached: bool,
    },
    /// Die Fläche wird unter dem Bildschirm verschoben.
    Pan {
        /// Zeigerposition auf dem Bildschirm beim Start.
        start_screen: Point<f64, Logical>,
        initial_viewport: Point<f64, Logical>,
    },
}

pub struct DragGrab {
    pub start_data: GrabStartData<InfinityDesk>,
    pub kind: DragKind,
}

impl DragGrab {
    /// Ziehen an einem Fenster: bewegt sein ganzes Cluster.
    pub fn move_window(state: &InfinityDesk, start_data: GrabStartData<InfinityDesk>, window: Window) -> Self {
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
                started: Instant::now(),
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
        // Während des Ziehens bekommt kein Programm die Maus.
        handle.motion(data, None, event);

        let pointer_delta = event.location - self.start_data.location;
        match &mut self.kind {
            DragKind::MoveCluster {
                grabbed,
                members,
                started,
                detached,
            } => {
                // Ruck erkennen: schnell weit gezogen, direkt nach dem Start.
                let distance = pointer_delta.x.hypot(pointer_delta.y);
                if !*detached && members.len() > 1 && started.elapsed() < RUCK_TIME && distance >= RUCK_DISTANCE {
                    *detached = true;
                    tracing::info!("Ruck: Fenster aus dem Cluster gelöst");
                    // Die anderen Fenster zurück an ihren Platz.
                    for m in members.iter().filter(|m| &m.window != grabbed) {
                        data.space.map_element(m.window.clone(), m.initial_loc, false);
                    }
                    let space = &data.space;
                    data.clusters.remove(grabbed, |w| space.element_geometry(w));
                    members.retain(|m| &m.window == grabbed);
                }

                let delta = cluster_delta(data, members, pointer_delta.to_i32_round());
                for m in members.iter() {
                    data.space.map_element(m.window.clone(), m.initial_loc + delta, false);
                }
            }
            DragKind::Pan {
                start_screen,
                initial_viewport,
            } => {
                // In Bildschirmkoordinaten rechnen, weil sich der Canvas unter dem Zeiger mitbewegt.
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

    fn button(&mut self, data: &mut InfinityDesk, handle: &mut PointerInnerHandle<'_, InfinityDesk>, event: &ButtonEvent) {
        handle.button(data, event);
        if !handle.current_pressed().contains(&self.start_data.button) {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(&mut self, data: &mut InfinityDesk, handle: &mut PointerInnerHandle<'_, InfinityDesk>, details: AxisFrame) {
        handle.axis(data, details)
    }

    fn frame(&mut self, data: &mut InfinityDesk, handle: &mut PointerInnerHandle<'_, InfinityDesk>) {
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

    /// Maustaste losgelassen: berührt das Cluster jetzt andere Fenster, wird angedockt.
    fn unset(&mut self, data: &mut InfinityDesk) {
        let DragKind::MoveCluster { members, .. } = &self.kind else {
            return;
        };
        let moved: Vec<(Window, Rect)> = members
            .iter()
            .filter_map(|m| Some((m.window.clone(), data.space.element_geometry(&m.window)?)))
            .collect();
        let others: Vec<(Window, Rect)> = data
            .space
            .elements()
            .filter(|w| !members.iter().any(|m| &m.window == *w))
            .filter_map(|w| Some((w.clone(), data.space.element_geometry(w)?)))
            .collect();

        for (mw, mr) in &moved {
            for (ow, or) in &others {
                if touching(mr, or) {
                    tracing::info!("Angedockt");
                    data.clusters.join(mw, ow);
                }
            }
        }
    }
}
