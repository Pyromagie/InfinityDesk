//! Snapping and clusters.
//!
//! A cluster is a group of windows that were docked to each other and move together.
//! Membership is stored explicitly (not just derived from geometry), so a cluster does not fall apart
//! just because a client changes its size by a few pixels.
//!
//! This module is pure logic without Wayland: `K` is the window key (`Window` in the compositor,
//! plain numbers in the tests).

use std::{collections::HashMap, hash::Hash};

use smithay::utils::{Logical, Point, Rectangle, Size};

pub type Rect = Rectangle<i32, Logical>;

/// Edges closer than this (in screen pixels, so it works at any zoom) snap to each other.
pub const SNAP_THRESHOLD: f64 = 15.0;
/// Edges at most this far apart count as "touching" (connected).
pub const TOUCH_TOLERANCE: i32 = 2;
/// Docking also lines up the other edge (tops or left edges), but only if that edge is at most
/// this many times the snap distance away. Otherwise a window would jump far to line up.
const ALIGN_FACTOR: i32 = 4;

/// Which windows belong to which cluster. Windows without an entry stand alone.
#[derive(Debug)]
pub struct Clusters<K> {
    cluster_of: HashMap<K, u64>,
    next_id: u64,
}

impl<K: Eq + Hash + Clone> Default for Clusters<K> {
    fn default() -> Self {
        Self {
            cluster_of: HashMap::new(),
            next_id: 0,
        }
    }
}

impl<K: Eq + Hash + Clone> Clusters<K> {
    /// All windows in the same cluster as `k` (including `k`). Standing alone: just `k`.
    pub fn members(&self, k: &K) -> Vec<K> {
        match self.cluster_of.get(k) {
            Some(id) => self
                .cluster_of
                .iter()
                .filter(|(_, c)| *c == id)
                .map(|(w, _)| w.clone())
                .collect(),
            None => vec![k.clone()],
        }
    }

    /// Merges the clusters of `a` and `b` into one.
    pub fn join(&mut self, a: &K, b: &K) {
        let x = match self.cluster_of.get(a) {
            Some(&x) => x,
            None => {
                self.next_id += 1;
                self.cluster_of.insert(a.clone(), self.next_id);
                self.next_id
            }
        };
        // If `b` was in another cluster, move that whole cluster over to `x`.
        if let Some(y) = self.cluster_of.insert(b.clone(), x)
            && y != x
        {
            self.cluster_of
                .values_mut()
                .filter(|c| **c == y)
                .for_each(|c| *c = x);
        }
    }

    /// Removes `k` from its cluster. The rest is regrouped based on geometry,
    /// because without `k` a cluster may fall apart into several pieces.
    pub fn remove(&mut self, k: &K, rect_of: impl Fn(&K) -> Option<Rect>) {
        let members = self.members(k);
        for w in &members {
            self.cluster_of.remove(w);
        }
        let rest: Vec<K> = members.into_iter().filter(|w| w != k).collect();
        self.join_touching(&rest, &rect_of);
    }

    /// Takes several windows out of their cluster together; among themselves they stay
    /// connected wherever they touch.
    pub fn detach(&mut self, group: &[K], rect_of: impl Fn(&K) -> Option<Rect>) {
        for k in group {
            self.remove(k, &rect_of);
        }
        self.join_touching(group, &rect_of);
    }

    /// Connects every pair of the given windows that lies edge to edge.
    fn join_touching(&mut self, windows: &[K], rect_of: impl Fn(&K) -> Option<Rect>) {
        let rects: Vec<(&K, Rect)> = windows
            .iter()
            .filter_map(|w| rect_of(w).map(|r| (w, r)))
            .collect();
        for (i, (a, ra)) in rects.iter().enumerate() {
            for (b, rb) in &rects[i + 1..] {
                if touching(ra, rb) {
                    self.join(a, b);
                }
            }
        }
    }
}

/// Do two windows lie edge to edge (with real overlap along the edge, not just corner to corner)?
pub fn touching(a: &Rect, b: &Rect) -> bool {
    let (al, ar, at, ab) = edges(a);
    let (bl, br, bt, bb) = edges(b);
    let h_overlap = ar.min(br) - al.max(bl) > 0;
    let v_overlap = ab.min(bb) - at.max(bt) > 0;
    let side_by_side = (ar - bl).abs() <= TOUCH_TOLERANCE || (al - br).abs() <= TOUCH_TOLERANCE;
    let stacked = (ab - bt).abs() <= TOUCH_TOLERANCE || (at - bb).abs() <= TOUCH_TOLERANCE;
    (side_by_side && v_overlap) || (stacked && h_overlap)
}

/// Computes the correction by which the moving windows must be shifted to dock to a nearby
/// window. An edge within `threshold` (canvas pixels) of a neighbor docks to it, and the other
/// axis always lines up with that neighbor (top edges or left edges flush), so clusters form a
/// clean grid. The closest dock wins; ties go to the smallest alignment jump.
/// A spot that is already taken by another window is skipped.
pub fn snap_delta(moving: &[Rect], others: &[Rect], threshold: i32) -> Point<i32, Logical> {
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
            let candidates = [
                (v_overlap, ol - mr, ot - mt, true),  // left of o, tops flush
                (v_overlap, or - ml, ot - mt, true),  // right of o, tops flush
                (h_overlap, ot - mb, ol - ml, false), // above o, left edges flush
                (h_overlap, ob - mt, ol - ml, false), // below o, left edges flush
            ];
            for (faces, dock, align, side_by_side) in candidates {
                let score = (dock.abs(), align.abs());
                let delta = if side_by_side {
                    (dock, align)
                } else {
                    (align, dock)
                }
                .into();
                if faces
                    && dock.abs() <= threshold
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

/// The size a window docked to `o` gets, so the cluster stays a clean grid:
/// side by side it takes the height of `o`, stacked the width.
pub fn grid_size(m: &Rect, o: &Rect) -> Size<i32, Logical> {
    let (ml, mr, ..) = edges(m);
    let (ol, or, ..) = edges(o);
    let side_by_side = (mr - ol).abs() <= TOUCH_TOLERANCE || (ml - or).abs() <= TOUCH_TOLERANCE;
    if side_by_side {
        (m.size.w, o.size.h).into()
    } else {
        (o.size.w, m.size.h).into()
    }
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
    fn snaps_to_the_left_and_aligns_top() {
        let other = rect(1000, 500, 400, 300);
        // 10 px gap to the left edge of `other`, 6 px lower.
        let moving = rect(590, 506, 400, 300);
        let d = snap_delta(&[moving], &[other], 15);
        assert_eq!(d, (10, -6).into());
    }

    #[test]
    fn snaps_below() {
        let other = rect(0, 0, 400, 300);
        let moving = rect(3, 312, 400, 300);
        let d = snap_delta(&[moving], &[other], 15);
        assert_eq!(d, (-3, -12).into());
    }

    #[test]
    fn docking_aligns_nearby_edges() {
        // Docks to the right of `other`, 40 px lower: tops are lined up.
        let other = rect(0, 0, 400, 300);
        let moving = rect(405, 40, 400, 500);
        assert_eq!(snap_delta(&[moving], &[other], 15), (-5, -40).into());
    }

    #[test]
    fn taller_window_far_below_does_not_jump_up() {
        // A tall window in the gap between two windows, 200 px lower than them: it must not
        // jump up to line up its top, it simply does not dock.
        let left = rect(0, 0, 400, 300);
        let right = rect(810, 0, 400, 300);
        let moving = rect(405, 200, 400, 600);
        assert_eq!(snap_delta(&[moving], &[left, right], 15), (0, 0).into());
    }

    #[test]
    fn docks_beside_a_column_instead_of_jumping_onto_it() {
        // Column: `top` above `bottom`. A window approaches the column from the right, 20 px away.
        let top = rect(0, 0, 400, 300);
        let bottom = rect(0, 300, 400, 300);
        let moving = rect(420, 5, 400, 300);
        assert_eq!(snap_delta(&[moving], &[top, bottom], 27), (-20, -5).into());
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
        assert_eq!(snap_delta(&[moving], &grid, 15), (0, 0).into());
    }

    #[test]
    fn grid_size_matches_neighbor() {
        let other = rect(0, 0, 400, 300);
        // Side by side: same height. Below: same width.
        assert_eq!(
            grid_size(&rect(400, 0, 200, 500), &other),
            (200, 300).into()
        );
        assert_eq!(
            grid_size(&rect(0, 300, 200, 500), &other),
            (400, 500).into()
        );
    }

    #[test]
    fn far_away_does_not_snap() {
        let other = rect(0, 0, 400, 300);
        let moving = rect(2000, 2000, 400, 300);
        assert_eq!(snap_delta(&[moving], &[other], 15), (0, 0).into());
    }

    #[test]
    fn same_column_far_apart_does_not_align() {
        // Same x column but vertically far apart: left edges must not align.
        let other = rect(0, 0, 400, 300);
        let moving = rect(5, 1000, 400, 300);
        assert_eq!(snap_delta(&[moving], &[other], 15), (0, 0).into());
    }

    #[test]
    fn cluster_snaps_as_a_whole() {
        // Two moving windows; only the right one is close to `other`.
        let a = rect(0, 0, 300, 300);
        let b = rect(300, 0, 300, 300);
        let other = rect(608, 0, 300, 300);
        assert_eq!(snap_delta(&[a, b], &[other], 15), (8, 0).into());
    }

    #[test]
    fn touching_detection() {
        let a = rect(0, 0, 100, 100);
        assert!(touching(&a, &rect(100, 50, 100, 100)));
        assert!(touching(&a, &rect(20, 101, 100, 100)));
        // Corner to corner only: no cluster.
        assert!(!touching(&a, &rect(100, 100, 100, 100)));
        // Gap.
        assert!(!touching(&a, &rect(120, 0, 100, 100)));
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
        let rects = HashMap::from([
            (1, rect(0, 0, 100, 100)),
            (2, rect(100, 0, 100, 100)),
            (3, rect(200, 0, 100, 100)),
        ]);
        let mut c = Clusters::default();
        c.join(&1, &2);
        c.join(&2, &3);
        c.detach(&[2, 3], |k| rects.get(k).copied());
        assert_eq!(c.members(&1), vec![1]);
        assert_eq!(sorted(c.members(&2)), vec![2, 3]);
    }

    #[test]
    fn removing_splits_cluster() {
        // Row 1 | 2 | 3. Removing 2 splits the cluster into 1 and 3.
        let rects = HashMap::from([
            (1, rect(0, 0, 100, 100)),
            (2, rect(100, 0, 100, 100)),
            (3, rect(200, 0, 100, 100)),
        ]);
        let mut c = Clusters::default();
        c.join(&1, &2);
        c.join(&2, &3);
        c.remove(&2, |k| rects.get(k).copied());
        assert_eq!(c.members(&1), vec![1]);
        assert_eq!(c.members(&3), vec![3]);
    }

    #[test]
    fn removing_keeps_connected_rest() {
        // 2×2 block: 1 2 / 3 4. Removing 1 leaves 2, 3, 4 connected via 4.
        let rects = HashMap::from([
            (1, rect(0, 0, 100, 100)),
            (2, rect(100, 0, 100, 100)),
            (3, rect(0, 100, 100, 100)),
            (4, rect(100, 100, 100, 100)),
        ]);
        let mut c = Clusters::default();
        for (a, b) in [(1, 2), (1, 3), (2, 4), (3, 4)] {
            c.join(&a, &b);
        }
        c.remove(&1, |k| rects.get(k).copied());
        assert_eq!(sorted(c.members(&2)), vec![2, 3, 4]);
    }

    fn sorted(mut v: Vec<i32>) -> Vec<i32> {
        v.sort();
        v
    }
}
