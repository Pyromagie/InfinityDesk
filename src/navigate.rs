//! Getting around with the keyboard: focusing, swapping and resizing windows by direction,
//! moving and zooming the view, and the overview.
//!
//! Which window lies in a direction is pure geometry (`next_in`, tested below).

use smithay::{
    desktop::Window,
    utils::{Logical, Point},
};

use crate::{
    InfinityDesk,
    cluster::{Rect, TOUCH_TOLERANCE, touching},
    grabs::min_size,
    resize::{self, Edge},
};

/// Keyboard resizing changes a size by this part of the screen.
const RESIZE_STEP: i32 = 10;
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
    fn cluster_rects(&self, window: &Window) -> (Vec<Window>, Vec<Rect>) {
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
                let c = self.canvas.viewport_center().to_i32_round();
                (0..rects.len()).min_by_key(|&i| {
                    let d = rects[i].loc + rects[i].size.downscale(2).to_point() - c;
                    d.x as i64 * d.x as i64 + d.y as i64 * d.y as i64
                })
            }
        };
        if let Some(i) = next {
            self.focus_window(&windows[i]);
            self.canvas.reveal(rects[i]);
            self.apply_viewport();
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
        self.canvas.reveal(rects[b]);
        self.apply_viewport();
    }

    /// Mod+Shift+arrow: right/up makes the active window wider/taller, left/down narrower/lower,
    /// by a tenth of the screen; neighbors follow like when dragging the edge. The right (bottom)
    /// edge moves, unless only the left (top) one has a neighbor: then that one, so a row that
    /// fills the screen keeps filling it.
    pub fn resize_in(&mut self, dir: Dir) {
        let Some(active) = self.active.clone() else {
            return;
        };
        let (windows, rects) = self.cluster_rects(&active);
        let Some(i) = windows.iter().position(|w| w == &active) else {
            return;
        };
        // Right and up make it bigger, left and down smaller.
        let (axis, forward) = dir.axis();
        let grow = if axis == 0 { forward } else { !forward };
        let r = rects[i];
        let (start, end) = span(&r, axis);
        let neighbor_at = |line: i32, side: fn((i32, i32)) -> i32| {
            (rects.iter().enumerate()).any(|(j, o)| {
                j != i && touching(&r, o) && (side(span(o, axis)) - line).abs() <= TOUCH_TOLERANCE
            })
        };
        let only_before = neighbor_at(start, |s| s.1) && !neighbor_at(end, |s| s.0);
        let edge = if only_before { Edge::Start } else { Edge::End };
        let screen = self.canvas.screen_size();
        let step = [screen.w, screen.h][axis] / RESIZE_STEP;
        let step = if grow == (edge == Edge::End) {
            step
        } else {
            -step
        };
        let mut edges = [Edge::None; 2];
        edges[axis] = edge;
        let mut delta = [0, 0];
        delta[axis] = step;
        let follow = resize::plan(&rects, i, edges, false);
        let min: Vec<_> = windows.iter().map(min_size).collect();
        let new = resize::apply(&rects, &follow, &min, (delta[0], delta[1]).into());
        let rect = new[i];
        self.resize_windows(&windows, &follow, new);
        self.canvas.reveal(rect);
        self.apply_viewport();
    }

    /// Mod+Ctrl+Shift+arrow: moves the view by a quarter of the screen.
    pub fn pan(&mut self, dir: Dir) {
        let (axis, forward) = dir.axis();
        let screen = self.canvas.screen_size();
        let step = [screen.w, screen.h][axis] as f64 / PAN_STEP / self.canvas.zoom();
        let mut d = [0.0, 0.0];
        d[axis] = if forward { step } else { -step };
        let loc = self.canvas.viewport_loc() + Point::from((d[0], d[1]));
        self.canvas.set_viewport_loc(loc);
        self.apply_viewport();
    }

    /// Mod+Plus / Mod+Minus: zooms around the middle of the screen.
    pub fn zoom_by(&mut self, zoom_in: bool) {
        let screen = self.canvas.screen_size().to_f64();
        let middle = Point::from((screen.w / 2.0, screen.h / 2.0));
        let factor = if zoom_in { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
        self.canvas.zoom_at(middle, factor);
        self.apply_viewport();
    }

    /// Mod+C: the active window's cluster goes to the middle of the screen.
    pub fn center_cluster(&mut self) {
        let Some(active) = self.active.clone() else {
            return;
        };
        let (_, rects) = self.cluster_rects(&active);
        if let Some(bbox) = rects.into_iter().reduce(|a, b| a.merge(b)) {
            let middle = bbox.loc + bbox.size.downscale(2).to_point();
            self.canvas.center_viewport_on(middle.to_f64());
            self.apply_viewport();
        }
    }

    /// Mod+Tab: zooms out until all windows fit on the screen. Again: back to the view before,
    /// or, if another window was chosen meanwhile (click or Mod+arrow), to that window's cluster.
    pub fn toggle_overview(&mut self) {
        if let Some(overview) = self.overview.take() {
            match self
                .active
                .clone()
                .filter(|a| overview.active.as_ref() != Some(a))
            {
                Some(chosen) => self.show_cluster(&chosen),
                None => self.canvas.set_view(overview.view.0, overview.view.1),
            }
            self.apply_viewport();
            return;
        }
        let all = (self.space.elements())
            .filter_map(|w| self.space.element_geometry(w))
            .reduce(|a, b| a.merge(b));
        let area = all.unwrap_or_else(|| Rect::from_size(self.canvas.size()));
        let margin = (area.size.w / OVERVIEW_MARGIN, area.size.h / OVERVIEW_MARGIN);
        let margin = margin.0.max(margin.1);
        self.overview = Some(Overview {
            view: (self.canvas.viewport_loc(), self.canvas.zoom()),
            active: self.active.clone(),
        });
        self.canvas.show(Rect::new(
            area.loc - Point::from((margin, margin)),
            area.size + (2 * margin, 2 * margin).into(),
        ));
        self.apply_viewport();
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
            self.canvas.reveal(rect);
            self.apply_viewport();
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
        self.overview = None;
        self.focus_window(window);
        self.show_cluster(window);
        self.apply_viewport();
    }

    /// Shows a window's cluster in the middle of the screen, at 1:1 if it fits.
    fn show_cluster(&mut self, window: &Window) {
        let (_, rects) = self.cluster_rects(window);
        if let Some(bbox) = rects.into_iter().reduce(|a, b| a.merge(b)) {
            self.canvas.show(bbox);
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
