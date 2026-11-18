//! Travel routing that avoids crossing walls: a travel that would leave the part (out
//! over air or across a hole) and come back is sent along the inside of the outer wall
//! instead, so it needs no retraction and leaves no string on the outside.
//!
//! Approach: the layer's outer wall loops are the boundaries. Each is widened by just
//! under half a line width to the part's surface, and the straight travel is tested
//! against those surface loops. Wherever it leaves the part and comes back through the
//! same loop, that stretch is replaced by a walk along the wall loop (the shorter way
//! round) between the points nearest the exit and the re-entry. A travel that starts or
//! ends outside the part, or that would have to cross into another island, can't stay
//! inside: no route is returned and the caller travels straight as before.

use crate::geometry::Point2;
use crate::slicer::{area, contains, offset_left, Polygon};

/// A route longer than this many times the straight distance isn't worth it: the caller
/// retracts and travels straight instead.
pub const MAX_DETOUR: f32 = 2.0;

/// The points to travel through from `from` to `to` (ending with `to`, without `from`)
/// staying inside the part, given the layer's outer wall loops `walls` and the line
/// width. `None` when the travel can't stay inside or the detour is too long.
pub fn inside_route(
    from: Point2,
    to: Point2,
    walls: &[Polygon],
    line_width: f32,
) -> Option<Vec<Point2>> {
    let straight = from.distance(to);
    if straight < 1e-6 {
        return Some(vec![to]);
    }
    // Each stretch of the travel over air, as (t at exit, t at re-entry, wall loop index).
    let mut detours: Vec<(f32, f32, usize)> = Vec::new();
    for (wi, wall) in walls.iter().enumerate() {
        if wall.len() < 3 {
            continue;
        }
        let surface = offset_left(wall, -0.45 * line_width).unwrap_or_else(|| wall.clone());
        let hole = area(wall) < 0.0;
        // Material is inside an outer loop, and outside a hole's loop.
        let mut in_material = contains(&surface, from) != hole;
        let ts = crossings(from, to, &surface);
        if !in_material {
            // The travel starts outside this loop (another island): fine unless it goes
            // into that island, which means crossing air to get there.
            if ts.is_empty() {
                continue;
            }
            return None;
        }
        let mut out_at: Option<f32> = None;
        for t in ts {
            if in_material {
                out_at = Some(t);
            } else {
                // Coming back in: pair it with where the travel went out.
                let t0 = out_at.take()?;
                detours.push((t0, t, wi));
            }
            in_material = !in_material;
        }
        if !in_material {
            // Ends outside the part (or started outside and never came in).
            return None;
        }
    }
    detours.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut route = Vec::new();
    let at = |t: f32| Point2::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
    for (t0, t1, wi) in detours {
        route.extend(walk_along(&walls[wi], at(t0), at(t1)));
    }
    route.push(to);
    let length: f32 = std::iter::once(from)
        .chain(route.iter().copied())
        .collect::<Vec<_>>()
        .windows(2)
        .map(|w| w[0].distance(w[1]))
        .sum();
    (length <= MAX_DETOUR * straight).then_some(route)
}

/// Where the segment `a`-`b` crosses the loop, as fractions along the segment, in order.
fn crossings(a: Point2, b: Point2, poly: &[Point2]) -> Vec<f32> {
    let n = poly.len();
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let mut ts: Vec<f32> = (0..n)
        .filter_map(|i| {
            let (p, q) = (poly[i], poly[(i + 1) % n]);
            let (ex, ey) = (q.x - p.x, q.y - p.y);
            let denom = dx * ey - dy * ex;
            if denom.abs() < 1e-12 {
                return None;
            }
            let (wx, wy) = (p.x - a.x, p.y - a.y);
            let t = (wx * ey - wy * ex) / denom;
            let u = (wx * dy - wy * dx) / denom;
            ((0.0..=1.0).contains(&t) && (0.0..1.0).contains(&u)).then_some(t)
        })
        .collect();
    ts.sort_by(|x, y| x.total_cmp(y));
    ts
}

/// The point of the loop's edge `i` nearest to `p`.
fn nearest_on_edge(poly: &[Point2], i: usize, p: Point2) -> Point2 {
    let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
    let (ex, ey) = (b.x - a.x, b.y - a.y);
    let len2 = (ex * ex + ey * ey).max(1e-12);
    let t = (((p.x - a.x) * ex + (p.y - a.y) * ey) / len2).clamp(0.0, 1.0);
    Point2::new(a.x + ex * t, a.y + ey * t)
}

/// The edge of the loop nearest to `p`, and the nearest point on it.
fn nearest(poly: &[Point2], p: Point2) -> (usize, Point2) {
    (0..poly.len())
        .map(|i| (i, nearest_on_edge(poly, i, p)))
        .min_by(|a, b| a.1.distance(p).total_cmp(&b.1.distance(p)))
        .unwrap_or((0, p))
}

/// The way along the loop from the point nearest `from` to the point nearest `to`, the
/// shorter way round, including both of those points.
fn walk_along(poly: &[Point2], from: Point2, to: Point2) -> Vec<Point2> {
    let n = poly.len();
    let (i, p) = nearest(poly, from);
    let (j, q) = nearest(poly, to);
    if i == j {
        return vec![p, q];
    }
    let mut forward = vec![p];
    let mut k = i;
    while k != j {
        k = (k + 1) % n;
        forward.push(poly[k]);
    }
    forward.push(q);
    let mut backward = vec![p];
    let mut k = i;
    while k != j {
        backward.push(poly[k]);
        k = (k + n - 1) % n;
    }
    backward.push(q);
    let length = |v: &[Point2]| v.windows(2).map(|w| w[0].distance(w[1])).sum::<f32>();
    if length(&forward) <= length(&backward) {
        forward
    } else {
        backward
    }
}
