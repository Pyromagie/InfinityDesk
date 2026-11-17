//! Assembles what gets drawn each frame: layer surfaces, windows (zoomed) and the canvas dot grid.

use smithay::{
    backend::renderer::{
        Color32F,
        element::{
            AsRenderElements, Id, Kind, render_elements, solid::SolidColorRenderElement,
            surface::WaylandSurfaceRenderElement, utils::RescaleRenderElement,
        },
        gles::{
            GlesPixelProgram, GlesRenderer, Uniform, UniformName, UniformType,
            element::PixelShaderElement,
        },
        utils::CommitCounter,
    },
    desktop::{Space, Window, layer_map_for_output},
    output::Output,
    utils::{Logical, Physical, Rectangle, Scale},
    wayland::shell::wlr_layer::Layer,
};

use crate::canvas::Canvas;

/// Distance between grid dots in canvas pixels ...
const GRID_SPACING: f64 = 100.0;
/// ... doubled while zoomed out so far that dots would be closer than this on screen.
const GRID_MIN_SCREEN_SPACING: f64 = 40.0;
/// Size of a grid dot in pixels.
const GRID_DOT_SIZE: f32 = 2.0;
const GRID_DOT_COLOR: [f32; 4] = [0.28, 0.28, 0.32, 1.0];

const OUTLINE_COLOR: [f32; 4] = [0.35, 0.65, 1.0, 1.0];
/// Thickness of outlines in pixels.
pub const OUTLINE_WIDTH: i32 = 4;

render_elements! {
    pub DeskElement<=GlesRenderer>;
    Surface = WaylandSurfaceRenderElement<GlesRenderer>,
    Zoomed = RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>,
    Grid = PixelShaderElement,
    Solid = SolidColorRenderElement,
}

/// Draws the dot grid on the GPU in one pass, instead of one element per dot.
const GRID_SHADER: &str = r#"
precision highp float;
varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif
uniform float spacing;
uniform float dot_size;
uniform vec2 offset;
uniform vec4 color;

void main() {
    // Distance (in screen pixels) past the last grid point, shifted so dots are centered on it.
    vec2 d = mod(v_coords * size + offset + dot_size / 2.0, spacing);
    gl_FragColor = (d.x < dot_size && d.y < dot_size) ? color * alpha : vec4(0.0);
}
"#;

pub fn compile_grid_shader(renderer: &mut GlesRenderer) -> GlesPixelProgram {
    let uniforms = [
        UniformName::new("spacing", UniformType::_1f),
        UniformName::new("dot_size", UniformType::_1f),
        UniformName::new("offset", UniformType::_2f),
        UniformName::new("color", UniformType::_4f),
    ];
    renderer
        .compile_custom_pixel_shader(GRID_SHADER, &uniforms)
        .expect("grid shader must compile")
}

/// Dot grid on the canvas, so panning is visible even on empty space.
fn grid_element(shader: &GlesPixelProgram, canvas: &Canvas) -> Option<PixelShaderElement> {
    let mut spacing = GRID_SPACING;
    while spacing * canvas.zoom() < GRID_MIN_SCREEN_SPACING {
        spacing *= 2.0;
    }
    let spacing = spacing * canvas.zoom();
    // Only the part of the screen that shows canvas (zoomed far out, it is surrounded by nothing).
    let origin = canvas.canvas_to_screen((0.0, 0.0).into());
    let canvas_rect = Rectangle::new(origin, canvas.size().to_f64().upscale(canvas.zoom()));
    let screen = Rectangle::from_size(canvas.viewport().size.upscale(canvas.zoom()));
    let area = canvas_rect.intersection(screen)?.to_i32_round();
    let offset = area.loc.to_f64() - origin;
    let [r, g, b, a] = GRID_DOT_COLOR;
    let uniforms = vec![
        Uniform::new("spacing", spacing as f32),
        Uniform::new("dot_size", GRID_DOT_SIZE),
        Uniform::new("offset", (offset.x as f32, offset.y as f32)),
        Uniform::new("color", (r, g, b, a)),
    ];
    Some(PixelShaderElement::new(
        shader.clone(),
        area,
        None,
        1.0,
        uniforms,
        Kind::Unspecified,
    ))
}

/// A blue frame around a window (selected, or pulsing when unlocked) or a selection box.
pub struct Outline {
    /// Screen rectangle the frame is drawn around.
    pub rect: Rectangle<i32, Logical>,
    /// Extra distance from the rectangle (negative: drawn inside it).
    pub grow: i32,
    pub alpha: f32,
}

fn outline_elements(outline: &Outline) -> Vec<SolidColorRenderElement> {
    let Outline { rect, grow, alpha } = *outline;
    let color = Color32F::from(OUTLINE_COLOR) * alpha;
    let o = grow + OUTLINE_WIDTH;
    let (x, y, w, h) = (
        rect.loc.x - o,
        rect.loc.y - o,
        rect.size.w + 2 * o,
        rect.size.h + 2 * o,
    );
    let edges = [
        (x, y, w, OUTLINE_WIDTH),
        (x, y + h - OUTLINE_WIDTH, w, OUTLINE_WIDTH),
        (x, y, OUTLINE_WIDTH, h),
        (x + w - OUTLINE_WIDTH, y, OUTLINE_WIDTH, h),
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

/// All elements of a frame, front to back: outlines, top layers, windows, grid, bottom layers.
pub fn desk_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    space: &Space<Window>,
    canvas: &Canvas,
    grid_shader: &GlesPixelProgram,
    outlines: &[Outline],
) -> Vec<DeskElement> {
    let mut elements: Vec<DeskElement> = outlines
        .iter()
        .flat_map(outline_elements)
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
    elements.extend(grid_element(grid_shader, canvas).map(DeskElement::Grid));
    let lower = layer_elements(renderer, output, &[Layer::Bottom, Layer::Background]);
    elements.extend(lower.into_iter().map(DeskElement::Surface));
    elements
}
