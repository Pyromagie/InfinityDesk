//! Snapping and clusters.
//!
//! A cluster is a group of windows that were docked to each other and move together.
//! Which windows are docked to which is stored explicitly (not derived from geometry), so a
//! cluster does not fall apart just because a client changes its size by a few pixels.
//!
//! This module is pure logic without Wayland: `K` is the window key (`Window` in the compositor,
//! plain numbers in the tests).

use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
};

use smithay::utils::{Logical, Point, Rectangle, Size};

pub type Rect = Rectangle<i32, Logical>;

/// Edges at most this far apart count as "touching" (connected).
pub const TOUCH_TOLERANCE: i32 = 2;
/// Docking also lines up the other edge (tops or left edges), but only if that edge is at most
/// this many times the snap distance away. Otherwise a window would jump far to line up.
const ALIGN_FACTOR: i32 = 4;
/// A window pushed too far, already over its neighbor, still docks at the edge up to this many
/// times the snap distance deep, so docking does not need precise aiming.
const OVERSHOOT_FACTOR: i32 = 4;

/// Which windows are docked to which. A cluster is everything connected through these links;
/// a window without links stands alone.
#[derive(Debug)]
pub struct Clusters<K> {
    links: HashMap<K, HashSet<K>>,
}

impl<K: Eq + Hash + Clone> Default for Clusters<K> {
    fn default() -> Self {
        Self {
            links: HashMap::new(),
        }
    }
}

impl<K: Eq + Hash + Clone> Clusters<K> {
    /// All windows in the same cluster as `k` (including `k`). Standing alone: just `k`.
    pub fn members(&self, k: &K) -> Vec<K> {
        let mut members = vec![k.clone()];
        let mut i = 0;
        while let Some(w) = members.get(i) {
            let new: Vec<K> = (self.links.get(w).into_iter().flatten())
                .filter(|n| !members.contains(n))
                .cloned()
                .collect();
            members.extend(new);
            i += 1;
        }
        members
    }

    /// Docks `a` and `b` to each other, merging their clusters.
    pub fn join(&mut self, a: &K, b: &K) {
        if a != b {
            self.links.entry(a.clone()).or_default().insert(b.clone());
            self.links.entry(b.clone()).or_default().insert(a.clone());
        }
    }

    /// Removes `k` from its cluster. Without it, the rest may fall apart into several clusters.
    pub fn remove(&mut self, k: &K) {
        for n in self.links.get(k).cloned().into_iter().flatten() {
            self.unlink(k, &n);
        }
    }

    /// `a` and `b` trade places: each gets the other's links.
    pub fn swap(&mut self, a: &K, b: &K) {
        let links = |k: &K| self.links.get(k).cloned().unwrap_or_default();
        let (to_a, to_b) = (links(a), links(b));
        self.remove(a);
        self.remove(b);
        let other = |k: &K| match k {
            k if k == a => b.clone(),
            k if k == b => a.clone(),
            k => k.clone(),
        };
        for n in &to_a {
            self.join(b, &other(n));
        }
        for n in &to_b {
            self.join(a, &other(n));
        }
    }

    /// Takes several windows out of their cluster together; among themselves they stay docked.
    pub fn detach(&mut self, group: &[K]) {
        for k in group {
            for n in self.links.get(k).cloned().into_iter().flatten() {
                if !group.contains(&n) {
                    self.unlink(k, &n);
                }
            }
        }
    }

    fn unlink(&mut self, a: &K, b: &K) {
        for (x, y) in [(a, b), (b, a)] {
            if let Some(set) = self.links.get_mut(x) {
                set.remove(y);
                if set.is_empty() {
                    self.links.remove(x);
                }
            }
        }
    }
}

/// Do two windows lie edge to edge, `gap` apart (with real overlap along the edge, not just
/// corner to corner)?
pub fn touching(a: &Rect, b: &Rect, gap: i32) -> bool {
    let (al, ar, at, ab) = edges(a);
    let (bl, br, bt, bb) = edges(b);
    let h_overlap = ar.min(br) - al.max(bl) > 0;
    let v_overlap = ab.min(bb) - at.max(bt) > 0;
    let near = |d: i32| (d - gap).abs() <= TOUCH_TOLERANCE;
    let side_by_side = near(bl - ar) || near(al - br);
    let stacked = near(bt - ab) || near(at - bb);
    (side_by_side && v_overlap) || (stacked && h_overlap)
}

/// Computes the correction by which the moving windows must be shifted to dock to a nearby
/// window, `gap` apart. An edge within `threshold` (canvas pixels) of its spot docks, and the other
/// axis always lines up with that neighbor, so there is a clean corner: side by side the tops or
/// the bottoms are flush, stacked the left or the right edges (whichever pair is closer, so
/// windows of different sizes dock too). A window pushed past the edge, already overlapping its
/// neighbor, docks too (up to `OVERSHOOT_FACTOR` times `threshold` deep). The closest dock wins;
/// ties go to the smallest alignment jump. A spot that is already taken by another window is
/// skipped.
pub fn snap_delta(
    moving: &[Rect],
    others: &[Rect],
    threshold: i32,
    gap: i32,
) -> Point<i32, Logical> {
    // (dock distance, alignment distance, delta)
    let mut best: Option<(i32, i32, Point<i32, Logical>)> = None;
    for m in moving {
        let (ml, mr, mt, mb) = edges(m);
        for o in others {
            let (ol, or, ot, ob) = edges(o);
            // Only windows that really face each other can dock: side by side they must overlap
            // vertically, stacked horizontally. Otherwise the alignment could jump across.
            let v_overlap = mt < ob && ot < mb;
            let h_overlap = ml < or && ol < mr;
            // The way to each docking spot. `apart` is the dock distance with its sign:
            // positive = still apart, negative = already too close (pushed too far).
            let (left, right) = (ol - gap - mr, or + gap - ml);
            let (up, down) = (ot - gap - mb, ob + gap - mt);
            let candidates = [
                (v_overlap, left, left, ot - mt, true), // left of o, tops flush
                (v_overlap, left, left, ob - mb, true), // left of o, bottoms flush
                (v_overlap, right, -right, ot - mt, true), // right of o, tops flush
                (v_overlap, right, -right, ob - mb, true), // right of o, bottoms flush
                (h_overlap, up, up, ol - ml, false),    // above o, left edges flush
                (h_overlap, up, up, or - mr, false),    // above o, right edges flush
                (h_overlap, down, -down, ol - ml, false), // below o, left edges flush
                (h_overlap, down, -down, or - mr, false), // below o, right edges flush
            ];
            for (faces, dock, apart, align, side_by_side) in candidates {
                let score = (dock.abs(), align.abs());
                let delta = if side_by_side {
                    (dock, align)
                } else {
                    (align, dock)
                }
                .into();
                if faces
                    && apart <= threshold
                    && -apart <= threshold * OVERSHOOT_FACTOR
                    && align.abs() <= threshold * ALIGN_FACTOR
                    && best.is_none_or(|(d, a, _)| score < (d, a))
                    && is_free(moving, others, delta)
                {
                    best = Some((score.0, score.1, delta));
                }
            }
        }
    }
    best.map_or((0, 0).into(), |(_, _, delta)| delta)
}

/// Would the moving windows, shifted by `delta`, lie on top of another window?
fn is_free(moving: &[Rect], others: &[Rect], delta: Point<i32, Logical>) -> bool {
    moving.iter().all(|m| {
        others
            .iter()
            .all(|o| !Rect::new(m.loc + delta, m.size).overlaps(*o))
    })
}

/// Where a new window of `size` docks to the active window `a`, `gap` apart: right of it (tops
/// flush), else below (left edges flush), left of it, above. The spot must lie within the canvas
/// and not on another window; spots that are `visible` on the screen come first.
pub fn dock_spot(
    a: &Rect,
    size: Size<i32, Logical>,
    others: &[Rect],
    visible: &Rect,
    canvas: &Rect,
    gap: i32,
) -> Option<Rect> {
    let (al, ar, at, ab) = edges(a);
    let spots = [
        (ar + gap, at),
        (al, ab + gap),
        (al - gap - size.w, at),
        (al, at - gap - size.h),
    ]
    .map(|loc| Rect::new(loc.into(), size));
    let free = |r: &&Rect| canvas.contains_rect(**r) && others.iter().all(|o| !r.overlaps(*o));
    (spots.iter().filter(free))
        .find(|r| visible.contains_rect(**r))
        .or_else(|| spots.iter().find(free))
        .copied()
}

/// (left, right, top, bottom)
fn edges(r: &Rect) -> (i32, i32, i32, i32) {
    (r.loc.x, r.loc.x + r.size.w, r.loc.y, r.loc.y + r.size.h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn swapped_windows_trade_links() {
        // A row 1-2-3, plus 4 below 1. 1 and 3 trade places: 3-2-1, with 4 below 3.
        let mut c = Clusters::default();
        c.join(&1, &2);
        c.join(&2, &3);
        c.join(&1, &4);
        c.swap(&1, &3);
        let linked = |c: &Clusters<i32>, k| {
            let mut v: Vec<i32> = c.links.get(&k).into_iter().flatten().copied().collect();
            v.sort();
            v
        };
        assert_eq!(linked(&c, 3), vec![2, 4]);
        assert_eq!(linked(&c, 1), vec![2]);
        assert_eq!(linked(&c, 2), vec![1, 3]);
        assert_eq!(linked(&c, 4), vec![3]);
        // Neighbors trading places stay linked to each other.
        c.swap(&2, &3);
        assert_eq!(linked(&c, 2), vec![3, 4]);
        assert_eq!(linked(&c, 3), vec![1, 2]);
    }

    #[test]
    fn snaps_to_the_left_and_aligns_top() {
        let other = rect(1000, 500, 400, 300);
        // 10 px gap to the left edge of `other`, 6 px lower.
        let moving = rect(590, 506, 400, 300);
        let d = snap_delta(&[moving], &[other], 15, 0);
        assert_eq!(d, (10, -6).into());
    }

    #[test]
    fn snaps_below() {
        let other = rect(0, 0, 400, 300);
        let moving = rect(3, 312, 400, 300);
        let d = snap_delta(&[moving], &[other], 15, 0);
        assert_eq!(d, (-3, -12).into());
    }

    #[test]
    fn docking_aligns_nearby_edges() {
        // Docks to the right of `other`, 40 px lower: tops are lined up.
        let other = rect(0, 0, 400, 300);
        let moving = rect(405, 40, 400, 500);
        assert_eq!(snap_delta(&[moving], &[other], 15, 0), (-5, -40).into());
    }

    #[test]
    fn taller_window_far_below_does_not_jump_up() {
        // A tall window in the gap between two windows, 200 px lower than them: it must not
        // jump up to line up its top, it simply does not dock.
        let left = rect(0, 0, 400, 300);
        let right = rect(810, 0, 400, 300);
        let moving = rect(405, 200, 400, 600);
        assert_eq!(snap_delta(&[moving], &[left, right], 15, 0), (0, 0).into());
    }

    #[test]
    fn docks_beside_a_column_instead_of_jumping_onto_it() {
        // Column: `top` above `bottom`. A window approaches the column from the right, 20 px away.
        let top = rect(0, 0, 400, 300);
        let bottom = rect(0, 300, 400, 300);
        let moving = rect(420, 5, 400, 300);
        assert_eq!(
            snap_delta(&[moving], &[top, bottom], 27, 0),
            (-20, -5).into()
        );
    }

    #[test]
    fn does_not_dock_onto_an_occupied_spot() {
        // 2×2 grid; a window dragged into the bottom-right one must not snap onto it.
        let grid = [
            rect(0, 0, 400, 300),
            rect(400, 0, 400, 300),
            rect(0, 300, 400, 300),
            rect(400, 300, 400, 300),
        ];
        let moving = rect(410, 310, 400, 300);
        assert_eq!(snap_delta(&[moving], &grid, 15, 0), (0, 0).into());
    }

    #[test]
    fn side_by_side_aligns_bottoms_when_they_are_closer() {
        // Smaller window right of a tall one, near its bottom: bottoms are lined up.
        let other = rect(0, 0, 400, 800);
        let moving = rect(408, 470, 300, 300);
        assert_eq!(snap_delta(&[moving], &[other], 15, 0), (-8, 30).into());
    }

    #[test]
    fn stacked_aligns_right_edges_when_they_are_closer() {
        // Wider window below a narrow one, right edges 30 px apart: right edges are lined up.
        let other = rect(500, 0, 400, 300);
        let moving = rect(70, 310, 800, 300);
        assert_eq!(snap_delta(&[moving], &[other], 15, 0), (30, -10).into());
    }

    #[test]
    fn wide_window_docks_into_the_corner_of_an_l() {
        // The case from the screenshot: a small window left of a tall one; a wide window below
        // the small one docks with its right edge flush, so it also touches the tall window.
        let small = rect(500, 0, 400, 300);
        let tall = rect(900, 0, 400, 700);
        let moving = rect(75, 310, 800, 300);
        let d = snap_delta(&[moving], &[small, tall], 15, 0);
        assert_eq!(d, (25, -10).into());
        let docked = Rect::new(moving.loc + d, moving.size);
        assert!(touching(&docked, &small, 0));
        assert!(touching(&docked, &tall, 0));
    }

    #[test]
    fn overshooting_still_docks() {
        // Pushed 40 px too far into `other` from the left (more than the snap distance):
        // it still docks at the edge.
        let other = rect(1000, 0, 400, 300);
        let moving = rect(640, 0, 400, 300);
        assert_eq!(snap_delta(&[moving], &[other], 15, 0), (-40, 0).into());
        // Pushed 40 px too far up into a window above it.
        let moving = rect(1000, 260, 400, 300);
        assert_eq!(snap_delta(&[moving], &[other], 15, 0), (0, 40).into());
    }

    #[test]
    fn approaching_still_needs_the_snap_distance() {
        // 40 px gap (not overlapping): too far to dock.
        let other = rect(1000, 0, 400, 300);
        assert_eq!(
            snap_delta(&[rect(560, 0, 400, 300)], &[other], 15, 0),
            (0, 0).into()
        );
        // Pushed 70 px deep, beyond 4 × 15: no dock either.
        assert_eq!(
            snap_delta(&[rect(670, 0, 400, 300)], &[other], 15, 0),
            (0, 0).into()
        );
    }

    #[test]
    fn new_window_docks_right_of_the_active_one() {
        let a = rect(1000, 1000, 400, 300);
        let area = rect(0, 0, 3000, 2000);
        let size = (500, 200).into();
        let spot = dock_spot(&a, size, &[a], &area, &area, 0);
        assert_eq!(spot, Some(rect(1400, 1000, 500, 200)));
        // With a gap, it keeps the gap.
        let spot = dock_spot(&a, size, &[a], &area, &area, 16);
        assert_eq!(spot, Some(rect(1416, 1000, 500, 200)));
    }

    #[test]
    fn new_window_takes_the_next_free_side() {
        let a = rect(1000, 1000, 400, 300);
        let size = (500, 200).into();
        // Something on the right: below.
        let right = rect(1500, 900, 300, 300);
        let canvas = rect(0, 0, 3000, 2000);
        let spot = dock_spot(&a, size, &[a, right], &canvas, &canvas, 0);
        assert_eq!(spot, Some(rect(1000, 1300, 500, 200)));
        // Right and below off the screen: left.
        let screen = rect(400, 700, 1000, 600);
        let spot = dock_spot(&a, size, &[a], &screen, &canvas, 0);
        assert_eq!(spot, Some(rect(500, 1000, 500, 200)));
        // No room on the screen: right of it anyway (the view then moves there).
        let screen = rect(1000, 1000, 400, 300);
        let spot = dock_spot(&a, size, &[a], &screen, &canvas, 0);
        assert_eq!(spot, Some(rect(1400, 1000, 500, 200)));
        // No room on the canvas either.
        assert_eq!(dock_spot(&a, size, &[a], &screen, &a, 0), None);
    }

    #[test]
    fn docking_keeps_the_gap() {
        // Right of `other`, 10 px past the gap and 6 px lower: it docks 16 px apart, tops flush.
        let other = rect(0, 0, 400, 300);
        let moving = rect(426, 6, 400, 300);
        let d = snap_delta(&[moving], &[other], 15, 16);
        assert_eq!(d, (-10, -6).into());
        assert!(touching(
            &Rect::new(moving.loc + d, moving.size),
            &other,
            16
        ));
        // Below it, pushed into the gap: back out to the gap.
        let moving = rect(0, 305, 400, 300);
        assert_eq!(snap_delta(&[moving], &[other], 15, 16), (0, 11).into());
        // Edge to edge without the gap does not count as docked.
        assert!(!touching(&other, &rect(400, 0, 400, 300), 16));
    }

    #[test]
    fn far_away_does_not_snap() {
        let other = rect(0, 0, 400, 300);
        let moving = rect(2000, 2000, 400, 300);
        assert_eq!(snap_delta(&[moving], &[other], 15, 0), (0, 0).into());
    }

    #[test]
    fn same_column_far_apart_does_not_align() {
        // Same x column but vertically far apart: left edges must not align.
        let other = rect(0, 0, 400, 300);
        let moving = rect(5, 1000, 400, 300);
        assert_eq!(snap_delta(&[moving], &[other], 15, 0), (0, 0).into());
    }

    #[test]
    fn cluster_snaps_as_a_whole() {
        // Two moving windows; only the right one is close to `other`.
        let a = rect(0, 0, 300, 300);
        let b = rect(300, 0, 300, 300);
        let other = rect(608, 0, 300, 300);
        assert_eq!(snap_delta(&[a, b], &[other], 15, 0), (8, 0).into());
    }

    #[test]
    fn touching_detection() {
        let a = rect(0, 0, 100, 100);
        assert!(touching(&a, &rect(100, 50, 100, 100), 0));
        assert!(touching(&a, &rect(20, 101, 100, 100), 0));
        // Corner to corner only: no cluster.
        assert!(!touching(&a, &rect(100, 100, 100, 100), 0));
        // Gap.
        assert!(!touching(&a, &rect(120, 0, 100, 100), 0));
    }

    #[test]
    fn join_clusters() {
        let mut c = Clusters::default();
        c.join(&1, &2);
        c.join(&3, &4);
        assert_eq!(sorted(c.members(&1)), vec![1, 2]);
        c.join(&2, &3);
        assert_eq!(sorted(c.members(&4)), vec![1, 2, 3, 4]);
        assert_eq!(c.members(&9), vec![9]);
    }

    #[test]
    fn detaching_a_group_keeps_it_together() {
        // Row 1 | 2 | 3. Taking 2 and 3 out leaves 1 alone and 2+3 as their own cluster.
        let mut c = Clusters::default();
        c.join(&1, &2);
        c.join(&2, &3);
        c.detach(&[2, 3]);
        assert_eq!(c.members(&1), vec![1]);
        assert_eq!(sorted(c.members(&2)), vec![2, 3]);
    }

    #[test]
    fn removing_splits_cluster() {
        // Row 1 | 2 | 3. Removing 2 splits the cluster into 1 and 3.
        let mut c = Clusters::default();
        c.join(&1, &2);
        c.join(&2, &3);
        c.remove(&2);
        assert_eq!(c.members(&1), vec![1]);
        assert_eq!(c.members(&3), vec![3]);
    }

    #[test]
    fn removing_keeps_connected_rest() {
        // 2×2 block: 1 2 / 3 4. Removing 1 leaves 2, 3, 4 connected via 4.
        let mut c = Clusters::default();
        for (a, b) in [(1, 2), (1, 3), (2, 4), (3, 4)] {
            c.join(&a, &b);
        }
        c.remove(&1);
        assert_eq!(sorted(c.members(&2)), vec![2, 3, 4]);
    }

    fn sorted(mut v: Vec<i32>) -> Vec<i32> {
        v.sort();
        v
    }
}
