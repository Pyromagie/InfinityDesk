//! Assembles what gets drawn each frame: windows and the canvas dot grid.

use smithay::{
    backend::renderer::{
        element::{Id, Kind, render_elements, solid::SolidColorRenderElement, surface::WaylandSurfaceRenderElement},
        gles::GlesRenderer,
        utils::CommitCounter,
    },
    desktop::{
        Space, Window,
        space::{SpaceRenderElements, space_render_elements},
    },
    output::Output,
    utils::{Physical, Point, Rectangle},
};

use crate::canvas::Canvas;

/// Distance between grid dots in logical pixels.
const GRID_SPACING: f64 = 100.0;
/// Size of a grid dot in pixels.
const GRID_DOT_SIZE: i32 = 2;
const GRID_DOT_COLOR: [f32; 4] = [0.28, 0.28, 0.32, 1.0];

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
            let loc: Point<i32, Physical> = (screen.x - GRID_DOT_SIZE / 2, screen.y - GRID_DOT_SIZE / 2).into();
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

/// All elements of a frame, front to back: windows, then the grid.
pub fn desk_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    space: &Space<Window>,
    canvas: &Canvas,
) -> Vec<DeskElement> {
    let mut elements: Vec<DeskElement> = space_render_elements(renderer, [space], output, 1.0)
        .unwrap_or_default()
        .into_iter()
        .map(DeskElement::Space)
        .collect();
    elements.extend(grid_elements(canvas).into_iter().map(DeskElement::Solid));
    elements
}
