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
#[derive(Default)]
pub struct Grid {
    /// Reused ids so damage tracking stays stable.
    ids: Vec<Id>,
}

impl Grid {
    fn elements(&mut self, canvas: &Canvas) -> Vec<SolidColorRenderElement> {
        let vp = canvas.viewport();
        let first_x = (vp.loc.x / GRID_SPACING).ceil() * GRID_SPACING;
        let first_y = (vp.loc.y / GRID_SPACING).ceil() * GRID_SPACING;

        let mut out = Vec::new();
        let mut y = first_y;
        while y < vp.loc.y + vp.size.h {
            let mut x = first_x;
            while x < vp.loc.x + vp.size.w {
                let i = out.len();
                if i >= self.ids.len() {
                    self.ids.push(Id::new());
                }
                let screen = canvas.canvas_to_screen((x, y).into());
                let loc: Point<i32, Physical> = (
                    screen.x.round() as i32 - GRID_DOT_SIZE / 2,
                    screen.y.round() as i32 - GRID_DOT_SIZE / 2,
                )
                    .into();
                out.push(SolidColorRenderElement::new(
                    self.ids[i].clone(),
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
}

/// All elements of a frame, front to back: windows, then the grid.
pub fn desk_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    space: &Space<Window>,
    canvas: &Canvas,
    grid: &mut Grid,
) -> Vec<DeskElement> {
    let mut elements: Vec<DeskElement> = space_render_elements(renderer, [space], output, 1.0)
        .unwrap_or_default()
        .into_iter()
        .map(DeskElement::Space)
        .collect();
    elements.extend(grid.elements(canvas).into_iter().map(DeskElement::Solid));
    elements
}
