//! Arc fitting: runs of short straight segments that lie on a circle are written as one
//! G2/G3 arc instead. Round walls come out smoother on the printer (it moves along a
//! true arc instead of stopping at each facet) and the file gets much shorter.
//!
//! A run becomes an arc when every point of it is within `POINT_TOL_MM` of the circle
//! through its first, middle and last points, and the middle of every segment is within
//! `CHORD_TOL_MM` of it (the faces of a round part saved as an STL bulge in by about
//! that much; corners of a straight-sided shape bulge in far more, so they stay lines).

use crate::geometry::Point2;

/// How far (mm) a point of the path may be from the arc that replaces it.
pub const POINT_TOL_MM: f32 = 0.01;
/// How far (mm) the middle of a segment may be from the arc.
pub const CHORD_TOL_MM: f32 = 0.05;
/// Fewest segments an arc replaces.
const MIN_SEGMENTS: usize = 3;
/// Arcs tighter or flatter than this (radius, mm) stay lines.
const MIN_RADIUS_MM: f32 = 0.5;
const MAX_RADIUS_MM: f32 = 500.0;
/// Longest turn (radians) of one arc, short of a full circle so start and end differ.
const MAX_SWEEP: f32 = 1.9 * std::f32::consts::PI;

/// One move of a path after fitting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Move {
    /// Straight to the point.
    Line(Point2),
    /// Along the circle around `centre` to `to`, counter-clockwise (G3) or clockwise (G2).
    Arc {
        to: Point2,
        centre: Point2,
        ccw: bool,
    },
}

impl Move {
    /// Where the move ends.
    pub fn end(&self) -> Point2 {
        match *self {
            Move::Line(p) | Move::Arc { to: p, .. } => p,
        }
    }

    /// The length of the move (mm), starting from `from`.
    pub fn length(&self, from: Point2) -> f32 {
        match *self {
            Move::Line(p) => from.distance(p),
            Move::Arc { to, centre, ccw } => centre.distance(from) * sweep(from, to, centre, ccw),
        }
    }
}

/// The moves that follow `pts[0]` along the path: arcs where runs of points fit one,
/// straight lines elsewhere. Their ends are the path's points, in order.
///
/// Greedy: from each point, the run is grown one point at a time for as long as it
/// still fits one arc; the longest fitting run becomes the arc.
pub fn fit_arcs(pts: &[Point2]) -> Vec<Move> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < pts.len() {
        let mut best = None;
        let mut j = i + MIN_SEGMENTS;
        while j < pts.len() {
            match arc_through(&pts[i..=j]) {
                Some(m) => {
                    best = Some((j, m));
                    j += 1;
                }
                None => break,
            }
        }
        match best {
            Some((j, m)) => {
                out.push(m);
                i = j;
            }
            None => {
                out.push(Move::Line(pts[i + 1]));
                i += 1;
            }
        }
    }
    out
}

/// The arc through the first, middle and last points of `run`, when every point and
/// every segment's middle are close enough to it and the run turns one way, by less
/// than a full circle.
fn arc_through(run: &[Point2]) -> Option<Move> {
    let (a, b, c) = (run[0], run[run.len() / 2], run[run.len() - 1]);
    // The circle through three points (worked in f64: nearly straight runs give tiny
    // denominators).
    let (ax, ay, bx, by, cx, cy) = (
        a.x as f64, a.y as f64, b.x as f64, b.y as f64, c.x as f64, c.y as f64,
    );
    let d = 2.0 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by));
    if d.abs() < 1e-9 {
        return None;
    }
    let (a2, b2, c2) = (ax * ax + ay * ay, bx * bx + by * by, cx * cx + cy * cy);
    let ux = (a2 * (by - cy) + b2 * (cy - ay) + c2 * (ay - by)) / d;
    let uy = (a2 * (cx - bx) + b2 * (ax - cx) + c2 * (bx - ax)) / d;
    let centre = Point2::new(ux as f32, uy as f32);
    let r = ((ax - ux).powi(2) + (ay - uy).powi(2)).sqrt() as f32;
    if !(MIN_RADIUS_MM..=MAX_RADIUS_MM).contains(&r) {
        return None;
    }
    let ccw = (bx - ax) * (cy - by) - (by - ay) * (cx - bx) > 0.0;
    let off = |p: Point2| (centre.distance(p) - r).abs();
    let mut turned = 0.0;
    for w in run.windows(2) {
        let mid = Point2::new((w[0].x + w[1].x) / 2.0, (w[0].y + w[1].y) / 2.0);
        if off(w[1]) > POINT_TOL_MM || off(mid) > CHORD_TOL_MM {
            return None;
        }
        // Each step goes forward around the circle (a step back shows as nearly a turn).
        let step = sweep(w[0], w[1], centre, ccw);
        if step > std::f32::consts::PI {
            return None;
        }
        turned += step;
    }
    (turned <= MAX_SWEEP).then_some(Move::Arc { to: c, centre, ccw })
}

/// The angle (radians, 0..2 pi) turned going from `a` to `b` around `c`.
fn sweep(a: Point2, b: Point2, c: Point2, ccw: bool) -> f32 {
    let ang = |p: Point2| (p.y - c.y).atan2(p.x - c.x);
    let mut d = ang(b) - ang(a);
    if !ccw {
        d = -d;
    }
    d.rem_euclid(std::f32::consts::TAU)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn polygon(cx: f32, cy: f32, r: f32, sides: usize, from: usize, to: usize) -> Vec<Point2> {
        (from..=to)
            .map(|i| {
                let a = i as f32 * std::f32::consts::TAU / sides as f32;
                Point2::new(cx + r * a.cos(), cy + r * a.sin())
            })
            .collect()
    }

    /// The distance of `p` from the arc of `m` (starting at `from`) when `p` lies along
    /// it, else from its ends.
    fn off_arc(p: Point2, from: Point2, m: &Move) -> f32 {
        match *m {
            Move::Line(to) => {
                let (dx, dy) = (to.x - from.x, to.y - from.y);
                let len2 = (dx * dx + dy * dy).max(1e-12);
                let t = (((p.x - from.x) * dx + (p.y - from.y) * dy) / len2).clamp(0.0, 1.0);
                p.distance(Point2::new(from.x + t * dx, from.y + t * dy))
            }
            Move::Arc { to, centre, ccw } => {
                let r = centre.distance(from);
                let along = sweep(from, p, centre, ccw) <= sweep(from, to, centre, ccw) + 1e-4;
                if along {
                    (centre.distance(p) - r).abs()
                } else {
                    p.distance(from).min(p.distance(to))
                }
            }
        }
    }

    /// Every point of `pts` is within `tol` of the move that covers it.
    fn check_cover(pts: &[Point2], moves: &[Move], tol: f32) {
        let mut from = pts[0];
        let mut k = 1;
        for m in moves {
            while k < pts.len() && pts[k] != m.end() {
                let d = off_arc(pts[k], from, m);
                assert!(d <= tol, "point {k} is {d} mm off {m:?}");
                k += 1;
            }
            assert!(k < pts.len(), "move {m:?} doesn't end on a path point");
            k += 1;
            from = m.end();
        }
        assert_eq!(k, pts.len(), "moves stop short of the path's end");
    }

    #[test]
    fn test_arcs_replace_a_faceted_circle() {
        // A 64-sided circle, radius 10 (faces bulge in 0.012 mm), all the way round.
        let pts = polygon(5.0, -3.0, 10.0, 64, 0, 64);
        let moves = fit_arcs(&pts);
        assert!(moves.len() <= 3, "{moves:?}");
        assert!(moves
            .iter()
            .all(|m| matches!(m, Move::Arc { ccw: true, .. })));
        check_cover(&pts, &moves, POINT_TOL_MM);
        let total: f32 = {
            let mut from = pts[0];
            moves
                .iter()
                .map(|m| {
                    let l = m.length(from);
                    from = m.end();
                    l
                })
                .sum()
        };
        assert!(
            (total - std::f32::consts::TAU * 10.0).abs() < 0.05,
            "{total}"
        );
    }

    #[test]
    fn test_arcs_follow_both_directions() {
        // An S: a quarter circle turning left, then one turning right.
        let mut pts = polygon(0.0, 10.0, 10.0, 64, 48, 64);
        // Around (20, 10) clockwise from (10, 10) to (20, 20).
        let right: Vec<Point2> = polygon(20.0, 10.0, 10.0, 64, 16, 32)
            .into_iter()
            .rev()
            .collect();
        // The second quarter starts where the first ends, (10, 10).
        assert!(right[0].distance(*pts.last().unwrap()) < 1e-4);
        pts.extend(right.into_iter().skip(1));
        let moves = fit_arcs(&pts);
        check_cover(&pts, &moves, POINT_TOL_MM);
        let dirs: Vec<bool> = moves
            .iter()
            .filter_map(|m| match m {
                Move::Arc { ccw, .. } => Some(*ccw),
                Move::Line(_) => None,
            })
            .collect();
        assert!(dirs.contains(&true) && dirs.contains(&false), "{moves:?}");
        assert!(moves.len() <= 4, "{moves:?}");
    }

    #[test]
    fn test_arcs_leave_corners_and_straight_lines() {
        // The four corners of a square lie on one circle, but the sides don't.
        let square = vec![
            Point2::new(0.0, 0.0),
            Point2::new(10.0, 0.0),
            Point2::new(10.0, 10.0),
            Point2::new(0.0, 10.0),
            Point2::new(0.0, 0.0),
        ];
        assert!(fit_arcs(&square).iter().all(|m| matches!(m, Move::Line(_))));
        let straight: Vec<Point2> = (0..20).map(|i| Point2::new(i as f32, 2.0)).collect();
        let moves = fit_arcs(&straight);
        assert!(moves.iter().all(|m| matches!(m, Move::Line(_))));
        assert_eq!(moves.len(), 19);
        // A coarse circle (8 sides) is too far off a true one.
        let coarse = polygon(0.0, 0.0, 10.0, 8, 0, 8);
        assert!(fit_arcs(&coarse).iter().all(|m| matches!(m, Move::Line(_))));
    }
}
