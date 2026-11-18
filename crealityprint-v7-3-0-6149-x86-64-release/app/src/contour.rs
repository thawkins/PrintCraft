//! Contour lines: where a function sampled on a square grid crosses zero, traced with
//! marching squares and joined into curves. Used by gyroid infill and variable-width
//! walls.

use crate::geometry::Point2;
use std::collections::HashMap;

/// The curves where `f` (sampled at grid nodes `(i, j)`, i in 0..=nx, j in 0..=ny, at
/// `lo + (i, j) * step`) crosses zero. A node counts as above zero when `f >= 0`; on a
/// saddle square the average of its corners decides which corners connect. A closed
/// curve ends where it starts.
pub fn contour_lines(
    lo: Point2,
    step: f32,
    nx: usize,
    ny: usize,
    f: &dyn Fn(usize, usize) -> f32,
) -> Vec<Vec<Point2>> {
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
    join(&segments)
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
