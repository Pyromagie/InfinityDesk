//! Getting around with the keyboard: focusing, swapping and resizing windows by direction,
//! moving and zooming the view, and the overview.
//!
//! Which window lies in a direction is pure geometry (`next_in`, tested below).

use smithay::{
    backend::input::InputTime,
    desktop::Window,
    utils::{Logical, Point},
};

use crate::{
    InfinityDesk,
    canvas::Canvas,
    cluster::{Rect, TOUCH_TOLERANCE},
    resize::{self, Edge},
};

/// Keyboard panning moves the view by this part of the screen.
const PAN_STEP: f64 = 4.0;
/// Zoom change per key press.
const ZOOM_STEP: f64 = 1.25;
/// Space around all windows in the overview, as a part of their size.
const OVERVIEW_MARGIN: i32 = 20;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl Dir {
    pub const ALL: [Dir; 4] = [Dir::Left, Dir::Right, Dir::Up, Dir::Down];

    /// The xkb name of its arrow key, also its name in the config (any case).
    pub fn key(self) -> &'static str {
        match self {
            Dir::Left => "Left",
            Dir::Right => "Right",
            Dir::Up => "Up",
            Dir::Down => "Down",
        }
    }

    pub fn parse(text: &str) -> Option<Dir> {
        Dir::ALL
            .into_iter()
            .find(|d| d.key().eq_ignore_ascii_case(text))
    }

    /// The axis (0 = x, 1 = y), and whether the direction points to larger coordinates.
    fn axis(self) -> (usize, bool) {
        match self {
            Dir::Left => (0, false),
            Dir::Right => (0, true),
            Dir::Up => (1, false),
            Dir::Down => (1, true),
        }
    }
}

/// Start and end of a rectangle along an axis.
fn span(r: &Rect, axis: usize) -> (i32, i32) {
    match axis {
        0 => (r.loc.x, r.loc.x + r.size.w),
        _ => (r.loc.y, r.loc.y + r.size.h),
    }
}

/// The window next to `from` in direction `dir` (an index into `others`): of the windows lying
/// completely beyond that edge, the closest one, where lying off to the side counts double.
/// On a tie, the one whose middle is closest across the direction wins.
pub fn next_in(dir: Dir, from: &Rect, others: &[Rect]) -> Option<usize> {
    let (axis, forward) = dir.axis();
    let (f_start, f_end) = span(from, axis);
    let (p_start, p_end) = span(from, 1 - axis);
    (others.iter().enumerate())
        .filter_map(|(i, r)| {
            let (start, end) = span(r, axis);
            let gap = if forward {
                start - f_end
            } else {
                f_start - end
            };
            if gap < -TOUCH_TOLERANCE {
                return None;
            }
            let (q_start, q_end) = span(r, 1 - axis);
            let side = (q_start - p_end).max(p_start - q_end).max(0);
            let middle = ((q_start + q_end) - (p_start + p_end)).abs();
            Some(((gap.max(0) + 2 * side, middle), i))
        })
        .min()
        .map(|(_, i)| i)
}

/// The overview (Mod+Tab) is open.
pub struct Overview {
    /// The view before (viewport position and zoom), to go back to.
    view: (Point<f64, Logical>, f64),
    /// The active window when it was opened; if another one gets chosen, the view goes there.
    active: Option<Window>,
}

impl InfinityDesk {
    /// The windows of `window`'s cluster and where they are (or are about to be).
    pub fn cluster_rects(&self, window: &Window) -> (Vec<Window>, Vec<Rect>) {
        (self.clusters.members(window).into_iter())
            .filter_map(|w| Some((w.clone(), self.target_rect(&w)?)))
            .unzip()
    }

    /// Mod+arrow: focuses the next window in `dir`, also in another cluster; the view follows.
    /// With no active window: the one closest to the middle of the screen.
    pub fn focus_in(&mut self, dir: Dir) {
        let (windows, rects): (Vec<Window>, Vec<Rect>) = (self.space.elements())
            .filter_map(|w| Some((w.clone(), self.target_rect(w)?)))
            .unzip();
        let from = self.active.as_ref().and_then(|a| self.target_rect(a));
        let next = match from {
            Some(from) => next_in(dir, &from, &rects),
            None => {
                let c = self.monitor.canvas.viewport_center().to_i32_round();
                (0..rects.len()).min_by_key(|&i| {
                    let d = rects[i].loc + rects[i].size.downscale(2).to_point() - c;
                    d.x as i64 * d.x as i64 + d.y as i64 * d.y as i64
                })
            }
        };
        if let Some(i) = next {
            self.focus_window(&windows[i]);
            self.reveal(rects[i]);
        }
    }

    /// Mod+Ctrl+arrow: the active window swaps places with its neighbor in the cluster (each
    /// takes the other's place and size, and its docking links).
    pub fn swap_in(&mut self, dir: Dir) {
        let Some(active) = self.active.clone() else {
            return;
        };
        let (windows, rects) = self.cluster_rects(&active);
        let Some(a) = windows.iter().position(|w| w == &active) else {
            return;
        };
        let others: Vec<usize> = (0..rects.len()).filter(|&i| i != a).collect();
        let other_rects: Vec<Rect> = others.iter().map(|&i| rects[i]).collect();
        let Some(b) = next_in(dir, &rects[a], &other_rects).map(|k| others[k]) else {
            return;
        };
        self.clusters.swap(&windows[a], &windows[b]);
        self.swap_slot_anchor(&windows[a], &windows[b]);
        self.resize_to(&windows[a], rects[b], None);
        self.resize_to(&windows[b], rects[a], None);
        self.focus_window(&active);
        self.reveal(rects[b]);
    }

    /// Mod+Shift+arrow: the right (bottom) edge of the active window moves towards the arrow, so
    /// right/down make a half window full, left/up a full window half. Neighbors follow like
    /// when dragging that edge.
    pub fn resize_in(&mut self, dir: Dir) {
        let Some(active) = self.active.clone() else {
            return;
        };
        let (axis, forward) = dir.axis();
        let mut edges = [Edge::None; 2];
        edges[axis] = Edge::End;
        let toggle = self.plan_toggle(&active, edges, false);
        // Which way the right (bottom) edge goes, also if the left (top) one moves instead.
        let delta = [toggle.delta.x, toggle.delta.y][axis] * toggle.sign[axis];
        if delta == 0 || (delta > 0) != forward {
            return;
        }
        // The view shows where the window ends up.
        let end = resize::apply(&toggle.rects, &toggle.follow, &toggle.min, toggle.delta);
        let i = toggle.windows.iter().position(|w| *w == active);
        if let Some(&rect) = i.and_then(|i| end.get(i)) {
            self.reveal(rect);
        }
        let to = toggle.delta;
        self.glide_resize(toggle, Point::default(), to);
    }

    /// Mod+Ctrl+Shift+arrow: moves the view by a quarter of the screen.
    pub fn pan(&mut self, dir: Dir) {
        let (axis, forward) = dir.axis();
        let screen = self.monitor.canvas.screen_size();
        let step = [screen.w, screen.h][axis] as f64 / PAN_STEP;
        self.glide_view(|c| {
            let mut d = [0.0, 0.0];
            d[axis] = if forward { step } else { -step } / c.zoom();
            c.set_viewport_loc(c.viewport_loc() + Point::from((d[0], d[1])));
        });
    }

    /// Mod+Plus / Mod+Minus: zooms around the middle of the screen.
    pub fn zoom_by(&mut self, zoom_in: bool) {
        let screen = self.monitor.canvas.screen_size().to_f64();
        let middle = Point::from((screen.w / 2.0, screen.h / 2.0));
        let factor = if zoom_in { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
        self.glide_view(|c| c.zoom_at(middle, factor));
    }

    /// After panning by hand: the view snaps into the grid of a window on the screen if it is
    /// almost there (see `Canvas::snap_to_grid`). Dialogs float freely and do not count.
    pub fn snap_view(&mut self) {
        if !self.config.snap_view {
            return;
        }
        let viewport = self.monitor.canvas.viewport().to_i32_round();
        let rects: Vec<Rect> = (self.space.elements())
            .filter(|w| w.toplevel().is_some_and(|t| t.parent().is_none()))
            .filter_map(|w| self.space.element_geometry(w))
            .filter(|r| r.overlaps(viewport))
            .collect();
        let gaps = self.config.gaps;
        self.glide_view(|c| c.snap_to_grid(&rects, gaps));
    }

    /// Mod+C: the active window's cluster goes to the middle of the screen.
    pub fn center_cluster(&mut self) {
        let Some(active) = self.active.clone() else {
            return;
        };
        let (_, rects) = self.cluster_rects(&active);
        if let Some(bbox) = rects.into_iter().reduce(|a, b| a.merge(b)) {
            let middle = bbox.loc + bbox.size.downscale(2).to_point();
            self.glide_view(|c| c.center_viewport_on(middle.to_f64()));
        }
    }

    /// Mod+Tab: zooms out until all windows fit on the screen. Again: back to the view before,
    /// or, if another window was chosen meanwhile (click or Mod+arrow), to that window's cluster.
    pub fn toggle_overview(&mut self) {
        if let Some(overview) = self.monitor.overview.take() {
            self.emit("view.overview", &[], Some(false));
            match self
                .active
                .clone()
                .filter(|a| overview.active.as_ref() != Some(a))
            {
                Some(chosen) => self.show_cluster(&chosen),
                None => self.glide_view(|c| c.set_view(overview.view.0, overview.view.1)),
            }
            return;
        }
        let all = (self.space.elements())
            .filter_map(|w| self.space.element_geometry(w))
            .reduce(|a, b| a.merge(b));
        let area = all.unwrap_or_else(|| Rect::from_size(self.monitor.canvas.size()));
        let margin = (area.size.w / OVERVIEW_MARGIN, area.size.h / OVERVIEW_MARGIN);
        let margin = margin.0.max(margin.1);
        self.emit("view.overview", &[], Some(true));
        self.monitor.overview = Some(Overview {
            view: self.monitor.canvas.target(),
            active: self.active.clone(),
        });
        let area = Rect::new(
            area.loc - Point::from((margin, margin)),
            area.size + (2 * margin, 2 * margin).into(),
        );
        self.glide_view(|c| c.show(area));
    }

    /// Alt+Tab: goes one window further back in the order they were used; the view follows.
    /// The choice is made when the modifiers are released (see `end_switching`).
    pub fn switch_recent(&mut self) {
        if self.recent.len() < 2 {
            return;
        }
        let next = self.switching.map_or(1, |i| (i + 1) % self.recent.len());
        self.switching = Some(next);
        let window = self.recent[next].clone();
        self.focus_window(&window);
        if let Some(rect) = self.target_rect(&window) {
            self.reveal(rect);
        }
    }

    /// The modifiers of Alt+Tab were released: the window shown now becomes the most recent.
    pub fn end_switching(&mut self) {
        if let Some(i) = self.switching.take() {
            let window = self.recent.remove(i);
            self.recent.insert(0, window);
        }
    }

    /// A window is gone: out of the Alt+Tab order (a running switch starts over).
    pub fn forget_recent(&mut self, window: &Window) {
        if self.recent.contains(window) {
            self.recent.retain(|w| w != window);
            self.switching = None;
        }
    }

    /// A window was clicked in the overview: it gets focused and the overview closes there.
    pub fn choose_in_overview(&mut self, window: &Window) {
        if self.monitor.overview.take().is_some() {
            self.emit("view.overview", &[], Some(false));
        }
        self.focus_window(window);
        self.show_cluster(window);
    }

    /// Shows a window's cluster in the middle of the screen, at 1:1 if it fits.
    fn show_cluster(&mut self, window: &Window) {
        let (_, rects) = self.cluster_rects(window);
        if let Some(bbox) = rects.into_iter().reduce(|a, b| a.merge(b)) {
            self.glide_view(|c| c.show(bbox));
        }
    }

    /// Changes the view with `change` and lets it glide there (see `Canvas::glide`).
    pub fn glide_view(&mut self, change: impl FnOnce(&mut Canvas)) {
        self.monitor.canvas.glide(self.config.animation, change);
        self.apply_viewport();
    }

    /// Focuses a window and shows it.
    pub fn go_to_window(&mut self, window: &Window) {
        let rect = self.target_rect(window);
        if let Some(rect) = rect {
            self.go_to_monitor_showing(rect);
        }
        if (self.monitor.fullscreen.as_ref()).is_some_and(|f| &f.window != window) {
            self.end_fullscreen();
        }
        self.focus_window(window);
        if let Some(rect) = rect {
            self.reveal(rect);
        }
    }

    /// The view glides just far enough that `rect` is on the screen (see `Canvas::reveal`). Is
    /// it on another monitor already, that one becomes the current one instead.
    pub fn reveal(&mut self, rect: Rect) {
        self.go_to_monitor_showing(rect);
        let gaps = self.config.gaps;
        self.glide_view(|c| c.reveal(rect, gaps));
    }

    /// Before each frame: gliding views move on (the pointer stays put on the screen), and
    /// windows that have arrived stop gliding.
    pub fn step_animations(&mut self) {
        let duration = self.config.animation;
        self.window_glides
            .retain(|_, (_, start)| start.elapsed() < duration);
        self.step_resize_glide();
        let mut current_moved = false;
        for i in 0..self.monitor.list.len() {
            let moved = self.monitor.list[i].canvas.step();
            if moved {
                self.on_monitor(i, |state| state.apply_viewport());
            }
            current_moved |= moved && i == self.monitor.current();
        }
        if current_moved {
            let screen = self.pointer_screen;
            self.pointer_moved(
                self.monitor.canvas.screen_to_canvas(screen),
                InputTime::now(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn next_window_in_a_row() {
        let row = [
            rect(0, 0, 100, 100),
            rect(100, 0, 100, 100),
            rect(200, 0, 100, 100),
        ];
        assert_eq!(next_in(Dir::Right, &row[1], &row), Some(2));
        assert_eq!(next_in(Dir::Left, &row[1], &row), Some(0));
        assert_eq!(next_in(Dir::Right, &row[2], &row), None);
        assert_eq!(next_in(Dir::Up, &row[1], &row), None);
    }

    #[test]
    fn next_window_in_an_l_shape() {
        // A tall window on the left, two stacked on the right.
        let l = [
            rect(0, 0, 100, 200),
            rect(100, 0, 100, 100),
            rect(100, 100, 100, 100),
        ];
        assert_eq!(next_in(Dir::Left, &l[2], &l), Some(0));
        assert_eq!(next_in(Dir::Down, &l[1], &l), Some(2));
        assert_eq!(next_in(Dir::Up, &l[2], &l), Some(1));
        // Both right windows are equally close: the first one.
        assert_eq!(next_in(Dir::Right, &l[0], &l), Some(1));
        // Up and down from the tall window: nothing.
        assert_eq!(next_in(Dir::Down, &l[0], &l), None);
    }

    #[test]
    fn windows_straight_ahead_win_over_closer_ones_off_to_the_side() {
        let from = rect(0, 0, 100, 100);
        let others = [rect(150, 300, 100, 100), rect(500, 50, 100, 100)];
        assert_eq!(next_in(Dir::Right, &from, &others), Some(1));
        // Much closer, only a bit off to the side: that one.
        let others = [rect(120, 110, 100, 100), rect(900, 0, 100, 100)];
        assert_eq!(next_in(Dir::Right, &from, &others), Some(0));
    }

    #[test]
    fn overlapping_windows_are_not_beyond_the_edge() {
        let from = rect(0, 0, 100, 100);
        let others = [rect(50, 0, 100, 100), rect(99, 0, 100, 100)];
        // The one overlapping by 1 px still counts as docked next to it.
        assert_eq!(next_in(Dir::Right, &from, &others), Some(1));
    }

    #[test]
    fn directions_are_named_like_the_arrow_keys() {
        assert_eq!(Dir::parse("left"), Some(Dir::Left));
        assert_eq!(Dir::parse("DOWN"), Some(Dir::Down));
        assert_eq!(Dir::parse("forward"), None);
    }
}
