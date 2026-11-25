//! The monitors. All show the same canvas, each with a view (viewport and zoom) of its own.

use std::ops::{Deref, DerefMut};

use smithay::{
    backend::input::InputTime,
    desktop::{Window, WindowSurfaceType, layer_map_for_output},
    input::pointer::ClickGrab,
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Size},
    wayland::{
        compositor::{TraversalAction, with_surface_tree_downward},
        fractional_scale::with_fractional_scale,
    },
};

use crate::{
    InfinityDesk, arrange::Fullscreen, canvas::Canvas, cluster::Rect, grabs::DragGrab,
    navigate::Overview,
};

pub struct Monitor {
    /// `None`: no monitor is connected, this one only keeps the view until there is one.
    pub output: Option<Output>,
    /// The canvas and this monitor's viewport.
    pub canvas: Canvas,
    /// The fullscreen window (Mod+F).
    pub fullscreen: Option<Fullscreen>,
    /// The open overview (Mod+Tab).
    pub overview: Option<Overview>,
}

/// All monitors (at least one). Stands for the current one, the one the pointer is on: keys,
/// gestures and new windows act there.
pub struct Monitors {
    pub list: Vec<Monitor>,
    current: usize,
}

impl Deref for Monitors {
    type Target = Monitor;

    fn deref(&self) -> &Monitor {
        &self.list[self.current]
    }
}

impl DerefMut for Monitors {
    fn deref_mut(&mut self) -> &mut Monitor {
        &mut self.list[self.current]
    }
}

impl Monitors {
    /// No monitor yet; the view starts in the middle of the canvas, leaving room in every
    /// direction.
    pub fn new(canvas_size: Size<i32, Logical>) -> Self {
        let mut monitor = Monitor {
            output: None,
            canvas: Canvas::new(canvas_size),
            fullscreen: None,
            overview: None,
        };
        let size = canvas_size.to_f64();
        (monitor.canvas).center_viewport_on((size.w / 2.0, size.h / 2.0).into());
        Self {
            list: vec![monitor],
            current: 0,
        }
    }

    pub fn current(&self) -> usize {
        self.current
    }

    pub fn outputs(&self) -> impl Iterator<Item = &Output> {
        self.list.iter().filter_map(|m| m.output.as_ref())
    }

    fn position(&self, output: &Output) -> Option<usize> {
        (self.list.iter()).position(|m| m.output.as_ref() == Some(output))
    }
}

impl InfinityDesk {
    /// Runs `f` with monitor `i` as the current one.
    pub fn on_monitor<R>(&mut self, i: usize, f: impl FnOnce(&mut Self) -> R) -> R {
        let current = std::mem::replace(&mut self.monitor.current, i);
        let result = f(self);
        self.monitor.current = current;
        result
    }

    /// The monitor that shows a place on the canvas: the current one if it does (also if none
    /// does), else the first one.
    pub fn monitor_showing(&self, rect: Rect) -> usize {
        let shows = |m: &Monitor| m.canvas.viewport().overlaps(rect.to_f64());
        let current = self.monitor.current;
        match shows(&self.monitor) {
            true => current,
            false => (self.monitor.list.iter().position(shows)).unwrap_or(current),
        }
    }

    /// The monitor a window belongs to (see `monitor_showing`): it decides the window's sizes
    /// and scale.
    pub fn monitor_of(&self, window: &Window) -> usize {
        let rect = self.target_rect(window);
        rect.map_or(self.monitor.current, |r| self.monitor_showing(r))
    }

    /// The monitor a window is fullscreen on.
    pub fn fullscreen_monitor(&self, window: &Window) -> Option<usize> {
        let is = |m: &Monitor| m.fullscreen.as_ref().is_some_and(|f| &f.window == window);
        self.monitor.list.iter().position(is)
    }

    /// The output showing a layer surface (a bar, launcher or wallpaper).
    pub fn layer_output(&self, surface: &WlSurface) -> Option<Output> {
        let shows = |o: &&Output| {
            let map = layer_map_for_output(o);
            (map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)).is_some()
        };
        self.monitor.outputs().find(shows).cloned()
    }

    /// A monitor was connected. Its view starts beside the first one's, as the monitors stand.
    pub fn add_output(&mut self, output: Output) {
        let first = &self.monitor.list[0];
        match &first.output {
            None => self.monitor.list[0].output = Some(output.clone()),
            Some(first_output) => {
                let canvas = Canvas::new(first.canvas.size());
                let (loc, first_output) = (first.canvas.target().0, first_output.clone());
                self.monitor.list.push(Monitor {
                    output: Some(output.clone()),
                    canvas,
                    fullscreen: None,
                    overview: None,
                });
                self.arrange_outputs();
                let offset = output.current_location() - first_output.current_location();
                let canvas = &mut self.monitor.list.last_mut().unwrap().canvas;
                canvas.set_screen_size(screen_size(&output));
                canvas.set_view(loc + offset.to_f64(), 1.0);
            }
        }
        self.output_changed(&output);
    }

    /// The monitors stand where the config says, the others to the right of them side by side,
    /// in the order they were connected.
    fn arrange_outputs(&mut self) {
        let place = |output: &Output, loc: Point<i32, Logical>| {
            if output.current_location() != loc {
                output.change_current_state(None, None, None, Some(loc));
            }
            loc.x + screen_size(output).w
        };
        let position = |o: &Output| self.config.output(&o.name()).position;
        let mut x = (self.monitor.outputs())
            .filter_map(|o| Some(place(o, position(o)?.into())))
            .max()
            .unwrap_or(0);
        for output in self.monitor.outputs().filter(|o| position(o).is_none()) {
            x = place(output, (x, 0).into());
        }
    }

    /// A mouse or touchpad moves the pointer by `delta`, also onto another monitor beside the
    /// current one: there it is at another place of the canvas. A window being dragged comes
    /// along; other drags stay on their monitor.
    pub fn move_pointer(&mut self, delta: Point<f64, Logical>, time: InputTime) {
        let pointer = self.seat.get_pointer().unwrap();
        let stays = pointer.with_grab(|_, grab| match grab.downcast_ref::<DragGrab>() {
            Some(drag) => !drag.moves_windows(),
            // A button held on a program.
            None => grab.is::<ClickGrab<Self>>(),
        });
        let rects: Vec<Rect> = (self.monitor.list.iter())
            .map(|m| {
                let loc = m.output.as_ref().map(|o| o.current_location());
                Rect::new(loc.unwrap_or_default(), m.canvas.screen_size())
            })
            .collect();
        let pos = self.pointer_screen + delta;
        let current = self.monitor.current;
        let pos = match across(&rects, current, pos) {
            Some((i, pos)) if i == current || !stays.unwrap_or(false) => {
                self.monitor.current = i;
                pos
            }
            _ => {
                let max = rects[current].size.to_f64() - Size::from((1.0, 1.0));
                let clamp = |v: f64, max: f64| v.min(max).max(0.0);
                (clamp(pos.x, max.w), clamp(pos.y, max.h)).into()
            }
        };
        self.pointer_moved(self.monitor.canvas.screen_to_canvas(pos), time);
    }

    /// A monitor changed its size or scale (nested window resized, monitor settings): bars,
    /// the view and a fullscreen window follow, and apps learn the new scale.
    pub fn output_changed(&mut self, output: &Output) {
        let Some(i) = self.monitor.position(output) else {
            return;
        };
        self.arrange_outputs();
        layer_map_for_output(output).arrange();
        self.on_monitor(i, |state| {
            state.monitor.canvas.set_screen_size(screen_size(output));
            state.apply_viewport();
            state.fit_fullscreen();
        });
        for layer in layer_map_for_output(output).layers() {
            set_scale(layer.wl_surface(), output);
        }
        for window in self.space.elements() {
            self.update_scale(window);
        }
    }

    /// Tells a window the scale of its monitor.
    pub fn update_scale(&self, window: &Window) {
        if let Some(output) = &self.monitor.list[self.monitor_of(window)].output {
            set_scale(window.toplevel().unwrap().wl_surface(), output);
        }
    }

    /// Makes monitor `i` the current one; the pointer goes to its middle. Not during a drag.
    pub fn focus_monitor(&mut self, i: usize) {
        if i == self.monitor.current || self.seat.get_pointer().unwrap().is_grabbed() {
            return;
        }
        self.monitor.current = i;
        let middle = self.monitor.canvas.viewport_center();
        self.pointer_moved(middle, InputTime::now());
    }

    /// If `rect` is not on the current monitor but completely on another one, that one becomes
    /// the current one (a window fullscreen there no longer covers it).
    pub fn go_to_monitor_showing(&mut self, rect: Rect) {
        let shows = |m: &Monitor| m.canvas.target_viewport().contains_rect(rect.to_f64());
        if shows(&self.monitor) {
            return;
        }
        if let Some(i) = self.monitor.list.iter().position(shows) {
            self.focus_monitor(i);
            let covered = |f: &Fullscreen| self.target_rect(&f.window) != Some(rect);
            if self.monitor.fullscreen.as_ref().is_some_and(covered) {
                self.end_fullscreen();
            }
        }
    }
}

/// The monitor under `pos` (a position on monitor `current`, maybe beyond its edges), and the
/// position on it. `None`: there is none.
fn across(
    rects: &[Rect],
    current: usize,
    pos: Point<f64, Logical>,
) -> Option<(usize, Point<f64, Logical>)> {
    let pos = pos + rects[current].loc.to_f64();
    // The current one first, should monitors overlap.
    (std::iter::once(current).chain(0..rects.len()))
        .find(|&i| rects[i].to_f64().contains(pos))
        .map(|i| (i, pos - rects[i].loc.to_f64()))
}

/// The screen's size in logical pixels: its resolution, turned and divided by its scale.
pub fn screen_size(output: &Output) -> Size<i32, Logical> {
    let mode = output.current_mode().unwrap();
    let scale = output.current_scale().fractional_scale();
    let size = output.current_transform().transform_size(mode.size);
    size.to_f64().to_logical(scale).to_i32_round()
}

/// Tells a surface and its subsurfaces the scale to draw at.
fn set_scale(surface: &WlSurface, output: &Output) {
    let scale = output.current_scale().fractional_scale();
    with_surface_tree_downward(
        surface,
        (),
        |_, _, _| TraversalAction::DoChildren(()),
        |_, states, _| with_fractional_scale(states, |f| f.set_preferred_scale(scale)),
        |_, _, _| true,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pointer_crosses_where_monitors_touch() {
        // A small monitor to the right of the first one, 100 px lower, and one above.
        let rect = |x, y, w, h| Rect::new((x, y).into(), (w, h).into());
        let rects = [
            rect(0, 0, 1920, 1080),
            rect(1920, 100, 1280, 720),
            rect(0, -1080, 1920, 1080),
        ];
        let from = |i, x: f64, y: f64| across(&rects, i, (x, y).into());
        assert_eq!(from(0, 1919.0, 500.0), Some((0, (1919.0, 500.0).into())));
        assert_eq!(from(0, 1925.0, 500.0), Some((1, (5.0, 400.0).into())));
        assert_eq!(from(1, -5.0, 400.0), Some((0, (1915.0, 500.0).into())));
        assert_eq!(from(0, 300.0, -1.0), Some((2, (300.0, 1079.0).into())));
        // Beside the small monitor there is nothing.
        assert_eq!(from(0, 1925.0, 50.0), None);
        assert_eq!(from(1, 5.0, -50.0), None);
        // Overlapping (mirrored) monitors: it stays.
        let same = [rects[0], rects[0]];
        assert_eq!(
            across(&same, 1, (5.0, 5.0).into()),
            Some((1, (5.0, 5.0).into()))
        );
    }
}
