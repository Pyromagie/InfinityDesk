//! The huge, bounded canvas and the viewport (the "camera").
//!
//! Coordinates: (0, 0) is the top-left corner of the canvas, everything in logical pixels.
//! The viewport is the part of the canvas currently shown on screen. With zoom < 1 it is larger
//! than the screen: zoom 0.5 shows twice the width and height.
//!
//! This module is pure logic without Wayland, so it is easy to test.

use smithay::utils::{Logical, Point, Rectangle, Size};

/// Default canvas size (to become configurable).
pub const DEFAULT_CANVAS_WIDTH: i32 = 20_000;
pub const DEFAULT_CANVAS_HEIGHT: i32 = 12_000;

#[derive(Debug, Clone)]
pub struct Canvas {
    size: Size<i32, Logical>,
    screen: Size<i32, Logical>,
    /// Top-left corner of the viewport in canvas coordinates.
    viewport_loc: Point<f64, Logical>,
    /// 1.0 = windows at their real size; smaller = zoomed out.
    zoom: f64,
}

impl Canvas {
    pub fn new(size: Size<i32, Logical>) -> Self {
        Self {
            size,
            screen: (0, 0).into(),
            viewport_loc: (0.0, 0.0).into(),
            zoom: 1.0,
        }
    }

    pub fn size(&self) -> Size<i32, Logical> {
        self.size
    }

    pub fn zoom(&self) -> f64 {
        self.zoom
    }

    pub fn viewport_loc(&self) -> Point<f64, Logical> {
        self.viewport_loc
    }

    pub fn viewport(&self) -> Rectangle<f64, Logical> {
        let size = (
            self.screen.w as f64 / self.zoom,
            self.screen.h as f64 / self.zoom,
        );
        Rectangle::new(self.viewport_loc, size.into())
    }

    pub fn viewport_center(&self) -> Point<f64, Logical> {
        let vp = self.viewport();
        vp.loc + Point::from((vp.size.w / 2.0, vp.size.h / 2.0))
    }

    /// The screen was resized; the viewport keeps its center.
    pub fn set_screen_size(&mut self, size: Size<i32, Logical>) {
        let center = self.viewport_center();
        self.screen = size;
        self.zoom = self.zoom.clamp(self.min_zoom(), 1.0);
        self.center_viewport_on(center);
    }

    /// Moves the viewport; it stays inside the canvas (or centered on it when zoomed out
    /// further than the canvas) and on whole pixels.
    pub fn set_viewport_loc(&mut self, loc: Point<f64, Logical>) {
        let vp = self.viewport().size;
        self.viewport_loc = (
            clamp_axis(loc.x, vp.w, self.size.w as f64).round(),
            clamp_axis(loc.y, vp.h, self.size.h as f64).round(),
        )
            .into();
    }

    /// Centers the viewport on a canvas point.
    pub fn center_viewport_on(&mut self, point: Point<f64, Logical>) {
        let vp = self.viewport().size;
        self.set_viewport_loc(point - Point::from((vp.w / 2.0, vp.h / 2.0)));
    }

    /// Zooms by `factor`, keeping the canvas point under `screen` in place.
    /// Never closer than 1:1, never further out than the whole canvas fitting on screen.
    pub fn zoom_at(&mut self, screen: Point<f64, Logical>, factor: f64) {
        let anchor = self.screen_to_canvas(screen);
        self.zoom = (self.zoom * factor).clamp(self.min_zoom(), 1.0);
        self.set_viewport_loc(anchor - screen.downscale(self.zoom));
    }

    /// Zoom at which the whole canvas fits on the screen.
    fn min_zoom(&self) -> f64 {
        let w = self.screen.w as f64 / self.size.w as f64;
        let h = self.screen.h as f64 / self.size.h as f64;
        w.min(h).min(1.0)
    }

    pub fn canvas_to_screen(&self, canvas: Point<f64, Logical>) -> Point<f64, Logical> {
        (canvas - self.viewport_loc).upscale(self.zoom)
    }

    pub fn screen_to_canvas(&self, screen: Point<f64, Logical>) -> Point<f64, Logical> {
        self.viewport_loc + screen.downscale(self.zoom)
    }

    /// Adjusts a window position so the window lies completely inside the canvas.
    pub fn clamp_window_loc(
        &self,
        loc: Point<i32, Logical>,
        window_size: Size<i32, Logical>,
    ) -> Point<i32, Logical> {
        (
            clamp_axis(loc.x as f64, window_size.w as f64, self.size.w as f64) as i32,
            clamp_axis(loc.y as f64, window_size.h as f64, self.size.h as f64) as i32,
        )
            .into()
    }

    /// Position at which a window of the given size is centered in the viewport.
    pub fn centered_in_viewport(&self, window_size: Size<i32, Logical>) -> Point<i32, Logical> {
        let c = self.viewport_center();
        let loc = Point::from((
            c.x - window_size.w as f64 / 2.0,
            c.y - window_size.h as f64 / 2.0,
        ))
        .to_i32_round();
        self.clamp_window_loc(loc, window_size)
    }
}

/// Keeps an object of length `len` at position `pos` within `0..max`.
/// If the object is larger than the area, it is centered on it.
fn clamp_axis(pos: f64, len: f64, max: f64) -> f64 {
    if len >= max {
        (max - len) / 2.0
    } else {
        pos.clamp(0.0, max - len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas() -> Canvas {
        let mut c = Canvas::new((10_000, 6_000).into());
        c.set_screen_size((1920, 1080).into());
        c
    }

    #[test]
    fn viewport_stays_inside_canvas() {
        let mut c = canvas();
        c.set_viewport_loc((-500.0, -500.0).into());
        assert_eq!(c.viewport_loc(), (0.0, 0.0).into());
        c.set_viewport_loc((99_999.0, 99_999.0).into());
        assert_eq!(
            c.viewport_loc(),
            ((10_000 - 1920) as f64, (6_000 - 1080) as f64).into()
        );
    }

    #[test]
    fn resize_keeps_center() {
        let mut c = canvas();
        c.center_viewport_on((5_000.0, 3_000.0).into());
        c.set_screen_size((800, 600).into());
        assert_eq!(c.viewport_center(), (5_000.0, 3_000.0).into());
    }

    #[test]
    fn center_viewport() {
        let mut c = canvas();
        c.center_viewport_on((5_000.0, 3_000.0).into());
        assert_eq!(c.viewport_center(), (5_000.0, 3_000.0).into());
    }

    #[test]
    fn screen_canvas_conversion() {
        let mut c = canvas();
        c.set_viewport_loc((1_000.0, 2_000.0).into());
        let p = (1_010.0, 2_020.0).into();
        assert_eq!(c.canvas_to_screen(p), (10.0, 20.0).into());
        assert_eq!(c.screen_to_canvas((10.0, 20.0).into()), p);
    }

    #[test]
    fn zoom_keeps_point_under_pointer() {
        let mut c = canvas();
        c.set_viewport_loc((1_000.0, 1_000.0).into());
        let pointer = (960.0, 540.0).into();
        let before = c.screen_to_canvas(pointer);
        c.zoom_at(pointer, 0.5);
        assert_eq!(c.zoom(), 0.5);
        assert_eq!(c.screen_to_canvas(pointer), before);
        assert_eq!(c.canvas_to_screen(before), pointer);
    }

    #[test]
    fn zoom_limits() {
        let mut c = canvas();
        c.zoom_at((0.0, 0.0).into(), 10.0);
        assert_eq!(c.zoom(), 1.0);
        // Whole canvas fits: limited by the height, 1080 / 6000.
        c.zoom_at((0.0, 0.0).into(), 0.0001);
        assert_eq!(c.zoom(), 0.18);
        assert_eq!(c.viewport().size.h, 6_000.0);
    }

    #[test]
    fn window_stays_inside_canvas() {
        let c = canvas();
        let size = (800, 600).into();
        assert_eq!(
            c.clamp_window_loc((-50, 9_999).into(), size),
            (0, 6_000 - 600).into()
        );
    }

    #[test]
    fn new_window_centered() {
        let mut c = canvas();
        c.set_viewport_loc((1_000.0, 1_000.0).into());
        let loc = c.centered_in_viewport((800, 600).into());
        assert_eq!(loc, (1_000 + 960 - 400, 1_000 + 540 - 300).into());
    }

    #[test]
    fn screen_larger_than_canvas_centers_it() {
        let mut c = Canvas::new((1_000, 1_000).into());
        c.set_screen_size((1920, 1080).into());
        c.set_viewport_loc((300.0, 300.0).into());
        assert_eq!(c.viewport_loc(), (-460.0, -40.0).into());
    }
}
