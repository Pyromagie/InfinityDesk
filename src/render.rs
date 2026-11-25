//! Assembles what gets drawn each frame: layer surfaces, windows (zoomed, with rounded corners),
//! frames around windows and the canvas dot grid.

use smithay::{
    backend::renderer::{
        Frame,
        element::{
            AsRenderElements, Element, Id, Kind, RenderElement, UnderlyingStorage,
            memory::MemoryRenderBufferRenderElement,
            render_elements,
            solid::{SolidColorBuffer, SolidColorRenderElement},
            surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            utils::RescaleRenderElement,
        },
        gles::{
            GlesError, GlesFrame, GlesPixelProgram, GlesRenderer, GlesTexProgram, Uniform,
            UniformName, UniformType, element::PixelShaderElement,
        },
        utils::{CommitCounter, DamageSet, OpaqueRegions},
    },
    desktop::{PopupManager, Space, Window, layer_map_for_output},
    output::Output,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{
        Buffer, Logical, Physical, Point, Rectangle, Scale, Transform, user_data::UserDataMap,
    },
    wayland::shell::wlr_layer::Layer,
};

use crate::{canvas::Canvas, config::Config};

/// Grid dots are spaced further apart (doubled) zoomed out so far that dots would be closer than this on screen.
const GRID_MIN_SCREEN_SPACING: f64 = 40.0;
/// Size of a grid dot in pixels.
const GRID_DOT_SIZE: f32 = 2.0;

/// Thickness of outlines in pixels.
pub const OUTLINE_WIDTH: i32 = 4;

type Zoomed = RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>;

render_elements! {
    pub DeskElement<=GlesRenderer>;
    Surface = WaylandSurfaceRenderElement<GlesRenderer>,
    Zoomed = Zoomed,
    Rounded = Rounded,
    Shader = PixelShaderElement,
    Solid = SolidColorRenderElement,
    Cursor = MemoryRenderBufferRenderElement<GlesRenderer>,
}

/// Distance of a point `p` (from the center) outside a box with rounded corners (negative inside).
const ROUNDED_BOX: &str = r#"
float rounded_box(vec2 p, vec2 half_size, float radius) {
    vec2 q = abs(p) - half_size + radius;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
}
"#;

/// A frame of `width` pixels along the edge of the area, its outer corners rounded by `radius`.
const FRAME_SHADER: &str = r#"
precision highp float;
varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif
uniform vec4 color;
uniform float width;
uniform float radius;
//_ROUNDED_BOX_

void main() {
    vec2 p = v_coords * size - size / 2.0;
    float outer = rounded_box(p, size / 2.0, radius);
    float inner = rounded_box(p, size / 2.0 - width, max(radius - width, 0.0));
    // Smooth over one pixel at both edges.
    float coverage = clamp(0.5 - outer, 0.0, 1.0) * clamp(0.5 + inner, 0.0, 1.0);
    gl_FragColor = color * alpha * coverage;
}
"#;

/// Smithay's texture shader, but only inside the window's rectangle with rounded corners. The
/// rectangle (`rect`: left, bottom, right, top) is in framebuffer pixels, as `gl_FragCoord`.
const CLIP_SHADER: &str = r#"
#version 100
//_DEFINES_
#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif
precision highp float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif
uniform float alpha;
varying vec2 v_coords;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif
uniform vec4 rect;
uniform float radius;
//_ROUNDED_BOX_

void main() {
    vec4 color = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0) * alpha;
#else
    color = color * alpha;
#endif
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif
    vec2 half_size = (rect.zw - rect.xy) / 2.0;
    float dist = rounded_box(gl_FragCoord.xy - rect.xy - half_size, half_size, radius);
    gl_FragColor = color * clamp(0.5 - dist, 0.0, 1.0);
}
"#;

/// What is kept from frame to frame: the grid, the frames around windows and the shaders.
pub struct Scene {
    grid: Grid,
    frame_shader: GlesPixelProgram,
    /// One element per frame, with the uniforms it was last given (width, radius, color).
    frames: Vec<(PixelShaderElement, [f32; 6])>,
    clip_shader: GlesTexProgram,
}

impl Scene {
    pub fn new(renderer: &mut GlesRenderer) -> Self {
        let frame_uniforms = [
            UniformName::new("color", UniformType::_4f),
            UniformName::new("width", UniformType::_1f),
            UniformName::new("radius", UniformType::_1f),
        ];
        let frame = FRAME_SHADER.replace("//_ROUNDED_BOX_", ROUNDED_BOX);
        let frame_shader = (renderer.compile_custom_pixel_shader(frame, &frame_uniforms))
            .expect("frame shader must compile");
        let clip_uniforms = [
            UniformName::new("rect", UniformType::_4f),
            UniformName::new("radius", UniformType::_1f),
        ];
        let clip = CLIP_SHADER.replace("//_ROUNDED_BOX_", ROUNDED_BOX);
        let clip_shader = (renderer.compile_custom_texture_shader(clip, &clip_uniforms))
            .expect("clip shader must compile");
        Self {
            grid: Grid::new(renderer),
            frame_shader,
            frames: Vec::new(),
            clip_shader,
        }
    }

    /// The element for frame number `i`; it only changes (and is redrawn) when the frame does.
    fn frame(&mut self, i: usize, outline: &Outline) -> PixelShaderElement {
        if self.frames.len() <= i {
            let shader = self.frame_shader.clone();
            let element = PixelShaderElement::new(
                shader,
                Rectangle::default(),
                None,
                1.0,
                Vec::new(),
                Kind::Unspecified,
            );
            self.frames.push((element, [-1.0; 6]));
        }
        let Outline {
            rect,
            grow,
            width,
            radius,
            color,
            ..
        } = *outline;
        let o = grow + width;
        let area = Rectangle::new(
            rect.loc - Point::from((o, o)),
            rect.size + (2 * o, 2 * o).into(),
        );
        // Around rounded corners the frame's outer edge is rounded a bit more, so it keeps its width.
        let radius = if radius > 0.0 { radius + o as f64 } else { 0.0 };
        let [r, g, b, a] = color;
        let now = [width as f32, radius as f32, r, g, b, a];
        let (element, uniforms) = &mut self.frames[i];
        element.resize(area, None);
        if *uniforms != now {
            *uniforms = now;
            element.update_uniforms(vec![
                Uniform::new("color", (r, g, b, a)),
                Uniform::new("width", now[0]),
                Uniform::new("radius", now[1]),
            ]);
        }
        element.clone()
    }
}

/// A window's surfaces, cut to the window's rectangle with rounded corners (shadows a program
/// draws around itself are left out too).
pub struct Rounded {
    inner: Zoomed,
    shader: GlesTexProgram,
    /// The window on the screen, in physical pixels.
    rect: Rectangle<f64, Physical>,
    radius: f64,
}

impl Element for Rounded {
    fn id(&self) -> &Id {
        self.inner.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.inner.location(scale)
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }

    /// The corners show what is behind them, so they are not opaque.
    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        let geo = self.geometry(scale);
        let rect = self.rect.to_i32_round::<i32>();
        let (x, y) = (rect.loc.x - geo.loc.x, rect.loc.y - geo.loc.y);
        let (w, h) = (rect.size.w, rect.size.h);
        let r = (self.radius.ceil() as i32).min(w / 2).min(h / 2);
        // The window without its corners: a column through the middle and the two sides.
        let parts: [Rectangle<i32, Physical>; 3] = [
            Rectangle::new((x + r, y).into(), (w - 2 * r, h).into()),
            Rectangle::new((x, y + r).into(), (r, h - 2 * r).into()),
            Rectangle::new((x + w - r, y + r).into(), (r, h - 2 * r).into()),
        ];
        let opaque = self.inner.opaque_regions(scale);
        (opaque.iter())
            .flat_map(|o| parts.iter().filter_map(|p| o.intersection(*p)))
            .collect()
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl RenderElement<GlesRenderer> for Rounded {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), GlesError> {
        // Where the window lies in the framebuffer: the frame's projection turns screen pixels
        // into GL's -1..1 (this takes care of rotated and flipped screens).
        let p = frame.projection().map(f64::from);
        let size = frame.transformation().transform_size(frame.output_size());
        let to_framebuffer = |x: f64, y: f64| {
            let gl = (p[0] * x + p[3] * y + p[6], p[1] * x + p[4] * y + p[7]);
            (
                (gl.0 + 1.0) / 2.0 * size.w as f64,
                (gl.1 + 1.0) / 2.0 * size.h as f64,
            )
        };
        let (loc, end) = (self.rect.loc, self.rect.loc + self.rect.size);
        let (a, b) = (to_framebuffer(loc.x, loc.y), to_framebuffer(end.x, end.y));
        let rect = (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1));
        let uniforms = vec![
            Uniform::new("rect", rect_uniform(rect)),
            Uniform::new("radius", self.radius as f32),
        ];
        frame.override_default_tex_program(self.shader.clone(), uniforms);
        let drawn = self
            .inner
            .draw(frame, src, dst, damage, opaque_regions, cache);
        frame.clear_tex_program_override();
        drawn
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

fn rect_uniform((a, b, c, d): (f64, f64, f64, f64)) -> (f32, f32, f32, f32) {
    (a as f32, b as f32, c as f32, d as f32)
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

/// A frame around a window (active, selected, or pulsing when unlocked) or a selection box.
pub struct Outline {
    /// Screen rectangle the frame is drawn around.
    pub rect: Rectangle<i32, Logical>,
    /// Extra distance from the rectangle (negative: drawn inside it).
    pub grow: i32,
    pub width: i32,
    /// Of the rectangle's corners on the screen (0: square).
    pub radius: f64,
    /// Premultiplied RGBA.
    pub color: [f32; 4],
    /// The frame lies right behind this window (above the windows behind it); else on top.
    pub window: Option<Window>,
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

/// While the screen is locked: only the lock.
pub fn lock_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    surface: Option<WlSurface>,
) -> Vec<DeskElement> {
    let scale = output.current_scale().fractional_scale();
    let surface = surface.into_iter();
    let elements: Vec<WaylandSurfaceRenderElement<_>> = surface
        .flat_map(|s| {
            let kind = Kind::Unspecified;
            render_elements_from_surface_tree(renderer, &s, (0, 0), scale, 1.0, kind)
        })
        .collect();
    elements.into_iter().map(DeskElement::Surface).collect()
}

/// The elements of a frame behind the bars, front to back: the upper layers, frames without a
/// window, windows (each with its frames), grid, bottom layers. The `fullscreen` window keeps
/// square corners.
#[allow(clippy::too_many_arguments)]
pub fn desk_elements(
    renderer: &mut GlesRenderer,
    output: &Output,
    space: &Space<Window>,
    canvas: &Canvas,
    scene: &mut Scene,
    config: &Config,
    upper_layers: &[Layer],
    offsets: &[(Window, Point<i32, Logical>)],
    outlines: &[Outline],
    fullscreen: Option<&Window>,
) -> Vec<DeskElement> {
    let upper = layer_elements(renderer, output, upper_layers);
    let mut elements: Vec<DeskElement> = upper.into_iter().map(DeskElement::Surface).collect();
    let mut frames: Vec<_> = (outlines.iter().enumerate())
        .map(|(i, o)| (o.window.as_ref(), DeskElement::Shader(scene.frame(i, o))))
        .collect();
    let mut frames_of = |window: Option<&Window>| {
        let (theirs, rest) = frames.drain(..).partition(|(w, _)| *w == window);
        frames = rest;
        theirs.into_iter().map(|(_, e)| e).collect::<Vec<_>>()
    };
    elements.extend(frames_of(None));
    // Windows front to back, gliding ones on their way (`offsets`). Only those inside the
    // viewport are rendered (culling), then shrunk by the zoom.
    let viewport = canvas.viewport().to_i32_round();
    let scale = output.current_scale().fractional_scale();
    let zoom = canvas.zoom();
    let windows = (space.elements().rev()).filter_map(|w| {
        let offset = (offsets.iter().find(|(o, _)| o == w)).map_or_default(|(_, p)| *p);
        let bbox = space.element_bbox(w)?;
        let loc = space.element_location(w)? - w.geometry().loc + offset - viewport.loc;
        (viewport.overlaps(Rectangle::new(bbox.loc + offset, bbox.size))).then_some((w, loc))
    });
    for (w, loc) in windows {
        let zoomed = |e| RescaleRenderElement::from_element(e, (0, 0).into(), zoom);
        let surface = w.toplevel().unwrap().wl_surface();
        let at = loc.to_physical_precise_round(scale);
        // Popups (menus) first, they are not cut.
        for (popup, popup_offset) in PopupManager::popups_for_surface(surface) {
            let offset = w.geometry().loc + popup_offset - popup.geometry().loc;
            let at = at + offset.to_physical_precise_round(scale);
            let popup = render_elements_from_surface_tree(
                renderer,
                popup.wl_surface(),
                at,
                scale,
                1.0,
                Kind::Unspecified,
            );
            elements.extend(popup.into_iter().map(|e| DeskElement::Zoomed(zoomed(e))));
        }
        let surfaces: Vec<WaylandSurfaceRenderElement<_>> =
            render_elements_from_surface_tree(renderer, surface, at, scale, 1.0, Kind::Unspecified);
        let geo = w.geometry();
        let rect = Rectangle::new(
            (loc + geo.loc).to_f64().to_physical(scale).upscale(zoom),
            geo.size.to_f64().to_physical(scale).upscale(zoom),
        );
        let radius = (config.corner_radius as f64 * scale * zoom)
            .min(rect.size.w / 2.0)
            .min(rect.size.h / 2.0);
        if radius > 0.0 && fullscreen != Some(w) {
            elements.extend(surfaces.into_iter().map(|e| {
                DeskElement::Rounded(Rounded {
                    inner: zoomed(e),
                    shader: scene.clip_shader.clone(),
                    rect,
                    radius,
                })
            }));
        } else {
            elements.extend(surfaces.into_iter().map(|e| DeskElement::Zoomed(zoomed(e))));
        }
        elements.extend(frames_of(Some(w)));
    }
    let grid = (scene.grid).element(canvas, config.grid_spacing, config.grid_color);
    elements.extend(grid.map(DeskElement::Shader));
    let lower = layer_elements(renderer, output, &[Layer::Bottom, Layer::Background]);
    elements.extend(lower.into_iter().map(DeskElement::Surface));
    elements
}
