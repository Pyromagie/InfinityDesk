//! Resizing a window inside its cluster: the dragged edge takes its neighbors along.
//!
//! Default: neighbors at the dragged edge (the whole seam) shrink or grow with it, the cluster
//! stays closed. With Shift: the windows on the dragged side of the seam change size, everything
//! in the cluster beyond the edge moves along.
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
///
/// The dragged edge moves together with the whole seam it belongs to: the windows across it, and
/// through them further windows on the dragged window's side whose edge lies on the same line
/// (e.g. the window below the dragged one, when both sit next to one tall window). Otherwise the
/// cluster would tear open there.
pub fn plan(rects: &[Rect], dragged: usize, edges: [Edge; 2], shift: bool) -> Vec<[Follow; 2]> {
    let Some(&d) = rects.get(dragged) else {
        return Vec::new();
    };
    let mut follow = vec![[Follow::None; 2]; rects.len()];
    for axis in [0, 1] {
        let (d_start, d_end) = span(&d, axis);
        let line = match edges[axis] {
            Edge::None => continue,
            Edge::Start => d_start,
            Edge::End => d_end,
        };
        let dragged_side = edges[axis] == Edge::End;
        let seam = seam(rects, dragged, axis, line);
        for (i, r) in rects.iter().enumerate() {
            let (start, end) = span(r, axis);
            let beyond = match edges[axis] {
                Edge::End => start >= line - TOUCH_TOLERANCE,
                _ => end <= line + TOUCH_TOLERANCE,
            };
            follow[i][axis] = match seam[i] {
                // Windows on the dragged side of the seam are resized like the dragged one.
                Some(end_on_line) if end_on_line == dragged_side => {
                    if end_on_line {
                        Follow::End
                    } else {
                        Follow::Start
                    }
                }
                // With Shift, everything beyond the edge moves instead of being resized.
                _ if shift && beyond => Follow::Shift,
                Some(true) => Follow::End,
                Some(false) => Follow::Start,
                None => Follow::None,
            };
        }
    }
    follow
}

/// The windows forming the seam at `line` (along `axis`) that `rects[dragged]` lies on: per
/// window `Some(true)` if its end edge lies on the line, `Some(false)` its start edge, `None` if
/// it is not part of the seam. Windows join when they face a member across the line.
fn seam(rects: &[Rect], dragged: usize, axis: usize, line: i32) -> Vec<Option<bool>> {
    let on_line = |r: &Rect| {
        let (start, end) = span(r, axis);
        let near = |x: i32| (x - line).abs() <= TOUCH_TOLERANCE;
        if near(end) {
            Some(true)
        } else if near(start) {
            Some(false)
        } else {
            None
        }
    };
    let mut member: Vec<Option<bool>> = vec![None; rects.len()];
    member[dragged] = on_line(&rects[dragged]);
    let mut queue = vec![dragged];
    while let Some(k) = queue.pop() {
        let (k_start, k_end) = span(&rects[k], 1 - axis);
        for (j, r) in rects.iter().enumerate() {
            let (start, end) = span(r, 1 - axis);
            let side = on_line(r);
            let facing = side.is_some() && side != member[k] && start < k_end && k_start < end;
            if member[j].is_none() && facing {
                member[j] = side;
                queue.push(j);
            }
        }
    }
    member
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

/// The edges of `r` grabbed at `pos`: up to `outer` outside the window or `inner` inside it.
/// `None` deeper inside the window or further away. Beyond a corner both edges are grabbed.
pub fn border_edges(
    r: &Rect,
    pos: Point<f64, Logical>,
    inner: f64,
    outer: f64,
) -> Option<[Edge; 2]> {
    let p = [pos.x, pos.y];
    let edges = [0, 1].map(|axis| {
        let (start, end) = span(r, axis);
        let (start, end) = (start as f64, end as f64);
        match p[axis] {
            v if v < start - outer || v > end + outer => None,
            v if v < start + inner => Some(Edge::Start),
            v if v > end - inner => Some(Edge::End),
            _ => Some(Edge::None),
        }
    });
    match edges {
        [Some(Edge::None), Some(Edge::None)] => None,
        [Some(x), Some(y)] => Some([x, y]),
        _ => None,
    }
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
    fn bottom_edge_takes_the_window_below_along() {
        // A above B; dragging A's bottom edge down shrinks B from the top.
        let rects = vec![rect(0, 0, 100, 100), rect(0, 100, 100, 100)];
        let follow = plan(&rects, 0, [Edge::None, Edge::End], false);
        let new = apply(&rects, &follow, &min()[..2], (0, 30).into());
        assert_eq!(new[0], rect(0, 0, 100, 130));
        assert_eq!(new[1], rect(0, 130, 100, 70));
    }

    #[test]
    fn with_shift_the_left_edge_moves_everything_left_of_it() {
        let rects = row();
        let follow = plan(&rects, 2, [Edge::Start, Edge::None], true);
        let new = apply(&rects, &follow, &min(), (-30, 0).into());
        assert_eq!(new[0], rect(-30, 0, 100, 100));
        assert_eq!(new[1], rect(70, 0, 100, 100));
        assert_eq!(new[2], rect(170, 0, 130, 100));
    }

    /// The case from the video: a tall window on the left, two windows stacked on the right.
    fn l_shape() -> Vec<Rect> {
        vec![
            rect(0, 0, 400, 600),     // left
            rect(400, 0, 300, 300),   // top right
            rect(400, 300, 300, 300), // bottom right
        ]
    }

    #[test]
    fn the_whole_seam_moves_when_grabbed_from_either_window() {
        let rects = l_shape();
        // Grabbing the left edge of the top-right window: the bottom-right one comes along too.
        let follow = plan(&rects, 1, [Edge::Start, Edge::None], false);
        let new = apply(&rects, &follow, &min(), (-50, 0).into());
        assert_eq!(new[0], rect(0, 0, 350, 600));
        assert_eq!(new[1], rect(350, 0, 350, 300));
        assert_eq!(new[2], rect(350, 300, 350, 300));
        // Grabbing the right edge of the left window gives the same result.
        let follow = plan(&rects, 0, [Edge::End, Edge::None], false);
        assert_eq!(apply(&rects, &follow, &min(), (-50, 0).into()), new);
    }

    #[test]
    fn with_shift_the_seam_stays_closed() {
        let rects = l_shape();
        // Top-right window grows to the left; the left window moves away, so the bottom-right
        // window grows along instead of leaving a gap.
        let follow = plan(&rects, 1, [Edge::Start, Edge::None], true);
        let new = apply(&rects, &follow, &min(), (-50, 0).into());
        assert_eq!(new[0], rect(-50, 0, 400, 600));
        assert_eq!(new[1], rect(350, 0, 350, 300));
        assert_eq!(new[2], rect(350, 300, 350, 300));
    }

    #[test]
    fn a_column_without_a_neighbor_across_does_not_follow() {
        // A above B, nothing to their right: widening A leaves B alone.
        let rects = vec![rect(0, 0, 100, 100), rect(0, 100, 100, 100)];
        let follow = plan(&rects, 0, [Edge::End, Edge::None], false);
        assert_eq!(follow[1], [Follow::None, Follow::None]);
    }

    #[test]
    fn border_zone_around_the_edges() {
        let r = rect(100, 100, 200, 200);
        let at = |x: f64, y: f64| border_edges(&r, (x, y).into(), 5.0, 10.0);
        // Outside the left edge, just inside the right edge, below the bottom edge.
        assert_eq!(at(92.0, 200.0), Some([Edge::Start, Edge::None]));
        assert_eq!(at(297.0, 200.0), Some([Edge::End, Edge::None]));
        assert_eq!(at(200.0, 308.0), Some([Edge::None, Edge::End]));
        // Beyond the top-right corner: both edges.
        assert_eq!(at(305.0, 95.0), Some([Edge::End, Edge::Start]));
        // Inside the window, or too far away: nothing.
        assert_eq!(at(200.0, 200.0), None);
        assert_eq!(at(80.0, 200.0), None);
        assert_eq!(at(200.0, 50.0), None);
    }

    #[test]
    fn window_below_is_not_a_neighbor_of_the_right_edge() {
        // A above B; dragging A's right edge must not touch B.
        let rects = vec![rect(0, 0, 100, 100), rect(0, 100, 100, 100)];
        let follow = plan(&rects, 0, [Edge::End, Edge::None], false);
        assert_eq!(follow[1], [Follow::None, Follow::None]);
    }
}
