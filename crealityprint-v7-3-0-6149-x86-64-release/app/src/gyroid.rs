//! Gyroid infill: on each layer, the lines where the gyroid surface
//! `sin X cos Y + sin Y cos Z + sin Z cos X = 0` crosses that height. The surface is
//! one continuous, curving wall, so the lines wave and turn from layer to layer, and
//! the infill is about equally strong in every direction.
//!
//! The lines are traced with marching squares over the infill area (about 12 samples
//! per period), joined into long curves, and cut where they leave the area (or enter a
//! part of it to skip), each cut found exactly by bisection.

use crate::geometry::Point2;
use std::collections::HashMap;

/// The gyroid's period is this many times the line spacing, which gives about the
/// density the spacing does for straight lines (the figure Cura uses).
const PERIOD_PER_SPACING: f32 = 2.41;
/// Samples per period when tracing.
const SAMPLES_PER_PERIOD: f32 = 12.0;

/// The gyroid lines at height `z` (mm) for lines `spacing` mm apart, over the box
/// `lo`..`hi`, cut to where `inside` is true. Pieces shorter than `min_len` are left
/// out.
pub fn gyroid_lines(
    lo: Point2,
    hi: Point2,
    spacing: f32,
    z: f32,
    min_len: f32,
    inside: &dyn Fn(Point2) -> bool,
) -> Vec<Vec<Point2>> {
    let period = spacing * PERIOD_PER_SPACING;
    let k = std::f32::consts::TAU / period;
    let step = period / SAMPLES_PER_PERIOD;
    let nx = (((hi.x - lo.x) / step).ceil() as usize).max(1);
    let ny = (((hi.y - lo.y) / step).ceil() as usize).max(1);
    let (sz, cz) = (z * k).sin_cos();
    let xs: Vec<(f32, f32)> = (0..=nx)
        .map(|i| ((lo.x + i as f32 * step) * k).sin_cos())
        .collect();
    let ys: Vec<(f32, f32)> = (0..=ny)
        .map(|j| ((lo.y + j as f32 * step) * k).sin_cos())
        .collect();
    let f = |i: usize, j: usize| {
        let ((sx, cx), (sy, cy)) = (xs[i], ys[j]);
        sx * cy + sy * cz + sz * cx
    };
    let node = |i: usize, j: usize| Point2::new(lo.x + i as f32 * step, lo.y + j as f32 * step);
    // Where the surface crosses the edge between two nodes.
    let cross = |a: (usize, usize), b: (usize, usize)| {
        let (fa, fb) = (f(a.0, a.1), f(b.0, b.1));
        let t = if (fa - fb).abs() < 1e-12 {
            0.5
        } else {
            fa / (fa - fb)
        };
        let (pa, pb) = (node(a.0, a.1), node(b.0, b.1));
        Point2::new(pa.x + (pb.x - pa.x) * t, pa.y + (pb.y - pa.y) * t)
    };
    // Edge ids: horizontal edge (i, j)-(i+1, j) and vertical edge (i, j)-(i, j+1).
    let h_edge = |i: usize, j: usize| 2 * (j * (nx + 1) + i);
    let v_edge = |i: usize, j: usize| 2 * (j * (nx + 1) + i) + 1;
    let mut segments: Vec<[(usize, Point2); 2]> = Vec::new();
    for j in 0..ny {
        for i in 0..nx {
            let corners = [f(i, j), f(i + 1, j), f(i + 1, j + 1), f(i, j + 1)];
            let case = corners
                .iter()
                .enumerate()
                .fold(0, |c, (b, v)| c | (usize::from(*v >= 0.0) << b));
            // The four edges: bottom, right, top, left.
            let bottom = (h_edge(i, j), (i, j), (i + 1, j));
            let right = (v_edge(i + 1, j), (i + 1, j), (i + 1, j + 1));
            let top = (h_edge(i, j + 1), (i, j + 1), (i + 1, j + 1));
            let left = (v_edge(i, j), (i, j), (i, j + 1));
            let e = |edge: (usize, (usize, usize), (usize, usize))| (edge.0, cross(edge.1, edge.2));
            let centre = corners.iter().sum::<f32>() >= 0.0;
            let pairs: Vec<[(usize, Point2); 2]> = match case {
                0 | 15 => Vec::new(),
                1 | 14 => vec![[e(left), e(bottom)]],
                2 | 13 => vec![[e(bottom), e(right)]],
                3 | 12 => vec![[e(left), e(right)]],
                4 | 11 => vec![[e(right), e(top)]],
                6 | 9 => vec![[e(bottom), e(top)]],
                7 | 8 => vec![[e(left), e(top)]],
                // Saddles: the centre's sign says which corners connect.
                5 if centre => vec![[e(left), e(top)], [e(bottom), e(right)]],
                5 => vec![[e(left), e(bottom)], [e(right), e(top)]],
                10 if centre => vec![[e(left), e(bottom)], [e(right), e(top)]],
                _ => vec![[e(left), e(top)], [e(bottom), e(right)]],
            };
            segments.extend(pairs);
        }
    }
    let curves = join(&segments);
    curves
        .iter()
        .flat_map(|c| clip(c, inside))
        .filter(|piece| length(piece) >= min_len)
        .collect()
}

/// Joins segments sharing an edge point into curves.
fn join(segments: &[[(usize, Point2); 2]]) -> Vec<Vec<Point2>> {
    let mut at: HashMap<usize, Vec<usize>> = HashMap::new();
    for (s, seg) in segments.iter().enumerate() {
        for (id, _) in seg {
            at.entry(*id).or_default().push(s);
        }
    }
    let mut used = vec![false; segments.len()];
    let mut out = Vec::new();
    for start in 0..segments.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let [a, b] = segments[start];
        // Grow forwards from b, then backwards from a.
        let mut forward = vec![a.1, b.1];
        let mut backward = Vec::new();
        for (end, list) in [(b.0, &mut forward), (a.0, &mut backward)] {
            let mut id = end;
            while let Some(&next) = at.get(&id).and_then(|v| v.iter().find(|&&s| !used[s])) {
                used[next] = true;
                let [p, q] = segments[next];
                let (far_id, far) = if p.0 == id { q } else { p };
                list.push(far);
                id = far_id;
            }
        }
        backward.reverse();
        backward.extend(forward);
        out.push(backward);
    }
    out
}

/// The parts of `curve` where `inside` holds, each crossing found by bisection. (Also
/// used for honeycomb infill.)
pub fn clip(curve: &[Point2], inside: &dyn Fn(Point2) -> bool) -> Vec<Vec<Point2>> {
    let edge = |a: Point2, b: Point2| {
        // `a` is inside, `b` isn't: the last inside point between them.
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..10 {
            let mid = (lo + hi) / 2.0;
            if inside(lerp(a, b, mid)) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lerp(a, b, lo)
    };
    let mut out = Vec::new();
    let mut piece: Vec<Point2> = Vec::new();
    let mut prev: Option<(Point2, bool)> = None;
    for &p in curve {
        let now = inside(p);
        match prev {
            Some((q, true)) if !now => {
                piece.push(edge(q, p));
                out.push(std::mem::take(&mut piece));
            }
            Some((q, false)) if now => piece.push(edge(p, q)),
            _ => {}
        }
        if now {
            piece.push(p);
        }
        prev = Some((p, now));
    }
    if piece.len() >= 2 {
        out.push(piece);
    }
    out.retain(|p| p.len() >= 2);
    out
}

fn lerp(a: Point2, b: Point2, t: f32) -> Point2 {
    Point2::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

pub fn length(curve: &[Point2]) -> f32 {
    curve.windows(2).map(|w| w[0].distance(w[1])).sum()
}

/// A quick inside test for an area made of loops (outlines and holes): each loop edge
/// is filed under the 1 mm bands of height it spans, so a test only crosses the edges
/// of its own band.
pub struct AreaTest {
    y0: f32,
    bands: Vec<Vec<(Point2, Point2)>>,
}

/// Band height (mm) for `AreaTest`.
const BAND_MM: f32 = 1.0;

impl AreaTest {
    pub fn new(loops: &[Vec<Point2>]) -> Self {
        let y0 = loops.iter().flatten().map(|p| p.y).fold(f32::MAX, f32::min);
        let y1 = loops.iter().flatten().map(|p| p.y).fold(f32::MIN, f32::max);
        if y0 > y1 {
            return Self {
                y0: 0.0,
                bands: Vec::new(),
            };
        }
        let count = ((y1 - y0) / BAND_MM) as usize + 1;
        let mut bands = vec![Vec::new(); count];
        let band = |y: f32| (((y - y0) / BAND_MM) as usize).min(count - 1);
        for l in loops {
            for k in 0..l.len() {
                let (a, b) = (l[k], l[(k + 1) % l.len()]);
                for list in &mut bands[band(a.y.min(b.y))..=band(a.y.max(b.y))] {
                    list.push((a, b));
                }
            }
        }
        Self { y0, bands }
    }

    /// True when `p` is inside an odd number of the loops (in the area, not a hole).
    pub fn contains(&self, p: Point2) -> bool {
        if p.y < self.y0 {
            return false;
        }
        let Some(edges) = self.bands.get(((p.y - self.y0) / BAND_MM) as usize) else {
            return false;
        };
        // The same crossing rule as `slicer::contains`, edge by edge.
        edges
            .iter()
            .filter(|(a, b)| {
                (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x
            })
            .count()
            % 2
            == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slicer::contains;

    fn square(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<Point2> {
        vec![
            Point2::new(x0, y0),
            Point2::new(x1, y0),
            Point2::new(x1, y1),
            Point2::new(x0, y1),
        ]
    }

    #[test]
    fn test_gyroid_lines_are_long_connected_curves_inside_the_region() {
        // A 30 mm square with a 10 mm square hole; lines 1.68 mm apart (25 % infill).
        let outer = square(-15.0, -15.0, 15.0, 15.0);
        let hole = square(-5.0, -5.0, 5.0, 5.0);
        let inside = |p: Point2| contains(&outer, p) && !contains(&hole, p);
        let lines = gyroid_lines(
            Point2::new(-15.0, -15.0),
            Point2::new(15.0, 15.0),
            1.68,
            3.0,
            0.42,
            &inside,
        );
        assert!(!lines.is_empty());
        let near = |p: &Point2, r: &[Point2]| {
            // Inside, or on the edge within a hair.
            let lo = r
                .iter()
                .fold((f32::MAX, f32::MAX), |a, q| (a.0.min(q.x), a.1.min(q.y)));
            let hi = r
                .iter()
                .fold((f32::MIN, f32::MIN), |a, q| (a.0.max(q.x), a.1.max(q.y)));
            p.x > lo.0 + 0.01 && p.x < hi.0 - 0.01 && p.y > lo.1 + 0.01 && p.y < hi.1 - 0.01
        };
        for l in &lines {
            for p in l {
                assert!(p.x.abs() <= 15.01 && p.y.abs() <= 15.01, "{p:?} outside");
                assert!(!near(p, &hole), "{p:?} in the hole");
            }
        }
        // Connected: few, long curves rather than many short dashes.
        let total: f32 = lines.iter().map(|l| length(l)).sum();
        let mean = total / lines.len() as f32;
        assert!(
            mean > 8.0,
            "mean piece {mean} mm over {} pieces",
            lines.len()
        );
        // About the density asked for: line length x spacing is about the area (800 mm2),
        // within 25 %.
        let covered = total * 1.68;
        assert!((covered / 800.0 - 1.0).abs() < 0.25, "{covered}");
    }

    #[test]
    fn test_gyroid_pattern_changes_with_height() {
        let outer = square(0.0, 0.0, 20.0, 20.0);
        let inside = |p: Point2| contains(&outer, p);
        let at = |z: f32| {
            gyroid_lines(
                Point2::new(0.0, 0.0),
                Point2::new(20.0, 20.0),
                1.68,
                z,
                0.42,
                &inside,
            )
        };
        let (a, b) = (at(1.0), at(2.0));
        assert_ne!(format!("{a:?}"), format!("{b:?}"));
        // A full period (2.41 x 1.68 mm) up, the pattern repeats.
        let c = at(1.0 + 1.68 * PERIOD_PER_SPACING);
        let ends = |l: &[Vec<Point2>]| l.len();
        assert_eq!(ends(&a), ends(&c));
    }

    #[test]
    fn test_area_test_agrees_with_the_plain_inside_test() {
        let outer = square(-15.0, -15.0, 15.0, 15.0);
        let hole: Vec<Point2> = square(-5.0, -5.0, 5.0, 5.0).into_iter().rev().collect();
        let tri = vec![
            Point2::new(20.0, 0.0),
            Point2::new(30.0, 3.3),
            Point2::new(22.0, 9.7),
        ];
        let loops = vec![outer.clone(), hole.clone(), tri.clone()];
        let quick = AreaTest::new(&loops);
        for i in 0..400 {
            for j in 0..300 {
                let p = Point2::new(-20.0 + i as f32 * 0.137, -20.0 + j as f32 * 0.113);
                let plain = loops.iter().filter(|l| contains(l, p)).count() % 2 == 1;
                assert_eq!(quick.contains(p), plain, "{p:?}");
            }
        }
    }
}
