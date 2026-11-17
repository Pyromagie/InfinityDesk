//! Assembles what gets drawn each frame: windows and the canvas dot grid.

use smithay::{
    backend::renderer::{
        Color32F,
        element::{
            Id, Kind, render_elements, solid::SolidColorRenderElement,
            surface::WaylandSurfaceRenderElement,
        },
        gles::GlesRenderer,
        utils::CommitCounter,
    },
    desktop::{
        Space, Window,
        space::{SpaceRenderElements, space_render_elements},
    },
    output::Output,
    utils::{Logical, Physical, Point, Rectangle},
};

use crate::canvas::Canvas;

/// Distance between grid dots in logical pixels.
const GRID_SPACING: f64 = 100.0;
/// Size of a grid dot in pixels.
const GRID_DOT_SIZE: i32 = 2;
const GRID_DOT_COLOR: [f32; 4] = [0.28, 0.28, 0.32, 1.0];

const PULSE_COLOR: [f32; 4] = [0.35, 0.65, 1.0, 1.0];
/// Thickness of the pulse ring and how far it grows outward (in pixels).
const PULSE_WIDTH: i32 = 4;
const PULSE_GROW: f64 = 16.0;

render_elements! {
    pub DeskElement<=GlesRenderer>;
    Space = SpaceRenderElements<GlesRenderer, WaylandSurfaceRenderElement<GlesRenderer>>,
    Solid = SolidColorRenderElement,
}

/// Dot grid on the canvas, so panning is visible even on empty space.
fn grid_elements(canvas: &Canvas) -> Vec<SolidColorRenderElement> {
    let vp = canvas.viewport();
    let mut out = Vec::new();
    let mut y = (vp.loc.y / GRID_SPACING).ceil() * GRID_SPACING;
    while y < vp.loc.y + vp.size.h {
        let mut x = (vp.loc.x / GRID_SPACING).ceil() * GRID_SPACING;
        while x < vp.loc.x + vp.size.w {
            let screen = canvas.canvas_to_screen((x, y).into()).to_i32_round::<i32>();
            let loc: Point<i32, Physical> =
                (screen.x - GRID_DOT_SIZE / 2, screen.y - GRID_DOT_SIZE / 2).into();
            out.push(SolidColorRenderElement::new(
                Id::new(),
                Rectangle::new(loc, (GRID_DOT_SIZE, GRID_DOT_SIZE).into()),
                CommitCounter::default(),
                GRID_DOT_COLOR,
                Kind::Unspecified,
            ));
            x += GRID_SPACING;
        }
        y += GRID_SPACING;
    }
    out
}

/// A ring around a window that grows outward and fades: the window was unlocked from its cluster.
fn pulse_elements(rect: Rectangle<i32, Logical>, t: f64) -> Vec<SolidColorRenderElement> {
    let color = Color32F::from(PULSE_COLOR) * (1.0 - t) as f32;
    let o = (t * PULSE_GROW) as i32 + PULSE_WIDTH;
    let (x, y, w, h) = (
        rect.loc.x - o,
        rect.loc.y - o,
        rect.size.w + 2 * o,
        rect.size.h + 2 * o,
    );
    let edges = [
        (x, y, w, PULSE_WIDTH),
        (x, y + h - PULSE_WIDTH, w, PULSE_WIDTH),
        (x, y, PULSE_WIDTH, h),
        (x + w - PULSE_WIDTH, y, PULSE_WIDTH, h),
    ];
    edges
        .into_iter()
        .map(|(x, y, w, h)| {
            let geo = Rectangle::<i32, Physical>::new((x, y).into(), (w, h).into());
            SolidColorRenderElement::new(
                Id::new(),
                geo,
                CommitCounter::default(),
                color,
                Kind::Unspecified,
            )
        })
        .collect()
}

/// All elements of a frame, front to back: windows, then the grid.
pub fn desk_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    space: &Space<Window>,
    canvas: &Canvas,
    pulse: Option<(Rectangle<i32, Logical>, f64)>,
) -> Vec<DeskElement> {
    let mut elements: Vec<DeskElement> = pulse
        .map(|(rect, t)| pulse_elements(rect, t))
        .unwrap_or_default()
        .into_iter()
        .map(DeskElement::Solid)
        .collect();
    elements.extend(
        space_render_elements(renderer, [space], output, 1.0)
            .unwrap_or_default()
            .into_iter()
            .map(DeskElement::Space),
    );
    elements.extend(grid_elements(canvas).into_iter().map(DeskElement::Solid));
    elements
}
