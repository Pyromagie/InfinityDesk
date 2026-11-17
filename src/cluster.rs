//! Snapping and clusters.
//!
//! A cluster is a group of windows that were docked to each other and move together.
//! Membership is stored explicitly (not just derived from geometry), so a cluster does not fall apart
//! just because a client changes its size by a few pixels.
//!
//! This module is pure logic without Wayland: `K` is the window key (`Window` in the compositor,
//! plain numbers in the tests).

use std::{collections::HashMap, hash::Hash};

use smithay::utils::{Logical, Point, Rectangle};

pub type Rect = Rectangle<i32, Logical>;

/// Edges closer than this (in pixels) snap to each other.
pub const SNAP_THRESHOLD: i32 = 15;
/// Edges at most this far apart count as "touching" (connected).
pub const TOUCH_TOLERANCE: i32 = 2;

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
        let ca = self.cluster_of.get(a).copied();
        let cb = self.cluster_of.get(b).copied();
        match (ca, cb) {
            (Some(x), Some(y)) if x == y => {}
            (Some(x), Some(y)) => {
                for c in self.cluster_of.values_mut() {
                    if *c == y {
                        *c = x;
                    }
                }
            }
            (Some(x), None) => {
                self.cluster_of.insert(b.clone(), x);
            }
            (None, Some(y)) => {
                self.cluster_of.insert(a.clone(), y);
            }
            (None, None) => {
                let id = self.next_id;
                self.next_id += 1;
                self.cluster_of.insert(a.clone(), id);
                self.cluster_of.insert(b.clone(), id);
            }
        }
    }

    /// Removes `k` from its cluster. The rest is regrouped based on geometry,
    /// because without `k` a cluster may fall apart into several pieces.
    pub fn remove(&mut self, k: &K, rect_of: impl Fn(&K) -> Option<Rect>) {
        let Some(id) = self.cluster_of.remove(k) else {
            return;
        };
        let rest: Vec<K> = self
            .cluster_of
            .iter()
            .filter(|(_, c)| **c == id)
            .map(|(w, _)| w.clone())
            .collect();
        for w in &rest {
            self.cluster_of.remove(w);
        }

        let rects: Vec<(K, Rect)> = rest.into_iter().filter_map(|w| rect_of(&w).map(|r| (w, r))).collect();
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

/// Computes the correction by which the moving windows must be shifted so they snap to nearby
/// edges of the other windows. Per axis, the smallest correction wins.
///
/// Two kinds of snapping:
/// - **Docking:** right edge to left edge (or bottom to top) when the windows are next to each other.
/// - **Aligning:** top edges flush (or left edges) when the windows are docking to each other.
///   This produces clean corners, like in a 2×2 cluster.
pub fn snap_delta(moving: &[Rect], others: &[Rect], threshold: i32) -> Point<i32, Logical> {
    let mut best_x: Option<i32> = None;
    let mut best_y: Option<i32> = None;
    let consider = |best: &mut Option<i32>, d: i32| {
        if d.abs() <= threshold && best.is_none_or(|b| d.abs() < b.abs()) {
            *best = Some(d);
        }
    };

    for m in moving {
        let (ml, mr, mt, mb) = edges(m);
        for o in others {
            let (ol, or, ot, ob) = edges(o);

            // Do the ranges overlap (with some tolerance) on the other axis?
            let v_near = mt < ob + threshold && ot < mb + threshold;
            let h_near = ml < or + threshold && ol < mr + threshold;
            // Are they close edge to edge?
            let x_adjacent = (mr - ol).abs() <= threshold || (ml - or).abs() <= threshold;
            let y_adjacent = (mb - ot).abs() <= threshold || (mt - ob).abs() <= threshold;

            if v_near {
                consider(&mut best_x, ol - mr); // dock left of o
                consider(&mut best_x, or - ml); // dock right of o
            }
            if h_near {
                consider(&mut best_y, ot - mb); // dock above o
                consider(&mut best_y, ob - mt); // dock below o
            }
            if y_adjacent && h_near {
                consider(&mut best_x, ol - ml); // left edges flush
                consider(&mut best_x, or - mr); // right edges flush
            }
            if x_adjacent && v_near {
                consider(&mut best_y, ot - mt); // top edges flush
                consider(&mut best_y, ob - mb); // bottom edges flush
            }
        }
    }

    (best_x.unwrap_or(0), best_y.unwrap_or(0)).into()
}

/// Bounding box of several rectangles.
pub fn bounding_box(rects: &[Rect]) -> Option<Rect> {
    rects.iter().copied().reduce(|a, b| a.merge(b))
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
        let d = snap_delta(&[moving], &[other], SNAP_THRESHOLD);
        assert_eq!(d, (10, -6).into());
    }

    #[test]
    fn snaps_below() {
        let other = rect(0, 0, 400, 300);
        let moving = rect(3, 312, 400, 300);
        let d = snap_delta(&[moving], &[other], SNAP_THRESHOLD);
        assert_eq!(d, (-3, -12).into());
    }

    #[test]
    fn far_away_does_not_snap() {
        let other = rect(0, 0, 400, 300);
        let moving = rect(2000, 2000, 400, 300);
        assert_eq!(snap_delta(&[moving], &[other], SNAP_THRESHOLD), (0, 0).into());
    }

    #[test]
    fn same_column_far_apart_does_not_align() {
        // Same x column but vertically far apart: left edges must not align.
        let other = rect(0, 0, 400, 300);
        let moving = rect(5, 1000, 400, 300);
        assert_eq!(snap_delta(&[moving], &[other], SNAP_THRESHOLD), (0, 0).into());
    }

    #[test]
    fn cluster_snaps_as_a_whole() {
        // Two moving windows; only the right one is close to `other`.
        let a = rect(0, 0, 300, 300);
        let b = rect(300, 0, 300, 300);
        let other = rect(608, 0, 300, 300);
        assert_eq!(snap_delta(&[a, b], &[other], SNAP_THRESHOLD), (8, 0).into());
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
    fn removing_splits_cluster() {
        // Row 1 | 2 | 3. Removing 2 splits the cluster into 1 and 3.
        let rects = HashMap::from([(1, rect(0, 0, 100, 100)), (2, rect(100, 0, 100, 100)), (3, rect(200, 0, 100, 100))]);
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
