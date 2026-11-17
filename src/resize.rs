//! Resizing a window inside its cluster: the dragged edge takes its neighbors along.
//!
//! Default: neighbors directly at the dragged edge shrink or grow with it, the cluster stays closed.
//! With Shift: only the dragged window changes size, everything in the cluster beyond the edge
//! moves along.
//!
//! Pure geometry without Wayland, so it is easy to test.

use smithay::utils::{Logical, Point, Size};

use crate::cluster::{Rect, TOUCH_TOLERANCE};

/// Which edge of the dragged window moves along one axis.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Edge {
    None,
    /// Left or top.
    Start,
    /// Right or bottom.
    End,
}

/// How a window follows the drag along one axis.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Follow {
    None,
    /// Its left/top edge moves (the right/bottom edge stays).
    Start,
    /// Its right/bottom edge moves.
    End,
    /// It moves as a whole.
    Shift,
}

/// For every window of the cluster: how it follows when `rects[dragged]` is resized at `edges`
/// (x and y axis).
pub fn plan(rects: &[Rect], dragged: usize, edges: [Edge; 2], shift: bool) -> Vec<[Follow; 2]> {
    let Some(&d) = rects.get(dragged) else {
        return Vec::new();
    };
    rects
        .iter()
        .enumerate()
        .map(|(i, r)| {
            [0, 1].map(|axis| {
                let edge = edges[axis];
                let (start, end) = span(r, axis);
                let (d_start, d_end) = span(&d, axis);
                // Neighbors must overlap with the dragged window along the other axis.
                let (o_start, o_end) = span(r, 1 - axis);
                let (do_start, do_end) = span(&d, 1 - axis);
                let beside = o_start < do_end && do_start < o_end;
                let near = |a: i32, b: i32| (a - b).abs() <= TOUCH_TOLERANCE;
                match edge {
                    Edge::None => Follow::None,
                    _ if i == dragged && edge == Edge::Start => Follow::Start,
                    _ if i == dragged => Follow::End,
                    Edge::End if shift && start >= d_end - TOUCH_TOLERANCE => Follow::Shift,
                    Edge::Start if shift && end <= d_start + TOUCH_TOLERANCE => Follow::Shift,
                    Edge::End if !shift && beside && near(start, d_end) => Follow::Start,
                    Edge::Start if !shift && beside && near(end, d_start) => Follow::End,
                    _ => Follow::None,
                }
            })
        })
        .collect()
}

/// New rectangles for a pointer movement of `delta`, limited so that no window gets smaller than
/// its minimum size.
pub fn apply(
    rects: &[Rect],
    follow: &[[Follow; 2]],
    min: &[Size<i32, Logical>],
    delta: Point<i32, Logical>,
) -> Vec<Rect> {
    let mut d = [delta.x, delta.y];
    for axis in [0, 1] {
        for ((r, f), m) in rects.iter().zip(follow).zip(min) {
            let (start, end) = span(r, axis);
            let (len, min_len) = (end - start, [m.w, m.h][axis]);
            match f[axis] {
                Follow::Start => d[axis] = d[axis].min(len - min_len),
                Follow::End => d[axis] = d[axis].max(min_len - len),
                _ => {}
            }
        }
    }
    rects
        .iter()
        .zip(follow)
        .map(|(r, f)| {
            let mut r = *r;
            for axis in [0, 1] {
                let (pos, len) = match axis {
                    0 => (&mut r.loc.x, &mut r.size.w),
                    _ => (&mut r.loc.y, &mut r.size.h),
                };
                match f[axis] {
                    Follow::Start => (*pos, *len) = (*pos + d[axis], *len - d[axis]),
                    Follow::End => *len += d[axis],
                    Follow::Shift => *pos += d[axis],
                    Follow::None => {}
                }
            }
            r
        })
        .collect()
}

/// (start, end) of a rectangle along an axis (0 = x, 1 = y).
fn span(r: &Rect, axis: usize) -> (i32, i32) {
    match axis {
        0 => (r.loc.x, r.loc.x + r.size.w),
        _ => (r.loc.y, r.loc.y + r.size.h),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::utils::Rectangle;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    /// Row A | B | C, each 100×100.
    fn row() -> Vec<Rect> {
        vec![
            rect(0, 0, 100, 100),
            rect(100, 0, 100, 100),
            rect(200, 0, 100, 100),
        ]
    }

    fn min() -> [Size<i32, Logical>; 3] {
        [(20, 20).into(); 3]
    }

    #[test]
    fn neighbor_shrinks_with_the_edge() {
        let rects = row();
        let follow = plan(&rects, 0, [Edge::End, Edge::None], false);
        let new = apply(&rects, &follow, &min(), (30, 0).into());
        assert_eq!(new[0], rect(0, 0, 130, 100));
        assert_eq!(new[1], rect(130, 0, 70, 100));
        assert_eq!(new[2], rect(200, 0, 100, 100));
    }

    #[test]
    fn with_shift_everything_beyond_moves() {
        let rects = row();
        let follow = plan(&rects, 0, [Edge::End, Edge::None], true);
        let new = apply(&rects, &follow, &min(), (30, 0).into());
        assert_eq!(new[0], rect(0, 0, 130, 100));
        assert_eq!(new[1], rect(130, 0, 100, 100));
        assert_eq!(new[2], rect(230, 0, 100, 100));
    }

    #[test]
    fn left_edge_takes_the_left_neighbor_along() {
        let rects = row();
        let follow = plan(&rects, 1, [Edge::Start, Edge::None], false);
        let new = apply(&rects, &follow, &min(), (-40, 0).into());
        assert_eq!(new[0], rect(0, 0, 60, 100));
        assert_eq!(new[1], rect(60, 0, 140, 100));
        assert_eq!(new[2], rect(200, 0, 100, 100));
    }

    #[test]
    fn neighbors_do_not_shrink_below_their_minimum() {
        let rects = row();
        let follow = plan(&rects, 0, [Edge::End, Edge::None], false);
        let new = apply(&rects, &follow, &min(), (500, 0).into());
        assert_eq!(new[0], rect(0, 0, 180, 100));
        assert_eq!(new[1], rect(180, 0, 20, 100));
    }

    #[test]
    fn window_below_is_not_a_neighbor_of_the_right_edge() {
        // A above B; dragging A's right edge must not touch B.
        let rects = vec![rect(0, 0, 100, 100), rect(0, 100, 100, 100)];
        let follow = plan(&rects, 0, [Edge::End, Edge::None], false);
        assert_eq!(follow[1], [Follow::None, Follow::None]);
    }
}
