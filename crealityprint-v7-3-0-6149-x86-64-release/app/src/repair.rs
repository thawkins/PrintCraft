//! Mesh repair on import: merges vertices that sit at (almost) the same point, drops
//! triangles that collapsed to a line or a point, turns triangles so neighbours agree on
//! which side is outside, closes simple holes with a fan of triangles, and turns closed
//! parts that are inside out.
//!
//! A clean mesh comes back exactly as it went in (same triangles, same order), so
//! repairing never changes the G-code of a model that needed no repair.

use crate::geometry::Vec3;
use crate::model::Mesh;
use std::collections::HashMap;
use std::fmt;

/// What a repair changed; all zero for a clean mesh.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Repairs {
    /// Points moved onto a neighbour less than `MERGE_MM` away.
    pub merged_vertices: usize,
    /// Triangles dropped because two of their corners are the same point.
    pub dropped_degenerate: usize,
    /// Triangles turned the other way round.
    pub flipped: usize,
    /// Holes closed.
    pub holes_closed: usize,
}

impl Repairs {
    /// True when nothing was changed.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

impl fmt::Display for Repairs {
    /// For example "merged 3 vertices, flipped 2 faces, closed 1 hole".
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let parts: Vec<String> = [
            (self.merged_vertices, "merged", "vertex", "vertices"),
            (
                self.dropped_degenerate,
                "removed",
                "flat face",
                "flat faces",
            ),
            (self.flipped, "flipped", "face", "faces"),
            (self.holes_closed, "closed", "hole", "holes"),
        ]
        .into_iter()
        .filter(|(n, ..)| *n > 0)
        .map(|(n, verb, one, many)| format!("{verb} {}", plural(n, one, many)))
        .collect();
        write!(f, "{}", parts.join(", "))
    }
}

/// Points closer than this (mm) are taken to be the same point.
const MERGE_MM: f32 = 1e-4;

/// `mesh` repaired, and what was changed.
pub fn repair(mesh: Mesh) -> (Mesh, Repairs) {
    let mut report = Repairs::default();
    let (points, corners, merged) = merge_points(&mesh);
    report.merged_vertices = merged;

    // Triangles as point numbers; a triangle with two corners on one point is dropped.
    let mut tris: Vec<[u32; 3]> = Vec::with_capacity(mesh.triangles.len());
    for &[a, b, c] in corners.as_chunks::<3>().0 {
        if a == b || b == c || c == a {
            report.dropped_degenerate += 1;
        } else {
            tris.push([a, b, c]);
        }
    }
    let kept = tris.clone();

    orient_neighbours(&mut tris);
    report.holes_closed = close_holes(&mut tris);
    outward(&mut tris, &points);
    report.flipped = kept
        .iter()
        .zip(&tris)
        .filter(|(before, after)| before != after)
        .count();

    if report.is_empty() {
        return (mesh, report);
    }
    let triangles = tris.iter().map(|t| t.map(|i| points[i as usize])).collect();
    (Mesh { triangles }, report)
}

/// Every distinct point of the mesh (merged within `MERGE_MM`), each triangle corner as
/// a point number, and how many distinct input points were moved onto another.
fn merge_points(mesh: &Mesh) -> (Vec<Vec3>, Vec<u32>, usize) {
    // Identical points first: the common case, and it keeps the neighbour search below
    // to one look per distinct point.
    let mut exact: HashMap<[u32; 3], u32> = HashMap::new();
    let mut distinct: Vec<Vec3> = Vec::new();
    let corner_ids: Vec<u32> = mesh
        .triangles
        .iter()
        .flatten()
        .map(|v| {
            *exact
                .entry([v.x.to_bits(), v.y.to_bits(), v.z.to_bits()])
                .or_insert_with(|| {
                    distinct.push(*v);
                    (distinct.len() - 1) as u32
                })
        })
        .collect();

    // Then each distinct point joins the first kept point within MERGE_MM, found
    // through a grid of MERGE_MM cells (a match can only be in a neighbouring cell).
    let cell = |v: Vec3| [v.x, v.y, v.z].map(|c| (c / MERGE_MM).floor() as i64);
    let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
    let mut points: Vec<Vec3> = Vec::new();
    let mut moved = 0;
    let target: Vec<u32> = distinct
        .iter()
        .map(|&v| {
            let [x, y, z] = cell(v);
            let near = (-1..=1)
                .flat_map(|dx| (-1..=1).flat_map(move |dy| (-1..=1).map(move |dz| [dx, dy, dz])))
                .filter_map(|[dx, dy, dz]| grid.get(&[x + dx, y + dy, z + dz]))
                .flatten()
                .copied()
                .find(|&p| (points[p as usize] - v).length() <= MERGE_MM);
            match near {
                Some(p) => {
                    moved += 1;
                    p
                }
                None => {
                    points.push(v);
                    let id = (points.len() - 1) as u32;
                    grid.entry([x, y, z]).or_default().push(id);
                    id
                }
            }
        })
        .collect();
    let corners = corner_ids.iter().map(|&i| target[i as usize]).collect();
    (points, corners, moved)
}

/// An edge as its two point numbers, lower first.
fn edge_key(a: u32, b: u32) -> (u32, u32) {
    (a.min(b), a.max(b))
}

/// The three edges of a triangle, in its winding order.
fn edges(t: [u32; 3]) -> [(u32, u32); 3] {
    [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])]
}

/// For each edge, the triangles that use it.
fn edge_users(tris: &[[u32; 3]]) -> HashMap<(u32, u32), Vec<usize>> {
    let mut users: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (i, t) in tris.iter().enumerate() {
        for (a, b) in edges(*t) {
            users.entry(edge_key(a, b)).or_default().push(i);
        }
    }
    users
}

/// True when triangle `t` runs along edge `(a, b)` from `a` to `b`.
fn runs(t: [u32; 3], a: u32, b: u32) -> bool {
    edges(t).contains(&(a, b))
}

fn flip(t: &mut [u32; 3]) {
    t.swap(1, 2);
}

/// The parts of the mesh: triangles joined through edges that exactly two use.
fn parts(tris: &[[u32; 3]], users: &HashMap<(u32, u32), Vec<usize>>) -> Vec<Vec<usize>> {
    let mut seen = vec![false; tris.len()];
    let mut out = Vec::new();
    for seed in 0..tris.len() {
        if seen[seed] {
            continue;
        }
        seen[seed] = true;
        let mut part = vec![seed];
        let mut next = 0;
        while next < part.len() {
            let t = tris[part[next]];
            next += 1;
            for (a, b) in edges(t) {
                if let [x, y] = users[&edge_key(a, b)][..] {
                    for u in [x, y] {
                        if !seen[u] {
                            seen[u] = true;
                            part.push(u);
                        }
                    }
                }
            }
        }
        out.push(part);
    }
    out
}

/// Turns triangles so that two sharing an edge run along it in opposite directions
/// (both then face the same side). Within each part, the facing most of its triangles
/// already had is kept.
fn orient_neighbours(tris: &mut [[u32; 3]]) {
    let users = edge_users(tris);
    let mut done = vec![false; tris.len()];
    for seed in 0..tris.len() {
        if done[seed] {
            continue;
        }
        done[seed] = true;
        let mut part = vec![seed];
        let mut turned = Vec::new();
        let mut next = 0;
        while next < part.len() {
            let t = tris[part[next]];
            next += 1;
            for (a, b) in edges(t) {
                let [x, y] = users[&edge_key(a, b)][..] else {
                    continue;
                };
                for u in [x, y] {
                    if done[u] {
                        continue;
                    }
                    done[u] = true;
                    // A neighbour running the same way faces the other side.
                    if runs(tris[u], a, b) {
                        flip(&mut tris[u]);
                        turned.push(u);
                    }
                    part.push(u);
                }
            }
        }
        if turned.len() * 2 > part.len() {
            for &i in &part {
                flip(&mut tris[i]);
            }
        }
    }
}

/// Closes holes: loops of edges that only one triangle uses, each filled with a fan of
/// triangles from its first point. A loop that passes a point twice, or one with as
/// many edges as its part has triangles (the rim of an open sheet, not a hole in a
/// solid), is left open. Returns the number of holes closed.
fn close_holes(tris: &mut Vec<[u32; 3]>) -> usize {
    let users = edge_users(tris);
    // Each open edge, in its triangle's direction; the hole runs the other way.
    let mut from: HashMap<u32, Vec<(u32, usize)>> = HashMap::new();
    for (key, list) in &users {
        if let [i] = list[..] {
            let (a, b) = if runs(tris[i], key.0, key.1) {
                (key.0, key.1)
            } else {
                (key.1, key.0)
            };
            from.entry(a).or_default().push((b, i));
        }
    }
    let part_of: HashMap<usize, usize> = parts(tris, &users)
        .iter()
        .flat_map(|p| p.iter().map(move |&i| (i, p.len())))
        .collect();

    let mut starts: Vec<u32> = from.keys().copied().collect();
    starts.sort_unstable();
    let mut used: HashMap<u32, bool> = HashMap::new();
    let mut fills = Vec::new();
    let mut closed = 0;
    for start in starts {
        if used.contains_key(&start) {
            continue;
        }
        let mut ring = vec![start];
        let mut part_size = 0;
        let ok = loop {
            let cur = *ring.last().unwrap_or(&start);
            let Some(&[(next, tri)]) = from.get(&cur).map(|v| &v[..]) else {
                break false;
            };
            part_size = part_size.max(part_of.get(&tri).copied().unwrap_or(0));
            if next == start {
                break true;
            }
            if ring.contains(&next) || used.contains_key(&next) {
                break false;
            }
            ring.push(next);
        };
        for &p in &ring {
            used.insert(p, true);
        }
        if !ok || ring.len() < 3 || ring.len() >= part_size {
            continue;
        }
        for i in 1..ring.len() - 1 {
            fills.push([ring[0], ring[i + 1], ring[i]]);
        }
        closed += 1;
    }
    tris.extend(fills);
    closed
}

/// Turns closed parts (every edge shared by exactly two triangles) that enclose a
/// negative volume, so their faces point outwards.
fn outward(tris: &mut [[u32; 3]], points: &[Vec3]) {
    let users = edge_users(tris);
    for part in parts(tris, &users) {
        let closed = part.iter().all(|&i| {
            edges(tris[i])
                .iter()
                .all(|&(a, b)| users[&edge_key(a, b)].len() == 2)
        });
        if !closed {
            continue;
        }
        let volume: f64 = part
            .iter()
            .map(|&i| {
                let [a, b, c] = tris[i].map(|p| points[p as usize]);
                f64::from(a.dot(b.cross(c)))
            })
            .sum();
        if volume < 0.0 {
            for &i in &part {
                flip(&mut tris[i]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Triangle;
    use crate::slicer::{area, layer_outline};

    fn cube() -> Mesh {
        Mesh::cube(Vec3::new(20.0, 20.0, 20.0))
    }

    fn flip(t: Triangle) -> Triangle {
        [t[0], t[2], t[1]]
    }

    /// Signed volume of the mesh; positive when the faces point outwards.
    fn volume(m: &Mesh) -> f32 {
        m.triangles
            .iter()
            .map(|t| t[0].dot(t[1].cross(t[2])) / 6.0)
            .sum()
    }

    /// The outlines at a few heights as rounded boxes and areas, for comparing two
    /// meshes (the corner points along an edge depend on how faces were split).
    fn outlines(m: &Mesh) -> Vec<Vec<[i32; 5]>> {
        [0.3, 5.0, 10.0, 15.0, 19.7]
            .into_iter()
            .map(|z| {
                let mut polys: Vec<[i32; 5]> = layer_outline(&m.triangles, z)
                    .iter()
                    .map(|p| {
                        let (mut lo, mut hi) = (p[0], p[0]);
                        for q in p {
                            (lo.x, lo.y) = (lo.x.min(q.x), lo.y.min(q.y));
                            (hi.x, hi.y) = (hi.x.max(q.x), hi.y.max(q.y));
                        }
                        [lo.x, lo.y, hi.x, hi.y, area(p)].map(|v| v.round() as i32)
                    })
                    .collect();
                polys.sort();
                polys
            })
            .collect()
    }

    #[test]
    fn test_repair_leaves_a_clean_mesh_exactly_as_it_was() {
        let clean = cube();
        let (out, r) = repair(clean.clone());
        assert!(r.is_empty(), "{r:?}");
        assert_eq!(out.triangles, clean.triangles);
        let cylinder = Mesh::cylinder(5.0, 3.0, 10.0, 32);
        let (out, r) = repair(cylinder.clone());
        assert!(r.is_empty(), "{r:?}");
        assert_eq!(out.triangles, cylinder.triangles);
    }

    #[test]
    fn test_repair_cube_with_missing_face_and_flipped_faces_slices_like_a_clean_cube() {
        let clean = cube();
        let mut broken = clean.clone();
        // Side x = +10 (triangles 6 and 7) is missing; the front side (4, 5) is flipped.
        broken.triangles.drain(6..8);
        broken.triangles[4] = flip(broken.triangles[4]);
        broken.triangles[5] = flip(broken.triangles[5]);
        // Its faces no longer enclose the cube (support and overhang checks read them).
        assert!(
            (volume(&broken) - 8000.0).abs() > 100.0,
            "{}",
            volume(&broken)
        );
        let (fixed, r) = repair(broken);
        assert_eq!(r.holes_closed, 1, "{r:?}");
        assert_eq!(r.flipped, 2, "{r:?}");
        assert_eq!(fixed.triangles.len(), 12);
        assert_eq!(outlines(&fixed), outlines(&clean));
        assert!((volume(&fixed) - 8000.0).abs() < 1.0, "{}", volume(&fixed));
    }

    #[test]
    fn test_repair_turns_an_inside_out_part_the_right_way() {
        let mut m = cube();
        m.triangles = m.triangles.into_iter().map(flip).collect();
        assert!(volume(&m) < 0.0);
        let (fixed, r) = repair(m);
        assert_eq!(r.flipped, 12, "{r:?}");
        assert!(volume(&fixed) > 0.0);
    }

    #[test]
    fn test_repair_merges_near_vertices_and_drops_collapsed_triangles() {
        let mut m = cube();
        // One corner of one triangle is a hair away from where its neighbours have it.
        // (Not its first copy, which the others would join.)
        m.triangles[7][2].x += 0.00002;
        // A triangle whose corners collapse onto one point once merged.
        let p = m.triangles[0][0];
        m.triangles.push([
            p,
            p + Vec3::new(0.00001, 0.0, 0.0),
            p + Vec3::new(0.0, 0.00001, 0.0),
        ]);
        let (fixed, r) = repair(m);
        // The moved corner, and two corners of the collapsed triangle.
        assert_eq!(r.merged_vertices, 3, "{r:?}");
        assert_eq!(r.dropped_degenerate, 1, "{r:?}");
        assert_eq!(fixed.triangles, cube().triangles);
    }

    #[test]
    fn test_repair_open_sheet_faces_agree_and_its_rim_is_not_a_hole() {
        // A flat square of two triangles, the second turned over. Its rim is the whole
        // edge of the part, not a hole in it, so it stays open; the faces must agree,
        // facing up as the first one does.
        let a = Vec3::new(0.0, 0.0, 1.0);
        let b = Vec3::new(10.0, 0.0, 1.0);
        let c = Vec3::new(10.0, 10.0, 1.0);
        let d = Vec3::new(0.0, 10.0, 1.0);
        let sheet = Mesh {
            triangles: vec![[a, b, c], [a, d, c]],
        };
        let (fixed, r) = repair(sheet);
        assert_eq!(r.flipped, 1, "{r:?}");
        assert_eq!(r.holes_closed, 0, "{r:?}");
        assert_eq!(fixed.triangles, vec![[a, b, c], [a, c, d]]);
    }

    #[test]
    fn test_repair_report_reads_as_a_sentence() {
        let r = Repairs {
            merged_vertices: 3,
            dropped_degenerate: 0,
            flipped: 1,
            holes_closed: 2,
        };
        assert_eq!(
            r.to_string(),
            "merged 3 vertices, flipped 1 face, closed 2 holes"
        );
    }
}
