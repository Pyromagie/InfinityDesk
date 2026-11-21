//! Layout templates (Mod+R): a cluster is arranged so that it fills exactly one screen, e.g.
//! half | half, or a half and two quarters.
//!
//! This module is pure logic without Wayland: the templates, where their slots lie, which window
//! goes into which slot, and where the menu shows them.

use smithay::utils::{Logical, Point, Size};

use crate::cluster::Rect;

/// A slot of a template in twelfths of the screen: `[left, top, right, bottom]`.
type Slot = [i32; 4];

/// The templates in menu order (keys 1–5), all in the 2×2 grid of window sizes. Slots are in
/// reading order; free slots are filled by new windows in this order.
pub const TEMPLATES: &[&[Slot]] = &[
    &[[0, 0, 12, 12]],                                             // whole screen
    &[[0, 0, 6, 12], [6, 0, 12, 12]],                              // half | half
    &[[0, 0, 6, 12], [6, 0, 12, 6], [6, 6, 12, 12]],               // half | two quarters
    &[[0, 0, 6, 6], [6, 0, 12, 12], [0, 6, 6, 12]],                // two quarters | half
    &[[0, 0, 6, 6], [6, 0, 12, 6], [0, 6, 6, 12], [6, 6, 12, 12]], // four quarters
];

/// The slots of `template` within `area`, `gap` apart from each other and from the area's edges.
pub fn slots(template: usize, area: Rect, gap: i32) -> Vec<Rect> {
    let x = |t: i32| area.loc.x + t * (area.size.w - gap) / 12;
    let y = |t: i32| area.loc.y + t * (area.size.h - gap) / 12;
    (TEMPLATES[template].iter())
        .map(|&[l, t, r, b]| {
            let (left, top) = (x(l) + gap, y(t) + gap);
            Rect::new((left, top).into(), (x(r) - left, y(b) - top).into())
        })
        .collect()
}

/// Which slot each window goes into: `active` into `active_slot` (if given), every other window
/// into the free slot closest to where it lies within the cluster now. Needs at least as many
/// slots as windows.
pub fn assign(
    windows: &[Rect],
    active: usize,
    active_slot: Option<usize>,
    slots: &[Rect],
) -> Vec<usize> {
    // Compare positions relative to the cluster and to the template (0..1 on both axes).
    let center = |r: &Rect, area: Rect| {
        let x = (r.loc.x - area.loc.x) as f64 + r.size.w as f64 / 2.0;
        let y = (r.loc.y - area.loc.y) as f64 + r.size.h as f64 / 2.0;
        (x / area.size.w.max(1) as f64, y / area.size.h.max(1) as f64)
    };
    let bbox = |rects: &[Rect]| rects.iter().copied().reduce(|a, b| a.merge(b));
    let (Some(cluster), Some(template)) = (bbox(windows), bbox(slots)) else {
        return Vec::new();
    };
    let mut pairs = Vec::new();
    for (w, wr) in windows.iter().enumerate() {
        for (s, sr) in slots.iter().enumerate() {
            let (a, b) = (center(wr, cluster), center(sr, template));
            pairs.push(((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2), w, s));
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut slot_of = vec![usize::MAX; windows.len()];
    let mut taken = vec![false; slots.len()];
    if let Some(s) = active_slot {
        slot_of[active] = s;
        taken[s] = true;
    }
    for (_, w, s) in pairs {
        if slot_of[w] == usize::MAX && !taken[s] {
            slot_of[w] = s;
            taken[s] = true;
        }
    }
    slot_of
}

/// Size of a template picture in the menu (its height follows the screen's shape).
const THUMB_WIDTH: i32 = 240;
/// Space between the pictures and around them.
const MENU_GAP: i32 = 24;
const MENU_COLUMNS: i32 = 5;
/// Gap between the slots within a picture.
const SLOT_GAP: i32 = 4;

/// The menu panel on the screen, centered.
pub fn menu_panel(screen: Size<i32, Logical>) -> Rect {
    let thumb_h = THUMB_WIDTH * screen.h / screen.w.max(1);
    let rows = (TEMPLATES.len() as i32 + MENU_COLUMNS - 1) / MENU_COLUMNS;
    let w = MENU_COLUMNS * (THUMB_WIDTH + MENU_GAP) + MENU_GAP;
    let h = rows * (thumb_h + MENU_GAP) + MENU_GAP;
    Rect::new(
        ((screen.w - w) / 2, (screen.h - h) / 2).into(),
        (w, h).into(),
    )
}

/// Where each slot of each template is drawn in the menu (screen coordinates).
pub fn menu_slots(screen: Size<i32, Logical>) -> Vec<Vec<Rect>> {
    let panel = menu_panel(screen);
    let thumb_h = THUMB_WIDTH * screen.h / screen.w.max(1);
    (0..TEMPLATES.len())
        .map(|i| {
            let (col, row) = (i as i32 % MENU_COLUMNS, i as i32 / MENU_COLUMNS);
            let thumb = Rect::new(
                (
                    panel.loc.x + MENU_GAP + col * (THUMB_WIDTH + MENU_GAP),
                    panel.loc.y + MENU_GAP + row * (thumb_h + MENU_GAP),
                )
                    .into(),
                (THUMB_WIDTH, thumb_h).into(),
            );
            slots(i, thumb, SLOT_GAP)
        })
        .collect()
}

/// The template and slot under a screen point in the menu.
pub fn menu_hit(screen: Size<i32, Logical>, pos: Point<f64, Logical>) -> Option<(usize, usize)> {
    let menu = menu_slots(screen);
    menu.iter().enumerate().find_map(|(t, slots)| {
        let s = slots.iter().position(|r| r.to_f64().contains(pos))?;
        Some((t, s))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn slots_tile_the_area_without_gaps() {
        let area = rect(100, 50, 3440, 1440);
        for t in 0..TEMPLATES.len() {
            let s = slots(t, area, 0);
            let total: i32 = s.iter().map(|r| r.size.w * r.size.h).sum();
            assert_eq!(total, 3440 * 1440, "template {t}");
            for (i, a) in s.iter().enumerate() {
                assert!(area.contains_rect(*a));
                assert!(s[i + 1..].iter().all(|b| !a.overlaps(*b)), "template {t}");
            }
        }
        // Halves of an odd width still meet exactly.
        let s = slots(1, rect(0, 0, 1001, 600), 0);
        assert_eq!(s[0], rect(0, 0, 500, 600));
        assert_eq!(s[1], rect(500, 0, 501, 600));
    }

    #[test]
    fn slots_keep_the_gap() {
        // Half | two quarters on 3440×1440 with 16 px gaps: 16 px around and between the slots.
        let s = slots(2, rect(0, 0, 3440, 1440), 16);
        assert_eq!(s[0], rect(16, 16, 1696, 1408));
        assert_eq!(s[1], rect(1728, 16, 1696, 696));
        assert_eq!(s[2], rect(1728, 728, 1696, 696));
    }

    #[test]
    fn windows_go_to_the_closest_slot() {
        // Half | two quarters, cluster: left window, right top, right bottom (shuffled order).
        let s = slots(2, rect(0, 0, 1200, 600), 0);
        let windows = [
            rect(500, 300, 500, 300),
            rect(0, 0, 500, 600),
            rect(500, 0, 500, 300),
        ];
        assert_eq!(assign(&windows, 1, None, &s), vec![2, 0, 1]);
    }

    #[test]
    fn the_active_window_goes_where_it_was_clicked() {
        let s = slots(1, rect(0, 0, 1200, 600), 0);
        // Left window active, but put into the right half: the other one takes the left half.
        let windows = [rect(0, 0, 500, 600), rect(500, 0, 500, 600)];
        assert_eq!(assign(&windows, 0, Some(1), &s), vec![1, 0]);
        // A single window in a template with more slots: the rest stays free.
        assert_eq!(
            assign(
                &windows[..1],
                0,
                Some(2),
                &slots(2, rect(0, 0, 1200, 600), 0)
            ),
            vec![2]
        );
    }

    #[test]
    fn menu_hit_finds_template_and_slot() {
        let screen = (3440, 1440).into();
        let menu = menu_slots(screen);
        assert_eq!(menu.len(), TEMPLATES.len());
        assert!(
            menu.iter()
                .flatten()
                .all(|r| menu_panel(screen).contains_rect(*r))
        );
        // The middle of the second slot of the fourth template.
        let r = menu[3][1];
        let pos = (r.loc.x as f64 + r.size.w as f64 / 2.0, r.loc.y as f64 + 5.0).into();
        assert_eq!(menu_hit(screen, pos), Some((3, 1)));
        // The gap between two slots and the panel corner hit nothing.
        let gap = (r.loc.x as f64 - 2.0, r.loc.y as f64 + 5.0).into();
        assert_eq!(menu_hit(screen, gap), None);
        assert_eq!(menu_hit(screen, menu_panel(screen).loc.to_f64()), None);
    }
}
