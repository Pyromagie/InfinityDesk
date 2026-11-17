//! Assembles what gets drawn each frame: layer surfaces, windows (zoomed) and the canvas dot grid.

use smithay::{
    backend::renderer::{
        Color32F,
        element::{
            AsRenderElements, Id, Kind, render_elements, solid::SolidColorRenderElement,
            surface::WaylandSurfaceRenderElement, utils::RescaleRenderElement,
        },
        gles::GlesRenderer,
        utils::CommitCounter,
    },
    desktop::{Space, Window, layer_map_for_output},
    output::Output,
    utils::{Logical, Physical, Point, Rectangle, Scale},
    wayland::shell::wlr_layer::Layer,
};

use crate::canvas::Canvas;

/// Distance between grid dots in canvas pixels ...
const GRID_SPACING: f64 = 100.0;
/// ... doubled while zoomed out so far that dots would be closer than this on screen.
const GRID_MIN_SCREEN_SPACING: f64 = 40.0;
/// Size of a grid dot in pixels.
const GRID_DOT_SIZE: i32 = 2;
const GRID_DOT_COLOR: [f32; 4] = [0.28, 0.28, 0.32, 1.0];

const PULSE_COLOR: [f32; 4] = [0.35, 0.65, 1.0, 1.0];
/// Thickness of the pulse ring and how far it grows outward (in pixels).
const PULSE_WIDTH: i32 = 4;
const PULSE_GROW: f64 = 16.0;

render_elements! {
    pub DeskElement<=GlesRenderer>;
    Surface = WaylandSurfaceRenderElement<GlesRenderer>,
    Zoomed = RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>,
    Solid = SolidColorRenderElement,
}

/// Dot grid on the canvas, so panning is visible even on empty space.
fn grid_elements(canvas: &Canvas) -> Vec<SolidColorRenderElement> {
    let mut spacing = GRID_SPACING;
    while spacing * canvas.zoom() < GRID_MIN_SCREEN_SPACING {
        spacing *= 2.0;
    }
    // Only the part of the viewport that shows canvas (zoomed far out, it is surrounded by nothing).
    let size = canvas.size().to_f64();
    let vp = canvas.viewport();
    let (right, bottom) = (
        (vp.loc.x + vp.size.w).min(size.w),
        (vp.loc.y + vp.size.h).min(size.h),
    );
    let mut out = Vec::new();
    let mut y = (vp.loc.y.max(0.0) / spacing).ceil() * spacing;
    while y <= bottom {
        let mut x = (vp.loc.x.max(0.0) / spacing).ceil() * spacing;
        while x <= right {
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
            x += spacing;
        }
        y += spacing;
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

/// Layer surfaces (launchers, bars, wallpapers) sit on the screen and are never zoomed.
fn layer_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    layers: &[Layer],
) -> Vec<WaylandSurfaceRenderElement<GlesRenderer>> {
    let map = layer_map_for_output(output);
    map.layers()
        .rev()
        .filter(|l| layers.contains(&l.layer()))
        .filter_map(|l| Some((l, map.layer_geometry(l)?.loc)))
        .flat_map(|(l, loc)| l.render_elements(renderer, loc.to_physical(1), Scale::from(1.0), 1.0))
        .collect()
}

/// All elements of a frame, front to back: pulse, top layers, windows, grid, bottom layers.
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
    let upper = layer_elements(renderer, output, &[Layer::Overlay, Layer::Top]);
    elements.extend(upper.into_iter().map(DeskElement::Surface));
    // Only windows inside the viewport are rendered (culling), then shrunk by the zoom.
    let viewport = canvas.viewport().to_i32_round();
    let windows = space.render_elements_for_region(renderer, &viewport, 1.0, 1.0);
    elements.extend(windows.into_iter().map(|e| {
        DeskElement::Zoomed(RescaleRenderElement::from_element(
            e,
            (0, 0).into(),
            canvas.zoom(),
        ))
    }));
    elements.extend(grid_elements(canvas).into_iter().map(DeskElement::Solid));
    let lower = layer_elements(renderer, output, &[Layer::Bottom, Layer::Background]);
    elements.extend(lower.into_iter().map(DeskElement::Surface));
    elements
}
