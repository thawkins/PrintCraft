//! Variable-width walls (after Arachne): walls follow the shape where they fit, and
//! where a part is too thin for them a single line down its middle takes over, as wide
//! as the space left, so thin features print without gaps or overlaps.
//!
//! The layer's distance from the outline is sampled on a grid a third of a line width
//! apart, only within a band near the outline (deeper than all the walls, nothing
//! changes). Wall k is the contour at (k + 1/2) line widths, kept where the shape is
//! thick enough for it and its partner on the far side: where the deepest point
//! nearby is at least (k + 3/4) line widths in. Along the middle of what is thinner
//! (the ridge of the distance, thinned to one sample wide) runs a centre line as wide
//! as twice the depth left after the walls that fit, from half to one and a half line
//! widths; thinner spaces are left empty.

use super::{PathKind, Polygon, ToolPath};
use crate::geometry::Point2;

/// Narrowest centre line laid, as a share of the line width.
const MIN_WIDTH: f32 = 0.5;
/// A wall is kept where the shape is this many line widths deeper than its middle.
const FIT: f32 = 0.25;

/// The walls (and centre lines) of a layer's `outline`, innermost first.
pub(super) fn variable_walls(outline: &[Polygon], walls: u32, lw: f32) -> Vec<ToolPath> {
    let field = Field::new(outline, walls, lw);
    let mut out = field.centre_lines(walls, lw);
    for k in (0..walls).rev() {
        let kind = if k == 0 {
            PathKind::OuterWall
        } else {
            PathKind::InnerWall
        };
        out.extend(field.wall(k, lw, kind));
    }
    out
}

/// Signed distance from the outline (positive inside), sampled near it.
struct Field {
    lo: Point2,
    step: f32,
    nx: usize,
    ny: usize,
    /// Distance at each node, clamped to +-`reach`.
    d: Vec<f32>,
    reach: f32,
    /// The outline's edges.
    edges: Vec<(Point2, Point2)>,
}

impl Field {
    fn new(outline: &[Polygon], walls: u32, lw: f32) -> Self {
        let step = lw / 3.0;
        let reach = lw * (walls as f32 + 1.0);
        let pad = reach + step * 2.0;
        let (mut lo, mut hi) = (
            Point2::new(f32::MAX, f32::MAX),
            Point2::new(f32::MIN, f32::MIN),
        );
        for p in outline.iter().flatten() {
            (lo.x, lo.y) = (lo.x.min(p.x), lo.y.min(p.y));
            (hi.x, hi.y) = (hi.x.max(p.x), hi.y.max(p.y));
        }
        if lo.x > hi.x {
            return Self {
                lo: Point2::new(0.0, 0.0),
                step,
                nx: 0,
                ny: 0,
                d: Vec::new(),
                reach,
                edges: Vec::new(),
            };
        }
        let lo = Point2::new(lo.x - pad, lo.y - pad);
        let nx = ((hi.x + pad - lo.x) / step).ceil() as usize;
        let ny = ((hi.y + pad - lo.y) / step).ceil() as usize;
        let w = nx + 1;
        // Unsigned distance within `reach` of each edge.
        let mut d = vec![reach; w * (ny + 1)];
        for l in outline {
            for k in 0..l.len() {
                let (a, b) = (l[k], l[(k + 1) % l.len()]);
                let i0 = (((a.x.min(b.x) - reach - lo.x) / step).floor().max(0.0)) as usize;
                let i1 = ((((a.x.max(b.x) + reach - lo.x) / step).ceil()) as usize).min(nx);
                let j0 = (((a.y.min(b.y) - reach - lo.y) / step).floor().max(0.0)) as usize;
                let j1 = ((((a.y.max(b.y) + reach - lo.y) / step).ceil()) as usize).min(ny);
                for j in j0..=j1 {
                    for i in i0..=i1 {
                        let p = Point2::new(lo.x + i as f32 * step, lo.y + j as f32 * step);
                        let e = segment_distance(p, a, b);
                        let cell = &mut d[j * w + i];
                        *cell = cell.min(e);
                    }
                }
            }
        }
        // Inside or out, row by row (crossings of each row, even-odd).
        for j in 0..=ny {
            let y = lo.y + j as f32 * step;
            let mut xs: Vec<f32> = Vec::new();
            for l in outline {
                for k in 0..l.len() {
                    let (a, b) = (l[k], l[(k + 1) % l.len()]);
                    if (a.y > y) != (b.y > y) {
                        xs.push(a.x + (y - a.y) * (b.x - a.x) / (b.y - a.y));
                    }
                }
            }
            xs.sort_by(|a, b| a.total_cmp(b));
            let mut inside = vec![false; w];
            for pair in xs.as_chunks::<2>().0 {
                let i0 = ((pair[0] - lo.x) / step).ceil().max(0.0) as usize;
                let i1 = ((pair[1] - lo.x) / step).floor();
                if i1 < 0.0 {
                    continue;
                }
                for flag in inside.iter_mut().take((i1 as usize).min(nx) + 1).skip(i0) {
                    *flag = true;
                }
            }
            for (i, inn) in inside.iter().enumerate() {
                if !inn {
                    d[j * w + i] = -d[j * w + i];
                }
            }
        }
        let edges = outline
            .iter()
            .flat_map(|l| (0..l.len()).map(move |k| (l[k], l[(k + 1) % l.len()])))
            .collect();
        Self {
            lo,
            step,
            nx,
            ny,
            d,
            reach,
            edges,
        }
    }

    fn at(&self, i: usize, j: usize) -> f32 {
        self.d[j * (self.nx + 1) + i]
    }

    fn node(&self, i: usize, j: usize) -> Point2 {
        Point2::new(
            self.lo.x + i as f32 * self.step,
            self.lo.y + j as f32 * self.step,
        )
    }

    /// The depth of the ridge at node (i, j) (an inner node), allowing for the ridge
    /// passing between nodes: the distance rises and falls at about slope 1 on either
    /// side of a ridge, so across it, with neighbours a and b a step s away, the peak is
    /// (a + b) / 2 + s. The smallest such estimate over the four directions (the one
    /// most nearly across), at least the node's own value and at most half a step more.
    fn ridge_depth(&self, i: usize, j: usize) -> f32 {
        let d = self.at(i, j);
        let s = self.step;
        let diag = s * std::f32::consts::SQRT_2;
        let pairs = [
            ((1, 0), (-1, 0), s),
            ((0, 1), (0, -1), s),
            ((1, 1), (-1, -1), diag),
            ((1, -1), (-1, 1), diag),
        ];
        let across = pairs
            .iter()
            .map(|&((ax, ay), (bx, by), step)| {
                let a = self.at((i as i64 + ax) as usize, (j as i64 + ay) as usize);
                let b = self.at((i as i64 + bx) as usize, (j as i64 + by) as usize);
                (a + b) / 2.0 + step
            })
            .fold(f32::MAX, f32::min);
        across.clamp(d, d + s / 2.0)
    }

    /// How deep the shape gets climbing uphill from `p` (stopping once at `need`): at
    /// a corner the climb carries on inwards; on a thin part it stops on the ridge
    /// rather than climbing along it.
    fn climb(&self, p: Point2, need: f32) -> f32 {
        let mut i = (((p.x - self.lo.x) / self.step).round().max(1.0) as usize).min(self.nx - 1);
        let mut j = (((p.y - self.lo.y) / self.step).round().max(1.0) as usize).min(self.ny - 1);
        for _ in 0..10_000 {
            let d = self.at(i, j);
            if d >= need {
                return d;
            }
            if self.is_ridge(i, j) && self.thin_across(i, j) {
                return self.ridge_depth(i, j);
            }
            let mut best = (d, i, j);
            for (dx, dy) in [
                (1, 0),
                (-1, 0),
                (0, 1),
                (0, -1),
                (1, 1),
                (1, -1),
                (-1, 1),
                (-1, -1),
            ] {
                let (x, y) = ((i as i64 + dx) as usize, (j as i64 + dy) as usize);
                if x == 0 || y == 0 || x >= self.nx || y >= self.ny {
                    continue;
                }
                if self.at(x, y) > best.0 {
                    best = (self.at(x, y), x, y);
                }
            }
            if best.1 == i && best.2 == j {
                return self.ridge_depth(i, j);
            }
            (i, j) = (best.1, best.2);
        }
        self.at(i, j)
    }

    /// Whether inner node (i, j) is at least as deep as both its neighbours along some
    /// direction (and deeper than one): on a ridge of the distance.
    fn is_ridge(&self, i: usize, j: usize) -> bool {
        let d = self.at(i, j);
        [(1, 0), (0, 1), (1, 1), (1, -1)].iter().any(|&(ax, ay)| {
            let a = self.at((i as i64 + ax) as usize, (j as i64 + ay) as usize);
            let b = self.at((i as i64 - ax) as usize, (j as i64 - ay) as usize);
            d >= a && d >= b && (d > a || d > b)
        })
    }

    /// Whether the shape is thin at node (i, j): its nearest edges lie on opposite
    /// sides (at least 120 degrees apart), not round a corner.
    fn thin_across(&self, i: usize, j: usize) -> bool {
        let p = self.node(i, j);
        let reach = self.at(i, j) + self.step * 1.5;
        let dirs: Vec<Point2> = self
            .edges
            .iter()
            .filter(|(a, b)| {
                p.x >= a.x.min(b.x) - reach
                    && p.x <= a.x.max(b.x) + reach
                    && p.y >= a.y.min(b.y) - reach
                    && p.y <= a.y.max(b.y) + reach
            })
            .filter_map(|&(a, b)| {
                let q = nearest_on_segment(p, a, b);
                let len = p.distance(q);
                (len <= reach && len > 1e-6)
                    .then(|| Point2::new((q.x - p.x) / len, (q.y - p.y) / len))
            })
            .collect();
        dirs.iter()
            .any(|u| dirs.iter().any(|v| u.x * v.x + u.y * v.y <= -0.5))
    }

    /// Wall `k`: the contour at (k + 1/2) line widths, where it fits.
    fn wall(&self, k: u32, lw: f32, kind: PathKind) -> Vec<ToolPath> {
        if self.nx == 0 {
            return Vec::new();
        }
        let level = lw * (k as f32 + 0.5);
        let need = lw * (k as f32 + 0.5 + FIT);
        let curves =
            crate::contour::contour_lines(self.lo, self.step, self.nx, self.ny, &|i, j| {
                self.at(i, j) - level
            });
        let mut out = Vec::new();
        for c in curves {
            let fits: Vec<bool> = c.iter().map(|p| self.climb(*p, need) >= need).collect();
            let closed = c.len() > 3 && c[0].distance(c[c.len() - 1]) < 1e-4;
            if closed && fits.iter().all(|f| *f) {
                let mut points = c;
                points.pop();
                out.push(ToolPath {
                    kind,
                    points,
                    closed: true,
                    width: None,
                });
                continue;
            }
            let mut run: Vec<Point2> = Vec::new();
            for (p, f) in c.iter().zip(&fits) {
                if *f {
                    run.push(*p);
                } else if run.len() >= 2 {
                    out.push(open(kind, std::mem::take(&mut run), None));
                } else {
                    run.clear();
                }
            }
            if run.len() >= 2 {
                out.push(open(kind, run, None));
            }
        }
        out.retain(|p| length(&p.points) >= lw);
        out
    }

    /// Centre lines where the shape is too thin for the next wall pair: along the
    /// ridge of the distance, as wide as twice the depth left after the walls that fit.
    fn centre_lines(&self, walls: u32, lw: f32) -> Vec<ToolPath> {
        let (w, h) = (self.nx + 1, self.ny + 1);
        if self.nx == 0 {
            return Vec::new();
        }
        // The walls that fit at a ridge node (by its depth, as the walls judge it), and
        // the width a centre line there would be.
        let width = |i: usize, j: usize| -> Option<(u32, f32)> {
            let d = self.ridge_depth(i, j);
            let fitting = (0..walls)
                .filter(|k| d >= lw * (*k as f32 + 0.5 + FIT))
                .count() as u32;
            if fitting == walls {
                return None;
            }
            let wd = 2.0 * (d - fitting as f32 * lw);
            (wd >= lw * MIN_WIDTH).then_some((fitting, wd))
        };
        let mut ridge = vec![false; w * h];
        for j in 1..h - 1 {
            for i in 1..w - 1 {
                let d = self.at(i, j);
                if d <= 0.0 || d >= self.reach {
                    continue;
                }
                ridge[j * w + i] =
                    self.is_ridge(i, j) && width(i, j).is_some() && self.thin_across(i, j);
            }
        }
        thin(&mut ridge, w, h);
        let mut out = Vec::new();
        // Specks (chains of one or two samples, often across a ridge two samples wide)
        // aren't lines.
        for chain in chains(&ridge, w, h).into_iter().filter(|c| c.len() >= 3) {
            // The widths along the chain, smoothed over five samples (the sampled depth
            // jitters); split where the smoothed width moves by 0.15 line widths, each
            // piece one width.
            let raw: Vec<(Point2, u32, f32)> = chain
                .iter()
                .filter_map(|&(i, j)| width(i, j).map(|(m, wd)| (self.node(i, j), m, wd)))
                .collect();
            let pts: Vec<(Point2, u32, f32)> = (0..raw.len())
                .map(|k| {
                    let near = &raw[k.saturating_sub(2)..(k + 3).min(raw.len())];
                    let wd = near.iter().map(|p| p.2).sum::<f32>() / near.len() as f32;
                    (raw[k].0, raw[k].1, wd)
                })
                .collect();
            let mut piece: Vec<(Point2, u32, f32)> = Vec::new();
            let bucket = |wd: f32| (wd / (lw * 0.15)).round() as i32;
            for p in pts {
                if let Some(last) = piece.last() {
                    let jump = last.0.distance(p.0) > self.step * 1.5;
                    if jump || bucket(piece[0].2) != bucket(p.2) || last.1 != p.1 {
                        let end = if jump { None } else { Some(p) };
                        out.extend(centre_piece(&piece, end, lw));
                        piece.clear();
                    }
                }
                piece.push(p);
            }
            out.extend(centre_piece(&piece, None, lw));
        }
        out
    }
}

/// One centre-line piece of steady width, continued to `end` (the next piece's first
/// point) so the pieces join.
fn centre_piece(
    piece: &[(Point2, u32, f32)],
    end: Option<(Point2, u32, f32)>,
    lw: f32,
) -> Option<ToolPath> {
    let mut points: Vec<Point2> = piece.iter().map(|p| p.0).collect();
    if let Some(e) = end {
        points.push(e.0);
    }
    if points.len() < 2 {
        return None;
    }
    let wd = piece.iter().map(|p| p.2).sum::<f32>() / piece.len() as f32;
    let kind = if piece[0].1 == 0 {
        PathKind::OuterWall
    } else {
        PathKind::InnerWall
    };
    // Below a wall pair that doesn't fit, the space is under 1.5 line widths.
    let wd = wd.clamp(lw * MIN_WIDTH, lw * (1.0 + 2.0 * FIT));
    Some(open(kind, straighten(&points), Some(wd)))
}

fn open(kind: PathKind, points: Vec<Point2>, width: Option<f32>) -> ToolPath {
    ToolPath {
        kind,
        points,
        closed: false,
        width,
    }
}

fn length(points: &[Point2]) -> f32 {
    points.windows(2).map(|w| w[0].distance(w[1])).sum()
}

fn nearest_on_segment(p: Point2, a: Point2, b: Point2) -> Point2 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    Point2::new(a.x + dx * t, a.y + dy * t)
}

fn segment_distance(p: Point2, a: Point2, b: Point2) -> f32 {
    p.distance(nearest_on_segment(p, a, b))
}

/// `line` without points on the straight line between their neighbours.
fn straighten(line: &[Point2]) -> Vec<Point2> {
    let mut out: Vec<Point2> = Vec::with_capacity(line.len());
    for &p in line {
        if out.len() >= 2 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            let cross = (b.x - a.x) * (p.y - b.y) - (b.y - a.y) * (p.x - b.x);
            if cross.abs() < 1e-4 {
                out.pop();
            }
        }
        out.push(p);
    }
    out
}

/// Thins a mask to lines one sample wide (Zhang-Suen).
fn thin(mask: &mut [bool], w: usize, h: usize) {
    loop {
        let mut changed = false;
        for pass in 0..2 {
            let mut drop = Vec::new();
            for j in 1..h - 1 {
                for i in 1..w - 1 {
                    if !mask[j * w + i] {
                        continue;
                    }
                    // Neighbours P2..P9, clockwise from above.
                    let at = |dx: i64, dy: i64| {
                        mask[((j as i64 + dy) as usize) * w + (i as i64 + dx) as usize]
                    };
                    let n = [
                        at(0, 1),
                        at(1, 1),
                        at(1, 0),
                        at(1, -1),
                        at(0, -1),
                        at(-1, -1),
                        at(-1, 0),
                        at(-1, 1),
                    ];
                    let count = n.iter().filter(|b| **b).count();
                    let turns = (0..8).filter(|&k| !n[k] && n[(k + 1) % 8]).count();
                    let (p2, p4, p6, p8) = (n[0], n[2], n[4], n[6]);
                    let side = if pass == 0 {
                        !(p2 && p4 && p6) && !(p4 && p6 && p8)
                    } else {
                        !(p2 && p4 && p8) && !(p2 && p6 && p8)
                    };
                    if (2..=6).contains(&count) && turns == 1 && side {
                        drop.push(j * w + i);
                    }
                }
            }
            changed |= !drop.is_empty();
            for c in drop {
                mask[c] = false;
            }
        }
        if !changed {
            break;
        }
    }
}

/// The set samples of `mask` joined into chains (through their 8 neighbours),
/// starting from loose ends.
fn chains(mask: &[bool], w: usize, h: usize) -> Vec<Vec<(usize, usize)>> {
    let near = |c: usize| -> Vec<usize> {
        let (i, j) = ((c % w) as i64, (c / w) as i64);
        let mut v = Vec::new();
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (x, y) = (i + dx, j + dy);
                if (dx, dy) != (0, 0) && x >= 0 && y >= 0 && x < w as i64 && y < h as i64 {
                    let n = y as usize * w + x as usize;
                    if mask[n] {
                        v.push(n);
                    }
                }
            }
        }
        v
    };
    let mut used = vec![false; mask.len()];
    let mut out = Vec::new();
    // Loose ends first, then whatever is left (loops).
    let ends: Vec<usize> = (0..mask.len())
        .filter(|&c| mask[c] && near(c).len() == 1)
        .collect();
    let rest: Vec<usize> = (0..mask.len()).filter(|&c| mask[c]).collect();
    for start in ends.into_iter().chain(rest) {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut chain = vec![(start % w, start / w)];
        let mut c = start;
        while let Some(&n) = near(c).iter().find(|&&n| !used[n]) {
            used[n] = true;
            chain.push((n % w, n / w));
            c = n;
        }
        out.push(chain);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wedge 4 mm wide at x = 0, narrowing to a point at x = 20.
    fn wedge() -> Vec<Point2> {
        vec![
            Point2::new(0.0, -2.0),
            Point2::new(20.0, 0.0),
            Point2::new(0.0, 2.0),
        ]
    }

    #[test]
    fn test_variable_walls_cover_a_tapered_wedge_without_gaps() {
        let lw = 0.42;
        let paths = variable_walls(&[wedge()], 2, lw);
        assert!(!paths.is_empty());
        // Every line is inside the wedge and within the allowed widths.
        for p in &paths {
            let wd = p.width.unwrap_or(lw);
            assert!(
                (lw * MIN_WIDTH - 1e-3..=lw * 1.5 + 1e-3).contains(&wd),
                "width {wd}"
            );
            for q in &p.points {
                let half = 2.0 * (1.0 - q.x / 20.0);
                assert!(q.x >= -1e-3 && q.y.abs() <= half + 1e-3, "{q:?} outside");
            }
        }
        // Continuous: every 0.25 mm along the wedge, down to where it is half a line
        // wide (x = 19.5), some line crosses.
        let crosses = |x: f32| {
            paths.iter().any(|p| {
                p.points
                    .windows(2)
                    .any(|w| (w[0].x - x) * (w[1].x - x) <= 0.0 && (w[0].x - w[1].x).abs() > 0.0)
            })
        };
        let gaps: Vec<f32> = (1..76)
            .map(|k| k as f32 * 0.25)
            .filter(|x| !crosses(*x))
            .collect();
        assert!(gaps.is_empty(), "no line at x = {gaps:?}");
        // Centre lines and walls don't overlap: a centre line keeps about half its
        // width plus half a wall's from the nearest wall line, except just where a wall
        // pair ends and the centre line takes over (within 0.6 mm of a wall's end).
        let walls_only: Vec<&ToolPath> = paths.iter().filter(|p| p.width.is_none()).collect();
        let wall_lines: Vec<[Point2; 2]> = walls_only
            .iter()
            .flat_map(|p| {
                p.points
                    .windows(2)
                    .map(|w| [w[0], w[1]])
                    .collect::<Vec<_>>()
            })
            .collect();
        let wall_ends: Vec<Point2> = walls_only
            .iter()
            .filter(|p| !p.closed)
            .flat_map(|p| [p.points[0], p.points[p.points.len() - 1]])
            .collect();
        for c in paths.iter().filter(|p| p.width.is_some()) {
            let wd = c.width.unwrap_or(lw);
            for q in &c.points {
                let near = wall_lines
                    .iter()
                    .map(|[a, b]| segment_distance(*q, *a, *b))
                    .fold(f32::MAX, f32::min);
                let at_an_end = wall_ends.iter().any(|e| e.distance(*q) < 0.6);
                assert!(
                    at_an_end || near > (wd + lw) / 2.0 * 0.8,
                    "{q:?}: {near} from a wall, width {wd}"
                );
            }
        }
        // Some centre line is narrower than a line width and some wider (the taper).
        let widths: Vec<f32> = paths.iter().filter_map(|p| p.width).collect();
        assert!(
            widths.iter().any(|w| *w < lw * 0.9) && widths.iter().any(|w| *w > lw * 1.1),
            "{widths:?}"
        );
    }

    #[test]
    fn test_variable_walls_on_a_round_outline_are_plain_loops() {
        // A 48-sided circle 10 mm across: its many corners must not grow centre lines.
        let circle: Vec<Point2> = (0..48)
            .map(|k| {
                let a = std::f32::consts::TAU * k as f32 / 48.0;
                Point2::new(5.0 * a.cos(), 5.0 * a.sin())
            })
            .collect();
        let paths = variable_walls(&[circle], 2, 0.42);
        assert_eq!(
            paths.len(),
            2,
            "{:?}",
            paths.iter().map(|p| (p.kind, p.width)).collect::<Vec<_>>()
        );
        assert!(paths.iter().all(|p| p.closed && p.width.is_none()));
    }

    #[test]
    fn test_variable_walls_on_a_wide_square_are_plain_loops() {
        let square = vec![
            Point2::new(-10.0, -10.0),
            Point2::new(10.0, -10.0),
            Point2::new(10.0, 10.0),
            Point2::new(-10.0, 10.0),
        ];
        let paths = variable_walls(&[square], 2, 0.42);
        assert_eq!(paths.len(), 2, "{} paths", paths.len());
        assert!(paths.iter().all(|p| p.closed && p.width.is_none()));
        assert_eq!(paths[1].kind, PathKind::OuterWall);
        // The outer wall runs half a line width in.
        let max = paths[1]
            .points
            .iter()
            .map(|p| p.x.abs().max(p.y.abs()))
            .fold(0.0, f32::max);
        assert!((max - 9.79).abs() < 0.05, "{max}");
    }
}
