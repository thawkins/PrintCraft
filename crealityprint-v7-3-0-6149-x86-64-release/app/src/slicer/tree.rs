//! Tree supports: thin branches from each overhang that lean together and join on
//! the way down, keeping clear of the model.
//!
//! Tips are placed under the overhang points, one per square of the support line
//! spacing, the top Z gap below the overhang. Going down a layer at a time, every
//! branch thickens from the tip diameter to the branch diameter over 10 mm, leans
//! (at most 40 degrees from upright) towards the nearest other branch within 10 mm, and
//! joins it when they touch. A branch must stay the XY gap clear of the model: when it
//! isn't, it steps away from the model along the grid's distance field, and if that
//! doesn't clear it, it ends. A branch over the model (or within the bottom Z gap of
//! it) stops there; the rest reach the plate. Each branch is printed as rings one line
//! width apart filling its circle.

use super::{Grid, PathKind, PrintSettings, ToolPath, CELL_MM};
use crate::geometry::Point2;

/// Branches lean at most this far from upright (degrees).
const LEAN_DEG: f32 = 40.0;
/// Branches grow from tip to full width over this much height (mm).
const GROW_MM: f32 = 10.0;
/// Branches lean towards others at most this far away (mm).
const REACH_MM: f32 = 10.0;

#[derive(Clone, Copy, Debug)]
struct Branch {
    at: Point2,
    radius: f32,
}

/// Tree support lines for every layer, and the cells where branches stand on the plate
/// (for the support raft). `overhangs` are (cell, height) points to hold up, `masks`
/// the model's cells per layer, `heights` the layers' (top, height).
pub(super) fn tree_supports(
    grid: &Grid,
    overhangs: &[(usize, f32)],
    masks: &[Vec<bool>],
    heights: &[(f32, f32)],
    settings: &PrintSettings,
    lw: f32,
) -> (Vec<Vec<ToolPath>>, Vec<bool>) {
    let n = heights.len();
    let mut out = vec![Vec::new(); n];
    let mut plate = vec![false; grid.cells()];
    if n == 0 {
        return (out, plate);
    }
    let tip_r = settings.tree_tip_diameter / 2.0;
    let branch_r = (settings.tree_branch_diameter / 2.0).max(tip_r);
    let gap = settings.support_xy_gap;
    // Tips: one per square of the line spacing that has overhang in it, at the lowest
    // overhang there, starting at the layer the top Z gap below it.
    let step = ((settings.support_spacing / CELL_MM).round() as usize).max(1);
    let mut first_model = vec![n; grid.cells()];
    for (i, mask) in masks.iter().enumerate().rev() {
        for (f, m) in first_model.iter_mut().zip(mask) {
            if *m {
                *f = i;
            }
        }
    }
    let mut squares: std::collections::HashMap<(usize, usize), (usize, f32)> =
        std::collections::HashMap::new();
    for &(cell, h) in overhangs {
        let key = ((cell % grid.w) / step, (cell / grid.w) / step);
        let e = squares.entry(key).or_insert((cell, h));
        if h < e.1 {
            *e = (cell, h);
        }
    }
    let mut starts: Vec<Vec<Point2>> = vec![Vec::new(); n];
    let mut tips: Vec<(usize, f32)> = squares.into_values().collect();
    tips.sort_by_key(|(c, _)| *c);
    for (cell, h) in tips {
        let k = heights
            .partition_point(|(z, lh)| z + settings.support_top_gap.unwrap_or(lh.max(0.15)) < h);
        if k > 0 && (!settings.supports_on_plate_only || first_model[cell] >= k) {
            starts[k - 1].push(grid.center(cell % grid.w, cell / grid.w));
        }
    }

    let mut branches: Vec<Branch> = Vec::new();
    for i in (0..n).rev() {
        let (top, height) = heights[i];
        // Distance (mm) from each free cell to the model on this layer.
        let free: Vec<bool> = masks[i].iter().map(|m| !m).collect();
        let dist = super::lightning::distance_to_outside(grid, &free);
        let clear =
            |b: &Branch| dist[grid.index_of(b.at)] * CELL_MM - CELL_MM * 0.5 >= b.radius + gap;
        // Branches from the layer above lean, grow and keep clear.
        let lean = height * LEAN_DEG.to_radians().tan();
        let grow = (branch_r - tip_r) * height / GROW_MM;
        let above = std::mem::take(&mut branches);
        for (k, b) in above.iter().enumerate() {
            let mut next = Branch {
                at: b.at,
                radius: (b.radius + grow).min(branch_r.max(b.radius)),
            };
            let nearest = above
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != k)
                .map(|(_, o)| o.at)
                .min_by(|p, q| b.at.distance(*p).total_cmp(&b.at.distance(*q)))
                .filter(|o| b.at.distance(*o) < REACH_MM);
            if let Some(o) = nearest {
                next.at = toward(b.at, o, lean.min(b.at.distance(o) / 2.0));
            }
            if !clear(&next) {
                next.at = away(grid, &dist, next.at, lean * 2.0);
            }
            if clear(&next) {
                branches.push(next);
            }
        }
        // New tips (only where they clear the model).
        branches.extend(
            starts[i]
                .iter()
                .map(|&at| Branch { at, radius: tip_r })
                .filter(|b| clear(b)),
        );
        // Branches that touch join, at their middle, as wide as both together.
        branches = join(branches);
        // A branch over the model, or within the bottom Z gap of it, stops here.
        branches.retain(|b| {
            let c = grid.index_of(b.at);
            !(0..=i)
                .rev()
                .take_while(|&j| {
                    j == i || heights[j].0 > top - height - settings.support_bottom_gap
                })
                .any(|j| masks[j][c])
        });
        for b in &branches {
            out[i].extend(rings(b, lw));
        }
        if i == 0 {
            for b in &branches {
                let r = (b.radius / CELL_MM).ceil() as i64;
                let (cx, cy) = (
                    ((b.at.x - grid.x0) / CELL_MM) as i64,
                    ((b.at.y - grid.y0) / CELL_MM) as i64,
                );
                for y in (cy - r).max(0)..=(cy + r).min(grid.h as i64 - 1) {
                    for x in (cx - r).max(0)..=(cx + r).min(grid.w as i64 - 1) {
                        plate[y as usize * grid.w + x as usize] = true;
                    }
                }
            }
        }
    }
    (out, plate)
}

/// `from` moved `step` mm towards `to` (no further than `to`).
fn toward(from: Point2, to: Point2, step: f32) -> Point2 {
    let d = from.distance(to);
    if d < 1e-6 {
        return from;
    }
    let t = (step / d).min(1.0);
    Point2::new(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t)
}

/// `at` moved up to `step` mm away from the model: towards the neighbouring cell
/// farthest from it.
fn away(grid: &Grid, dist: &[f32], at: Point2, step: f32) -> Point2 {
    let c = grid.index_of(at);
    let (x, y) = ((c % grid.w) as i64, (c / grid.w) as i64);
    let mut best = (dist[c], c);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (nx, ny) = (x + dx, y + dy);
            if nx < 0 || ny < 0 || nx >= grid.w as i64 || ny >= grid.h as i64 {
                continue;
            }
            let nc = ny as usize * grid.w + nx as usize;
            if dist[nc] > best.0 {
                best = (dist[nc], nc);
            }
        }
    }
    toward(at, grid.center(best.1 % grid.w, best.1 / grid.w), step)
}

/// Joins branches that touch, until none do.
fn join(mut branches: Vec<Branch>) -> Vec<Branch> {
    loop {
        let mut pair = None;
        'search: for a in 0..branches.len() {
            for b in a + 1..branches.len() {
                let (p, q) = (branches[a], branches[b]);
                if p.at.distance(q.at) < (p.radius + q.radius) * 0.5 {
                    pair = Some((a, b));
                    break 'search;
                }
            }
        }
        let Some((a, b)) = pair else {
            return branches;
        };
        let (p, q) = (branches[a], branches.swap_remove(b));
        let (wp, wq) = (p.radius * p.radius, q.radius * q.radius);
        let t = wq / (wp + wq);
        branches[a] = Branch {
            at: Point2::new(
                p.at.x + (q.at.x - p.at.x) * t,
                p.at.y + (q.at.y - p.at.y) * t,
            ),
            radius: (wp + wq).sqrt().min(p.radius.max(q.radius) * 1.5),
        };
    }
}

/// A branch's circle filled with rings one line width apart, the outermost half a line
/// width inside its edge.
fn rings(b: &Branch, lw: f32) -> Vec<ToolPath> {
    let mut out = Vec::new();
    let mut r = (b.radius - lw / 2.0).max(lw * 0.25);
    loop {
        let sides = ((std::f32::consts::TAU * r / 0.8).ceil() as usize).clamp(6, 64);
        let points = (0..sides)
            .map(|k| {
                let a = std::f32::consts::TAU * k as f32 / sides as f32;
                Point2::new(b.at.x + r * a.cos(), b.at.y + r * a.sin())
            })
            .collect();
        out.push(ToolPath {
            kind: PathKind::Support,
            points,
            closed: true,
            width: None,
        });
        r -= lw;
        if r < lw * 0.5 {
            break;
        }
    }
    out
}
