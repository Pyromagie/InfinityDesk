//! The huge, bounded canvas and the viewport (the "camera").
//!
//! Coordinates: (0, 0) is the top-left corner of the canvas, everything in logical pixels.
//! The viewport is the part of the canvas currently shown on screen.
//!
//! This module is pure logic without Wayland, so it is easy to test.

use smithay::utils::{Logical, Point, Rectangle, Size};

/// Default canvas size (to become configurable).
pub const DEFAULT_CANVAS_WIDTH: i32 = 20_000;
pub const DEFAULT_CANVAS_HEIGHT: i32 = 12_000;

#[derive(Debug, Clone)]
pub struct Canvas {
    size: Size<i32, Logical>,
    /// Top-left corner of the viewport in canvas coordinates.
    viewport_loc: Point<f64, Logical>,
    /// Viewport size = screen size (at 1:1 zoom).
    viewport_size: Size<i32, Logical>,
}

impl Canvas {
    pub fn new(size: Size<i32, Logical>) -> Self {
        Self {
            size,
            viewport_loc: (0.0, 0.0).into(),
            viewport_size: (0, 0).into(),
        }
    }

    pub fn size(&self) -> Size<i32, Logical> {
        self.size
    }

    pub fn viewport_loc(&self) -> Point<f64, Logical> {
        self.viewport_loc
    }

    pub fn viewport(&self) -> Rectangle<f64, Logical> {
        Rectangle::new(self.viewport_loc, self.viewport_size.to_f64())
    }

    pub fn viewport_center(&self) -> Point<f64, Logical> {
        let s = self.viewport_size.to_f64();
        self.viewport_loc + Point::from((s.w / 2.0, s.h / 2.0))
    }

    /// The screen was resized.
    pub fn set_viewport_size(&mut self, size: Size<i32, Logical>) {
        self.viewport_size = size;
        self.set_viewport_loc(self.viewport_loc);
    }

    /// Moves the viewport; it always stays inside the canvas.
    pub fn set_viewport_loc(&mut self, loc: Point<f64, Logical>) {
        self.viewport_loc = (
            clamp_axis(loc.x, self.viewport_size.w, self.size.w),
            clamp_axis(loc.y, self.viewport_size.h, self.size.h),
        )
            .into();
    }

    /// Centers the viewport on a canvas point.
    pub fn center_viewport_on(&mut self, point: Point<f64, Logical>) {
        let s = self.viewport_size.to_f64();
        self.set_viewport_loc(point - Point::from((s.w / 2.0, s.h / 2.0)));
    }

    /// Converts a screen position into a canvas position.
    pub fn screen_to_canvas(&self, screen: Point<f64, Logical>) -> Point<f64, Logical> {
        screen + self.viewport_loc
    }

    /// Converts a canvas position into a screen position.
    pub fn canvas_to_screen(&self, canvas: Point<f64, Logical>) -> Point<f64, Logical> {
        canvas - self.viewport_loc
    }

    /// Adjusts a window position so the window lies completely inside the canvas.
    pub fn clamp_window_loc(&self, loc: Point<i32, Logical>, window_size: Size<i32, Logical>) -> Point<i32, Logical> {
        (
            clamp_axis(loc.x as f64, window_size.w, self.size.w) as i32,
            clamp_axis(loc.y as f64, window_size.h, self.size.h) as i32,
        )
            .into()
    }

    /// Position at which a window of the given size is centered in the viewport.
    pub fn centered_in_viewport(&self, window_size: Size<i32, Logical>) -> Point<i32, Logical> {
        let c = self.viewport_center();
        let loc = Point::from((c.x - window_size.w as f64 / 2.0, c.y - window_size.h as f64 / 2.0)).to_i32_round();
        self.clamp_window_loc(loc, window_size)
    }
}

/// Keeps an object of length `len` at position `pos` within `0..max`.
/// If the object is larger than the area, it is aligned to 0.
fn clamp_axis(pos: f64, len: i32, max: i32) -> f64 {
    let upper = (max - len).max(0) as f64;
    pos.clamp(0.0, upper)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas() -> Canvas {
        let mut c = Canvas::new((10_000, 6_000).into());
        c.set_viewport_size((1920, 1080).into());
        c
    }

    #[test]
    fn viewport_stays_inside_canvas() {
        let mut c = canvas();
        c.set_viewport_loc((-500.0, -500.0).into());
        assert_eq!(c.viewport_loc(), (0.0, 0.0).into());
        c.set_viewport_loc((99_999.0, 99_999.0).into());
        assert_eq!(c.viewport_loc(), ((10_000 - 1920) as f64, (6_000 - 1080) as f64).into());
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
        let p = c.screen_to_canvas((10.0, 20.0).into());
        assert_eq!(p, (1_010.0, 2_020.0).into());
        assert_eq!(c.canvas_to_screen(p), (10.0, 20.0).into());
    }

    #[test]
    fn window_stays_inside_canvas() {
        let c = canvas();
        let size = (800, 600).into();
        assert_eq!(c.clamp_window_loc((-50, 9_999).into(), size), (0, 6_000 - 600).into());
    }

    #[test]
    fn new_window_centered() {
        let mut c = canvas();
        c.set_viewport_loc((1_000.0, 1_000.0).into());
        let loc = c.centered_in_viewport((800, 600).into());
        assert_eq!(loc, (1_000 + 960 - 400, 1_000 + 540 - 300).into());
    }

    #[test]
    fn screen_larger_than_canvas() {
        let mut c = Canvas::new((1_000, 1_000).into());
        c.set_viewport_size((1920, 1080).into());
        c.set_viewport_loc((300.0, 300.0).into());
        assert_eq!(c.viewport_loc(), (0.0, 0.0).into());
    }
}
