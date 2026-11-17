//! Arranging windows on the screen: fullscreen (Alt+F) and layout templates (Alt+W), plus the
//! template menu. The template geometry itself lives in `layout.rs`.

use smithay::{
    desktop::Window,
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{IsAlive, Logical, Point, Size},
};

use crate::{
    InfinityDesk,
    cluster::{Rect, TOUCH_TOLERANCE, dock_spot, touching},
    layout,
    state::Resizing,
};

const MENU_COLOR: [f32; 4] = [0.15, 0.15, 0.18, 1.0];
const SLOT_COLOR: [f32; 4] = [0.33, 0.33, 0.39, 1.0];
/// Templates with fewer slots than the cluster has windows cannot be chosen.
const SLOT_DISABLED_COLOR: [f32; 4] = [0.2, 0.2, 0.23, 1.0];

/// A fullscreen window and what to restore when it leaves fullscreen.
pub struct Fullscreen {
    pub window: Window,
    /// Its place on the canvas before.
    rect: Rect,
    /// The view before (viewport position and zoom).
    view: (Point<f64, Logical>, f64),
}

/// The open template menu.
pub struct LayoutMenu {
    /// The active window; its cluster gets arranged.
    pub window: Window,
    /// Template and slot under the pointer.
    pub hover: Option<(usize, usize)>,
}

/// Slots of the last applied template that no window took. New windows fill them in order.
pub struct FreeSlots {
    /// A window of the template and where its slot was, so the slots move along with the cluster.
    anchor: Window,
    anchor_loc: Point<i32, Logical>,
    slots: Vec<Rect>,
}

impl InfinityDesk {
    /// Alt+F: the window covers the screen, on top of everything; again: back to its place.
    pub fn toggle_fullscreen(&mut self, window: &Window) {
        let same = self
            .fullscreen
            .as_ref()
            .is_some_and(|f| &f.window == window);
        self.end_fullscreen();
        if !same {
            self.start_fullscreen(window);
        }
    }

    /// The window takes the size of the screen and the view goes there at 1:1. The canvas
    /// underneath stays as it is; the window's neighbors are just covered meanwhile.
    pub fn start_fullscreen(&mut self, window: &Window) {
        let Some(rect) = self.space.element_geometry(window) else {
            return;
        };
        let view = (self.canvas.viewport_loc(), self.canvas.zoom());
        self.fullscreen = Some(Fullscreen {
            window: window.clone(),
            rect,
            view,
        });
        self.fit_fullscreen();
        self.focus_window(window);
    }

    /// Sizes the fullscreen window to the screen (also when the screen changes size).
    pub fn fit_fullscreen(&mut self) {
        let Some(window) = self.fullscreen.as_ref().map(|f| f.window.clone()) else {
            return;
        };
        let Some(rect) = self.space.element_geometry(&window) else {
            return;
        };
        let screen = self.canvas.screen_size();
        self.canvas.focus_on(Rect::new(rect.loc, screen));
        self.apply_viewport();
        let target = Rect::new(self.canvas.viewport_loc().to_i32_round(), screen);
        self.resize_to(&window, target, Some(true));
    }

    /// Puts the fullscreen window (if any) back to its place and restores the view.
    pub fn end_fullscreen(&mut self) {
        let Some(Fullscreen { window, rect, view }) = self.fullscreen.take() else {
            return;
        };
        self.canvas.set_view(view.0, view.1);
        self.apply_viewport();
        if window.alive() {
            self.resize_to(&window, rect, Some(false));
        }
    }

    /// Asks a window for a new size and puts it at `rect` (see `place_resized`).
    pub fn resize_to(&mut self, window: &Window, rect: Rect, fullscreen: Option<bool>) {
        let anchor_end = [false; 2];
        self.resizing
            .insert(window.clone(), Resizing { rect, anchor_end });
        let toplevel = window.toplevel().unwrap();
        toplevel.with_pending_state(|s| {
            s.size = Some(rect.size);
            match fullscreen {
                Some(true) => s.states.set(xdg_toplevel::State::Fullscreen),
                Some(false) => s.states.unset(xdg_toplevel::State::Fullscreen),
                None => false,
            };
        });
        toplevel.send_pending_configure();
        self.place_resized(window);
    }

    /// Alt+W: opens the template menu for the cluster of `window`, or closes it.
    pub fn toggle_layout_menu(&mut self, window: Option<Window>) {
        if self.layout_menu.take().is_some() {
            return;
        }
        self.end_fullscreen();
        self.layout_menu = window.map(|window| LayoutMenu {
            window,
            hover: None,
        });
    }

    /// Number of windows in the cluster the menu was opened for.
    fn menu_cluster_len(&self) -> usize {
        (self.layout_menu.as_ref()).map_or(0, |m| self.clusters.members(&m.window).len())
    }

    /// Chooses a template in the open menu, with the active window into `slot` (or wherever it
    /// fits best). Templates with too few slots do nothing.
    pub fn choose_template(&mut self, template: usize, slot: Option<usize>) {
        let enough = layout::TEMPLATES[template].len() >= self.menu_cluster_len();
        if let Some(menu) = self.layout_menu.take_if(|_| enough) {
            self.apply_template(&menu.window, template, slot);
        }
    }

    /// Arranges the cluster of `window` in `template` so that it fills exactly one screen,
    /// starting at the cluster's top-left corner, and shows it at 1:1.
    fn apply_template(&mut self, window: &Window, template: usize, slot: Option<usize>) {
        let (windows, rects): (Vec<Window>, Vec<Rect>) = (self.clusters.members(window))
            .into_iter()
            .filter_map(|w| Some((w.clone(), self.space.element_geometry(&w)?)))
            .unzip();
        let Some(bbox) = rects.iter().copied().reduce(|a, b| a.merge(b)) else {
            return;
        };
        let screen = self.canvas.screen_size();
        let area = Rect::new(self.canvas.clamp_window_loc(bbox.loc, screen), screen);
        let slots = layout::slots(template, area);
        let active = windows.iter().position(|w| w == window).unwrap_or(0);
        let slot_of = layout::assign(&rects, active, slot, &slots);

        // The cluster is docked anew: windows whose slots touch.
        for w in &windows {
            self.clusters.remove(w);
        }
        for (i, a) in windows.iter().enumerate() {
            for (j, b) in windows.iter().enumerate().skip(i + 1) {
                if touching(&slots[slot_of[i]], &slots[slot_of[j]]) {
                    self.clusters.join(a, b);
                }
            }
        }
        for (w, &s) in windows.iter().zip(&slot_of) {
            self.resize_to(w, slots[s], None);
        }
        self.free_slots = Some(FreeSlots {
            anchor: window.clone(),
            anchor_loc: slots[slot_of[active]].loc,
            slots: (slots.iter().enumerate())
                .filter(|(s, _)| !slot_of.contains(s))
                .map(|(_, r)| *r)
                .collect(),
        });
        self.canvas.focus_on(area);
        self.apply_viewport();
        self.focus_window(window);
    }

    /// A free template slot for a new window: the first one that is on the screen. Slots that
    /// another window covers by now are dropped.
    pub fn take_free_slot(&mut self) -> Option<Rect> {
        let free = self.free_slots.as_mut()?;
        let Some(anchor) = self.space.element_geometry(&free.anchor) else {
            self.free_slots = None;
            return None;
        };
        // Windows being resized count with the size they are about to get.
        let target = |w: &Window| {
            (self.resizing.get(w).map(|r| r.rect)).or_else(|| self.space.element_geometry(w))
        };
        let anchor = target(&free.anchor).unwrap_or(anchor);
        let shift = anchor.loc - free.anchor_loc;
        let covered = |slot: &Rect| {
            (self.space.elements().filter_map(target)).any(|r| {
                r.intersection(*slot)
                    .is_some_and(|i| i.size.w > TOUCH_TOLERANCE && i.size.h > TOUCH_TOLERANCE)
            })
        };
        let viewport = self.canvas.viewport().to_i32_round();
        free.slots
            .retain(|r| !covered(&Rect::new(r.loc + shift, r.size)));
        let i = (free.slots.iter())
            .position(|r| viewport.contains_rect(Rect::new(r.loc + shift, r.size)))?;
        let slot = free.slots.remove(i);
        Some(Rect::new(slot.loc + shift, slot.size))
    }

    /// The window that free slots move along with is gone: they are forgotten.
    pub fn forget_slot_anchor(&mut self, window: &Window) {
        if self
            .free_slots
            .as_ref()
            .is_some_and(|f| &f.anchor == window)
        {
            self.free_slots = None;
        }
    }

    /// Where a new window docks next to the active window (see `cluster::dock_spot`). Dialogs
    /// stay free, in the middle of the screen.
    pub fn spot_next_to_active(&self, window: &Window, size: Size<i32, Logical>) -> Option<Rect> {
        if window.toplevel().unwrap().parent().is_some() {
            return None;
        }
        let active = self.target_rect(self.active.as_ref()?)?;
        let others: Vec<Rect> = (self.space.elements())
            .filter_map(|w| self.target_rect(w))
            .collect();
        let canvas = Rect::from_size(self.canvas.size());
        let visible = self.canvas.viewport().to_i32_round();
        let match_size = self.config.match_size;
        dock_spot(&active, size, &others, &visible, &canvas, match_size)
    }

    /// Where a window is, or is about to be while it changes its size.
    fn target_rect(&self, window: &Window) -> Option<Rect> {
        (self.resizing.get(window).map(|r| r.rect)).or_else(|| self.space.element_geometry(window))
    }

    /// A new window that got a template slot or a spot next to the active window: docks to
    /// the windows around it.
    pub fn dock_into_slot(&mut self, window: &Window, slot: Rect) {
        let neighbors: Vec<Window> = (self.space.elements())
            .filter(|w| *w != window)
            .filter(|w| (self.space.element_geometry(w)).is_some_and(|r| touching(&r, &slot)))
            .cloned()
            .collect();
        for n in neighbors {
            self.clusters.join(window, &n);
        }
    }

    /// Solid rectangles drawn on top of everything (screen coordinates, front to back): the
    /// template menu.
    pub fn menu_bars(&self) -> Vec<(Rect, [f32; 4])> {
        let Some(menu) = &self.layout_menu else {
            return Vec::new();
        };
        let screen = self.canvas.screen_size();
        let needed = self.clusters.members(&menu.window).len();
        let mut bars = Vec::new();
        for (t, slots) in layout::menu_slots(screen).into_iter().enumerate() {
            let enabled = slots.len() >= needed;
            for (s, r) in slots.into_iter().enumerate() {
                let color = match (enabled, menu.hover) {
                    (false, _) => SLOT_DISABLED_COLOR,
                    (true, Some(hover)) if hover == (t, s) => self.config.accent,
                    _ => SLOT_COLOR,
                };
                bars.push((r, color));
            }
        }
        bars.push((layout::menu_panel(screen), MENU_COLOR));
        bars
    }
}
