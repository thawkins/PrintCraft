//! Planar cut: splits a mesh at a height into the part below and the part above, and
//! closes each part's cut with a flat face (a cap), holes included.
//!
//! Triangles crossing the plane are split along it. The cut edges are chained into
//! loops, sorted into outlines and holes by how deeply they nest, and each outline with
//! its holes is filled by ear clipping (holes are first joined to their outline by a
//! bridge edge). Every cap edge is a copy of a cut edge, so a closed mesh gives two
//! closed parts.

use crate::geometry::{Point2, Vec3};
use crate::model::{Mesh, Triangle};
use crate::slicer::{area, contains};
use std::collections::HashMap;

/// Which parts of a cut to keep.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CutKeep {
    #[default]
    Both,
    Lower,
    Upper,
}

impl CutKeep {
    pub const ALL: [CutKeep; 3] = [Self::Both, Self::Lower, Self::Upper];

    /// How it is named in the window.
    pub fn label(self) -> &'static str {
        match self {
            Self::Both => "Both parts",
            Self::Lower => "Lower part",
            Self::Upper => "Upper part",
        }
    }
}

/// How far (mm) the cut is moved off a corner that lies on its plane, so that no corner
/// sits exactly on it.
const OFF_CORNER_MM: f32 = 1e-3;

/// The parts of `triangles` below and above the height `z`, each closed with a cap
/// along the cut. A part is empty when nothing of the mesh is on its side.
pub fn cut(triangles: &[Triangle], z: f32) -> (Mesh, Mesh) {
    let z = off_corners(triangles, z);
    let mut lower: Vec<Triangle> = Vec::new();
    let mut upper: Vec<Triangle> = Vec::new();
    // Edges along the cut, as the lower part's triangles run along them.
    let mut cut_edges: Vec<(Point2, Point2)> = Vec::new();
    for t in triangles {
        let below = t.map(|v| v.z < z);
        match below.iter().filter(|b| **b).count() {
            3 => lower.push(*t),
            0 => upper.push(*t),
            n => {
                // `a` is the corner alone on its side; b and c follow it in order.
                let k = (0..3)
                    .find(|&i| (below[i] && n == 1) || (!below[i] && n == 2))
                    .unwrap_or(0);
                let (a, b, c) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
                let p = cut_point(a, b, z);
                let q = cut_point(a, c, z);
                let (pv, qv) = (Vec3::new(p.x, p.y, z), Vec3::new(q.x, q.y, z));
                let (alone, pair) = if n == 1 {
                    (&mut lower, &mut upper)
                } else {
                    (&mut upper, &mut lower)
                };
                pair.push([pv, b, c]);
                // A sliver (a, b and c in a line) cuts to one point: its pieces along
                // the cut enclose nothing, and their other edges cancel out.
                if key(p) == key(q) {
                    continue;
                }
                alone.push([a, pv, qv]);
                pair.push([pv, c, qv]);
                // The lower piece runs p -> q when `a` is below, q -> p otherwise.
                cut_edges.push(if n == 1 { (p, q) } else { (q, p) });
            }
        }
    }
    let lid = cap(&cut_edges);
    lower.extend(lid.iter().map(|t| t.map(|p| Vec3::new(p.x, p.y, z))));
    upper.extend(
        lid.iter()
            .map(|t| [t[0], t[2], t[1]].map(|p| Vec3::new(p.x, p.y, z))),
    );
    (Mesh { triangles: lower }, Mesh { triangles: upper })
}

/// `z`, moved up a little while any corner lies (almost) on it.
fn off_corners(triangles: &[Triangle], z: f32) -> f32 {
    let mut z = z;
    for _ in 0..100 {
        if !triangles
            .iter()
            .flatten()
            .any(|v| (v.z - z).abs() < OFF_CORNER_MM * 0.5)
        {
            break;
        }
        z += OFF_CORNER_MM;
    }
    z
}

/// Where edge `a`-`b` crosses height `z`. The ends are put in a fixed order, so the two
/// triangles sharing an edge get exactly the same point.
fn cut_point(a: Vec3, b: Vec3, z: f32) -> Point2 {
    let (a, b) = if (a.x, a.y, a.z) < (b.x, b.y, b.z) {
        (a, b)
    } else {
        (b, a)
    };
    let t = (z - a.z) / (b.z - a.z);
    Point2::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

fn key(p: Point2) -> (u32, u32) {
    (p.x.to_bits(), p.y.to_bits())
}

/// The lower part's cap: triangles facing up (counter-clockwise seen from above) that
/// fill the loops the cut edges make. The lower part runs along each edge in its given
/// direction, so the cap runs the other way.
fn cap(edges: &[(Point2, Point2)]) -> Vec<[Point2; 3]> {
    let mut from: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (i, (_, b)) in edges.iter().enumerate() {
        // Reversed: the cap's edge runs b -> a.
        from.entry(key(*b)).or_default().push(i);
    }
    let mut used = vec![false; edges.len()];
    let mut loops: Vec<Vec<Point2>> = Vec::new();
    for first in 0..edges.len() {
        if used[first] {
            continue;
        }
        used[first] = true;
        let start = edges[first].1;
        let mut ring = vec![start];
        let mut cur = edges[first].0;
        let closed = loop {
            if key(cur) == key(start) {
                break true;
            }
            ring.push(cur);
            let next = from
                .get(&key(cur))
                .and_then(|v| v.iter().copied().find(|&j| !used[j]));
            match next {
                Some(j) => {
                    used[j] = true;
                    cur = edges[j].0;
                }
                None => break false,
            }
        };
        // An open loop (a mesh with a hole at the cut) can't be capped.
        if closed && ring.len() >= 3 {
            loops.push(ring);
        }
    }
    fill_loops(loops)
}

/// Triangles covering the region inside `loops`: outlines and holes, told apart by
/// how many other loops each lies inside.
fn fill_loops(mut loops: Vec<Vec<Point2>>) -> Vec<[Point2; 3]> {
    let inside = |i: usize, loops: &[Vec<Point2>]| -> Vec<usize> {
        (0..loops.len())
            .filter(|&j| j != i && contains(&loops[j], loops[i][0]))
            .collect()
    };
    let depth: Vec<usize> = (0..loops.len()).map(|i| inside(i, &loops).len()).collect();
    for (l, d) in loops.iter_mut().zip(&depth) {
        // Outlines counter-clockwise, holes clockwise.
        if (area(l) > 0.0) != (d % 2 == 0) {
            l.reverse();
        }
    }
    let mut holes: Vec<Vec<usize>> = vec![Vec::new(); loops.len()];
    for i in (0..loops.len()).filter(|&i| depth[i] % 2 == 1) {
        // The hole belongs to the outline just around it.
        if let Some(&parent) = inside(i, &loops)
            .iter()
            .find(|&&j| depth[j] + 1 == depth[i])
        {
            holes[parent].push(i);
        }
    }
    let mut out = Vec::new();
    for i in (0..loops.len()).filter(|&i| depth[i].is_multiple_of(2)) {
        let mut poly = loops[i].clone();
        let mut hs: Vec<&Vec<Point2>> = holes[i].iter().map(|&h| &loops[h]).collect();
        // Rightmost holes first, so a later bridge never crosses an earlier one.
        hs.sort_by(|a, b| max_x(b).total_cmp(&max_x(a)));
        for h in hs {
            poly = bridge(poly, h);
        }
        out.extend(ear_clip(poly));
    }
    out
}

fn max_x(l: &[Point2]) -> f32 {
    l.iter().map(|p| p.x).fold(f32::MIN, f32::max)
}

/// (b - a) x (c - a): positive when a, b, c turn left.
fn turn(a: Point2, b: Point2, c: Point2) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

/// True when `p` is inside or on the edge of triangle a, b, c (counter-clockwise).
fn in_triangle(p: Point2, a: Point2, b: Point2, c: Point2) -> bool {
    turn(a, b, p) >= 0.0 && turn(b, c, p) >= 0.0 && turn(c, a, p) >= 0.0
}

/// The outline `outer` (counter-clockwise) with `hole` (clockwise) joined to it by a
/// two-way edge from the hole's rightmost point to an outline point it can see, giving
/// one loop with no hole (D. Eberly, "Triangulation by Ear Clipping").
fn bridge(outer: Vec<Point2>, hole: &[Point2]) -> Vec<Point2> {
    let (mi, m) = hole
        .iter()
        .copied()
        .enumerate()
        .max_by(|a, b| a.1.x.total_cmp(&b.1.x))
        .unwrap_or((0, hole[0]));
    let n = outer.len();
    // The nearest outline edge straight to the right of m, and its right-hand end.
    let mut best: Option<(f32, usize)> = None;
    for i in 0..n {
        let (a, b) = (outer[i], outer[(i + 1) % n]);
        if (a.y > m.y) == (b.y > m.y) {
            continue;
        }
        let x = a.x + (m.y - a.y) / (b.y - a.y) * (b.x - a.x);
        if x >= m.x && best.is_none_or(|(bx, _)| x < bx) {
            let end = if a.x > b.x { i } else { (i + 1) % n };
            best = Some((x, end));
        }
    }
    let Some((ix, mut pi)) = best else {
        return outer;
    };
    // A corner of the outline inside the triangle m, hit point, end would block the
    // view: take the one making the smallest angle with the ray instead.
    let (hit, p) = (Point2::new(ix, m.y), outer[pi]);
    let (t0, t1, t2) = if turn(m, hit, p) >= 0.0 {
        (m, hit, p)
    } else {
        (m, p, hit)
    };
    let mut best_angle = f32::MAX;
    for (i, &v) in outer.iter().enumerate() {
        let prev = outer[(i + n - 1) % n];
        let next = outer[(i + 1) % n];
        let reflex = turn(prev, v, next) <= 0.0;
        if i == pi || v == p || !reflex || !in_triangle(v, t0, t1, t2) {
            continue;
        }
        let angle = (v.y - m.y).abs().atan2(v.x - m.x);
        if angle < best_angle {
            best_angle = angle;
            pi = i;
        }
    }
    let mut out = Vec::with_capacity(n + hole.len() + 2);
    out.extend_from_slice(&outer[..=pi]);
    out.extend(hole[mi..].iter().chain(&hole[..=mi]).copied());
    out.push(outer[pi]);
    out.extend_from_slice(&outer[pi + 1..]);
    out
}

/// Triangles filling the simple counter-clockwise loop `poly`, cutting off one "ear"
/// (a corner whose triangle holds no other point of the loop) at a time.
fn ear_clip(poly: Vec<Point2>) -> Vec<[Point2; 3]> {
    let mut ring = poly;
    let mut out = Vec::with_capacity(ring.len());
    let mut i = 0;
    let mut misses = 0;
    while ring.len() > 3 {
        let n = ring.len();
        let (a, b, c) = (ring[(i + n - 1) % n], ring[i % n], ring[(i + 1) % n]);
        let t = turn(a, b, c);
        let ear = t >= 0.0
            && !ring
                .iter()
                .any(|&p| p != a && p != b && p != c && in_triangle(p, a, b, c));
        // After a full round with no ear (rounding), cut the next corner anyway so
        // the loop is always filled.
        if ear || misses >= n {
            out.push([a, b, c]);
            ring.remove(i % n);
            misses = 0;
            i = (i + n - 1) % n;
        } else {
            misses += 1;
            i = (i + 1) % n;
        }
    }
    if ring.len() == 3 {
        out.push([ring[0], ring[1], ring[2]]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SampleShape;

    /// Signed volume; positive when the faces point outwards.
    fn volume(m: &Mesh) -> f32 {
        m.triangles
            .iter()
            .map(|t| t[0].dot(t[1].cross(t[2])) / 6.0)
            .sum()
    }

    /// True when every edge is used by exactly two triangles, running opposite ways.
    fn closed(m: &Mesh) -> bool {
        let k = |v: Vec3| (v.x.to_bits(), v.y.to_bits(), v.z.to_bits());
        let mut count: HashMap<_, i32> = HashMap::new();
        for t in &m.triangles {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                *count.entry((k(a), k(b))).or_default() += 1;
            }
        }
        count
            .iter()
            .all(|(&(a, b), &n)| n == 1 && count.get(&(b, a)) == Some(&1))
    }

    fn cube() -> Mesh {
        Mesh::cube(Vec3::new(20.0, 20.0, 20.0))
    }

    #[test]
    fn test_cut_cube_at_half_height_gives_two_closed_halves() {
        let (lower, upper) = cut(&cube().triangles, 10.0);
        assert!(closed(&lower) && closed(&upper));
        assert!((volume(&lower) - 4000.0).abs() < 0.5, "{}", volume(&lower));
        assert!((volume(&upper) - 4000.0).abs() < 0.5, "{}", volume(&upper));
        assert!(lower.triangles.iter().flatten().all(|v| v.z <= 10.0));
        assert!(upper.triangles.iter().flatten().all(|v| v.z >= 10.0));
    }

    #[test]
    fn test_cut_tube_caps_are_rings() {
        let tube = SampleShape::Tube.mesh();
        let whole = volume(&tube);
        let (lower, upper) = cut(&tube.triangles, 10.0);
        assert!(closed(&lower) && closed(&upper));
        let sum = volume(&lower) + volume(&upper);
        assert!((sum - whole).abs() < 0.01 * whole, "{sum} vs {whole}");
        // The cap is a ring: its area is the outer disc less the inner one.
        let cap_area: f32 = lower
            .triangles
            .iter()
            .filter(|t| t.iter().all(|v| (v.z - 10.0).abs() < 1e-4))
            .map(|t| (t[1] - t[0]).cross(t[2] - t[0]).z / 2.0)
            .sum();
        let ring = whole / 25.0;
        assert!(
            (cap_area - ring).abs() < 0.01 * ring,
            "{cap_area} vs {ring}"
        );
    }

    #[test]
    fn test_cut_two_parts_and_a_plane_through_corners() {
        let mut two = cube();
        let other: Vec<Triangle> = cube()
            .triangles
            .iter()
            .map(|t| t.map(|v| v + Vec3::new(40.0, 0.0, 0.0)))
            .collect();
        two.triangles.extend(other);
        // At z = 20 the plane passes the cubes' top corners.
        for z in [5.0, 20.0, 0.0] {
            let (lower, upper) = cut(&two.triangles, z);
            assert!(closed(&lower), "z {z}");
            assert!(closed(&upper), "z {z}");
            let sum = volume(&lower) + volume(&upper);
            assert!((sum - 16000.0).abs() < 1.0, "z {z}: {sum}");
        }
    }

    #[test]
    fn test_cut_above_the_part_keeps_it_whole() {
        let (lower, upper) = cut(&cube().triangles, 30.0);
        assert!(upper.triangles.is_empty());
        assert_eq!(lower.triangles, cube().triangles);
    }

    #[test]
    fn test_cut_fills_a_concave_outline_with_a_hole() {
        // An L-shaped outline with a square hole in its long arm.
        let l = vec![
            Point2::new(0.0, 0.0),
            Point2::new(30.0, 0.0),
            Point2::new(30.0, 10.0),
            Point2::new(10.0, 10.0),
            Point2::new(10.0, 30.0),
            Point2::new(0.0, 30.0),
        ];
        let hole = vec![
            Point2::new(14.0, 3.0),
            Point2::new(14.0, 7.0),
            Point2::new(18.0, 7.0),
            Point2::new(18.0, 3.0),
        ];
        let tris = fill_loops(vec![l, hole]);
        let total: f32 = tris.iter().map(|t| turn(t[0], t[1], t[2]) / 2.0).sum();
        assert!((total - (500.0 - 16.0)).abs() < 1e-3, "{total}");
        assert!(
            tris.iter().all(|t| turn(t[0], t[1], t[2]) >= 0.0),
            "every triangle faces up"
        );
    }

    #[test]
    fn test_cut_through_slivers_of_a_real_model_stays_closed() {
        // XYZ.stl has thin sliver triangles (corners in a line) crossing 12.5 mm; each
        // cuts to a single point, which must not end up as an edge of the cap.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/models/XYZ.stl");
        let m = Mesh::load(&path).unwrap().recentred();
        assert!(closed(&m));
        let (lower, upper) = cut(&m.triangles, 12.5);
        assert!(closed(&lower) && closed(&upper));
        let sum = volume(&lower) + volume(&upper);
        assert!((sum - volume(&m)).abs() < 1.0, "{sum}");
    }
}
