//! Turns triangles into printable layers: outlines, walls, solid and sparse infill,
//! simple tree-free supports, and a skirt or brim. Rules are listed in MECHANICS.md.

use crate::geometry::{Bounds, Point2, Vec3};
use crate::model::Triangle;
use crate::profiles::{Adhesion, InfillPattern, PrintSettings};
use std::collections::HashMap;

pub type Polygon = Vec<Point2>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PathKind {
    Skirt,
    Brim,
    OuterWall,
    InnerWall,
    SolidInfill,
    SparseInfill,
    Support,
    Ironing,
    /// Solid lines of the visible top surface (the last layer under open air).
    TopSurface,
    /// Lines of a raft under the print or under supports.
    Raft,
    /// Solid lines laid over air (an unsupported area anchored at both ends), running
    /// across the shortest anchored span.
    Bridge,
    /// The prime tower of a two-nozzle print: each nozzle primes on it after a tool change.
    PrimeTower,
    /// The ooze shield of a two-nozzle print: a one-line wall around the print that the idle
    /// nozzle wipes its ooze on.
    OozeShield,
}

impl PathKind {
    pub const ALL: [PathKind; 13] = [
        Self::OuterWall,
        Self::InnerWall,
        Self::TopSurface,
        Self::SolidInfill,
        Self::Bridge,
        Self::SparseInfill,
        Self::Ironing,
        Self::Support,
        Self::Skirt,
        Self::Brim,
        Self::Raft,
        Self::PrimeTower,
        Self::OozeShield,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Skirt => "Skirt",
            Self::Brim => "Brim",
            Self::OuterWall => "Outer wall",
            Self::InnerWall => "Inner wall",
            Self::SolidInfill => "Top / bottom",
            Self::SparseInfill => "Infill",
            Self::Support => "Support",
            Self::Ironing => "Ironing",
            Self::TopSurface => "Top surface",
            Self::Raft => "Raft",
            Self::Bridge => "Bridge",
            Self::PrimeTower => "Prime tower",
            Self::OozeShield => "Ooze shield",
        }
    }

    pub fn gcode_type(self) -> &'static str {
        match self {
            Self::Skirt | Self::Brim => "SKIRT",
            Self::OuterWall => "WALL-OUTER",
            Self::InnerWall => "WALL-INNER",
            // Cura-style names have no bridge type; a bridge is solid skin to printer screens.
            Self::SolidInfill | Self::Bridge => "SKIN",
            Self::SparseInfill => "FILL",
            Self::Support => "SUPPORT",
            Self::Ironing => "IRONING",
            Self::TopSurface => "TOP-SURFACE",
            // Printer screens and other slicers treat a raft as support, and the prime tower
            // too: it isn't part of the model.
            Self::Raft | Self::PrimeTower | Self::OozeShield => "SUPPORT",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToolPath {
    pub kind: PathKind,
    pub points: Vec<Point2>,
    pub closed: bool,
}

#[derive(Clone, Debug)]
pub struct Layer {
    /// Height of the top of this layer above the plate.
    pub z: f32,
    pub height: f32,
    pub paths: Vec<ToolPath>,
}

#[derive(Clone, Debug, Default)]
pub struct SliceResult {
    pub layers: Vec<Layer>,
    pub line_width: f32,
}

/// Grid used for top/bottom detection and supports.
const CELL_MM: f32 = 1.0;
/// Support lines are laid on every n-th grid row (2 mm apart).
const SUPPORT_ROW_STEP: usize = 2;
const SKIRT_DISTANCE_MM: f32 = 5.0;
const SKIRT_LOOPS: usize = 2;
const BRIM_LINES: usize = 8;
/// Infill line ends reach this share of a line width into the innermost wall's band
/// (so the infill bead overlaps that wall by about 0.65 of a line width).
const INFILL_OVERLAP: f32 = 0.15;
/// Raft: layers, how far it reaches past the print (cells of 1 mm), the base layer's line
/// spacing, and the gap left between a full raft and the model.
pub const RAFT_LAYERS: usize = 3;
/// Bridge lines reach this many cells onto the supported area at each end, to anchor.
const BRIDGE_ANCHOR_CELLS: usize = 2;
/// Unsupported areas smaller than this many cells are left as normal solid infill.
const MIN_BRIDGE_CELLS: usize = 4;
/// Bridge directions tried, every 15 degrees over a half turn.
const BRIDGE_ANGLE_STEPS: usize = 12;
const RAFT_MARGIN_CELLS: usize = 3;
const SUPPORT_RAFT_MARGIN_CELLS: usize = 2;
const RAFT_BASE_SPACING_MM: f32 = 1.0;
const RAFT_AIR_GAP_MM: f32 = 0.15;
/// Gap between ironing passes.
const IRONING_SPACING_MM: f32 = 0.2;
/// Fuzzy skin: distance between wobble points and how far each may move.
const FUZZ_POINT_DIST_MM: f32 = 0.8;
const FUZZ_THICKNESS_MM: f32 = 0.3;

/// Slices the given plate triangles. `progress` receives values from 0 to 1.
pub fn slice(
    triangles: &[Triangle],
    settings: &PrintSettings,
    line_width: f32,
    progress: &dyn Fn(f32),
) -> SliceResult {
    let raw_bounds = Bounds::of(triangles.iter().flatten().copied());
    if !raw_bounds.is_valid() || raw_bounds.max.z <= 0.0 {
        return SliceResult {
            layers: Vec::new(),
            line_width,
        };
    }

    // A full raft: the model is lifted onto `raft_n` layers of raft lines.
    let full_raft = settings.adhesion == Adhesion::Raft;
    let raft_n = if full_raft { RAFT_LAYERS } else { 0 };
    let lifted: Vec<Triangle>;
    let triangles = if full_raft {
        let up = Vec3::new(0.0, 0.0, raft_height(settings));
        lifted = triangles.iter().map(|t| t.map(|v| v + up)).collect();
        &lifted[..]
    } else {
        triangles
    };
    let bounds = Bounds::of(triangles.iter().flatten().copied());

    let mut heights = layer_heights(bounds.max.z, settings);
    let n = heights.len();
    if raft_n >= n {
        return SliceResult {
            layers: Vec::new(),
            line_width,
        };
    }

    // 1. Outlines of every layer.
    let outlines = all_outlines(triangles, &heights, progress);

    // 2. Coarse grid masks, used to find top/bottom surfaces and support areas.
    let grid = Grid::new(&bounds);
    let masks: Vec<Vec<bool>> = outlines.iter().map(|o| grid.rasterize(o)).collect();
    let (mut supports, plate_cells) = if settings.supports {
        grid.supports(
            triangles,
            &masks,
            &heights,
            heights[raft_n].0,
            settings.supports_on_plate_only,
            settings.support_angle,
            line_width,
        )
    } else {
        (vec![Vec::new(); n], vec![false; grid.cells()])
    };

    // Rafts. A full raft spreads under the first model layer and every support column
    // that reaches the plate. Without one, supports that reach the plate still stand on
    // a small raft of their own so they stick to the bed, when "Raft under support" is on
    // (the default); the model itself sits on the plate as usual, with a one-cell gap
    // from those support rafts.
    let has_support_raft = settings.support_raft && plate_cells.iter().any(|c| *c);
    let raft_areas: Vec<Vec<bool>> = if full_raft {
        let under: Vec<bool> = masks[raft_n]
            .iter()
            .zip(&plate_cells)
            .map(|(m, s)| *m || *s)
            .collect();
        vec![grid.dilate(&under, RAFT_MARGIN_CELLS); raft_n]
    } else if has_support_raft {
        let spread = grid.dilate(&plate_cells, SUPPORT_RAFT_MARGIN_CELLS);
        (0..RAFT_LAYERS.min(n))
            .map(|i| {
                let near_model = grid.dilate(&masks[i], 1);
                spread
                    .iter()
                    .zip(&near_model)
                    .map(|(s, m)| *s && !*m)
                    .collect()
            })
            .collect()
    } else {
        Vec::new()
    };
    // The raft replaces the bottom of the support columns.
    for s in supports.iter_mut().take(raft_areas.len()) {
        s.clear();
    }
    // A small air gap between a full raft and the model makes them easy to part.
    if full_raft {
        for hz in heights.iter_mut().skip(raft_n) {
            hz.0 += RAFT_AIR_GAP_MM;
        }
    }

    // 3. Toolpaths per layer.
    let vase = settings.vase_mode;
    let wall_count = if vase { 1 } else { settings.walls };
    let mut layers = Vec::with_capacity(n);
    for i in 0..n {
        let (z, h) = heights[i];
        let mut paths = Vec::new();

        if i == 0 {
            paths.extend(adhesion_paths(&outlines[0], settings.adhesion, line_width));
        }
        if let Some(area) = raft_areas.get(i) {
            let from = last_point(&paths);
            paths.extend(order_open_paths(
                raft_paths(&grid, area, i, line_width),
                from,
            ));
        }
        // Counted from the model's own first layer (above a full raft).
        let model_layer = i.saturating_sub(raft_n);
        paths.extend(wall_paths(
            &outlines[i],
            model_layer,
            wall_count,
            line_width,
            settings,
        ));

        let region = infill_region(&outlines[i], wall_count, line_width);
        if !region.is_empty() && vase {
            // Vase mode: a solid bottom, then nothing but the spiral wall.
            if model_layer < settings.bottom_layers as usize {
                let lines = vase_bottom_paths(&region, i, line_width);
                let from = last_point(&paths);
                paths.extend(order_open_paths(lines, from));
            }
        } else if !region.is_empty() {
            let exposed = exposed_cells(&masks, i, settings);
            let top = top_cells(&grid, &masks, i);
            let angle = if i % 2 == 0 { 45f32 } else { 135f32 }.to_radians();
            // Bridges: only above the model's first layer (nothing below it but plate or
            // raft). Their areas are taken out of the normal solid and sparse infill.
            let bridges = if i > raft_n {
                bridge_areas(&grid, &masks, i)
            } else {
                Vec::new()
            };
            let mut not_bridged = vec![true; grid.cells()];
            for (area, _) in &bridges {
                for (n, b) in not_bridged.iter_mut().zip(area) {
                    *n &= !*b;
                }
            }
            if !bridges.is_empty() {
                let from = last_point(&paths);
                let lines = bridge_paths(&region, &grid, &bridges, line_width);
                paths.extend(order_open_paths(lines, from));
            }
            let solid_cells: Vec<bool> = exposed
                .iter()
                .zip(&not_bridged)
                .map(|(e, n)| *e && *n)
                .collect();
            let solid = solid_infill_paths(&region, &grid, &solid_cells, &top, angle, line_width);
            let from = last_point(&paths);
            paths.extend(order_open_paths(solid, from));
            if settings.infill_percent > 0.5 {
                let kept_off: Vec<bool> = exposed
                    .iter()
                    .zip(&not_bridged)
                    .map(|(e, n)| *e || !*n)
                    .collect();
                let from = last_point(&paths);
                paths.extend(sparse_infill_paths(
                    &region, &grid, &kept_off, i, line_width, settings, from,
                ));
            }
            // Only the very top layer of each surface is ironed. Fuzzy top wins: ironing
            // would flatten its texture.
            if settings.ironing && !settings.fuzzy_top && top.iter().any(|t| *t) {
                let ironing = ironing_paths(&region, &grid, &top, angle);
                let from = last_point(&paths);
                paths.extend(order_open_paths(ironing, from));
            }
        }

        let support = std::mem::take(&mut supports[i]);
        if support.iter().all(|p| !p.closed) {
            let from = last_point(&paths);
            paths.extend(order_open_paths(support, from));
        } else {
            paths.extend(support);
        }

        layers.push(Layer {
            z,
            height: h,
            paths,
        });
        if i % 4 == 0 {
            progress(0.4 + 0.6 * i as f32 / n as f32);
        }
    }
    progress(1.0);
    SliceResult { layers, line_width }
}

/// The outline of every layer, cut through the middle of its height. Reports progress
/// from 0 to 0.4.
fn all_outlines(
    triangles: &[Triangle],
    heights: &[(f32, f32)],
    progress: &dyn Fn(f32),
) -> Vec<Vec<Polygon>> {
    let n = heights.len();
    let mut outlines: Vec<Vec<Polygon>> = Vec::with_capacity(n);
    for (i, (z, h)) in heights.iter().enumerate() {
        outlines.push(layer_outline(triangles, z - h / 2.0));
        if i % 8 == 0 {
            progress(0.4 * i as f32 / n as f32);
        }
    }
    outlines
}

/// Wall loops of one layer, inner first and outer last for a cleaner outside surface.
/// Outer walls get fuzzy skin when it is on (never on the first layer, so the print
/// still sits flat).
fn wall_paths(
    outline: &[Polygon],
    layer: usize,
    wall_count: u32,
    line_width: f32,
    settings: &PrintSettings,
) -> Vec<ToolPath> {
    let mut walls: Vec<(PathKind, Polygon)> = Vec::new();
    for k in 0..wall_count {
        let d = line_width * (0.5 + k as f32);
        let kind = if k == 0 {
            PathKind::OuterWall
        } else {
            PathKind::InnerWall
        };
        for poly in outline {
            if let Some(mut p) = offset_left(poly, d) {
                if kind == PathKind::OuterWall && settings.fuzzy_skin && layer > 0 {
                    p = fuzz(&p, layer as u64);
                }
                walls.push((kind, p));
            }
        }
    }
    walls.reverse();
    walls
        .into_iter()
        .map(|(kind, points)| ToolPath {
            kind,
            points,
            closed: true,
        })
        .collect()
}

/// The area infill covers: inside the innermost wall, overlapping it slightly.
fn infill_region(outline: &[Polygon], wall_count: u32, line_width: f32) -> Vec<Polygon> {
    let inset = line_width * (wall_count as f32 - INFILL_OVERLAP).max(0.0);
    if inset > 0.0 {
        outline
            .iter()
            .filter_map(|p| offset_left(p, inset))
            .collect()
    } else {
        outline.to_vec()
    }
}

/// Vase mode's solid bottom: plain solid lines, alternating 45° and 135°.
fn vase_bottom_paths(region: &[Polygon], layer: usize, line_width: f32) -> Vec<ToolPath> {
    let angle = if layer % 2 == 0 { 45f32 } else { 135f32 }.to_radians();
    scan_fill(region, line_width, angle)
        .into_iter()
        .map(|seg| open_path(PathKind::SolidInfill, seg.to_vec()))
        .collect()
}

/// Grid cells of the visible top surface on layer `i`: model here, nothing above it.
fn top_cells(grid: &Grid, masks: &[Vec<bool>], i: usize) -> Vec<bool> {
    let n = masks.len();
    (0..grid.cells())
        .map(|c| masks[i][c] && (i + 1 == n || !masks[i + 1][c]))
        .collect()
}

/// The bridges of layer `i`: each a mask of the cells its lines cover and the direction
/// (radians) they run.
///
/// An unsupported cell is in this layer but not the one below. Each 4-connected group of
/// them is tried at `BRIDGE_ANGLE_STEPS` directions. For every cell, a line through it in
/// that direction is followed both ways out of the group; it is anchored when both ends
/// land on a cell that is in this layer and the one below. The direction with the most
/// anchored lines wins; among equals, the one with the shorter lines (the shortest span).
/// A group with no anchored line, or smaller than `MIN_BRIDGE_CELLS`, is an overhang, not
/// a bridge, and is left as normal solid infill. A bridge's cells are widened by
/// `BRIDGE_ANCHOR_CELLS` onto the supported area so its lines are anchored.
fn bridge_areas(grid: &Grid, masks: &[Vec<bool>], i: usize) -> Vec<(Vec<bool>, f32)> {
    let (here, below) = (&masks[i], &masks[i - 1]);
    let unsupported: Vec<bool> = here.iter().zip(below).map(|(h, b)| *h && !*b).collect();
    let mut taken = vec![false; grid.cells()];
    let mut out = Vec::new();
    for group in grid.groups(&unsupported) {
        if group.len() < MIN_BRIDGE_CELLS {
            continue;
        }
        let mut in_group = vec![false; grid.cells()];
        for c in &group {
            in_group[*c] = true;
        }
        let best = (0..BRIDGE_ANGLE_STEPS)
            .map(|k| {
                let angle = std::f32::consts::PI * k as f32 / BRIDGE_ANGLE_STEPS as f32;
                let (anchored, span) =
                    grid.spans(&group, &in_group, angle, |c| here[c] && below[c]);
                (angle, anchored, span)
            })
            .filter(|(_, anchored, _)| *anchored > 0)
            .max_by(|a, b| a.1.cmp(&b.1).then(b.2.total_cmp(&a.2)));
        let Some((angle, _, _)) = best else {
            continue;
        };
        let wide = grid.dilate(&in_group, BRIDGE_ANCHOR_CELLS);
        let area: Vec<bool> = wide
            .iter()
            .zip(here)
            .zip(&taken)
            .map(|((w, h), t)| *w && *h && !*t)
            .collect();
        for (t, a) in taken.iter_mut().zip(&area) {
            *t |= *a;
        }
        out.push((area, angle));
    }
    out
}

/// Bridge lines over each bridge's cells, at the bridge's direction.
fn bridge_paths(
    region: &[Polygon],
    grid: &Grid,
    bridges: &[(Vec<bool>, f32)],
    line_width: f32,
) -> Vec<ToolPath> {
    let mut paths = Vec::new();
    for (area, angle) in bridges {
        for seg in scan_fill(region, line_width, *angle) {
            for (points, inside) in split_by(&seg, |p| area[grid.index_of(p)]) {
                if inside {
                    paths.push(open_path(PathKind::Bridge, points));
                }
            }
        }
    }
    paths
}

/// Solid lines over the exposed (top/bottom) cells; the parts over `top` cells are
/// marked as the visible top surface.
fn solid_infill_paths(
    region: &[Polygon],
    grid: &Grid,
    exposed: &[bool],
    top: &[bool],
    angle: f32,
    line_width: f32,
) -> Vec<ToolPath> {
    let mut paths = Vec::new();
    for seg in scan_fill(region, line_width, angle) {
        for piece in split_by(&seg, |p| exposed[grid.index_of(p)]) {
            if !piece.1 {
                continue;
            }
            let solid = [piece.0[0], piece.0[1]];
            for part in split_by(&solid, |p| top[grid.index_of(p)]) {
                let kind = if part.1 {
                    PathKind::TopSurface
                } else {
                    PathKind::SolidInfill
                };
                paths.push(open_path(kind, part.0));
            }
        }
    }
    paths
}

/// Sparse infill lines in the chosen pattern, kept off the exposed cells.
fn sparse_infill_paths(
    region: &[Polygon],
    grid: &Grid,
    exposed: &[bool],
    layer: usize,
    line_width: f32,
    settings: &PrintSettings,
    from: Option<Point2>,
) -> Vec<ToolPath> {
    let mut paths: Vec<ToolPath> = Vec::new();
    let base = line_width * 100.0 / settings.infill_percent.min(100.0);
    let (angles, spacing) = infill_angles(settings.infill_pattern, layer, base);
    // Each direction is ordered on its own, so one set of lines is laid before the next
    // crosses it, starting where the previous set ended.
    for angle in angles {
        let mut set = Vec::new();
        for seg in scan_fill(region, spacing, angle.to_radians()) {
            for piece in split_by(&seg, |p| exposed[grid.index_of(p)]) {
                if !piece.1 {
                    set.push(open_path(PathKind::SparseInfill, piece.0));
                }
            }
        }
        let from = last_point(&paths).or(from);
        paths.extend(order_open_paths(set, from));
    }
    paths
}

/// Close-spaced ironing passes over the `top` cells, across the top lines (`angle` is
/// the solid-infill angle; ironing runs at right angles to it).
fn ironing_paths(region: &[Polygon], grid: &Grid, top: &[bool], angle: f32) -> Vec<ToolPath> {
    let mut paths = Vec::new();
    let angle = angle + std::f32::consts::FRAC_PI_2;
    for seg in scan_fill(region, IRONING_SPACING_MM, angle) {
        for piece in split_by(&seg, |p| top[grid.index_of(p)]) {
            if piece.1 {
                paths.push(open_path(PathKind::Ironing, piece.0));
            }
        }
    }
    paths
}

fn open_path(kind: PathKind, points: Vec<Point2>) -> ToolPath {
    ToolPath {
        kind,
        points,
        closed: false,
    }
}

/// Where the nozzle ends up after the last of `paths`.
fn last_point(paths: &[ToolPath]) -> Option<Point2> {
    let p = paths.last()?;
    if p.closed {
        p.points.first().copied()
    } else {
        p.points.last().copied()
    }
}

/// Bucket size for finding the nearest line end quickly.
const ORDER_CELL_MM: f32 = 3.0;

/// Puts open lines in printing order: each next line is the one whose nearer end is
/// closest to where the nozzle is, and it is flipped to start at that end. Lines split by
/// a hole or recess are therefore finished on one side before the nozzle moves to the
/// other side, instead of hopping over the gap on every row. `from` is where the nozzle is
/// before the first line.
pub fn order_open_paths(mut paths: Vec<ToolPath>, from: Option<Point2>) -> Vec<ToolPath> {
    paths.retain(|p| !p.points.is_empty());
    if paths.len() < 2 {
        return paths;
    }
    let cell = |p: Point2| {
        (
            (p.x / ORDER_CELL_MM).floor() as i32,
            (p.y / ORDER_CELL_MM).floor() as i32,
        )
    };
    let mut buckets: HashMap<(i32, i32), Vec<(usize, bool)>> = HashMap::new();
    let (mut lo, mut hi) = ((i32::MAX, i32::MAX), (i32::MIN, i32::MIN));
    for (i, p) in paths.iter().enumerate() {
        let ends = [p.points[0], p.points[p.points.len() - 1]];
        for (k, e) in ends.into_iter().enumerate() {
            let c = cell(e);
            lo = (lo.0.min(c.0), lo.1.min(c.1));
            hi = (hi.0.max(c.0), hi.1.max(c.1));
            buckets.entry(c).or_default().push((i, k == 1));
        }
    }
    let max_ring = (hi.0 - lo.0).max(hi.1 - lo.1) + 1;
    let mut used = vec![false; paths.len()];
    let mut slots: Vec<Option<ToolPath>> = paths.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(slots.len());
    let mut at = from.unwrap_or_else(|| slots[0].as_ref().unwrap().points[0]);
    for _ in 0..slots.len() {
        let c = cell(at);
        let mut best: Option<(f32, usize, bool)> = None;
        for r in 0..=max_ring {
            // Everything in ring r is at least (r - 1) cells away.
            if best.is_some_and(|(d, _, _)| d <= (r - 1).max(0) as f32 * ORDER_CELL_MM) {
                break;
            }
            for dx in -r..=r {
                for dy in -r..=r {
                    if dx.abs() != r && dy.abs() != r {
                        continue;
                    }
                    let Some(list) = buckets.get(&(c.0 + dx, c.1 + dy)) else {
                        continue;
                    };
                    for &(i, is_end) in list {
                        if used[i] {
                            continue;
                        }
                        let pts = &slots[i].as_ref().unwrap().points;
                        let e = if is_end { pts[pts.len() - 1] } else { pts[0] };
                        let d = e.distance(at);
                        if best.is_none_or(|(bd, bi, _)| d < bd || (d == bd && i < bi)) {
                            best = Some((d, i, is_end));
                        }
                    }
                }
            }
        }
        let Some((_, i, is_end)) = best else { break };
        used[i] = true;
        let mut p = slots[i].take().unwrap();
        if is_end {
            p.points.reverse();
        }
        at = p.points[p.points.len() - 1];
        out.push(p);
    }
    out
}

/// Line directions (degrees) and spacing for sparse infill. `base` is the spacing a
/// single set of lines would need for the chosen density; patterns with more line sets
/// space each set further apart so the density stays the same.
pub fn infill_angles(pattern: InfillPattern, layer: usize, base: f32) -> (Vec<f32>, f32) {
    match pattern {
        InfillPattern::Lines => (vec![if layer % 2 == 0 { 45.0 } else { 135.0 }], base),
        InfillPattern::Grid => (vec![45.0, 135.0], base * 2.0),
        InfillPattern::Triangles => (vec![0.0, 60.0, 120.0], base * 3.0),
    }
}

/// Fuzzy skin: points every FUZZ_POINT_DIST_MM along the loop, each pushed in or out by
/// up to FUZZ_THICKNESS_MM. The wobble is the same every time the plate is sliced.
pub fn fuzz(poly: &[Point2], seed: u64) -> Polygon {
    let n = poly.len();
    let mut out = Vec::new();
    let mut k = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut rand = || {
        k ^= k << 13;
        k ^= k >> 7;
        k ^= k << 17;
        (k % 10_000) as f32 / 10_000.0 * 2.0 - 1.0
    };
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let len = a.distance(b);
        if len < 1e-6 {
            continue;
        }
        let (nx, ny) = (-(b.y - a.y) / len, (b.x - a.x) / len);
        let steps = (len / FUZZ_POINT_DIST_MM).ceil().max(1.0) as usize;
        for s in 0..steps {
            let t = s as f32 / steps as f32;
            let d = rand() * FUZZ_THICKNESS_MM;
            out.push(Point2::new(
                a.x + (b.x - a.x) * t + nx * d,
                a.y + (b.y - a.y) * t + ny * d,
            ));
        }
    }
    out
}

/// (top of layer, layer thickness) for every layer.
pub fn layer_heights(max_z: f32, settings: &PrintSettings) -> Vec<(f32, f32)> {
    let first = settings.first_layer_height.max(0.05);
    let lh = settings.layer_height.max(0.04);
    let mut out = vec![(first, first)];
    let mut z = first;
    while z + lh * 0.5 < max_z {
        z += lh;
        out.push((z, lh));
    }
    out
}

/// Closed outlines where the plane at `zc` cuts the triangles. Outer loops run
/// counter-clockwise and holes clockwise, so the material is always on the left.
pub fn layer_outline(triangles: &[Triangle], zc: f32) -> Vec<Polygon> {
    let zc = zc + 1e-4;
    let mut segments: Vec<(Point2, Point2)> = Vec::new();
    for t in triangles {
        // A plane crosses a triangle on exactly 0 or 2 of its edges.
        let mut pts = [Point2::new(0.0, 0.0); 3];
        let mut cuts = 0;
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            if (a.z < zc) != (b.z < zc) {
                pts[cuts] = edge_cut(a, b, zc);
                cuts += 1;
            }
        }
        if cuts == 2 {
            let n = (t[1] - t[0]).cross(t[2] - t[0]);
            let (p, q) = (pts[0], pts[1]);
            // Outward normal on the right of the direction of travel.
            let right = (q.y - p.y) * n.x - (q.x - p.x) * n.y;
            if right >= 0.0 {
                segments.push((p, q));
            } else {
                segments.push((q, p));
            }
        }
    }
    let loops = chain_segments(&segments);
    orient_by_nesting(loops)
}

/// Point where an edge crosses height `zc`. Endpoints are put in a fixed order so the
/// two triangles sharing an edge get exactly the same point.
fn edge_cut(a: Vec3, b: Vec3, zc: f32) -> Point2 {
    let (a, b) = if (a.x, a.y, a.z) < (b.x, b.y, b.z) {
        (a, b)
    } else {
        (b, a)
    };
    let t = (zc - a.z) / (b.z - a.z);
    Point2::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

fn key(p: Point2) -> (i64, i64) {
    ((p.x * 1000.0).round() as i64, (p.y * 1000.0).round() as i64)
}

fn chain_segments(segments: &[(Point2, Point2)]) -> Vec<Polygon> {
    let mut starts: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    let mut ends: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, (a, b)) in segments.iter().enumerate() {
        starts.entry(key(*a)).or_default().push(i);
        ends.entry(key(*b)).or_default().push(i);
    }
    let mut used = vec![false; segments.len()];
    let mut loops = Vec::new();
    for first in 0..segments.len() {
        if used[first] {
            continue;
        }
        used[first] = true;
        let mut poly = vec![segments[first].0, segments[first].1];
        let start_key = key(segments[first].0);
        loop {
            let cur = *poly.last().unwrap();
            let k = key(cur);
            if k == start_key {
                poly.pop();
                break;
            }
            // Follow a segment starting here; accept a reversed one if the mesh is inconsistent.
            let next = starts
                .get(&k)
                .and_then(|v| v.iter().copied().find(|&j| !used[j]))
                .map(|j| (j, segments[j].1))
                .or_else(|| {
                    ends.get(&k)
                        .and_then(|v| v.iter().copied().find(|&j| !used[j]))
                        .map(|j| (j, segments[j].0))
                });
            match next {
                Some((j, p)) => {
                    used[j] = true;
                    poly.push(p);
                }
                None => break,
            }
        }
        let poly = simplify(poly);
        if poly.len() >= 3 && area(&poly).abs() > 0.01 {
            loops.push(poly);
        }
    }
    loops
}

fn simplify(poly: Polygon) -> Polygon {
    let mut out: Polygon = Vec::with_capacity(poly.len());
    for p in poly {
        if out.last().is_none_or(|q: &Point2| q.distance(p) > 0.02) {
            out.push(p);
        }
    }
    while out.len() > 2 && out[0].distance(*out.last().unwrap()) <= 0.02 {
        out.pop();
    }
    out
}

/// Signed area: positive for counter-clockwise loops.
pub fn area(poly: &[Point2]) -> f32 {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum::<f32>()
        * 0.5
}

pub fn contains(poly: &[Point2], p: Point2) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn orient_by_nesting(loops: Vec<Polygon>) -> Vec<Polygon> {
    let depths: Vec<usize> = loops
        .iter()
        .enumerate()
        .map(|(i, l)| {
            loops
                .iter()
                .enumerate()
                .filter(|(j, o)| *j != i && contains(o, l[0]))
                .count()
        })
        .collect();
    loops
        .into_iter()
        .zip(depths)
        .map(|(mut l, d)| {
            let want_ccw = d % 2 == 0;
            if (area(&l) > 0.0) != want_ccw {
                l.reverse();
            }
            l
        })
        .collect()
}

/// Moves every edge of a loop `d` mm to its left (into the material). A negative `d`
/// moves it outwards. Returns None when the loop would vanish or turn inside out.
pub fn offset_left(poly: &[Point2], d: f32) -> Option<Polygon> {
    let n = poly.len();
    if n < 3 {
        return None;
    }
    let left = |a: Point2, b: Point2| {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len = (dx * dx + dy * dy).sqrt().max(1e-9);
        (-dy / len, dx / len)
    };
    let out: Polygon = (0..n)
        .map(|i| {
            let (prev, cur, next) = (poly[(i + n - 1) % n], poly[i], poly[(i + 1) % n]);
            let n1 = left(prev, cur);
            let n2 = left(cur, next);
            let (mut bx, mut by) = (n1.0 + n2.0, n1.1 + n2.1);
            let len = (bx * bx + by * by).sqrt();
            if len < 1e-6 {
                bx = n1.0;
                by = n1.1;
            } else {
                bx /= len;
                by /= len;
            }
            let cos = (bx * n1.0 + by * n1.1).max(0.35);
            let s = d / cos;
            Point2::new(cur.x + bx * s, cur.y + by * s)
        })
        .collect();
    let (a0, a1) = (area(poly), area(&out));
    let perimeter: f32 = (0..n).map(|i| poly[i].distance(poly[(i + 1) % n])).sum();
    // Moving into an outer loop removes about perimeter × d of area; if that's more than
    // there is, the loop has collapsed (even if the leftover points look like a loop).
    let collapsed = d > 0.0 && a0 > 0.0 && a0 - perimeter * d <= 0.05;
    // Edges that now point backwards mean the loop folded over itself.
    let flipped: f32 = (0..n)
        .filter(|&i| {
            let j = (i + 1) % n;
            let (ox, oy) = (poly[j].x - poly[i].x, poly[j].y - poly[i].y);
            let (nx, ny) = (out[j].x - out[i].x, out[j].y - out[i].y);
            ox * nx + oy * ny < 0.0
        })
        .map(|i| poly[i].distance(poly[(i + 1) % n]))
        .sum();
    let folded = flipped > perimeter * 0.3;
    let shrinking_too_far = a0.signum() != a1.signum() || a1.abs() < 0.05;
    let grew_wrong_way = d > 0.0 && a0 > 0.0 && a1 > a0;
    if collapsed || folded || shrinking_too_far || grew_wrong_way {
        None
    } else {
        Some(out)
    }
}

/// Parallel lines `spacing` apart at `angle`, clipped to the region (even-odd rule).
/// Lines sit on a fixed grid so sparse infill lines up from layer to layer.
pub fn scan_fill(region: &[Polygon], spacing: f32, angle: f32) -> Vec<[Point2; 2]> {
    let (s, c) = angle.sin_cos();
    let to_local = |p: Point2| Point2::new(p.x * c + p.y * s, -p.x * s + p.y * c);
    let to_world = |p: Point2| Point2::new(p.x * c - p.y * s, p.x * s + p.y * c);
    let local: Vec<Polygon> = region
        .iter()
        .map(|p| p.iter().map(|q| to_local(*q)).collect())
        .collect();
    let (mut ymin, mut ymax) = (f32::MAX, f32::MIN);
    for p in local.iter().flatten() {
        ymin = ymin.min(p.y);
        ymax = ymax.max(p.y);
    }
    if ymin > ymax || spacing <= 0.0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut row = (ymin / spacing).ceil() as i64;
    let mut flip = false;
    loop {
        let y = row as f32 * spacing;
        if y > ymax {
            break;
        }
        let mut xs: Vec<f32> = Vec::new();
        for poly in &local {
            let n = poly.len();
            for i in 0..n {
                let (a, b) = (poly[i], poly[(i + 1) % n]);
                if (a.y > y) != (b.y > y) {
                    xs.push(a.x + (y - a.y) * (b.x - a.x) / (b.y - a.y));
                }
            }
        }
        xs.sort_by(|a, b| a.total_cmp(b));
        let (crossings, _) = xs.as_chunks::<2>();
        let mut pairs: Vec<[Point2; 2]> = crossings
            .iter()
            .filter(|p| p[1] - p[0] > spacing.min(0.4) * 0.5)
            .map(|p| {
                [
                    to_world(Point2::new(p[0], y)),
                    to_world(Point2::new(p[1], y)),
                ]
            })
            .collect();
        if flip {
            pairs.reverse();
            pairs.iter_mut().for_each(|p| p.swap(0, 1));
        }
        flip = !flip;
        out.extend(pairs);
        row += 1;
    }
    out
}

/// Cuts a line into runs that share the same answer to `test`.
fn split_by(seg: &[Point2; 2], test: impl Fn(Point2) -> bool) -> Vec<(Vec<Point2>, bool)> {
    let len = seg[0].distance(seg[1]);
    let steps = ((len / (CELL_MM * 0.5)).ceil() as usize).max(1);
    let at = |t: f32| {
        Point2::new(
            seg[0].x + (seg[1].x - seg[0].x) * t,
            seg[0].y + (seg[1].y - seg[0].y) * t,
        )
    };
    let mut out: Vec<(Vec<Point2>, bool)> = Vec::new();
    let mut run_start = 0.0f32;
    let mut current = test(at(0.5 / steps as f32));
    for k in 1..steps {
        let state = test(at((k as f32 + 0.5) / steps as f32));
        if state != current {
            let t = k as f32 / steps as f32;
            out.push((vec![at(run_start), at(t)], current));
            run_start = t;
            current = state;
        }
    }
    out.push((vec![at(run_start), at(1.0)], current));
    out.retain(|(p, _)| p[0].distance(p[1]) > 0.3);
    out
}

/// Cells of layer `i` that are near the top or bottom of the model and must be solid.
fn exposed_cells(masks: &[Vec<bool>], i: usize, settings: &PrintSettings) -> Vec<bool> {
    let cells = masks[i].len();
    let mut exposed = vec![false; cells];
    let lo = i as i64 - settings.bottom_layers as i64;
    let hi = i as i64 + settings.top_layers as i64;
    for j in lo..=hi {
        if j == i as i64 {
            continue;
        }
        if j < 0 || j >= masks.len() as i64 {
            return vec![true; cells];
        }
        for (e, m) in exposed.iter_mut().zip(&masks[j as usize]) {
            *e |= !*m;
        }
    }
    exposed
}

/// Total thickness of a full raft: the first layer plus `RAFT_LAYERS - 1` normal layers,
/// matching `layer_heights` so the raft ends exactly on a layer boundary.
pub fn raft_height(s: &PrintSettings) -> f32 {
    s.first_layer_height.max(0.05) + (RAFT_LAYERS - 1) as f32 * s.layer_height.max(0.04)
}

/// Raft lines over the `area` cells on raft layer `layer`. The bottom layer is open
/// (lines `RAFT_BASE_SPACING_MM` apart) to grip the bed; the layers above are solid and
/// cross each other, giving a flat top for what stands on it.
fn raft_paths(grid: &Grid, area: &[bool], layer: usize, lw: f32) -> Vec<ToolPath> {
    let (spacing, angle) = if layer == 0 {
        (RAFT_BASE_SPACING_MM, 0.0)
    } else if layer % 2 == 1 {
        (lw, std::f32::consts::FRAC_PI_2)
    } else {
        (lw, 0.0)
    };
    let mut out = Vec::new();
    for seg in scan_fill(&[grid.outline()], spacing, angle) {
        for (points, inside) in split_by(&seg, |p| area[grid.index_of(p)]) {
            if inside {
                out.push(open_path(PathKind::Raft, points));
            }
        }
    }
    out
}

fn adhesion_paths(first: &[Polygon], adhesion: Adhesion, lw: f32) -> Vec<ToolPath> {
    let mut out = Vec::new();
    match adhesion {
        // The raft is laid separately (`raft_paths`).
        Adhesion::None | Adhesion::Raft => {}
        Adhesion::Skirt => {
            let pts: Vec<Point2> = first.iter().flatten().copied().collect();
            let hull = convex_hull(pts);
            for k in (0..SKIRT_LOOPS).rev() {
                if let Some(p) = offset_left(&hull, -(SKIRT_DISTANCE_MM + lw * k as f32)) {
                    out.push(ToolPath {
                        kind: PathKind::Skirt,
                        points: p,
                        closed: true,
                    });
                }
            }
        }
        Adhesion::Brim => {
            for k in (0..BRIM_LINES).rev() {
                for poly in first.iter().filter(|p| area(p) > 0.0) {
                    if let Some(p) = offset_left(poly, -lw * (0.5 + k as f32)) {
                        out.push(ToolPath {
                            kind: PathKind::Brim,
                            points: p,
                            closed: true,
                        });
                    }
                }
            }
        }
    }
    out
}

/// Counter-clockwise convex hull (monotone chain).
pub fn convex_hull(mut pts: Vec<Point2>) -> Polygon {
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let cross =
        |o: Point2, a: Point2, b: Point2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let mut lower: Vec<Point2> = Vec::new();
    for p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], *p) <= 0.0 {
            lower.pop();
        }
        lower.push(*p);
    }
    let mut upper: Vec<Point2> = Vec::new();
    for p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], *p) <= 0.0 {
            upper.pop();
        }
        upper.push(*p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// A coarse square grid over the models' footprint.
struct Grid {
    x0: f32,
    y0: f32,
    w: usize,
    h: usize,
}

impl Grid {
    fn new(b: &Bounds) -> Self {
        // Room around the models for a raft's margin.
        let pad = CELL_MM * (RAFT_MARGIN_CELLS as f32 + 2.0);
        let x0 = b.min.x - pad;
        let y0 = b.min.y - pad;
        let w = ((b.size().x + 2.0 * pad) / CELL_MM).ceil() as usize + 1;
        let h = ((b.size().y + 2.0 * pad) / CELL_MM).ceil() as usize + 1;
        Self { x0, y0, w, h }
    }

    fn cells(&self) -> usize {
        self.w * self.h
    }

    fn index_of(&self, p: Point2) -> usize {
        let cx = (((p.x - self.x0) / CELL_MM) as isize).clamp(0, self.w as isize - 1) as usize;
        let cy = (((p.y - self.y0) / CELL_MM) as isize).clamp(0, self.h as isize - 1) as usize;
        cy * self.w + cx
    }

    fn center(&self, cx: usize, cy: usize) -> Point2 {
        Point2::new(
            self.x0 + (cx as f32 + 0.5) * CELL_MM,
            self.y0 + (cy as f32 + 0.5) * CELL_MM,
        )
    }

    /// Cells whose centre lies inside the outline.
    fn rasterize(&self, outline: &[Polygon]) -> Vec<bool> {
        let mut mask = vec![false; self.cells()];
        for cy in 0..self.h {
            let y = self.center(0, cy).y;
            let mut xs: Vec<f32> = Vec::new();
            for poly in outline {
                let n = poly.len();
                for i in 0..n {
                    let (a, b) = (poly[i], poly[(i + 1) % n]);
                    if (a.y > y) != (b.y > y) {
                        xs.push(a.x + (y - a.y) * (b.x - a.x) / (b.y - a.y));
                    }
                }
            }
            xs.sort_by(|a, b| a.total_cmp(b));
            for pair in xs.as_chunks::<2>().0 {
                let c0 = ((pair[0] - self.x0) / CELL_MM - 0.5).ceil().max(0.0) as usize;
                let c1 = ((pair[1] - self.x0) / CELL_MM - 0.5).floor();
                if c1 < 0.0 {
                    continue;
                }
                for cx in c0..=(c1 as usize).min(self.w - 1) {
                    mask[cy * self.w + cx] = true;
                }
            }
        }
        mask
    }

    /// Every (cell, height) where a downward-facing surface steeper than `angle_deg`
    /// (measured from vertical) needs something underneath it.
    fn overhangs(&self, triangles: &[Triangle], angle_deg: f32, min_z: f32) -> Vec<(usize, f32)> {
        let mut out = Vec::new();
        let threshold = angle_deg.to_radians().sin();
        for t in triangles {
            let n = (t[1] - t[0]).cross(t[2] - t[0]).normalized();
            if -n.z <= threshold {
                continue;
            }
            let lowest = t.iter().map(|v| v.z).fold(f32::MAX, f32::min);
            if lowest <= min_z {
                continue;
            }
            let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
            for v in t {
                x0 = x0.min(v.x);
                x1 = x1.max(v.x);
                y0 = y0.min(v.y);
                y1 = y1.max(v.y);
            }
            let tri2 = [
                Point2::new(t[0].x, t[0].y),
                Point2::new(t[1].x, t[1].y),
                Point2::new(t[2].x, t[2].y),
            ];
            let cx0 = ((x0 - self.x0) / CELL_MM).floor().max(0.0) as usize;
            let cy0 = ((y0 - self.y0) / CELL_MM).floor().max(0.0) as usize;
            let cx1 = (((x1 - self.x0) / CELL_MM) as usize).min(self.w - 1);
            let cy1 = (((y1 - self.y0) / CELL_MM) as usize).min(self.h - 1);
            for cy in cy0..=cy1 {
                for cx in cx0..=cx1 {
                    let p = self.center(cx, cy);
                    if contains(&tri2, p) {
                        // Height of the triangle's plane at this point.
                        // n.z is well below zero here (steep overhangs only).
                        let z = t[0].z - (n.x * (p.x - t[0].x) + n.y * (p.y - t[0].y)) / n.z;
                        out.push((cy * self.w + cx, z.max(lowest)));
                    }
                }
            }
        }
        out
    }

    /// Support lines for every layer. Worked out from the top down: a column starts just
    /// under each overhang and goes down until it lands on the plate or on the model.
    /// With `plate_only`, columns that would land on the model aren't made at all.
    /// Also returns the cells whose columns reach the plate. Faces whose lowest point is
    /// at or below `floor_z` rest on the plate (or raft) and need nothing under them.
    #[allow(clippy::too_many_arguments)]
    fn supports(
        &self,
        triangles: &[Triangle],
        masks: &[Vec<bool>],
        heights: &[(f32, f32)],
        floor_z: f32,
        plate_only: bool,
        angle_deg: f32,
        lw: f32,
    ) -> (Vec<Vec<ToolPath>>, Vec<bool>) {
        let n = heights.len();
        // Lowest layer with model in each cell (n when there is none).
        let mut first_model = vec![n; self.cells()];
        for (i, mask) in masks.iter().enumerate().rev() {
            for (f, m) in first_model.iter_mut().zip(mask) {
                if *m {
                    *f = i;
                }
            }
        }
        let mut starts: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (cell, h) in self.overhangs(triangles, angle_deg, floor_z) {
            // Highest layer that stays at least one layer below the overhang.
            let k = heights.partition_point(|(z, lh)| z + lh.max(0.15) < h);
            if k > 0 && (!plate_only || first_model[cell] >= k) {
                starts[k - 1].push(cell);
            }
        }
        let mut active = vec![false; self.cells()];
        let mut out = vec![Vec::new(); n];
        for i in (0..n).rev() {
            for &c in &starts[i] {
                active[c] = true;
            }
            for (a, m) in active.iter_mut().zip(&masks[i]) {
                if *m {
                    *a = false;
                }
            }
            out[i] = self.support_lines(&masks[i], &active, lw);
        }
        // After the bottom layer, the cells still active are the columns on the plate.
        (out, active)
    }

    /// `mask` grown by `r` cells in every direction (a square around each cell).
    /// The 4-connected groups of set cells in `mask`, each as a list of cell indices.
    fn groups(&self, mask: &[bool]) -> Vec<Vec<usize>> {
        let mut seen = vec![false; mask.len()];
        let mut groups = Vec::new();
        for start in 0..mask.len() {
            if !mask[start] || seen[start] {
                continue;
            }
            seen[start] = true;
            let (mut group, mut todo) = (Vec::new(), vec![start]);
            while let Some(c) = todo.pop() {
                group.push(c);
                let (cx, cy) = (c % self.w, c / self.w);
                let near = [
                    (cx > 0).then(|| c - 1),
                    (cx + 1 < self.w).then(|| c + 1),
                    (cy > 0).then(|| c - self.w),
                    (cy + 1 < self.h).then(|| c + self.w),
                ];
                for n in near.into_iter().flatten() {
                    if mask[n] && !seen[n] {
                        seen[n] = true;
                        todo.push(n);
                    }
                }
            }
            groups.push(group);
        }
        groups
    }

    /// For lines through each cell of `group` in direction `angle`: how many have both
    /// ends on an `anchor` cell (the first cell outside the group each way), and their
    /// mean length (mm).
    fn spans(
        &self,
        group: &[usize],
        in_group: &[bool],
        angle: f32,
        anchor: impl Fn(usize) -> bool,
    ) -> (usize, f32) {
        let step = 0.5 * CELL_MM;
        let (dx, dy) = (angle.cos() * step, angle.sin() * step);
        let outside = |p: Point2| {
            p.x < self.x0
                || p.y < self.y0
                || p.x >= self.x0 + self.w as f32 * CELL_MM
                || p.y >= self.y0 + self.h as f32 * CELL_MM
        };
        // Walks from `p` until it leaves the group: the cell reached (None off the grid)
        // and the distance walked.
        let walk = |mut p: Point2, sx: f32, sy: f32| {
            let mut dist = 0.0;
            loop {
                p = Point2::new(p.x + sx, p.y + sy);
                dist += step;
                if outside(p) {
                    return (None, dist);
                }
                let c = self.index_of(p);
                if !in_group[c] {
                    return (Some(c), dist);
                }
            }
        };
        let (mut anchored, mut total) = (0, 0.0);
        for &c in group {
            let p = self.center(c % self.w, c / self.w);
            let (a, da) = walk(p, dx, dy);
            let (b, db) = walk(p, -dx, -dy);
            if a.is_some_and(&anchor) && b.is_some_and(&anchor) {
                anchored += 1;
            }
            total += da + db;
        }
        (anchored, total / group.len().max(1) as f32)
    }

    fn dilate(&self, mask: &[bool], r: usize) -> Vec<bool> {
        let mut out = vec![false; mask.len()];
        for cy in 0..self.h {
            for cx in 0..self.w {
                if !mask[cy * self.w + cx] {
                    continue;
                }
                for y in cy.saturating_sub(r)..=(cy + r).min(self.h - 1) {
                    for x in cx.saturating_sub(r)..=(cx + r).min(self.w - 1) {
                        out[y * self.w + x] = true;
                    }
                }
            }
        }
        out
    }

    /// The whole grid as one rectangle.
    fn outline(&self) -> Polygon {
        let (x1, y1) = (
            self.x0 + self.w as f32 * CELL_MM,
            self.y0 + self.h as f32 * CELL_MM,
        );
        vec![
            Point2::new(self.x0, self.y0),
            Point2::new(x1, self.y0),
            Point2::new(x1, y1),
            Point2::new(self.x0, y1),
        ]
    }

    /// Lines along X through the active cells of every other row, keeping a one-cell
    /// gap from the model so supports peel off easily.
    fn support_lines(&self, mask: &[bool], active: &[bool], lw: f32) -> Vec<ToolPath> {
        let mut out = Vec::new();
        let near_model = |cx: usize, cy: usize| {
            let x_lo = cx.saturating_sub(1);
            let y_lo = cy.saturating_sub(1);
            (y_lo..=(cy + 1).min(self.h - 1))
                .any(|y| (x_lo..=(cx + 1).min(self.w - 1)).any(|x| mask[y * self.w + x]))
        };
        for (row_no, cy) in (0..self.h).step_by(SUPPORT_ROW_STEP).enumerate() {
            let mut runs: Vec<(usize, usize)> = Vec::new();
            let mut start = None;
            for cx in 0..=self.w {
                let on = cx < self.w && active[cy * self.w + cx] && !near_model(cx, cy);
                match (on, start) {
                    (true, None) => start = Some(cx),
                    (false, Some(s)) => {
                        runs.push((s, cx - 1));
                        start = None;
                    }
                    _ => {}
                }
            }
            if row_no % 2 == 1 {
                runs.reverse();
            }
            for (a, b) in runs {
                let y = self.center(a, cy).y;
                let xa = self.x0 + a as f32 * CELL_MM + lw / 2.0;
                let xb = self.x0 + (b + 1) as f32 * CELL_MM - lw / 2.0;
                if xb - xa < 0.5 {
                    continue;
                }
                let mut pts = vec![Point2::new(xa, y), Point2::new(xb, y)];
                if row_no % 2 == 1 {
                    pts.reverse();
                }
                out.push(ToolPath {
                    kind: PathKind::Support,
                    points: pts,
                    closed: false,
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Mesh, PlacedObject};

    #[test]
    fn a_degenerate_polygon_contains_nothing() {
        let p = Point2::new(0.0, 0.0);
        assert!(!contains(&[], p));
        assert!(!contains(&[p], p));
        assert!(!contains(&[p, Point2::new(1.0, 1.0)], p));
    }

    /// A box `size` with its bottom face centred on `at`.
    fn box_at(size: Vec3, at: Vec3) -> Vec<Triangle> {
        Mesh::cube(size)
            .triangles
            .iter()
            .map(|t| t.map(|v| v + at))
            .collect()
    }

    /// Two 10 x 10 x 10 mm pillars 20 mm apart, with a 40 x 10 x 2 mm deck across them.
    fn bridge_test() -> Vec<Triangle> {
        let pillar = Vec3::new(10.0, 10.0, 10.0);
        let mut tris = box_at(pillar, Vec3::new(-15.0, 0.0, 0.0));
        tris.extend(box_at(pillar, Vec3::new(15.0, 0.0, 0.0)));
        tris.extend(box_at(
            Vec3::new(40.0, 10.0, 2.0),
            Vec3::new(0.0, 0.0, 10.0),
        ));
        tris
    }

    fn bridge_layers(r: &SliceResult) -> Vec<usize> {
        (0..r.layers.len())
            .filter(|&i| r.layers[i].paths.iter().any(|p| p.kind == PathKind::Bridge))
            .collect()
    }

    #[test]
    fn test_bridge_spans_the_gap_between_pillars() {
        let s = PrintSettings::default();
        let r = slice(&bridge_test(), &s, 0.42, &|_| {});
        let layers = bridge_layers(&r);
        // Only the deck's first layer is over air.
        assert_eq!(layers.len(), 1, "{layers:?}");
        let layer = &r.layers[layers[0]];
        assert!(layer.z > 10.0 && layer.z < 10.5, "{}", layer.z);
        let bridge: Vec<&ToolPath> = layer
            .paths
            .iter()
            .filter(|p| p.kind == PathKind::Bridge)
            .collect();
        // The lines run along X, pillar to pillar.
        for p in &bridge {
            let (a, b) = (p.points[0], *p.points.last().unwrap());
            assert!((b.y - a.y).abs() < 0.01, "not along X: {a:?} -> {b:?}");
        }
        let xs = bridge.iter().flat_map(|p| p.points.iter().map(|q| q.x));
        let (lo, hi) = xs.fold((f32::MAX, f32::MIN), |(l, h), x| (l.min(x), h.max(x)));
        assert!(
            lo < -10.5 && hi > 10.5,
            "bridge {lo}..{hi} doesn't reach the pillars"
        );
        // The gap is fully bridged: no sparse or plain solid infill over it.
        assert!(!layer.paths.iter().any(|p| {
            matches!(p.kind, PathKind::SparseInfill | PathKind::SolidInfill)
                && p.points.iter().any(|q| q.x.abs() < 8.0)
        }));
    }

    #[test]
    fn test_bridge_not_on_a_supported_top_or_the_first_layer() {
        let s = PrintSettings::default();
        let cube = box_at(Vec3::new(20.0, 20.0, 10.0), Vec3::new(0.0, 0.0, 0.0));
        assert!(bridge_layers(&slice(&cube, &s, 0.42, &|_| {})).is_empty());
        let raft = PrintSettings {
            adhesion: Adhesion::Raft,
            ..PrintSettings::default()
        };
        assert!(bridge_layers(&slice(&cube, &raft, 0.42, &|_| {})).is_empty());
    }

    #[test]
    fn test_bridge_not_for_an_overhang_anchored_on_one_side() {
        // A 20 mm shelf sticking out of one pillar: nothing to land on at the far end.
        let mut tris = box_at(Vec3::new(10.0, 10.0, 10.0), Vec3::new(-15.0, 0.0, 0.0));
        tris.extend(box_at(
            Vec3::new(30.0, 10.0, 2.0),
            Vec3::new(-5.0, 0.0, 10.0),
        ));
        let r = slice(&tris, &PrintSettings::default(), 0.42, &|_| {});
        assert!(bridge_layers(&r).is_empty());
    }

    fn cube_tris(size: f32) -> Vec<Triangle> {
        PlacedObject::new("c", Mesh::cube(Vec3::new(size, size, size))).world_triangles()
    }

    #[test]
    fn cube_outline_is_one_ccw_square() {
        let outline = layer_outline(&cube_tris(20.0), 5.0);
        assert_eq!(outline.len(), 1);
        assert!((area(&outline[0]) - 400.0).abs() < 0.5);
    }

    #[test]
    fn tube_has_hole_running_clockwise() {
        let tube = PlacedObject::new("t", crate::model::SampleShape::Tube.mesh()).world_triangles();
        let outline = layer_outline(&tube, 5.0);
        assert_eq!(outline.len(), 2);
        assert_eq!(outline.iter().filter(|p| area(p) < 0.0).count(), 1);
    }

    #[test]
    fn layer_count_matches_height() {
        let s = PrintSettings::default();
        let h = layer_heights(10.0, &s);
        assert_eq!(h.len(), 50);
        assert!((h.last().unwrap().0 - 10.0).abs() < 1e-3);
    }

    #[test]
    fn inward_offset_shrinks_square() {
        let sq = vec![
            Point2::new(0.0, 0.0),
            Point2::new(10.0, 0.0),
            Point2::new(10.0, 10.0),
            Point2::new(0.0, 10.0),
        ];
        let o = offset_left(&sq, 1.0).unwrap();
        assert!((area(&o) - 64.0).abs() < 1e-3);
        assert!(offset_left(&sq, 6.0).is_none());
    }

    #[test]
    fn cube_slices_into_walls_and_infill() {
        let s = PrintSettings::default();
        let r = slice(&cube_tris(20.0), &s, 0.42, &|_| {});
        assert_eq!(r.layers.len(), 100);
        let first = &r.layers[0];
        assert!(first.paths.iter().any(|p| p.kind == PathKind::Skirt));
        assert!(first.paths.iter().any(|p| p.kind == PathKind::SolidInfill));
        let middle = &r.layers[50];
        assert!(middle
            .paths
            .iter()
            .any(|p| p.kind == PathKind::SparseInfill));
        assert!(!middle.paths.iter().any(|p| p.kind == PathKind::SolidInfill));
        assert_eq!(
            middle
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::OuterWall)
                .count(),
            1
        );
    }

    #[test]
    fn overhang_gets_support() {
        // A slab floating 10 mm up, held by a thin post in one corner.
        let mut tris =
            PlacedObject::new("slab", Mesh::cube(Vec3::new(30.0, 30.0, 3.0))).world_triangles();
        for t in &mut tris {
            for v in t.iter_mut() {
                v.z += 10.0;
            }
        }
        tris.extend(
            PlacedObject::new("post", Mesh::cube(Vec3::new(4.0, 4.0, 10.0))).world_triangles(),
        );
        let s = PrintSettings {
            supports: true,
            ..PrintSettings::default()
        };
        let r = slice(&tris, &s, 0.42, &|_| {});
        assert!(r.layers[10]
            .paths
            .iter()
            .any(|p| p.kind == PathKind::Support));
        let top = r.layers.last().unwrap();
        assert!(!top.paths.iter().any(|p| p.kind == PathKind::Support));
    }

    fn slab_on_post() -> Vec<Triangle> {
        let mut tris =
            PlacedObject::new("slab", Mesh::cube(Vec3::new(30.0, 30.0, 3.0))).world_triangles();
        tris.iter_mut().flatten().for_each(|v| v.z += 10.0);
        tris.extend(
            PlacedObject::new("post", Mesh::cube(Vec3::new(4.0, 4.0, 10.0))).world_triangles(),
        );
        tris
    }

    fn kinds(layer: &Layer) -> Vec<PathKind> {
        let mut k: Vec<PathKind> = layer.paths.iter().map(|p| p.kind).collect();
        k.dedup();
        k
    }

    #[test]
    fn supports_always_stand_on_a_raft() {
        let s = PrintSettings {
            supports: true,
            ..PrintSettings::default()
        };
        assert_eq!(s.adhesion, Adhesion::Skirt);
        let r = slice(&slab_on_post(), &s, 0.42, &|_| {});
        for (i, layer) in r.layers.iter().enumerate().take(RAFT_LAYERS) {
            let k = kinds(layer);
            assert!(k.contains(&PathKind::Raft), "layer {i}: {k:?}");
            assert!(!k.contains(&PathKind::Support), "layer {i}: {k:?}");
            // The post still stands on the plate, beside the support raft.
            assert!(k.contains(&PathKind::OuterWall), "layer {i}: {k:?}");
        }
        assert!(kinds(&r.layers[RAFT_LAYERS]).contains(&PathKind::Support));
        assert!(!kinds(&r.layers[RAFT_LAYERS]).contains(&PathKind::Raft));
        // The support raft keeps clear of the post.
        let post = |p: &Point2| p.x.abs() < 2.5 && p.y.abs() < 2.5;
        assert!(r.layers[0]
            .paths
            .iter()
            .filter(|p| p.kind == PathKind::Raft)
            .all(|p| !p.points.iter().any(post)));
        // No supports, no raft.
        let plain = slice(&slab_on_post(), &PrintSettings::default(), 0.42, &|_| {});
        assert!(plain
            .layers
            .iter()
            .all(|l| !kinds(l).contains(&PathKind::Raft)));
    }

    #[test]
    fn raft_under_support_can_be_turned_off() {
        let s = PrintSettings {
            supports: true,
            support_raft: false,
            ..PrintSettings::default()
        };
        assert!(PrintSettings::default().support_raft, "on by default");
        let r = slice(&slab_on_post(), &s, 0.42, &|_| {});
        assert!(r.layers.iter().all(|l| !kinds(l).contains(&PathKind::Raft)));
        // The supports go all the way down to the plate instead.
        assert!(kinds(&r.layers[0]).contains(&PathKind::Support));
        // A full raft still covers the supports whatever the switch says.
        let full = PrintSettings {
            adhesion: Adhesion::Raft,
            ..s
        };
        let r = slice(&slab_on_post(), &full, 0.42, &|_| {});
        assert_eq!(kinds(&r.layers[0]), vec![PathKind::Raft]);
    }

    #[test]
    fn a_raft_lifts_the_model() {
        let cube =
            PlacedObject::new("c", Mesh::cube(Vec3::new(20.0, 20.0, 10.0))).world_triangles();
        let plain = slice(&cube, &PrintSettings::default(), 0.42, &|_| {});
        let s = PrintSettings {
            adhesion: Adhesion::Raft,
            ..PrintSettings::default()
        };
        let r = slice(&cube, &s, 0.42, &|_| {});
        assert_eq!(r.layers.len(), plain.layers.len() + RAFT_LAYERS);
        for (i, layer) in r.layers.iter().enumerate().take(RAFT_LAYERS) {
            assert_eq!(kinds(layer), vec![PathKind::Raft], "layer {i}");
        }
        let first = &r.layers[RAFT_LAYERS];
        assert!(kinds(first).contains(&PathKind::OuterWall));
        let expected = raft_height(&s) + s.layer_height + RAFT_AIR_GAP_MM;
        assert!(
            (first.z - expected).abs() < 1e-4,
            "{} vs {expected}",
            first.z
        );
        // The raft reaches about 3 mm past the 20 mm cube.
        let reach = r.layers[0]
            .paths
            .iter()
            .flat_map(|p| &p.points)
            .fold(0f32, |m, p| m.max(p.x.abs()));
        assert!((12.0..14.5).contains(&reach), "{reach}");
        // The bottom of the cube is solid, as on the plate.
        assert!(kinds(first).contains(&PathKind::SolidInfill));
    }

    #[test]
    fn default_infill_is_25_percent() {
        assert_eq!(PrintSettings::default().infill_percent, 25.0);
    }

    #[test]
    fn support_can_stand_on_the_model() {
        // A wide base, and a slab hovering 10 mm above it, held by a post.
        let mut tris =
            PlacedObject::new("base", Mesh::cube(Vec3::new(40.0, 40.0, 4.0))).world_triangles();
        let mut slab =
            PlacedObject::new("slab", Mesh::cube(Vec3::new(30.0, 30.0, 3.0))).world_triangles();
        slab.iter_mut().flatten().for_each(|v| v.z += 14.0);
        let mut post =
            PlacedObject::new("post", Mesh::cube(Vec3::new(4.0, 4.0, 10.0))).world_triangles();
        post.iter_mut().flatten().for_each(|v| v.z += 4.0);
        tris.extend(slab);
        tris.extend(post);
        let s = PrintSettings {
            supports: true,
            ..PrintSettings::default()
        };
        let r = slice(&tris, &s, 0.42, &|_| {});
        // Layer 40 is at 8.2 mm: between the base (0-4 mm) and the slab (14 mm).
        assert!(r.layers[40]
            .paths
            .iter()
            .any(|p| p.kind == PathKind::Support));
        // Nothing inside the base itself.
        assert!(!r.layers[5]
            .paths
            .iter()
            .any(|p| p.kind == PathKind::Support));
    }

    #[test]
    fn plate_only_supports_never_stand_on_the_model() {
        let mut tris =
            PlacedObject::new("base", Mesh::cube(Vec3::new(40.0, 40.0, 4.0))).world_triangles();
        let mut slab =
            PlacedObject::new("slab", Mesh::cube(Vec3::new(30.0, 30.0, 3.0))).world_triangles();
        slab.iter_mut().flatten().for_each(|v| v.z += 14.0);
        tris.extend(slab);
        let s = PrintSettings {
            supports: true,
            supports_on_plate_only: true,
            ..PrintSettings::default()
        };
        let r = slice(&tris, &s, 0.42, &|_| {});
        assert!(!r.layers[40]
            .paths
            .iter()
            .any(|p| p.kind == PathKind::Support));
    }

    /// Travel moves longer than `limit` between consecutive lines.
    fn long_hops(paths: &[ToolPath], limit: f32) -> usize {
        paths
            .windows(2)
            .filter(|w| w[0].points.last().unwrap().distance(w[1].points[0]) > limit)
            .count()
    }

    #[test]
    fn infill_finishes_one_side_of_a_hole_before_the_other() {
        let square = |c: f32, h: f32| {
            vec![
                Point2::new(c - h, c - h),
                Point2::new(c + h, c - h),
                Point2::new(c + h, c + h),
                Point2::new(c - h, c + h),
            ]
        };
        // A 40 mm square with a 12 mm hole in the middle.
        let mut hole = square(0.0, 6.0);
        hole.reverse();
        let region = vec![square(0.0, 20.0), hole];
        let lines: Vec<ToolPath> = scan_fill(&region, 0.5, 0.0)
            .into_iter()
            .map(|s| open_path(PathKind::SolidInfill, s.to_vec()))
            .collect();
        // Straight row by row, the nozzle hops over the hole on all 24 rows it cuts.
        assert!(long_hops(&lines, 3.0) >= 20);
        let ordered = order_open_paths(lines.clone(), None);
        assert_eq!(ordered.len(), lines.len());
        assert!(
            long_hops(&ordered, 3.0) <= 3,
            "{}",
            long_hops(&ordered, 3.0)
        );
        let total =
            |p: &[ToolPath]| -> f32 { p.iter().map(|t| t.points[0].distance(t.points[1])).sum() };
        assert!((total(&ordered) - total(&lines)).abs() < 1e-2);
    }

    #[test]
    fn infill_patterns_keep_density() {
        let (a, sa) = infill_angles(InfillPattern::Lines, 0, 2.0);
        let (b, sb) = infill_angles(InfillPattern::Grid, 0, 2.0);
        let (c, sc) = infill_angles(InfillPattern::Triangles, 0, 2.0);
        assert_eq!((a.len(), b.len(), c.len()), (1, 2, 3));
        assert_eq!((sa, sb, sc), (2.0, 4.0, 6.0));
    }

    #[test]
    fn fuzzy_skin_stays_close_to_the_wall() {
        let sq = vec![
            Point2::new(0.0, 0.0),
            Point2::new(10.0, 0.0),
            Point2::new(10.0, 10.0),
            Point2::new(0.0, 10.0),
        ];
        let f = fuzz(&sq, 3);
        assert!(f.len() > 40);
        assert!(f
            .iter()
            .all(|p| p.x > -0.31 && p.x < 10.31 && p.y > -0.31 && p.y < 10.31));
        assert!((area(&f) - 100.0).abs() < 8.0);
    }
}
