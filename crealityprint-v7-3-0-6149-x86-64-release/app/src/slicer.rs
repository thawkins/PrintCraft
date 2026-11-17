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
}

impl PathKind {
    pub const ALL: [PathKind; 9] = [
        Self::OuterWall,
        Self::InnerWall,
        Self::TopSurface,
        Self::SolidInfill,
        Self::SparseInfill,
        Self::Ironing,
        Self::Support,
        Self::Skirt,
        Self::Brim,
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
        }
    }

    pub fn gcode_type(self) -> &'static str {
        match self {
            Self::Skirt | Self::Brim => "SKIRT",
            Self::OuterWall => "WALL-OUTER",
            Self::InnerWall => "WALL-INNER",
            Self::SolidInfill => "SKIN",
            Self::SparseInfill => "FILL",
            Self::Support => "SUPPORT",
            Self::Ironing => "IRONING",
            Self::TopSurface => "TOP-SURFACE",
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
    let bounds = Bounds::of(triangles.iter().flatten().copied());
    if !bounds.is_valid() || bounds.max.z <= 0.0 {
        return SliceResult {
            layers: Vec::new(),
            line_width,
        };
    }

    let heights = layer_heights(bounds.max.z, settings);
    let n = heights.len();

    // 1. Outlines of every layer.
    let outlines = all_outlines(triangles, &heights, progress);

    // 2. Coarse grid masks, used to find top/bottom surfaces and support areas.
    let grid = Grid::new(&bounds);
    let masks: Vec<Vec<bool>> = outlines.iter().map(|o| grid.rasterize(o)).collect();
    let mut supports = if settings.supports {
        grid.supports(
            triangles,
            &masks,
            &heights,
            settings.support_angle,
            settings.supports_on_plate_only,
            line_width,
        )
    } else {
        vec![Vec::new(); n]
    };

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
        paths.extend(wall_paths(
            &outlines[i],
            i,
            wall_count,
            line_width,
            settings,
        ));

        let region = infill_region(&outlines[i], wall_count, line_width);
        if !region.is_empty() && vase {
            // Vase mode: a solid bottom, then nothing but the spiral wall.
            if i < settings.bottom_layers as usize {
                paths.extend(vase_bottom_paths(&region, i, line_width));
            }
        } else if !region.is_empty() {
            let exposed = exposed_cells(&masks, i, settings);
            let top = top_cells(&grid, &masks, i);
            let angle = if i % 2 == 0 { 45f32 } else { 135f32 }.to_radians();
            paths.extend(solid_infill_paths(
                &region, &grid, &exposed, &top, angle, line_width,
            ));
            if settings.infill_percent > 0.5 {
                paths.extend(sparse_infill_paths(
                    &region, &grid, &exposed, i, line_width, settings,
                ));
            }
            // Only the very top layer of each surface is ironed. Fuzzy top wins: ironing
            // would flatten its texture.
            if settings.ironing && !settings.fuzzy_top && top.iter().any(|t| *t) {
                paths.extend(ironing_paths(&region, &grid, &top, angle));
            }
        }

        paths.append(&mut supports[i]);

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
) -> Vec<ToolPath> {
    let mut paths = Vec::new();
    let base = line_width * 100.0 / settings.infill_percent.min(100.0);
    let (angles, spacing) = infill_angles(settings.infill_pattern, layer, base);
    for angle in angles {
        for seg in scan_fill(region, spacing, angle.to_radians()) {
            for piece in split_by(&seg, |p| exposed[grid.index_of(p)]) {
                if !piece.1 {
                    paths.push(open_path(PathKind::SparseInfill, piece.0));
                }
            }
        }
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

fn adhesion_paths(first: &[Polygon], adhesion: Adhesion, lw: f32) -> Vec<ToolPath> {
    let mut out = Vec::new();
    match adhesion {
        Adhesion::None => {}
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
fn convex_hull(mut pts: Vec<Point2>) -> Polygon {
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
        let x0 = b.min.x - 2.0;
        let y0 = b.min.y - 2.0;
        let w = ((b.size().x + 4.0) / CELL_MM).ceil() as usize + 1;
        let h = ((b.size().y + 4.0) / CELL_MM).ceil() as usize + 1;
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
    fn supports(
        &self,
        triangles: &[Triangle],
        masks: &[Vec<bool>],
        heights: &[(f32, f32)],
        angle_deg: f32,
        plate_only: bool,
        lw: f32,
    ) -> Vec<Vec<ToolPath>> {
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
        for (cell, h) in self.overhangs(triangles, angle_deg, heights[0].0) {
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
        out
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
