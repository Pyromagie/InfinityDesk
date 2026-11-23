//! Resizing a window inside its cluster: its size toggles between half and full screen, and the
//! dragged edge takes its neighbors along. While dragging, the edge follows the pointer between
//! the two sizes; let go, it glides to the closer one.
//!
//! Default: neighbors at the dragged edge (the whole seam) shrink or grow with it, the cluster
//! stays closed. With Shift, or if the neighbors cannot (they would get smaller than half or
//! larger than full): the windows on the dragged side of the seam change size, everything in the
//! cluster beyond the edge moves along. If windows of the cluster would only touch at a corner
//! afterwards, the window changes size at its other edge instead, so the cluster stays together.
//!
//! Pure geometry without Wayland, so it is easy to test.

use smithay::utils::{Logical, Point, Size};

use crate::cluster::{Rect, TOUCH_TOLERANCE, touching};

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

/// How each window of a cluster follows a resize, per axis.
pub type Plan = Vec<[Follow; 2]>;

/// Toggling the size of `rects[dragged]` at `edges`: on each dragged axis, a window closer to
/// half the `screen` (with gaps) becomes full, else half. Returns how the windows follow (see
/// `plan`), the movement of the edges that toggles them, and per axis whether the edges move
/// with the pointer (1) or against it (-1: the other edge changes the size, so the cluster stays
/// together). `None` if the windows cannot take the new size (`min`, or neighbors beyond half or
/// full even with `shift`, or windows would end up on each other).
pub fn toggle(
    rects: &[Rect],
    min: &[Size<i32, Logical>],
    dragged: usize,
    edges: [Edge; 2],
    shift: bool,
    screen: Size<i32, Logical>,
    gap: i32,
) -> Option<(Plan, Point<i32, Logical>, [i32; 2])> {
    let d = rects.get(dragged)?;
    let screen = [screen.w, screen.h];
    let half = [0, 1].map(|axis| (screen[axis] - gap) / 2 - gap);
    let full = [0, 1].map(|axis| screen[axis] - 2 * gap);
    let len = |r: &Rect, axis: usize| [r.size.w, r.size.h][axis];
    let target = [0, 1].map(|axis| match 2 * len(d, axis) < half[axis] + full[axis] {
        true => full[axis],
        false => half[axis],
    });
    // How the windows follow when the window changes size at `edges`, and how far those edges
    // move. `together`: the cluster must stay connected. `along`: windows that would come apart
    // from the dragged one move along with its edges instead.
    let resize_at = |edges: [Edge; 2], together: bool, along: bool| {
        let delta = [0, 1].map(|axis| match edges[axis] {
            Edge::None => 0,
            Edge::Start => len(d, axis) - target[axis],
            Edge::End => target[axis] - len(d, axis),
        });
        let delta = Point::from((delta[0], delta[1]));
        // The dragged window gets exactly its new size, every other window that changes size
        // stays between half and full.
        let fits = |follow: &Plan| {
            let new = apply(rects, follow, min, delta);
            (rects.iter().zip(&new).enumerate()).all(|(i, (old, new))| {
                (0..2).all(|axis| match len(new, axis) {
                    l if i == dragged && edges[axis] != Edge::None => l == target[axis],
                    l if l == len(old, axis) => true,
                    l => half[axis] - TOUCH_TOLERANCE <= l && l <= full[axis] + TOUCH_TOLERANCE,
                })
            })
        };
        // Windows moving along per axis can land on others (one beyond both dragged edges moves
        // twice). Also checked with only one axis moved: while dragging, each axis follows the
        // pointer on its own (`following`).
        let sound = |follow: &Plan| {
            [(delta.x, 0), (0, delta.y), (delta.x, delta.y)]
                .into_iter()
                .all(|d| {
                    let new = apply(rects, follow, min, d.into());
                    let apart = (new.iter().enumerate())
                        .all(|(i, a)| new[i + 1..].iter().all(|b| !a.overlaps(*b)));
                    apart && (!together || connected(&new, gap))
                })
        };
        let moving_along = |mut follow: Plan| {
            let new = apply(rects, &follow, min, delta);
            let reached = reach(&new, dragged, gap);
            for (f, reached) in follow.iter_mut().zip(reached) {
                for axis in [0, 1] {
                    let moves = [delta.x, delta.y][axis] != 0;
                    if !reached && moves && f[axis] == Follow::None {
                        f[axis] = Follow::Shift;
                    }
                }
            }
            follow
        };
        let follow = [shift, true]
            .map(|shift| plan(rects, dragged, edges, shift, gap))
            .into_iter()
            .map(|follow| if along { moving_along(follow) } else { follow })
            .find(|follow| fits(follow) && sound(follow))?;
        Some((follow, delta))
    };
    // The grabbed edges, else the other edge on one or both axes.
    let other = |e: Edge| match e {
        Edge::Start => Edge::End,
        Edge::End => Edge::Start,
        Edge::None => Edge::None,
    };
    let resize = |flip: [bool; 2], together: bool, along: bool| {
        let edges = [0, 1].map(|axis| {
            if flip[axis] {
                other(edges[axis])
            } else {
                edges[axis]
            }
        });
        let (follow, delta) = resize_at(edges, together, along)?;
        Some((follow, delta, flip.map(|f| if f { -1 } else { 1 })))
    };
    // Only when the grabbed edges can move but the cluster would come apart: the other edge.
    // If that does not keep it together either (neighbors on both sides), the windows that
    // would come apart move along (the grabbed edges first); else nothing changes.
    resize([false, false], false, false)?;
    let together = connected(rects, gap);
    let flips = [[false, false], [true, false], [false, true], [true, true]];
    let flips = || {
        (flips.into_iter())
            .filter(|flip| (0..2).all(|axis| !flip[axis] || edges[axis] != Edge::None))
    };
    (flips().find_map(|flip| resize(flip, together, false)))
        .or_else(|| flips().find_map(|flip| resize(flip, together, together)))
}

/// Do the windows hang together, each touching another one along an edge (not just at a corner)?
fn connected(rects: &[Rect], gap: i32) -> bool {
    reach(rects, 0, gap).iter().all(|&r| r)
}

/// Which windows `rects[start]` reaches through windows touching along an edge.
fn reach(rects: &[Rect], start: usize, gap: i32) -> Vec<bool> {
    let mut reached = vec![false; rects.len()];
    let mut queue = vec![start];
    while let Some(k) = queue.pop() {
        for (j, r) in rects.iter().enumerate() {
            if !reached[j] && (j == k || touching(&rects[k], r, gap)) {
                reached[j] = true;
                queue.push(j);
            }
        }
    }
    reached
}

/// What to try, in this order, for a resize grabbed at `edges` once the pointer moved by `moved`:
/// at a corner both axes, else only the axis the pointer moves along more, else the other one.
pub fn tries(edges: [Edge; 2], moved: Point<f64, Logical>) -> Vec<[Edge; 2]> {
    let mut tries = vec![edges];
    if edges[0] != Edge::None && edges[1] != Edge::None {
        let only = |axis: usize| {
            let mut only = [Edge::None; 2];
            only[axis] = edges[axis];
            only
        };
        match moved.x.abs() >= moved.y.abs() {
            true => tries.extend([only(0), only(1)]),
            false => tries.extend([only(1), only(0)]),
        }
    }
    tries
}

/// Does a resize from `toggle` (its `delta` and `sign`) go the way the pointer moved, on every
/// axis that changes?
pub fn goes_along(delta: Point<i32, Logical>, sign: [i32; 2], moved: Point<f64, Logical>) -> bool {
    let way = [delta.x * sign[0], delta.y * sign[1]];
    let moved = [moved.x, moved.y];
    (0..2).all(|axis| way[axis] == 0 || way[axis] as f64 * moved[axis] > 0.0)
}

/// How far the dragged edges follow the pointer: as far as it moved, but only between the size
/// now and the other one (`toggle`).
pub fn following(toggle: Point<i32, Logical>, pointer: Point<i32, Logical>) -> Point<i32, Logical> {
    let clamp = |t: i32, p: i32| p.clamp(t.min(0), t.max(0));
    (clamp(toggle.x, pointer.x), clamp(toggle.y, pointer.y)).into()
}

/// Where the edges go when the pointer lets go: per axis the other size once the pointer moved
/// past half of the way (`toggle`), else back.
pub fn toggled(toggle: Point<i32, Logical>, pointer: Point<i32, Logical>) -> Point<i32, Logical> {
    let past_half = |t: i32, p: i32| if 2 * p * t > t * t { t } else { 0 };
    (
        past_half(toggle.x, pointer.x),
        past_half(toggle.y, pointer.y),
    )
        .into()
}

/// For every window of the cluster: how it follows when `rects[dragged]` is resized at `edges`
/// (x and y axis). Windows lie `gap` apart.
///
/// The dragged edge moves together with the whole seam it belongs to: the windows across it, and
/// through them further windows on the dragged window's side whose edge lies on the same line
/// (e.g. the window below the dragged one, when both sit next to one tall window). Otherwise the
/// cluster would tear open there.
fn plan(rects: &[Rect], dragged: usize, edges: [Edge; 2], shift: bool, gap: i32) -> Plan {
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
        let end_line = if dragged_side { line } else { line - gap };
        let seam = seam(rects, dragged, axis, end_line, gap);
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

/// The windows forming the seam (along `axis`) that `rects[dragged]` lies on: end edges at
/// `end_line`, start edges `gap` further. Per window `Some(true)` if its end edge lies on the
/// seam, `Some(false)` its start edge, `None` if it is not part of the seam. Windows join when
/// they face a member across the seam.
fn seam(rects: &[Rect], dragged: usize, axis: usize, end_line: i32, gap: i32) -> Vec<Option<bool>> {
    let on_line = |r: &Rect| {
        let (start, end) = span(r, axis);
        let near = |x: i32, line: i32| (x - line).abs() <= TOUCH_TOLERANCE;
        if near(end, end_line) {
            Some(true)
        } else if near(start, end_line + gap) {
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

    /// A small random number generator (xorshift), so the random tests need no extra crate and
    /// give the same cases every run.
    struct Random(u64);

    impl Random {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }
    }

    const SCREEN: (i32, i32) = (1920, 1080);
    const GAP: i32 = 16;

    /// A random cluster of up to 6 windows, each half or full screen size per axis (with gaps),
    /// docked one by one to a side of an earlier one with flush edges, like the user builds them.
    fn random_cluster(random: &mut Random) -> Vec<Rect> {
        let half = ((SCREEN.0 - GAP) / 2 - GAP, (SCREEN.1 - GAP) / 2 - GAP);
        let full = (SCREEN.0 - 2 * GAP, SCREEN.1 - 2 * GAP);
        let size = |r: &mut Random| {
            let w = [half.0, full.0][r.below(2)];
            (w, [half.1, full.1][r.below(2)])
        };
        let (w, h) = size(random);
        let mut rects = vec![rect(0, 0, w, h)];
        for _ in 0..20 {
            let n = rects[random.below(rects.len())];
            let (w, h) = size(random);
            let (x, y) = match random.below(4) {
                0 => (n.loc.x + n.size.w + GAP, n.loc.y),
                1 => (n.loc.x, n.loc.y + n.size.h + GAP),
                2 => (n.loc.x - GAP - w, n.loc.y + n.size.h - h),
                _ => (n.loc.x + n.size.w - w, n.loc.y - GAP - h),
            };
            let new = rect(x, y, w, h);
            if rects.len() < 6 && rects.iter().all(|r| !r.overlaps(new)) {
                rects.push(new);
            }
        }
        rects
    }

    /// Two windows that lie on each other.
    fn overlapping(rects: &[Rect]) -> Option<(usize, usize)> {
        (0..rects.len())
            .flat_map(|i| (i + 1..rects.len()).map(move |j| (i, j)))
            .find(|&(i, j)| rects[i].overlaps(rects[j]))
    }

    #[test]
    fn random_resizes_neither_put_windows_on_each_other_nor_apart() {
        let mut random = Random(0x9e37_79b9_7f4a_7c15);
        let edges = [Edge::None, Edge::Start, Edge::End];
        let (mut resized, mut steps, mut flipped) = (0, 0, 0);
        for case in 0..3000 {
            let mut rects = random_cluster(&mut random);
            let min = vec![Size::from((100, 100)); rects.len()];
            assert_eq!(overlapping(&rects), None);
            for step in 0..8 {
                let dragged = random.below(rects.len());
                let grabbed = [edges[random.below(3)], edges[random.below(3)]];
                let shift = random.below(2) == 0;
                let before = rects.clone();
                steps += 1;
                let Some((follow, delta, sign)) =
                    toggle(&rects, &min, dragged, grabbed, shift, SCREEN.into(), GAP)
                else {
                    continue;
                };
                resized += 1;
                flipped += usize::from(sign.contains(&-1));
                let together = connected(&before, GAP);
                // While dragging, the edges follow the pointer anywhere between the two sizes.
                let (x, y) = (delta.x, delta.y);
                for d in [
                    (x, 0),
                    (0, y),
                    (x / 3, y),
                    (x, y / 2),
                    (x / 2, y / 3),
                    (x, y),
                ] {
                    rects = apply(&before, &follow, &min, d.into());
                    if let Some((i, j)) = overlapping(&rects) {
                        panic!(
                            "case {case}, step {step}: window {dragged} at {grabbed:?} (Shift: \
                             {shift}), moved {d:?}: windows {i} and {j} overlap\n\
                             before: {before:?}\nafter: {rects:?}"
                        );
                    }
                    // Let go, each axis toggles on its own: in all these places, a cluster that
                    // hung together still does (on the way, a growing window only reaches its
                    // new neighbor at the end).
                    let end = [(x, 0), (0, y), (x, y)].contains(&d);
                    assert!(
                        !together || !end || connected(&rects, GAP),
                        "case {case}, step {step}: window {dragged} at {grabbed:?} (Shift: \
                         {shift}), moved {d:?}: the cluster came apart\n\
                         before: {before:?}\nafter: {rects:?}"
                    );
                }
            }
        }
        // Most steps must really resize, and some at the other edge, else the test checks little.
        assert!(
            resized * 2 > steps && flipped * 20 > resized,
            "only {resized} of {steps} steps resized, {flipped} at the other edge"
        );
    }

    #[test]
    fn neighbor_shrinks_with_the_edge() {
        let rects = row();
        let follow = plan(&rects, 0, [Edge::End, Edge::None], false, 0);
        let new = apply(&rects, &follow, &min(), (30, 0).into());
        assert_eq!(new[0], rect(0, 0, 130, 100));
        assert_eq!(new[1], rect(130, 0, 70, 100));
        assert_eq!(new[2], rect(200, 0, 100, 100));
    }

    #[test]
    fn with_shift_everything_beyond_moves() {
        let rects = row();
        let follow = plan(&rects, 0, [Edge::End, Edge::None], true, 0);
        let new = apply(&rects, &follow, &min(), (30, 0).into());
        assert_eq!(new[0], rect(0, 0, 130, 100));
        assert_eq!(new[1], rect(130, 0, 100, 100));
        assert_eq!(new[2], rect(230, 0, 100, 100));
    }

    #[test]
    fn left_edge_takes_the_left_neighbor_along() {
        let rects = row();
        let follow = plan(&rects, 1, [Edge::Start, Edge::None], false, 0);
        let new = apply(&rects, &follow, &min(), (-40, 0).into());
        assert_eq!(new[0], rect(0, 0, 60, 100));
        assert_eq!(new[1], rect(60, 0, 140, 100));
        assert_eq!(new[2], rect(200, 0, 100, 100));
    }

    #[test]
    fn neighbors_do_not_shrink_below_their_minimum() {
        let rects = row();
        let follow = plan(&rects, 0, [Edge::End, Edge::None], false, 0);
        let new = apply(&rects, &follow, &min(), (500, 0).into());
        assert_eq!(new[0], rect(0, 0, 180, 100));
        assert_eq!(new[1], rect(180, 0, 20, 100));
    }

    #[test]
    fn bottom_edge_takes_the_window_below_along() {
        // A above B; dragging A's bottom edge down shrinks B from the top.
        let rects = vec![rect(0, 0, 100, 100), rect(0, 100, 100, 100)];
        let follow = plan(&rects, 0, [Edge::None, Edge::End], false, 0);
        let new = apply(&rects, &follow, &min()[..2], (0, 30).into());
        assert_eq!(new[0], rect(0, 0, 100, 130));
        assert_eq!(new[1], rect(0, 130, 100, 70));
    }

    #[test]
    fn with_shift_the_left_edge_moves_everything_left_of_it() {
        let rects = row();
        let follow = plan(&rects, 2, [Edge::Start, Edge::None], true, 0);
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
        let follow = plan(&rects, 1, [Edge::Start, Edge::None], false, 0);
        let new = apply(&rects, &follow, &min(), (-50, 0).into());
        assert_eq!(new[0], rect(0, 0, 350, 600));
        assert_eq!(new[1], rect(350, 0, 350, 300));
        assert_eq!(new[2], rect(350, 300, 350, 300));
        // Grabbing the right edge of the left window gives the same result.
        let follow = plan(&rects, 0, [Edge::End, Edge::None], false, 0);
        assert_eq!(apply(&rects, &follow, &min(), (-50, 0).into()), new);
    }

    #[test]
    fn with_shift_the_seam_stays_closed() {
        let rects = l_shape();
        // Top-right window grows to the left; the left window moves away, so the bottom-right
        // window grows along instead of leaving a gap.
        let follow = plan(&rects, 1, [Edge::Start, Edge::None], true, 0);
        let new = apply(&rects, &follow, &min(), (-50, 0).into());
        assert_eq!(new[0], rect(-50, 0, 400, 600));
        assert_eq!(new[1], rect(350, 0, 350, 300));
        assert_eq!(new[2], rect(350, 300, 350, 300));
    }

    #[test]
    fn a_column_without_a_neighbor_across_does_not_follow() {
        // A above B, nothing to their right: widening A leaves B alone.
        let rects = vec![rect(0, 0, 100, 100), rect(0, 100, 100, 100)];
        let follow = plan(&rects, 0, [Edge::End, Edge::None], false, 0);
        assert_eq!(follow[1], [Follow::None, Follow::None]);
    }

    #[test]
    fn corner_drag_takes_the_right_windows_along_sideways() {
        // Bottom-right corner of the tall left window: the right windows follow its right edge,
        // nothing follows its bottom edge.
        let follow = plan(&l_shape(), 0, [Edge::End, Edge::End], false, 0);
        assert_eq!(follow[2], [Follow::Start, Follow::None]);
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
    fn neighbors_across_the_gap_follow() {
        // A | B and A above C, 16 px apart: A's right edge takes B along, its bottom edge C.
        let rects = vec![
            rect(0, 0, 100, 100),
            rect(116, 0, 100, 100),
            rect(0, 116, 100, 100),
        ];
        let follow = plan(&rects, 0, [Edge::End, Edge::End], false, 16);
        let new = apply(&rects, &follow, &min(), (30, 20).into());
        assert_eq!(new[1], rect(146, 0, 70, 100));
        assert_eq!(new[2], rect(0, 136, 100, 80));
        // Grabbed at B's left edge: the same seam.
        let follow = plan(&rects, 1, [Edge::Start, Edge::None], false, 16);
        assert_eq!(
            apply(&rects, &follow, &min(), (30, 0).into())[0],
            rect(0, 0, 130, 100)
        );
    }

    #[test]
    fn window_below_is_not_a_neighbor_of_the_right_edge() {
        // A above B; dragging A's right edge must not touch B.
        let rects = vec![rect(0, 0, 100, 100), rect(0, 100, 100, 100)];
        let follow = plan(&rects, 0, [Edge::End, Edge::None], false, 0);
        assert_eq!(follow[1], [Follow::None, Follow::None]);
    }

    /// Screen 1030×630 with 10 px gaps: half 500×300, full 1010×610.
    fn screen() -> Size<i32, Logical> {
        (1030, 630).into()
    }

    /// A and B side by side, full height, `a` and `b` wide.
    fn pair(a: i32, b: i32) -> Vec<Rect> {
        vec![rect(10, 10, a, 610), rect(20 + a, 10, b, 610)]
    }

    fn toggle_a(rects: &[Rect], shift: bool) -> Vec<Rect> {
        let edges = [Edge::End, Edge::None];
        let (follow, delta, _) = toggle(rects, &min()[..2], 0, edges, shift, screen(), 10).unwrap();
        apply(rects, &follow, &min()[..2], delta)
    }

    #[test]
    fn a_full_neighbor_gives_its_half() {
        // A half | B full: A becomes full, B half, the cluster keeps its width.
        assert_eq!(toggle_a(&pair(500, 1010), false), pair(1010, 500));
        // A full | B half: A becomes half, B full.
        assert_eq!(toggle_a(&pair(1010, 500), false), pair(500, 1010));
    }

    #[test]
    fn a_neighbor_that_cannot_moves_aside() {
        // A half | B half: B cannot get smaller, so it moves aside.
        assert_eq!(toggle_a(&pair(500, 500), false), pair(1010, 500));
        // A full | B full: B cannot get larger, so it moves closer.
        assert_eq!(toggle_a(&pair(1010, 1010), false), pair(500, 1010));
        // With Shift, B always moves.
        assert_eq!(toggle_a(&pair(500, 1010), true), pair(1010, 1010));
    }

    #[test]
    fn toggles_once_past_half_and_only_if_it_can() {
        // Full height, the bottom edge: it can only shrink, by 310, so from 155 px upwards.
        let rects = pair(500, 500);
        let edges = [Edge::None, Edge::End];
        let (_, delta, _) = toggle(&rects, &min()[..2], 0, edges, false, screen(), 10).unwrap();
        assert_eq!(delta, (0, -310).into());
        assert_eq!(toggled(delta, (0, -150).into()), (0, 0).into());
        assert_eq!(toggled(delta, (0, -160).into()), (0, -310).into());
        assert_eq!(toggled(delta, (0, 400).into()), (0, 0).into());
        // While dragging, the edge follows, but not beyond either size.
        assert_eq!(following(delta, (7, -150).into()), (0, -150).into());
        assert_eq!(following(delta, (0, -500).into()), (0, -310).into());
        assert_eq!(following(delta, (0, 400).into()), (0, 0).into());
        // A window that must stay taller than half cannot shrink, not even a bit.
        for h in [400, 610] {
            let min: [Size<i32, Logical>; 2] = [(20, h).into(), (20, 20).into()];
            assert_eq!(toggle(&rects, &min, 0, edges, false, screen(), 10), None);
        }
    }

    #[test]
    fn a_corner_tries_both_axes_then_the_one_moved_along_more() {
        let corner = [Edge::Start, Edge::End];
        let (x, y) = ([Edge::Start, Edge::None], [Edge::None, Edge::End]);
        assert_eq!(tries(corner, (-9.0, 3.0).into()), [corner, x, y]);
        assert_eq!(tries(corner, (2.0, 8.0).into()), [corner, y, x]);
        assert_eq!(tries(x, (2.0, 8.0).into()), [x]);
        // Only if every changing axis goes the way the pointer moved.
        assert!(goes_along((-300, 0).into(), [1, 1], (-5.0, 9.0).into()));
        assert!(goes_along((-300, 0).into(), [-1, 1], (5.0, 0.0).into()));
        assert!(!goes_along((-300, 0).into(), [1, 1], (5.0, 0.0).into()));
        assert!(!goes_along((-300, 200).into(), [1, 1], (-5.0, 0.0).into()));
    }

    #[test]
    fn shrinks_at_the_other_edge_to_stay_together() {
        // A full height, B half height next to its lower half. Pulling A's bottom edge up would
        // leave them touching at a corner only, so A's top edge comes down instead.
        let rects = [rect(10, 10, 500, 610), rect(520, 320, 500, 300)];
        let edges = [Edge::None, Edge::End];
        let (follow, delta, sign) =
            toggle(&rects, &min()[..2], 0, edges, false, screen(), 10).unwrap();
        assert_eq!((delta, sign), ((0, 310).into(), [1, -1]));
        let new = apply(&rects, &follow, &min()[..2], delta);
        assert_eq!(new, [rect(10, 320, 500, 300), rect(520, 320, 500, 300)]);
        // A neighbor at the top on one side and at the bottom on the other: the one that would
        // only touch at a corner moves along with the edge.
        let rects = [
            rect(520, 10, 500, 610),
            rect(1030, 10, 500, 300),
            rect(10, 320, 500, 300),
        ];
        let (follow, delta, sign) = toggle(&rects, &min(), 0, edges, false, screen(), 10).unwrap();
        assert_eq!((delta, sign), ((0, -310).into(), [1, 1]));
        let new = apply(&rects, &follow, &min(), delta);
        assert_eq!(
            new,
            [
                rect(520, 10, 500, 300),
                rect(1030, 10, 500, 300),
                rect(10, 10, 500, 300)
            ]
        );
    }

    #[test]
    fn a_neighbor_left_at_a_corner_moves_along() {
        // Pyro's case: top left half, a full width window, bottom right half. The full one gets
        // half as wide at its right edge, the bottom one moves along beneath it.
        let rects = [
            rect(0, 0, 1696, 1408),
            rect(0, 1424, 3408, 1408),
            rect(1712, 2848, 1696, 1408),
        ];
        let min = [Size::from((100, 100)); 3];
        let edges = [Edge::End, Edge::None];
        let (follow, delta, sign) =
            toggle(&rects, &min, 1, edges, false, (3440, 1440).into(), 16).unwrap();
        assert_eq!((delta, sign), ((-1712, 0).into(), [1, 1]));
        let new = apply(&rects, &follow, &min, delta);
        assert_eq!(
            new[1..],
            [rect(0, 1424, 1696, 1408), rect(0, 2848, 1696, 1408)]
        );
        // At its left edge, the top one moves along.
        let edges = [Edge::Start, Edge::None];
        let (follow, delta, _) =
            toggle(&rects, &min, 1, edges, false, (3440, 1440).into(), 16).unwrap();
        let new = apply(&rects, &follow, &min, delta);
        assert_eq!(
            new[..2],
            [rect(1712, 0, 1696, 1408), rect(1712, 1424, 1696, 1408)]
        );
    }
}
