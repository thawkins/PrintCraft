//! Lightning infill: thin branching trees inside a part, only where something above
//! needs holding up (the underside of a top surface). Everything else stays hollow.
//!
//! Worked out from the top down on the slicer's 1 mm grid. Where a solid top skin
//! starts above sparse infill, points needing support are placed `spacing` apart.
//! Going down a layer, each point moves one layer height towards the nearest wall (a
//! 45 degree slope) and is dropped once it reaches it. On every layer each point
//! sends a branch to the nearest wall down the grid's distance-to-wall field; a branch
//! stops where it meets one already laid, so nothing is laid twice, and its last step
//! is cut where it meets the infill area's edge, so it bonds to the wall.

use super::{Grid, Polygon};
use crate::geometry::Point2;
use std::collections::HashSet;

/// Lightning lines for every layer. `regions` are the layers' infill areas, `needs`
/// the cells where a top skin starts just above (from below the skin's lowest layer),
/// `heights` the layers' (top, height), and `spacing` how far apart points needing
/// support start.
pub(super) fn lightning_lines(
    grid: &Grid,
    regions: &[Vec<Polygon>],
    needs: &[Vec<bool>],
    heights: &[(f32, f32)],
    spacing: f32,
) -> Vec<Vec<Vec<Point2>>> {
    let n = regions.len();
    let mut out = vec![Vec::new(); n];
    let every = ((spacing / super::CELL_MM).round() as usize).max(1);
    let mut points: Vec<Point2> = Vec::new();
    for i in (0..n).rev() {
        let mask = grid.rasterize(&regions[i]);
        let dist = distance_to_outside(grid, &mask);
        // Points from the layer above move one layer height towards the wall.
        let step = heights.get(i + 1).map_or(0.0, |(_, h)| *h);
        let mut next: Vec<Point2> = points
            .iter()
            .filter_map(|&p| {
                let c = grid.index_of(p);
                if !mask[c] || dist[c] <= 1.0 {
                    return None;
                }
                let q = cell_point(grid, parent(grid, &dist, c));
                let gap = p.distance(q);
                if gap < 1e-6 {
                    return Some(p);
                }
                let t = (step / gap).min(1.0);
                Some(Point2::new(p.x + (q.x - p.x) * t, p.y + (q.y - p.y) * t))
            })
            .collect();
        // New points under the skin that starts just above.
        for c in (0..grid.cells()).filter(|&c| needs[i][c] && mask[c]) {
            let (cx, cy) = (c % grid.w, c / grid.w);
            if cx % every == 0 && cy % every == 0 {
                next.push(cell_point(grid, c));
            }
        }
        // Points closer than half the spacing merge.
        let mut seen = HashSet::new();
        let half = (spacing * 0.5).max(super::CELL_MM);
        next.retain(|p| seen.insert(((p.x / half).floor() as i64, (p.y / half).floor() as i64)));
        points = next;
        out[i] = trees(grid, &mask, &dist, &points, &regions[i]);
    }
    out
}

/// A grid cell's centre.
fn cell_point(grid: &Grid, c: usize) -> Point2 {
    grid.center(c % grid.w, c / grid.w)
}

/// For every cell, about how far (in cells) it is from the nearest cell outside
/// `mask` (0 outside): a two-pass chamfer distance, 1 across and 1.41 diagonally.
fn distance_to_outside(grid: &Grid, mask: &[bool]) -> Vec<f32> {
    let (w, h) = (grid.w as i64, grid.h as i64);
    let mut d: Vec<f32> = mask
        .iter()
        .map(|m| if *m { f32::MAX } else { 0.0 })
        .collect();
    let at = |d: &[f32], x: i64, y: i64| {
        if x < 0 || y < 0 || x >= w || y >= h {
            0.0
        } else {
            d[(y * w + x) as usize]
        }
    };
    let diag = std::f32::consts::SQRT_2;
    for y in 0..h {
        for x in 0..w {
            let c = (y * w + x) as usize;
            let best = [
                at(&d, x - 1, y) + 1.0,
                at(&d, x, y - 1) + 1.0,
                at(&d, x - 1, y - 1) + diag,
                at(&d, x + 1, y - 1) + diag,
            ]
            .into_iter()
            .fold(d[c], f32::min);
            d[c] = best;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let c = (y * w + x) as usize;
            let best = [
                at(&d, x + 1, y) + 1.0,
                at(&d, x, y + 1) + 1.0,
                at(&d, x + 1, y + 1) + diag,
                at(&d, x - 1, y + 1) + diag,
            ]
            .into_iter()
            .fold(d[c], f32::min);
            d[c] = best;
        }
    }
    d
}

/// The neighbouring cell closest to outside (straight neighbours first on a tie), or
/// the cell itself at the grid's edge.
fn parent(grid: &Grid, dist: &[f32], c: usize) -> usize {
    let (x, y) = ((c % grid.w) as i64, (c / grid.w) as i64);
    let mut best = (dist[c], c);
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
        let (nx, ny) = (x + dx, y + dy);
        if nx < 0 || ny < 0 || nx >= grid.w as i64 || ny >= grid.h as i64 {
            continue;
        }
        let nc = (ny * grid.w as i64 + nx) as usize;
        if dist[nc] < best.0 {
            best = (dist[nc], nc);
        }
    }
    best.1
}

/// One branch per point, down the distance field to the wall, stopping where it meets
/// a branch already laid; each cut at the infill area's edge.
fn trees(
    grid: &Grid,
    mask: &[bool],
    dist: &[f32],
    points: &[Point2],
    region: &[Polygon],
) -> Vec<Vec<Point2>> {
    let area = crate::gyroid::AreaTest::new(region);
    let inside = |p: Point2| area.contains(p);
    // Deepest points first, so the long branches are laid and the short ones join them.
    let mut order: Vec<&Point2> = points.iter().collect();
    order.sort_by(|a, b| {
        let (da, db) = (dist[grid.index_of(**a)], dist[grid.index_of(**b)]);
        db.total_cmp(&da)
            .then(a.x.total_cmp(&b.x))
            .then(a.y.total_cmp(&b.y))
    });
    let mut laid: HashSet<usize> = HashSet::new();
    let mut out = Vec::new();
    for &&p in &order {
        let mut c = grid.index_of(p);
        let mut branch = vec![p];
        if !laid.insert(c) {
            continue;
        }
        loop {
            let next = parent(grid, dist, c);
            if next == c {
                break;
            }
            branch.push(cell_point(grid, next));
            if !mask[next] || !laid.insert(next) {
                break;
            }
            c = next;
        }
        out.extend(crate::gyroid::clip(&straighten(&branch), &inside));
    }
    out.retain(|b| b.len() >= 2);
    out
}

/// `line` without the points that lie on the straight line between their neighbours.
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
