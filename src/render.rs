//! Assembles what gets drawn each frame: layer surfaces, windows (zoomed) and the canvas dot grid.

use smithay::{
    backend::renderer::{
        element::{
            AsRenderElements, Kind,
            memory::MemoryRenderBufferRenderElement,
            render_elements,
            solid::{SolidColorBuffer, SolidColorRenderElement},
            surface::WaylandSurfaceRenderElement,
            utils::RescaleRenderElement,
        },
        gles::{GlesRenderer, Uniform, UniformName, UniformType, element::PixelShaderElement},
    },
    desktop::{Space, Window, layer_map_for_output},
    output::Output,
    utils::{Logical, Point, Rectangle, Scale},
    wayland::shell::wlr_layer::Layer,
};

use crate::{canvas::Canvas, config::Config};

/// Grid dots are spaced further apart (doubled) zoomed out so far that dots would be closer than this on screen.
const GRID_MIN_SCREEN_SPACING: f64 = 40.0;
/// Size of a grid dot in pixels.
const GRID_DOT_SIZE: f32 = 2.0;

/// Thickness of outlines in pixels.
pub const OUTLINE_WIDTH: i32 = 4;

render_elements! {
    pub DeskElement<=GlesRenderer>;
    Surface = WaylandSurfaceRenderElement<GlesRenderer>,
    Zoomed = RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>,
    Grid = PixelShaderElement,
    Solid = SolidColorRenderElement,
    Cursor = MemoryRenderBufferRenderElement<GlesRenderer>,
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

/// The dot grid on the canvas, so panning is visible even on empty space. The element is kept
/// from frame to frame and only changes with the view or the settings, so a still screen is not
/// redrawn.
pub struct Grid {
    element: PixelShaderElement,
    /// Spacing, offset and color the shader's uniforms were last set from.
    uniforms: Option<[f32; 7]>,
}

impl Grid {
    pub fn new(renderer: &mut GlesRenderer) -> Self {
        let uniforms = [
            UniformName::new("spacing", UniformType::_1f),
            UniformName::new("dot_size", UniformType::_1f),
            UniformName::new("offset", UniformType::_2f),
            UniformName::new("color", UniformType::_4f),
        ];
        let shader = (renderer.compile_custom_pixel_shader(GRID_SHADER, &uniforms))
            .expect("grid shader must compile");
        let area = Rectangle::default();
        let element =
            PixelShaderElement::new(shader, area, None, 1.0, Vec::new(), Kind::Unspecified);
        Self {
            element,
            uniforms: None,
        }
    }

    /// The grid for the current view, with `spacing` (canvas pixels) between the dots.
    fn element(
        &mut self,
        canvas: &Canvas,
        mut spacing: f64,
        color: [f32; 4],
    ) -> Option<PixelShaderElement> {
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
        self.element.resize(area, None);
        let [r, g, b, a] = color;
        let now = [spacing as f32, offset.x as f32, offset.y as f32, r, g, b, a];
        if self.uniforms != Some(now) {
            self.uniforms = Some(now);
            self.element.update_uniforms(vec![
                Uniform::new("spacing", now[0]),
                Uniform::new("dot_size", GRID_DOT_SIZE),
                Uniform::new("offset", (now[1], now[2])),
                Uniform::new("color", (r, g, b, a)),
            ]);
        }
        Some(self.element.clone())
    }
}

/// A blue frame around a window (selected, or pulsing when unlocked) or a selection box.
pub struct Outline {
    /// Screen rectangle the frame is drawn around.
    pub rect: Rectangle<i32, Logical>,
    /// Extra distance from the rectangle (negative: drawn inside it).
    pub grow: i32,
    pub alpha: f32,
}

/// The four sides of a frame as screen rectangles with their color.
pub fn outline_bars(
    outline: &Outline,
    color: [f32; 4],
) -> [(Rectangle<i32, Logical>, [f32; 4]); 4] {
    let Outline { rect, grow, alpha } = *outline;
    let color = color.map(|c| c * alpha);
    let o = grow + OUTLINE_WIDTH;
    let (x, y, w, h) = (
        rect.loc.x - o,
        rect.loc.y - o,
        rect.size.w + 2 * o,
        rect.size.h + 2 * o,
    );
    [
        (x, y, w, OUTLINE_WIDTH),
        (x, y + h - OUTLINE_WIDTH, w, OUTLINE_WIDTH),
        (x, y, OUTLINE_WIDTH, h),
        (x + w - OUTLINE_WIDTH, y, OUTLINE_WIDTH, h),
    ]
    .map(|(x, y, w, h)| (Rectangle::new((x, y).into(), (w, h).into()), color))
}

/// Solid rectangles on the screen (frames, lit edges, the template menu). Their buffers are kept
/// from frame to frame, so only bars that changed get redrawn. They go in front of everything.
pub fn bar_elements(
    buffers: &mut Vec<SolidColorBuffer>,
    bars: &[(Rectangle<i32, Logical>, [f32; 4])],
    scale: f64,
) -> Vec<DeskElement> {
    if buffers.len() < bars.len() {
        buffers.resize_with(bars.len(), SolidColorBuffer::default);
    }
    (bars.iter().zip(buffers.iter_mut()))
        .map(|((rect, color), buffer)| {
            buffer.update(rect.size, *color);
            let loc = rect.loc.to_f64().to_physical(scale).to_i32_round();
            let element =
                SolidColorRenderElement::from_buffer(buffer, loc, scale, 1.0, Kind::Unspecified);
            DeskElement::Solid(element)
        })
        .collect()
}

/// Layer surfaces (launchers, bars, wallpapers) sit on the screen and are never zoomed.
fn layer_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    layers: &[Layer],
) -> Vec<WaylandSurfaceRenderElement<GlesRenderer>> {
    let scale = output.current_scale().fractional_scale();
    let map = layer_map_for_output(output);
    map.layers()
        .rev()
        .filter(|l| layers.contains(&l.layer()))
        .filter_map(|l| Some((l, map.layer_geometry(l)?.loc)))
        .flat_map(|(l, loc)| {
            let loc = loc.to_physical_precise_round(scale);
            l.render_elements(renderer, loc, Scale::from(scale), 1.0)
        })
        .collect()
}

/// The elements of a frame behind the bars, front to back: the upper layers, windows, grid,
/// bottom layers.
#[allow(clippy::too_many_arguments)]
pub fn desk_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    space: &Space<Window>,
    canvas: &Canvas,
    grid: &mut Grid,
    config: &Config,
    upper_layers: &[Layer],
    offsets: &[(Window, Point<i32, Logical>)],
) -> Vec<DeskElement> {
    let upper = layer_elements(renderer, output, upper_layers);
    let mut elements: Vec<DeskElement> = upper.into_iter().map(DeskElement::Surface).collect();
    // Windows front to back, gliding ones on their way (`offsets`). Only those inside the
    // viewport are rendered (culling), then shrunk by the zoom.
    let viewport = canvas.viewport().to_i32_round();
    let scale = output.current_scale().fractional_scale();
    let windows = (space.elements().rev())
        .filter_map(|w| {
            let offset = (offsets.iter().find(|(o, _)| o == w)).map_or_default(|(_, p)| *p);
            let bbox = space.element_bbox(w)?;
            let loc = space.element_location(w)? - w.geometry().loc + offset - viewport.loc;
            (viewport.overlaps(Rectangle::new(bbox.loc + offset, bbox.size))).then_some((w, loc))
        })
        .flat_map(|(w, loc)| {
            let loc = loc.to_physical_precise_round(scale);
            w.render_elements::<WaylandSurfaceRenderElement<_>>(renderer, loc, scale.into(), 1.0)
        });
    elements.extend(windows.map(|e| {
        DeskElement::Zoomed(RescaleRenderElement::from_element(
            e,
            (0, 0).into(),
            canvas.zoom(),
        ))
    }));
    let grid = grid.element(canvas, config.grid_spacing, config.grid_color);
    elements.extend(grid.map(DeskElement::Grid));
    let lower = layer_elements(renderer, output, &[Layer::Bottom, Layer::Background]);
    elements.extend(lower.into_iter().map(DeskElement::Surface));
    elements
}
