//! The pointer image, drawn by InfinityDesk itself when it runs standalone (nested, the host
//! draws it). It shows InfinityDesk's own image (resize arrows at window borders) or else what
//! the app under the pointer asks for: a named image from the cursor theme, an image the app
//! draws itself, or none.

use std::collections::HashMap;

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                surface::render_elements_from_surface_tree,
            },
            gles::GlesRenderer,
        },
    },
    input::pointer::{CursorIcon, CursorImageStatus, CursorImageSurfaceData},
    utils::{IsAlive, Logical, Point, Transform},
    wayland::compositor::with_states,
};

use crate::render::DeskElement;

/// Cursor size when `XCURSOR_SIZE` is not set.
const DEFAULT_SIZE: u32 = 24;

/// A picture and its hotspot (the point that clicks).
type Image = (MemoryRenderBuffer, Point<i32, Logical>);

/// Images from the cursor theme (config, else `XCURSOR_THEME`, `XCURSOR_SIZE`), loaded when first
/// needed.
pub struct Cursors {
    theme: xcursor::CursorTheme,
    size: u32,
    /// Per image (`None`: the theme does not have it).
    images: HashMap<CursorIcon, Option<Image>>,
}

impl Cursors {
    pub fn load(theme: Option<&str>, size: Option<u32>) -> Self {
        let name = (theme.map(String::from))
            .or_else(|| std::env::var("XCURSOR_THEME").ok())
            .unwrap_or_else(|| "default".into());
        let size = size.or_else(|| std::env::var("XCURSOR_SIZE").ok()?.parse().ok());
        tracing::info!("Cursor theme {name:?}");
        Self {
            theme: xcursor::CursorTheme::load(&name),
            size: size.unwrap_or(DEFAULT_SIZE),
            images: HashMap::new(),
        }
    }

    /// The picture for `icon`, or the default arrow if the theme does not have it.
    fn image(&mut self, icon: CursorIcon) -> Option<Image> {
        if !self.images.contains_key(&icon) {
            let image = self.read(icon);
            if image.is_none() {
                tracing::warn!("The cursor theme has no {:?} image", icon.name());
            }
            self.images.insert(icon, image);
        }
        match self.images[&icon].clone() {
            None if icon != CursorIcon::Default => self.image(CursorIcon::Default),
            image => image,
        }
    }

    fn read(&self, icon: CursorIcon) -> Option<Image> {
        let path = (std::iter::once(icon.name()).chain(icon.alt_names().iter().copied()))
            .find_map(|name| self.theme.load_icon(name))?;
        let images = xcursor::parser::parse_xcursor(&std::fs::read(path).ok()?)?;
        // The size closest to the wanted one; of animated cursors, the first picture.
        let nearest = (images.iter()).min_by_key(|i| (i.size as i64 - self.size as i64).abs())?;
        let image = images.iter().find(|i| i.size == nearest.size)?;
        let buffer = MemoryRenderBuffer::from_slice(
            &image.pixels_rgba,
            Fourcc::Argb8888,
            (image.width as i32, image.height as i32),
            1,
            Transform::Normal,
            None,
        );
        Some((buffer, (image.xhot as i32, image.yhot as i32).into()))
    }

    /// The pointer at `pos` (screen coordinates): `ours` if InfinityDesk shows something
    /// itself, otherwise what the app asks for in `status`.
    pub fn elements(
        &mut self,
        renderer: &mut GlesRenderer,
        pos: Point<f64, Logical>,
        ours: CursorIcon,
        status: &CursorImageStatus,
    ) -> Vec<DeskElement> {
        let icon = match status {
            _ if ours != CursorIcon::Default => ours,
            CursorImageStatus::Hidden => return Vec::new(),
            CursorImageStatus::Named(icon) => *icon,
            CursorImageStatus::Surface(surface) if surface.alive() => {
                let hotspot = with_states(surface, |states| {
                    let data = states.data_map.get::<CursorImageSurfaceData>();
                    data.map(|d| d.lock().unwrap().hotspot).unwrap_or_default()
                });
                let loc = (pos - hotspot.to_f64()).to_physical(1.0).to_i32_round();
                return render_elements_from_surface_tree(
                    renderer,
                    surface,
                    loc,
                    1.0,
                    1.0,
                    Kind::Cursor,
                );
            }
            CursorImageStatus::Surface(_) => CursorIcon::Default,
        };
        let Some((buffer, hotspot)) = self.image(icon) else {
            return Vec::new();
        };
        let loc = (pos - hotspot.to_f64()).to_physical(1.0);
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            loc,
            &buffer,
            None,
            None,
            None,
            Kind::Cursor,
        )
        .map(|e| vec![DeskElement::Cursor(e)])
        .unwrap_or_default()
    }
}
