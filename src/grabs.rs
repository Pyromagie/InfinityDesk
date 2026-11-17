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

use crate::InfinityDesk;

pub enum DragKind {
    /// Ein Fenster wird über den Canvas gezogen.
    MoveWindow {
        window: Window,
        initial_loc: Point<i32, Logical>,
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
    pub fn move_window(state: &InfinityDesk, start_data: GrabStartData<InfinityDesk>, window: Window) -> Self {
        let initial_loc = state.space.element_location(&window).unwrap_or_default();
        Self {
            start_data,
            kind: DragKind::MoveWindow { window, initial_loc },
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

        match &self.kind {
            DragKind::MoveWindow { window, initial_loc } => {
                let delta = event.location - self.start_data.location;
                let target = (initial_loc.to_f64() + delta).to_i32_round();
                let target = data.canvas.clamp_window_loc(target, window.geometry().size);
                data.space.map_element(window.clone(), target, true);
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

    fn unset(&mut self, _data: &mut InfinityDesk) {}
}
