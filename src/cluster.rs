//! Andocken (Snapping) und Cluster.
//!
//! Ein Cluster ist eine Gruppe von Fenstern, die aneinander angedockt wurden und sich gemeinsam bewegen.
//! Die Zugehörigkeit wird explizit gespeichert (nicht nur aus der Geometrie abgeleitet), damit ein Cluster
//! nicht zerfällt, nur weil ein Programm seine Größe minimal ändert.
//!
//! Dieses Modul ist reine Logik ohne Wayland: `K` ist der Fenster-Schlüssel (im Compositor `Window`,
//! in den Tests einfache Zahlen).

use std::{collections::HashMap, hash::Hash};

use smithay::utils::{Logical, Point, Rectangle};

pub type Rect = Rectangle<i32, Logical>;

/// Ab diesem Abstand (in Pixeln) rastet eine Kante an einer anderen ein.
pub const SNAP_THRESHOLD: i32 = 15;
/// Kanten mit höchstens diesem Abstand gelten als "berührend" (verbunden).
pub const TOUCH_TOLERANCE: i32 = 2;

/// Welche Fenster zu welchem Cluster gehören. Fenster ohne Eintrag stehen allein.
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
    /// Alle Fenster im selben Cluster wie `k` (inklusive `k`). Allein stehend: nur `k`.
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

    /// Verbindet die Cluster von `a` und `b` zu einem.
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

    /// Nimmt `k` aus seinem Cluster. Der Rest wird anhand der Geometrie neu aufgeteilt,
    /// denn ohne `k` kann ein Cluster in mehrere Teile zerfallen.
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

/// Liegen zwei Fenster Kante an Kante (mit echter Überlappung entlang der Kante, nicht nur an einer Ecke)?
pub fn touching(a: &Rect, b: &Rect) -> bool {
    let (al, ar, at, ab) = edges(a);
    let (bl, br, bt, bb) = edges(b);
    let h_overlap = ar.min(br) - al.max(bl) > 0;
    let v_overlap = ab.min(bb) - at.max(bt) > 0;
    let side_by_side = (ar - bl).abs() <= TOUCH_TOLERANCE || (al - br).abs() <= TOUCH_TOLERANCE;
    let stacked = (ab - bt).abs() <= TOUCH_TOLERANCE || (at - bb).abs() <= TOUCH_TOLERANCE;
    (side_by_side && v_overlap) || (stacked && h_overlap)
}

/// Berechnet die Korrektur, um die die bewegten Fenster verschoben werden müssen, damit sie an
/// nahen Kanten der anderen Fenster einrasten. Pro Achse gewinnt die kleinste Korrektur.
///
/// Zwei Arten von Einrasten:
/// - **Andocken:** rechte Kante an linke Kante (bzw. oben an unten), wenn die Fenster nebeneinander liegen.
/// - **Ausrichten:** obere Kanten bündig (bzw. linke Kanten), wenn die Fenster gerade aneinander docken.
///   Dadurch entstehen saubere Ecken wie im 2×2-Cluster.
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

            // Überlappen sich die Bereiche (mit etwas Toleranz) auf der jeweils anderen Achse?
            let v_near = mt < ob + threshold && ot < mb + threshold;
            let h_near = ml < or + threshold && ol < mr + threshold;
            // Liegen sie nah Kante an Kante?
            let x_adjacent = (mr - ol).abs() <= threshold || (ml - or).abs() <= threshold;
            let y_adjacent = (mb - ot).abs() <= threshold || (mt - ob).abs() <= threshold;

            if v_near {
                consider(&mut best_x, ol - mr); // links von o andocken
                consider(&mut best_x, or - ml); // rechts von o andocken
            }
            if h_near {
                consider(&mut best_y, ot - mb); // über o andocken
                consider(&mut best_y, ob - mt); // unter o andocken
            }
            if y_adjacent && h_near {
                consider(&mut best_x, ol - ml); // linke Kanten bündig
                consider(&mut best_x, or - mr); // rechte Kanten bündig
            }
            if x_adjacent && v_near {
                consider(&mut best_y, ot - mt); // obere Kanten bündig
                consider(&mut best_y, ob - mb); // untere Kanten bündig
            }
        }
    }

    (best_x.unwrap_or(0), best_y.unwrap_or(0)).into()
}

/// Umgebendes Rechteck mehrerer Rechtecke.
pub fn bounding_box(rects: &[Rect]) -> Option<Rect> {
    rects.iter().copied().reduce(|a, b| a.merge(b))
}

/// (links, rechts, oben, unten)
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
    fn rastet_rechts_daneben_ein_und_richtet_oben_aus() {
        let other = rect(1000, 500, 400, 300);
        // 10 px Lücke zur linken Kante von `other`, 6 px tiefer.
        let moving = rect(590, 506, 400, 300);
        let d = snap_delta(&[moving], &[other], SNAP_THRESHOLD);
        assert_eq!(d, (10, -6).into());
    }

    #[test]
    fn rastet_darunter_ein() {
        let other = rect(0, 0, 400, 300);
        let moving = rect(3, 312, 400, 300);
        let d = snap_delta(&[moving], &[other], SNAP_THRESHOLD);
        assert_eq!(d, (-3, -12).into());
    }

    #[test]
    fn weit_weg_kein_einrasten() {
        let other = rect(0, 0, 400, 300);
        let moving = rect(2000, 2000, 400, 300);
        assert_eq!(snap_delta(&[moving], &[other], SNAP_THRESHOLD), (0, 0).into());
    }

    #[test]
    fn untereinander_weit_entfernt_wird_nicht_ausgerichtet() {
        // Gleiche x-Spalte, aber vertikal weit weg: keine Ausrichtung der linken Kanten.
        let other = rect(0, 0, 400, 300);
        let moving = rect(5, 1000, 400, 300);
        assert_eq!(snap_delta(&[moving], &[other], SNAP_THRESHOLD), (0, 0).into());
    }

    #[test]
    fn cluster_rastet_als_ganzes_ein() {
        // Zwei bewegte Fenster; nur das rechte ist nah an `other`.
        let a = rect(0, 0, 300, 300);
        let b = rect(300, 0, 300, 300);
        let other = rect(608, 0, 300, 300);
        assert_eq!(snap_delta(&[a, b], &[other], SNAP_THRESHOLD), (8, 0).into());
    }

    #[test]
    fn beruehrung() {
        let a = rect(0, 0, 100, 100);
        assert!(touching(&a, &rect(100, 50, 100, 100)));
        assert!(touching(&a, &rect(20, 101, 100, 100)));
        // Nur Ecke an Ecke: kein Cluster.
        assert!(!touching(&a, &rect(100, 100, 100, 100)));
        // Lücke.
        assert!(!touching(&a, &rect(120, 0, 100, 100)));
    }

    #[test]
    fn cluster_verbinden() {
        let mut c = Clusters::default();
        c.join(&1, &2);
        c.join(&3, &4);
        assert_eq!(sorted(c.members(&1)), vec![1, 2]);
        c.join(&2, &3);
        assert_eq!(sorted(c.members(&4)), vec![1, 2, 3, 4]);
        assert_eq!(c.members(&9), vec![9]);
    }

    #[test]
    fn entfernen_teilt_cluster_auf() {
        // Reihe 1 | 2 | 3. Entfernt man die 2, zerfällt der Cluster in 1 und 3.
        let rects = HashMap::from([(1, rect(0, 0, 100, 100)), (2, rect(100, 0, 100, 100)), (3, rect(200, 0, 100, 100))]);
        let mut c = Clusters::default();
        c.join(&1, &2);
        c.join(&2, &3);
        c.remove(&2, |k| rects.get(k).copied());
        assert_eq!(c.members(&1), vec![1]);
        assert_eq!(c.members(&3), vec![3]);
    }

    #[test]
    fn entfernen_laesst_zusammenhaengenden_rest() {
        // 2×2-Block: 1 2 / 3 4. Entfernt man die 1, hängen 2, 3, 4 über die 4 noch zusammen.
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
