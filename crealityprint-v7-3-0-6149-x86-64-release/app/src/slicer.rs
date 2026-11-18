//! Turns triangles into printable layers: outlines, walls, solid and sparse infill,
//! simple tree-free supports, and a skirt or brim. Rules are listed in MECHANICS.md.

use crate::geometry::{Bounds, Point2, Vec3};
use crate::model::Triangle;
use crate::profiles::{Adhesion, InfillPattern, PrintSettings, SolidPattern, SupportStyle};
use std::collections::HashMap;

mod arachne;
mod lightning;
mod tree;

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
    /// One line down the middle of a space too narrow for another wall loop, as wide as
    /// the space (`ToolPath::width`).
    GapFill,
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
    pub const ALL: [PathKind; 14] = [
        Self::OuterWall,
        Self::InnerWall,
        Self::GapFill,
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
            Self::GapFill => "Gap fill",
            Self::PrimeTower => "Prime tower",
            Self::OozeShield => "Ooze shield",
        }
    }

    pub fn gcode_type(self) -> &'static str {
        match self {
            Self::Skirt | Self::Brim => "SKIRT",
            Self::OuterWall => "WALL-OUTER",
            // Gap fill sits between the walls; Cura-style names have no type of its own.
            Self::InnerWall | Self::GapFill => "WALL-INNER",
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
    /// The line's width (mm) when it isn't the slice's line width: gap fill is as wide as
    /// the gap it fills. `None` = the normal line width.
    pub width: Option<f32>,
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

/// Inside a support enforcer, any surface facing down by more than this (degrees from
/// upright) is supported.
const ENFORCED_OVERHANG_DEG: f32 = 1.0;

/// Whether `p` is inside the closed mesh `tris`: a ray straight up from it crosses the
/// surface an odd number of times.
fn inside_mesh(tris: &[Triangle], p: Vec3) -> bool {
    let q = Point2::new(p.x, p.y);
    tris.iter()
        .filter(|t| {
            let flat = t.map(|v| Point2::new(v.x, v.y));
            if !contains(&flat, q) {
                return false;
            }
            let n = (t[1] - t[0]).cross(t[2] - t[0]);
            if n.z.abs() < 1e-9 {
                return false;
            }
            // The triangle's plane at the point, above it or not.
            let z = t[0].z - (n.x * (p.x - t[0].x) + n.y * (p.y - t[0].y)) / n.z;
            z > p.z
        })
        .count()
        % 2
        == 1
}

/// Grid used for top/bottom detection and supports.
const CELL_MM: f32 = 1.0;
const SKIRT_DISTANCE_MM: f32 = 5.0;
const SKIRT_LOOPS: usize = 2;
const BRIM_LINES: usize = 8;
/// Raft: layers, how far it reaches past the print (cells of 1 mm), the base layer's line
/// spacing, and the gap left between a full raft and the model.
pub const RAFT_LAYERS: usize = 3;
/// Bridge lines reach this many cells onto the supported area at each end, to anchor.
const BRIDGE_ANCHOR_CELLS: usize = 2;
/// Unsupported areas smaller than this many cells are left as normal solid infill.
const MIN_BRIDGE_CELLS: usize = 4;
/// Bridge directions tried, every 15 degrees over a half turn.
const BRIDGE_ANGLE_STEPS: usize = 12;
/// Gaps narrower than this share of the line width are left empty: too thin to print.
const MIN_GAP_FILL: f32 = 0.3;
const RAFT_MARGIN_CELLS: usize = 3;
const SUPPORT_RAFT_MARGIN_CELLS: usize = 2;
const RAFT_BASE_SPACING_MM: f32 = 1.0;
const RAFT_AIR_GAP_MM: f32 = 0.15;
/// Gap between ironing passes.
const IRONING_SPACING_MM: f32 = 0.2;
/// Fuzzy skin: distance between wobble points and how far each may move.
const FUZZ_POINT_DIST_MM: f32 = 0.8;
const FUZZ_THICKNESS_MM: f32 = 0.3;

/// A region of a part that prints its infill differently: inside `triangles` (a closed
/// mesh, in plate coordinates) the infill density and pattern given replace the part's.
/// 100 % infill there is laid as solid infill.
#[derive(Clone, Debug, PartialEq)]
pub struct InfillModifier {
    pub triangles: Vec<Triangle>,
    pub infill_percent: Option<f32>,
    pub infill_pattern: Option<InfillPattern>,
}

/// Volumes that change where supports go, in plate coordinates (closed meshes). No
/// support is made for an overhang inside a blocker; every surface facing down inside
/// an enforcer is supported, even with supports off.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SupportVolumes {
    pub blockers: Vec<Vec<Triangle>>,
    pub enforcers: Vec<Vec<Triangle>>,
}

/// Slices the given plate triangles. `progress` receives values from 0 to 1. (The
/// window and the command line slice through `slice_groups`; tests use this.)
#[cfg(test)]
pub fn slice(
    triangles: &[Triangle],
    settings: &PrintSettings,
    line_width: f32,
    progress: &dyn Fn(f32),
) -> SliceResult {
    slice_modified(
        triangles,
        &[],
        &SupportVolumes::default(),
        None,
        settings,
        line_width,
        progress,
    )
}

/// Like [`slice`], with infill modifiers (where they overlap, a later one wins),
/// support volumes, and the layers to slice at (`None`: worked out from `triangles`;
/// parts sliced apart are given the whole plate's, so their layers line up).
pub fn slice_modified(
    triangles: &[Triangle],
    modifiers: &[InfillModifier],
    support_volumes: &SupportVolumes,
    layers: Option<&[(f32, f32)]>,
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

    let mut heights = match layers {
        Some(given) => given.to_vec(),
        None => plan_layers(triangles, settings, line_width),
    };
    let n = heights.len();
    if raft_n >= n {
        return SliceResult {
            layers: Vec::new(),
            line_width,
        };
    }

    // 1. Outlines of every layer, and of every modifier (lifted with the model).
    let mut outlines = all_outlines(triangles, &heights, progress);
    compensate(&mut outlines, settings, !full_raft);
    let lift = if full_raft {
        raft_height(settings)
    } else {
        0.0
    };
    let modifier_outlines: Vec<Vec<Vec<Polygon>>> = modifiers
        .iter()
        .map(|m| {
            let up = Vec3::new(0.0, 0.0, lift);
            let tris: Vec<Triangle> = m.triangles.iter().map(|t| t.map(|v| v + up)).collect();
            heights
                .iter()
                .map(|(z, h)| layer_outline(&tris, z - h / 2.0))
                .collect()
        })
        .collect();

    // 2. Coarse grid masks, used to find top/bottom surfaces and support areas.
    let grid = Grid::new(&bounds);
    let masks: Vec<Vec<bool>> = outlines.iter().map(|o| grid.rasterize(o)).collect();
    let enforced = !support_volumes.enforcers.is_empty();
    let (mut supports, plate_cells) = if settings.supports || enforced {
        let floor_z = heights[raft_n].0;
        let mut points = if settings.supports {
            grid.overhangs(triangles, settings.support_angle, floor_z)
        } else {
            Vec::new()
        };
        // The volumes are placed on the plate; a full raft lifts the model above them.
        let lifted = |volumes: &[Vec<Triangle>]| -> Vec<Vec<Triangle>> {
            let up = Vec3::new(
                0.0,
                0.0,
                if full_raft {
                    raft_height(settings)
                } else {
                    0.0
                },
            );
            volumes
                .iter()
                .map(|v| v.iter().map(|t| t.map(|p| p + up)).collect())
                .collect()
        };
        let at = |&(cell, z): &(usize, f32)| {
            let c = grid.center(cell % grid.w, cell / grid.w);
            Vec3::new(c.x, c.y, z)
        };
        if enforced {
            let enforcers = lifted(&support_volumes.enforcers);
            points.extend(
                grid.overhangs(triangles, ENFORCED_OVERHANG_DEG, floor_z)
                    .into_iter()
                    .filter(|p| enforcers.iter().any(|v| inside_mesh(v, at(p)))),
            );
        }
        let blockers = lifted(&support_volumes.blockers);
        points.retain(|p| !blockers.iter().any(|v| inside_mesh(v, at(p))));
        match settings.support_style {
            SupportStyle::Grid => grid.supports(&points, &masks, &heights, settings, line_width),
            SupportStyle::Tree => {
                tree::tree_supports(&grid, &points, &masks, &heights, settings, line_width)
            }
        }
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

    let vase = settings.vase_mode;
    // Cells at least ADAPTIVE_CUBIC_NEAR_MM inside each layer's outline, for adaptive
    // cubic (only worked out when it is used).
    let adaptive_used = settings.infill_pattern == InfillPattern::AdaptiveCubic
        || modifiers
            .iter()
            .any(|m| m.infill_pattern == Some(InfillPattern::AdaptiveCubic));
    let deep: Vec<Vec<bool>> = if adaptive_used {
        let r = (ADAPTIVE_CUBIC_NEAR_MM / CELL_MM).ceil() as usize;
        masks
            .iter()
            .map(|m| {
                let outside: Vec<bool> = m.iter().map(|c| !c).collect();
                grid.dilate(&outside, r).iter().map(|c| !c).collect()
            })
            .collect()
    } else {
        Vec::new()
    };
    let near_layers = (ADAPTIVE_CUBIC_NEAR_MM / settings.layer_height.max(0.04)).ceil() as usize;
    // Lightning: trees for every layer, worked out from the top down (only when used).
    let lightning_used = settings.infill_pattern == InfillPattern::Lightning
        || modifiers
            .iter()
            .any(|m| m.infill_pattern == Some(InfillPattern::Lightning));
    let lightning: Vec<Vec<Vec<Point2>>> = if lightning_used && !vase {
        let overlap = settings.infill_overlap / 100.0;
        let regions: Vec<Vec<Polygon>> = outlines
            .iter()
            .map(|o| infill_region(o, settings.walls, line_width, overlap))
            .collect();
        let exposed: Vec<Vec<bool>> = (0..n).map(|i| exposed_cells(&masks, i, settings)).collect();
        // Cells where a solid skin starts just above sparse infill.
        let needs: Vec<Vec<bool>> = (0..n)
            .map(|i| match exposed.get(i + 1) {
                Some(above) => above
                    .iter()
                    .zip(&exposed[i])
                    .map(|(a, e)| *a && !*e)
                    .collect(),
                None => vec![false; grid.cells()],
            })
            .collect();
        let spacing = line_width * 100.0 / settings.infill_percent.clamp(1.0, 100.0);
        lightning::lightning_lines(&grid, &regions, &needs, &heights, spacing)
    } else {
        Vec::new()
    };

    // 3. Toolpaths per layer.
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
        let variable = settings.variable_walls && !vase;
        if variable {
            // Thin parts get a centre line instead of walls (which also fills what gap
            // fill would).
            let mut walls = arachne::variable_walls(&outlines[i], wall_count, line_width);
            if settings.fuzzy_skin && model_layer > 0 {
                for w in walls
                    .iter_mut()
                    .filter(|w| w.closed && w.kind == PathKind::OuterWall)
                {
                    w.points = fuzz(&w.points, model_layer as u64);
                }
            }
            paths.extend(walls);
        } else {
            paths.extend(wall_paths(
                &outlines[i],
                model_layer,
                wall_count,
                line_width,
                settings,
            ));
        }

        if !vase && !variable {
            let overlap = settings.infill_overlap / 100.0;
            paths.extend(gap_fill_paths(
                &outlines[i],
                wall_count,
                line_width,
                overlap,
            ));
        }
        let overlap = settings.infill_overlap / 100.0;
        let region = infill_region(&outlines[i], wall_count, line_width, overlap);
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
            let area = SolidArea {
                grid: &grid,
                solid: &solid_cells,
                top: &top,
            };
            let patterns = (settings.top_pattern, settings.solid_pattern);
            let from = last_point(&paths);
            paths.extend(solid_infill_paths(
                &region, &area, angle, line_width, patterns, from,
            ));
            let kept_off: Vec<bool> = exposed
                .iter()
                .zip(&not_bridged)
                .map(|(e, n)| *e || !*n)
                .collect();
            // Adaptive cubic is dense where the part is less than ADAPTIVE_CUBIC_NEAR_MM
            // from empty space in any direction (`deep` holds the cells farther in).
            let deep_here: Vec<bool> = if deep.is_empty() {
                Vec::new()
            } else {
                let lo = i.checked_sub(near_layers);
                let hi = i + near_layers;
                match lo.filter(|_| hi < n) {
                    Some(lo) => (0..grid.cells())
                        .map(|c| deep[lo..=hi].iter().all(|d| d[c]))
                        .collect(),
                    None => vec![false; grid.cells()],
                }
            };
            let near_surface =
                |p: Point2| !deep_here.get(grid.index_of(p)).copied().unwrap_or(false);
            let infill_at = InfillAt {
                layer: i,
                z: z - h / 2.0,
                near_surface: &near_surface,
                lightning: lightning.get(i).map_or(&[], Vec::as_slice),
            };
            // Which modifier, if any, owns a point of this layer (a later one wins).
            let owner = |p: Point2| {
                modifier_outlines
                    .iter()
                    .rposition(|m| m[i].iter().filter(|poly| contains(poly, p)).count() % 2 == 1)
            };
            if settings.infill_percent > 0.5 {
                let skip = |p: Point2| kept_off[grid.index_of(p)] || owner(p).is_some();
                let from = last_point(&paths);
                paths.extend(sparse_infill_paths(
                    &region,
                    &skip,
                    &infill_at,
                    line_width,
                    settings.infill_percent,
                    settings.infill_pattern,
                    from,
                ));
            }
            for (k, m) in modifiers.iter().enumerate() {
                let percent = m.infill_percent.unwrap_or(settings.infill_percent);
                let pattern = m.infill_pattern.unwrap_or(settings.infill_pattern);
                let skip = |p: Point2| kept_off[grid.index_of(p)] || owner(p) != Some(k);
                let from = last_point(&paths);
                if percent >= 99.5 {
                    // Full infill is laid as solid infill, line against line.
                    let lines: Vec<ToolPath> = scan_fill(&region, line_width, angle)
                        .iter()
                        .flat_map(|seg| split_by(seg, skip))
                        .filter(|piece| !piece.1)
                        .map(|piece| open_path(PathKind::SolidInfill, piece.0))
                        .collect();
                    paths.extend(order_open_paths(lines, from));
                } else if percent > 0.5 {
                    paths.extend(sparse_infill_paths(
                        &region, &skip, &infill_at, line_width, percent, pattern, from,
                    ));
                }
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

/// The settings a set of parts prints with: `base`, and between two heights (mm above
/// the model's bottom, from inclusive, to exclusive) the settings of a height range.
/// Where ranges overlap, the later one wins.
#[derive(Clone, Debug, PartialEq)]
pub struct PartSettings {
    pub base: PrintSettings,
    pub ranges: Vec<(f32, f32, PrintSettings)>,
    /// Volumes inside which the infill differs, at every height.
    pub modifiers: Vec<InfillModifier>,
}

impl PartSettings {
    /// Settings with no height ranges.
    #[cfg(test)]
    pub fn plain(base: PrintSettings) -> Self {
        Self {
            base,
            ranges: Vec::new(),
            modifiers: Vec::new(),
        }
    }

    /// The settings at height `z` above the model's bottom.
    pub fn at(&self, z: f32) -> &PrintSettings {
        self.ranges
            .iter()
            .rev()
            .find(|(from, to, _)| z >= *from && z < *to)
            .map_or(&self.base, |(_, _, s)| s)
    }
}

/// Slices a plate whose parts don't all print with the plate's settings. `groups` are
/// the parts' triangles with the settings each set prints with (it may be the plate's);
/// `all` is every triangle on the plate. With no groups, or one on the plate's
/// settings, this is plain [`slice`].
///
/// Otherwise every group is sliced on its own, once for each set of settings it uses
/// (its own, and each height range's), and the layers are put together: layer heights
/// are the plate's for all of them, so layer `i` is the same height everywhere, and a
/// group's layer `i` comes from the slice with the settings at the middle of that
/// layer. What belongs to the plate as a whole, the skirt, brim, raft and supports,
/// comes from slicing `all` with the plate's settings (supports are a plate setting,
/// and stand on whatever is below them, from any part).
pub fn slice_groups(
    all: &[Triangle],
    groups: &[(Vec<Triangle>, PartSettings)],
    support_volumes: &SupportVolumes,
    plate: &PrintSettings,
    line_width: f32,
    progress: &dyn Fn(f32),
) -> SliceResult {
    let plain = match groups {
        [] => true,
        [(_, only)] => only.ranges.is_empty() && only.modifiers.is_empty() && only.base == *plate,
        _ => false,
    };
    if plain {
        return slice_modified(all, &[], support_volumes, None, plate, line_width, progress);
    }
    // Each group's distinct settings, and which of them each of its layers uses.
    let variants: Vec<Vec<&PrintSettings>> = groups
        .iter()
        .map(|(_, ps)| {
            let mut v = vec![&ps.base];
            for (_, _, s) in &ps.ranges {
                if !v.contains(&s) {
                    v.push(s);
                }
            }
            v
        })
        .collect();
    let n = variants.iter().map(Vec::len).sum::<usize>() as f32 + 1.0;
    // Variable layer heights come from the whole plate, so every slice's layers line up.
    let layers = plate
        .adaptive_layers
        .then(|| plate_layers(all, plate, line_width));
    let layers = layers.as_deref();
    let base = slice_modified(all, &[], support_volumes, layers, plate, line_width, &|v| {
        progress(v / n)
    });
    let mut done = 1.0;
    let slices: Vec<Vec<SliceResult>> = groups
        .iter()
        .zip(&variants)
        .map(|((tris, ps), vs)| {
            vs.iter()
                .map(|s| {
                    // Supports come from the plate's slice; leaving them out saves time.
                    let own = PrintSettings {
                        supports: false,
                        ..(*s).clone()
                    };
                    let at = done;
                    done += 1.0;
                    let none = SupportVolumes::default();
                    slice_modified(tris, &ps.modifiers, &none, layers, &own, line_width, &|v| {
                        progress((at + v) / n)
                    })
                })
                .collect()
        })
        .collect();
    // Heights count from the model's bottom, above a full raft and its air gap.
    let lift = if plate.adhesion == Adhesion::Raft {
        raft_height(plate) + RAFT_AIR_GAP_MM
    } else {
        0.0
    };
    let whole_plate = |k: PathKind| {
        matches!(
            k,
            PathKind::Skirt | PathKind::Brim | PathKind::Raft | PathKind::Support
        )
    };
    let layers = base
        .layers
        .into_iter()
        .enumerate()
        .map(|(i, layer)| {
            let middle = layer.z - layer.height / 2.0 - lift;
            let (support, mut paths): (Vec<ToolPath>, Vec<ToolPath>) = layer
                .paths
                .into_iter()
                .filter(|p| whole_plate(p.kind))
                .partition(|p| p.kind == PathKind::Support);
            for (((_, ps), vs), slices) in groups.iter().zip(&variants).zip(&slices) {
                let want = ps.at(middle);
                let k = vs.iter().position(|s| *s == want).unwrap_or(0);
                if let Some(l) = slices.get(k).and_then(|r| r.layers.get(i)) {
                    paths.extend(l.paths.iter().filter(|p| !whole_plate(p.kind)).cloned());
                }
            }
            paths.extend(support);
            Layer { paths, ..layer }
        })
        .collect();
    progress(1.0);
    SliceResult { layers, line_width }
}

/// Size compensation on the layer outlines: every hole grows by `hole_compensation`
/// all round, and, when the model sits on the plate (no raft), the first layer's
/// outlines are drawn in by `elephant_foot` (outer loops shrink, holes grow). A loop
/// that can't be offset (too small) is left as it is.
fn compensate(outlines: &mut [Vec<Polygon>], settings: &PrintSettings, on_plate: bool) {
    let shift = |poly: &mut Polygon, d: f32| {
        if let Some(p) = offset_left(poly, d) {
            *poly = p;
        }
    };
    if settings.hole_compensation > 0.0 {
        for poly in outlines.iter_mut().flatten() {
            if area(poly) < 0.0 {
                // A hole's loop runs the other way round: its left is the material.
                shift(poly, settings.hole_compensation);
            }
        }
    }
    if on_plate && settings.elephant_foot > 0.0 {
        if let Some(first) = outlines.first_mut() {
            for poly in first.iter_mut() {
                shift(poly, settings.elephant_foot);
            }
        }
    }
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
            width: None,
        })
        .collect()
}

/// Gap fill for shapes (outer loops, not holes) too narrow for all their walls and
/// infill. Inside the innermost wall that fits (or inside the whole shape when not even
/// the outer wall fits) is a space narrower than one more wall. Its width is found by
/// searching for the deepest offset of the shape that still exists; that almost
/// collapsed loop runs along the middle of the space, and one side of it, between its
/// two farthest-apart points, is the gap's centre line. The line is as wide as the
/// space, so it lays only the plastic the space holds. Spaces under `MIN_GAP_FILL` of a
/// line width stay empty.
///
/// Limits: a gap is filled with one line, so only strip-like spaces are filled well, and
/// shapes are offset one loop at a time (a hole's walls are not checked against the
/// outline's).
fn gap_fill_paths(
    outline: &[Polygon],
    wall_count: u32,
    line_width: f32,
    overlap: f32,
) -> Vec<ToolPath> {
    let mut paths = Vec::new();
    for poly in outline.iter().filter(|p| area(p) > 0.0) {
        let walls = (0..wall_count)
            .take_while(|k| offset_left(poly, line_width * (0.5 + *k as f32)).is_some())
            .count();
        let infill_fits =
            !infill_region(std::slice::from_ref(poly), wall_count, line_width, overlap).is_empty();
        if walls == wall_count as usize && infill_fits {
            continue;
        }
        // The inner edge of the innermost wall (the shape's own edge with no wall).
        let start = line_width * walls as f32;
        let first = if start > 0.0 {
            offset_left(poly, start)
        } else {
            Some(poly.clone())
        };
        let Some(mut core) = first else {
            continue;
        };
        let (mut lo, mut hi) = (start, start + line_width);
        if offset_left(poly, hi).is_some() {
            continue;
        }
        for _ in 0..12 {
            let mid = 0.5 * (lo + hi);
            match offset_left(poly, mid) {
                Some(c) => {
                    lo = mid;
                    core = c;
                }
                None => hi = mid,
            }
        }
        // `offset_left` gives up while a thin loop still has some thickness left; that
        // thickness (area over half the perimeter, for a thin loop) adds to the width.
        let perimeter: f32 = core.windows(2).map(|w| w[0].distance(w[1])).sum::<f32>()
            + core[core.len() - 1].distance(core[0]);
        let left = 2.0 * area(&core).abs() / perimeter.max(1e-6);
        let width = 2.0 * (lo - start) + left;
        if width < MIN_GAP_FILL * line_width {
            continue;
        }
        let mut path = open_path(PathKind::GapFill, centre_line(&core));
        path.width = Some(width.min(line_width * 1.5));
        paths.push(path);
    }
    paths
}

/// The middle line of a very thin loop: the loop's points from one of its two
/// farthest-apart points to the other, along the shorter way round.
fn centre_line(thin: &[Point2]) -> Vec<Point2> {
    let n = thin.len();
    let (mut a, mut b, mut far) = (0, 0, -1.0f32);
    for i in 0..n {
        for j in i + 1..n {
            let d = thin[i].distance(thin[j]);
            if d > far {
                (a, b, far) = (i, j, d);
            }
        }
    }
    let forward: Vec<Point2> = (a..=b).map(|i| thin[i]).collect();
    let backward: Vec<Point2> = (b..=a + n).map(|i| thin[i % n]).collect();
    let length = |p: &[Point2]| p.windows(2).map(|w| w[0].distance(w[1])).sum::<f32>();
    if length(&forward) <= length(&backward) {
        forward
    } else {
        backward
    }
}

/// The area infill covers: inside the innermost wall, overlapping it slightly.
fn infill_region(
    outline: &[Polygon],
    wall_count: u32,
    line_width: f32,
    overlap: f32,
) -> Vec<Polygon> {
    let inset = line_width * (wall_count as f32 - overlap).max(0.0);
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
/// The cells of a layer that are solid, and of those, the visible top surface.
struct SolidArea<'a> {
    grid: &'a Grid,
    solid: &'a [bool],
    top: &'a [bool],
}

/// The solid infill and top surface lines of a layer, in printing order from `from`:
/// each filled with its pattern (`patterns`: the top's, then the other solid layers').
fn solid_infill_paths(
    region: &[Polygon],
    area: &SolidArea,
    angle: f32,
    line_width: f32,
    patterns: (SolidPattern, SolidPattern),
    from: Option<Point2>,
) -> Vec<ToolPath> {
    let (grid, solid, top) = (area.grid, area.solid, area.top);
    if patterns == (SolidPattern::Lines, SolidPattern::Lines) {
        let mut paths = Vec::new();
        for seg in scan_fill(region, line_width, angle) {
            for piece in split_by(&seg, |p| solid[grid.index_of(p)]) {
                if !piece.1 {
                    continue;
                }
                let line = [piece.0[0], piece.0[1]];
                for part in split_by(&line, |p| top[grid.index_of(p)]) {
                    let kind = if part.1 {
                        PathKind::TopSurface
                    } else {
                        PathKind::SolidInfill
                    };
                    paths.push(open_path(kind, part.0));
                }
            }
        }
        return order_open_paths(paths, from);
    }
    let mut out: Vec<ToolPath> = Vec::new();
    let kinds = [
        (false, patterns.1, PathKind::SolidInfill),
        (true, patterns.0, PathKind::TopSurface),
    ];
    for (is_top, pattern, kind) in kinds {
        let keep = |p: Point2| {
            let c = grid.index_of(p);
            solid[c] && top[c] == is_top
        };
        let lines = || -> Vec<ToolPath> {
            scan_fill(region, line_width, angle)
                .iter()
                .flat_map(|seg| split_by(seg, keep))
                .filter(|piece| piece.1)
                .map(|piece| open_path(kind, piece.0))
                .collect()
        };
        let from = last_point(&out).or(from);
        let set = match pattern {
            SolidPattern::Lines => order_open_paths(lines(), from),
            // `scan_fill` gives the rows in order across, each the other way from the
            // last: kept as they come, every line is laid after its neighbour.
            SolidPattern::Monotonic => lines(),
            SolidPattern::Concentric => concentric_paths(region, line_width, &keep, kind),
        };
        out.extend(set);
    }
    out
}

/// Loops one line width apart working inwards from every loop of `region` (holes'
/// loops grow outwards into the material), each cut where it comes closer to another
/// loop of the region than to its own (so loops from two edges never overlap) and
/// where `keep` is false or it leaves the region. A loop kept whole is a closed path.
/// Each ring is made from the one before, a line width in; when an outline's rings
/// end whole, the little left in the middle is filled with straight lines.
fn concentric_paths(
    region: &[Polygon],
    line_width: f32,
    keep: &dyn Fn(Point2) -> bool,
    kind: PathKind,
) -> Vec<ToolPath> {
    let mut out: Vec<ToolPath> = Vec::new();
    // A hole's rings grow outwards; past the outline they must stop, not carry on.
    let inside = crate::gyroid::AreaTest::new(region);
    // Each loop's box, to skip loops too far away to matter.
    let boxes: Vec<(Point2, Point2)> = region
        .iter()
        .map(|p| loops_box(std::slice::from_ref(p)).unwrap_or_default())
        .collect();
    // Each loop's latest ring, while it still lays something.
    let mut rings: Vec<Option<Polygon>> = region
        .iter()
        .map(|p| offset_left(&merge_short_edges(p, line_width * 0.5), line_width * 0.5))
        .collect();
    for k in 0..10_000 {
        let d = line_width * (0.5 + k as f32);
        if rings.iter().all(Option::is_none) {
            break;
        }
        for (j, slot) in rings.iter_mut().enumerate() {
            let Some(ring) = slot.take() else {
                continue;
            };
            let own = |p: Point2| {
                keep(p)
                    && inside.contains(p)
                    && region.iter().enumerate().all(|(q, other)| {
                        let need = d - line_width * 0.25;
                        q == j
                            || distance_to_box(p, boxes[q]) >= need
                            || distance_to_loop(p, other) >= need
                    })
            };
            let mut closed = ring.clone();
            closed.push(ring[0]);
            let whole = closed.iter().all(|p| own(*p));
            if whole {
                out.push(ToolPath {
                    kind,
                    points: ring.clone(),
                    closed: true,
                    width: None,
                });
            } else {
                let pieces: Vec<ToolPath> = crate::gyroid::clip(&closed, &own)
                    .into_iter()
                    .filter(|piece| crate::gyroid::length(piece) >= line_width)
                    .map(|piece| open_path(kind, piece))
                    .collect();
                if pieces.is_empty() {
                    // Nothing of this loop's rings is left to lay.
                    continue;
                }
                out.extend(pieces);
            }
            // Short edges (a chamfered corner) would shrink past nothing and fold the
            // next ring over: they are merged away first.
            *slot = offset_left(&merge_short_edges(&ring, line_width), line_width);
            if slot.is_none() && whole && area(&ring) > 0.0 {
                out.extend(centre_lines(&ring, line_width, &own, kind));
            }
        }
    }
    out
}

/// Straight lines filling what is left inside the last whole ring `ring` of a
/// concentric fill, half a line width clear of it.
fn centre_lines(
    ring: &[Point2],
    line_width: f32,
    keep: &dyn Fn(Point2) -> bool,
    kind: PathKind,
) -> Vec<ToolPath> {
    let half = line_width * 0.5;
    scan_fill(&[ring.to_vec()], line_width, std::f32::consts::FRAC_PI_4)
        .into_iter()
        .filter_map(|[a, b]| {
            let len = a.distance(b);
            if len < line_width * 2.0 {
                return None;
            }
            let t = half / len;
            let lerp = |u: f32| Point2::new(a.x + (b.x - a.x) * u, a.y + (b.y - a.y) * u);
            let (p, q) = (lerp(t), lerp(1.0 - t));
            (keep(p) && keep(q)).then(|| open_path(kind, vec![p, q]))
        })
        .collect()
}

/// `poly` with every edge shorter than `min_len` merged into a point at its middle
/// (while at least three points are left).
fn merge_short_edges(poly: &[Point2], min_len: f32) -> Polygon {
    let mut out = poly.to_vec();
    while out.len() > 3 {
        let n = out.len();
        let Some((i, len)) = (0..n)
            .map(|i| (i, out[i].distance(out[(i + 1) % n])))
            .min_by(|a, b| a.1.total_cmp(&b.1))
        else {
            break;
        };
        if len >= min_len {
            break;
        }
        let j = (i + 1) % n;
        let mid = Point2::new((out[i].x + out[j].x) / 2.0, (out[i].y + out[j].y) / 2.0);
        out[i] = mid;
        out.remove(j);
    }
    out
}

/// The distance from `p` to the box `lo`..`hi` (0 inside it).
fn distance_to_box(p: Point2, (lo, hi): (Point2, Point2)) -> f32 {
    let dx = (lo.x - p.x).max(p.x - hi.x).max(0.0);
    let dy = (lo.y - p.y).max(p.y - hi.y).max(0.0);
    dx.hypot(dy)
}

/// The distance from `p` to the nearest edge of the loop `poly`.
fn distance_to_loop(p: Point2, poly: &[Point2]) -> f32 {
    let n = poly.len();
    (0..n)
        .map(|i| {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let len2 = dx * dx + dy * dy;
            let t = if len2 > 0.0 {
                (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            p.distance(Point2::new(a.x + dx * t, a.y + dy * t))
        })
        .fold(f32::MAX, f32::min)
}

/// Where sparse infill is laid: the layer, its middle height (mm), and whether a point
/// of it is near the part's surface (walls, top or bottom; used by adaptive cubic).
struct InfillAt<'a> {
    layer: usize,
    z: f32,
    near_surface: &'a dyn Fn(Point2) -> bool,
    /// This layer's lightning lines (empty when lightning isn't used, or nothing
    /// above needs holding up here).
    lightning: &'a [Vec<Point2>],
}

/// How far (mm) adaptive cubic keeps its full density from the part's surface.
const ADAPTIVE_CUBIC_NEAR_MM: f32 = 4.0;
/// How far cubic infill's lines move across per mm of height: the lattice is cubes
/// standing on a corner, whose faces lean 54.7 degrees from upright.
const CUBIC_SHIFT: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// The box round a set of loops, or `None` when there are none.
fn loops_box(region: &[Polygon]) -> Option<(Point2, Point2)> {
    let (mut lo, mut hi) = (
        Point2::new(f32::MAX, f32::MAX),
        Point2::new(f32::MIN, f32::MIN),
    );
    for p in region.iter().flatten() {
        (lo.x, lo.y) = (lo.x.min(p.x), lo.y.min(p.y));
        (hi.x, hi.y) = (hi.x.max(p.x), hi.y.max(p.y));
    }
    (lo.x <= hi.x).then_some((lo, hi))
}

/// Sparse infill lines in the chosen pattern, kept off points `skip` is true for.
fn sparse_infill_paths(
    region: &[Polygon],
    skip: &dyn Fn(Point2) -> bool,
    at: &InfillAt,
    line_width: f32,
    percent: f32,
    pattern: InfillPattern,
    from: Option<Point2>,
) -> Vec<ToolPath> {
    let mut paths: Vec<ToolPath> = Vec::new();
    let base = line_width * 100.0 / percent.min(100.0);
    if pattern == InfillPattern::Lightning {
        let keep = |p: Point2| !skip(p);
        let set: Vec<ToolPath> = at
            .lightning
            .iter()
            .flat_map(|l| crate::gyroid::clip(l, &keep))
            .filter(|l| crate::gyroid::length(l) >= line_width)
            .map(|l| open_path(PathKind::SparseInfill, l))
            .collect();
        return order_open_paths(set, from);
    }
    if matches!(pattern, InfillPattern::Gyroid | InfillPattern::Honeycomb) {
        let Some((lo, hi)) = loops_box(region) else {
            return paths;
        };
        let area = crate::gyroid::AreaTest::new(region);
        let inside = |p: Point2| area.contains(p) && !skip(p);
        let lines = if pattern == InfillPattern::Gyroid {
            crate::gyroid::gyroid_lines(lo, hi, base, at.z, line_width, &inside)
        } else {
            crate::honeycomb::honeycomb_lines(lo, hi, base, line_width, &inside)
        };
        let set: Vec<ToolPath> = lines
            .into_iter()
            .map(|l| open_path(PathKind::SparseInfill, l))
            .collect();
        return order_open_paths(set, from);
    }
    let (angles, spacing) = infill_angles(pattern, at.layer, base);
    let cubic = matches!(pattern, InfillPattern::Cubic | InfillPattern::AdaptiveCubic);
    let adaptive = pattern == InfillPattern::AdaptiveCubic;
    // Each direction is ordered on its own, so one set of lines is laid before the next
    // crosses it, starting where the previous set ended.
    for angle in angles {
        let offset = if cubic { at.z * CUBIC_SHIFT } else { 0.0 };
        let mut set = Vec::new();
        for (row, seg) in scan_fill_rows(region, spacing, angle.to_radians(), offset) {
            // Adaptive cubic: every other line only near the surface.
            let thin = adaptive && row.rem_euclid(2) == 1;
            let off = |p: Point2| skip(p) || (thin && !(at.near_surface)(p));
            for piece in split_by(&seg, off) {
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
        width: None,
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
        // Not straight lines: traced by `gyroid` and `honeycomb`, at the base spacing.
        InfillPattern::Gyroid | InfillPattern::Honeycomb => (Vec::new(), base),
        // Triangles' lines, moved with height (`sparse_infill_paths`).
        InfillPattern::Cubic | InfillPattern::AdaptiveCubic => (vec![0.0, 60.0, 120.0], base * 3.0),
        // Worked out for the whole part (`lightning`); not straight lines.
        InfillPattern::Lightning => (Vec::new(), base),
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
/// Steps (mm) in which variable layer height measures the model.
const ADAPTIVE_BIN_MM: f32 = 0.05;

/// The layers (top and height) for `triangles` on the plate (already lifted onto a
/// full raft): fixed heights, or variable ones with `adaptive_layers`.
pub fn plan_layers(
    triangles: &[Triangle],
    settings: &PrintSettings,
    line_width: f32,
) -> Vec<(f32, f32)> {
    let max_z = Bounds::of(triangles.iter().flatten().copied()).max.z;
    if settings.adaptive_layers {
        adaptive_layer_heights(triangles, max_z, settings, line_width)
    } else {
        layer_heights(max_z, settings)
    }
}

/// The layers of a plate holding `triangles` (on the plate; lifted here onto a full
/// raft, as `slice_modified` does).
pub fn plate_layers(
    triangles: &[Triangle],
    settings: &PrintSettings,
    line_width: f32,
) -> Vec<(f32, f32)> {
    if settings.adhesion == Adhesion::Raft {
        let up = Vec3::new(0.0, 0.0, raft_height(settings));
        let lifted: Vec<Triangle> = triangles.iter().map(|t| t.map(|v| v + up)).collect();
        plan_layers(&lifted, settings, line_width)
    } else {
        plan_layers(triangles, settings, line_width)
    }
}

/// Variable layer heights. Each layer is as thick as the most gently sloping surface
/// it crosses allows: a layer `h` thick leaves steps about `h * |n.z|` deep on a
/// surface whose normal is `n`, so it may be `tolerance / |n.z|` thick, between half
/// and one and a half times the layer height (and at most three quarters of a line
/// width). Flat tops and bottoms (|n.z| near 1) don't count: a layer edge lands on
/// them exactly. The quality setting moves the tolerance from the thickest layer
/// (0 %) to a quarter of the thinnest (100 %). Smoothing spreads each thin stretch
/// up and down by that many millimetres. Below the model's bottom (a raft) layers are
/// the normal height. Last, the layers above the bottom are stretched or squeezed a
/// little so the top layer ends exactly at `max_z`.
pub fn adaptive_layer_heights(
    triangles: &[Triangle],
    max_z: f32,
    settings: &PrintSettings,
    line_width: f32,
) -> Vec<(f32, f32)> {
    let first = settings.first_layer_height.max(0.05);
    let lh = settings.layer_height.max(0.04);
    let min_h = (lh * 0.5).max(0.04);
    let max_h = (lh * 1.5).min(line_width * 0.75).max(min_h);
    let q = (settings.adaptive_quality / 100.0).clamp(0.0, 1.0);
    let tolerance = max_h * (1.0 - q) + min_h * 0.25 * q;
    let bins = ((max_z / ADAPTIVE_BIN_MM).ceil() as usize).max(1);
    let bin = |z: f32| ((z / ADAPTIVE_BIN_MM).floor().max(0.0) as usize).min(bins - 1);
    let mut need = vec![max_h; bins];
    let mut bottom = f32::MAX;
    for t in triangles {
        let n = (t[1] - t[0]).cross(t[2] - t[0]);
        let len = n.length();
        let lo = t.iter().map(|v| v.z).fold(f32::MAX, f32::min);
        let hi = t.iter().map(|v| v.z).fold(f32::MIN, f32::max);
        bottom = bottom.min(lo);
        if len < 1e-9 {
            continue;
        }
        let nz = (n.z / len).abs();
        if nz > 0.999 {
            continue;
        }
        let h = (tolerance / nz.max(1e-6)).clamp(min_h, max_h);
        for b in &mut need[bin(lo)..=bin(hi)] {
            *b = b.min(h);
        }
    }
    // Smoothing: each step takes the thinnest need within the smoothing distance.
    let reach = (settings.adaptive_smoothing.max(0.0) / ADAPTIVE_BIN_MM).round() as usize;
    let need: Vec<f32> = (0..bins)
        .map(|b| {
            need[b.saturating_sub(reach)..=(b + reach).min(bins - 1)]
                .iter()
                .copied()
                .fold(max_h, f32::min)
        })
        .collect();
    let thinnest = |from: f32, to: f32| {
        need[bin(from)..=bin(to.max(from))]
            .iter()
            .copied()
            .fold(max_h, f32::min)
    };
    let mut out = vec![(first, first)];
    let mut z = first;
    while z + min_h * 0.5 < max_z {
        let h = if z < bottom - 1e-4 {
            lh
        } else {
            // Thinned until the layer's own span allows its thickness.
            let mut h = max_h;
            loop {
                let fits = thinnest(z, z + h);
                if fits >= h - 1e-6 {
                    break h;
                }
                h = fits;
            }
        };
        z += h;
        out.push((z, h));
    }
    // End exactly at the top: scale the layers above the bottom (not the first layer,
    // nor a raft's).
    let fixed: f32 = out
        .iter()
        .enumerate()
        .filter(|(i, (top, h))| *i == 0 || top - h < bottom - 1e-4)
        .map(|(_, (_, h))| h)
        .sum();
    let free = z - fixed;
    if free > 0.0 && max_z > fixed {
        let k = (max_z - fixed) / free;
        let mut top = 0.0;
        for (i, (t, h)) in out.iter_mut().enumerate() {
            if i > 0 && top + 1e-4 >= bottom {
                *h *= k;
            }
            top += *h;
            *t = top;
        }
    }
    out
}

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
    // A point in the middle of a straight edge doesn't move along the edge when the loop
    // shrinks, so near a corner it would end up beyond the shortened edge as a spike.
    let corners = corners_only(poly);
    let poly = &corners[..];
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

/// `poly` without repeated points and without points in the middle of a straight edge.
fn corners_only(poly: &[Point2]) -> Polygon {
    let mut out: Polygon = Vec::with_capacity(poly.len());
    for &p in poly {
        if out.last().is_none_or(|q| q.distance(p) > 1e-6) {
            out.push(p);
        }
    }
    while out.len() > 1 && out[0].distance(out[out.len() - 1]) <= 1e-6 {
        out.pop();
    }
    loop {
        let n = out.len();
        if n < 4 {
            return out;
        }
        let straight = (0..n).find(|&i| {
            let (a, b, c) = (out[(i + n - 1) % n], out[i], out[(i + 1) % n]);
            let (ux, uy) = (b.x - a.x, b.y - a.y);
            let (vx, vy) = (c.x - b.x, c.y - b.y);
            let cross = ux * vy - uy * vx;
            let lens = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
            ux * vx + uy * vy > 0.0 && cross.abs() <= 1e-5 * lens
        });
        match straight {
            Some(i) => {
                out.remove(i);
            }
            None => return out,
        }
    }
}

/// Parallel lines `spacing` apart at `angle`, clipped to the region (even-odd rule).
/// Lines sit on a fixed grid so sparse infill lines up from layer to layer.
pub fn scan_fill(region: &[Polygon], spacing: f32, angle: f32) -> Vec<[Point2; 2]> {
    scan_fill_rows(region, spacing, angle, 0.0)
        .into_iter()
        .map(|(_, seg)| seg)
        .collect()
}

/// Like [`scan_fill`], with the lines moved `offset` mm across their direction, each
/// with the number of its row (rows are `spacing` apart; row 0 passes through the
/// origin, moved by `offset`).
fn scan_fill_rows(
    region: &[Polygon],
    spacing: f32,
    angle: f32,
    offset: f32,
) -> Vec<(i64, [Point2; 2])> {
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
    let mut row = ((ymin - offset) / spacing).ceil() as i64;
    let mut flip = false;
    loop {
        let y = row as f32 * spacing + offset;
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
        out.extend(pairs.into_iter().map(|p| (row, p)));
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
                        width: None,
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
                            width: None,
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
        overhangs: &[(usize, f32)],
        masks: &[Vec<bool>],
        heights: &[(f32, f32)],
        settings: &PrintSettings,
        lw: f32,
    ) -> (Vec<Vec<ToolPath>>, Vec<bool>) {
        let plate_only = settings.supports_on_plate_only;
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
        for &(cell, h) in overhangs {
            // Highest layer that stays the top Z gap (one layer on Auto) below the
            // overhang.
            let k = heights.partition_point(|(z, lh)| {
                z + settings.support_top_gap.unwrap_or(lh.max(0.15)) < h
            });
            if k > 0 && (!plate_only || first_model[cell] >= k) {
                starts[k - 1].push(cell);
            }
        }
        let mut active = vec![false; self.cells()];
        // Layers since each column's top, for the interface layers.
        let mut age = vec![0u32; self.cells()];
        let look = SupportLook {
            near_cells: ((settings.support_xy_gap / CELL_MM).round() as usize).max(1),
            row_step: ((settings.support_spacing / CELL_MM).round() as usize).max(1),
        };
        let mut out = vec![Vec::new(); n];
        for i in (0..n).rev() {
            for &c in &starts[i] {
                active[c] = true;
                age[c] = 0;
            }
            for (a, m) in active.iter_mut().zip(&masks[i]) {
                if *m {
                    *a = false;
                }
            }
            // Bottom Z gap: no support just above the model it stands on.
            let (top, height) = heights[i];
            let resting = |c: usize| {
                settings.support_bottom_gap > 0.0
                    && (0..i)
                        .rev()
                        .take_while(|&j| heights[j].0 > top - height - settings.support_bottom_gap)
                        .any(|j| masks[j][c])
            };
            let here: Vec<bool> = (0..self.cells())
                .map(|c| active[c] && !resting(c))
                .collect();
            let interface: Vec<bool> = (0..self.cells())
                .map(|c| here[c] && age[c] < settings.support_interface_layers)
                .collect();
            out[i] = self.support_lines(&masks[i], &here, &interface, lw, &look);
            for (g, a) in age.iter_mut().zip(&active) {
                if *a {
                    *g += 1;
                }
            }
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
    fn support_lines(
        &self,
        mask: &[bool],
        active: &[bool],
        interface: &[bool],
        lw: f32,
        look: &SupportLook,
    ) -> Vec<ToolPath> {
        let mut out = Vec::new();
        let r = look.near_cells;
        let near_model = |cx: usize, cy: usize| {
            let x_lo = cx.saturating_sub(r);
            let y_lo = cy.saturating_sub(r);
            (y_lo..=(cy + r).min(self.h - 1))
                .any(|y| (x_lo..=(cx + r).min(self.w - 1)).any(|x| mask[y * self.w + x]))
        };
        for (row_no, cy) in (0..self.h).step_by(look.row_step).enumerate() {
            let mut runs: Vec<(usize, usize)> = Vec::new();
            let mut start = None;
            for cx in 0..=self.w {
                let c = cy * self.w + cx;
                let on = cx < self.w && active[c] && !interface[c] && !near_model(cx, cy);
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
                    width: None,
                });
            }
        }
        out.extend(self.interface_lines(interface, lw, &near_model));
        out
    }

    /// Dense support lines one line width apart, running across the normal support
    /// lines (along Y), over the `interface` cells clear of the model.
    fn interface_lines(
        &self,
        interface: &[bool],
        lw: f32,
        near_model: &dyn Fn(usize, usize) -> bool,
    ) -> Vec<ToolPath> {
        let mut out = Vec::new();
        if !interface.iter().any(|c| *c) {
            return out;
        }
        let count = (self.w as f32 * CELL_MM / lw).floor() as usize;
        for k in 0..count {
            let x = self.x0 + lw * (0.5 + k as f32);
            let cx = (((x - self.x0) / CELL_MM) as usize).min(self.w - 1);
            let mut runs: Vec<(usize, usize)> = Vec::new();
            let mut start = None;
            for cy in 0..=self.h {
                let on = cy < self.h && interface[cy * self.w + cx] && !near_model(cx, cy);
                match (on, start) {
                    (true, None) => start = Some(cy),
                    (false, Some(s)) => {
                        runs.push((s, cy - 1));
                        start = None;
                    }
                    _ => {}
                }
            }
            for (a, b) in runs {
                let ya = self.y0 + a as f32 * CELL_MM + lw / 2.0;
                let yb = self.y0 + (b + 1) as f32 * CELL_MM - lw / 2.0;
                if yb - ya < 0.5 {
                    continue;
                }
                let mut pts = vec![Point2::new(x, ya), Point2::new(x, yb)];
                if k % 2 == 1 {
                    pts.reverse();
                }
                out.push(ToolPath {
                    kind: PathKind::Support,
                    points: pts,
                    closed: false,
                    width: None,
                });
            }
        }
        out
    }
}

/// How support lines are laid out: how many cells they keep from the model, and every
/// how many rows a line runs.
struct SupportLook {
    near_cells: usize,
    row_step: usize,
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

    fn gap_fills(layer: &Layer) -> Vec<&ToolPath> {
        layer
            .paths
            .iter()
            .filter(|p| p.kind == PathKind::GapFill)
            .collect()
    }

    #[test]
    fn test_gap_fill_one_line_down_a_strip_too_narrow_for_two_walls() {
        let lw = 0.42;
        // Room for the outer wall all round, then 0.6 of a line width left in the middle.
        let strip = box_at(Vec3::new(20.0, 2.6 * lw, 2.0), Vec3::new(0.0, 0.0, 0.0));
        let r = slice(&strip, &PrintSettings::default(), lw, &|_| {});
        for layer in &r.layers {
            let gaps = gap_fills(layer);
            assert_eq!(gaps.len(), 1, "z {}", layer.z);
            let g = gaps[0];
            let width = g.width.expect("gap fill has its own width");
            assert!((width - 0.6 * lw).abs() < 0.1 * lw, "width {width}");
            let (a, b) = (g.points[0], *g.points.last().unwrap());
            assert!(a.distance(b) > 15.0, "too short: {a:?} -> {b:?}");
            assert!(
                g.points.iter().all(|p| p.y.abs() < 0.1),
                "off centre {:?}",
                g.points
            );
            assert!(!layer.paths.iter().any(|p| p.kind == PathKind::InnerWall));
        }
    }

    #[test]
    fn test_gap_fill_a_wall_too_thin_for_any_loop() {
        let lw = 0.42;
        let thin = box_at(Vec3::new(20.0, 0.8 * lw, 2.0), Vec3::new(0.0, 0.0, 0.0));
        let r = slice(&thin, &PrintSettings::default(), lw, &|_| {});
        let layer = &r.layers[3];
        let gaps = gap_fills(layer);
        assert_eq!(gaps.len(), 1);
        let width = gaps[0].width.unwrap();
        assert!((width - 0.8 * lw).abs() < 0.1 * lw, "width {width}");
        assert!(!layer.paths.iter().any(|p| p.kind == PathKind::OuterWall));
    }

    #[test]
    fn test_gap_fill_none_where_infill_fits_or_too_thin() {
        let cube = box_at(Vec3::new(20.0, 20.0, 2.0), Vec3::new(0.0, 0.0, 0.0));
        let r = slice(&cube, &PrintSettings::default(), 0.42, &|_| {});
        assert!(r.layers.iter().all(|l| gap_fills(l).is_empty()));
        // Two walls that just touch leave nothing worth filling.
        let snug = box_at(
            Vec3::new(20.0, 4.0 * 0.42 + 0.05, 2.0),
            Vec3::new(0.0, 0.0, 0.0),
        );
        let r = slice(&snug, &PrintSettings::default(), 0.42, &|_| {});
        assert!(r.layers.iter().all(|l| gap_fills(l).is_empty()));
    }

    /// The largest |x| of any point of `kind` on layer `li`.
    fn reach(r: &SliceResult, li: usize, kind: PathKind) -> f32 {
        r.layers[li]
            .paths
            .iter()
            .filter(|p| p.kind == kind)
            .flat_map(|p| p.points.iter().map(|q| q.x.abs()))
            .fold(0.0, f32::max)
    }

    #[test]
    fn test_infill_overlap_moves_the_infill_ends() {
        let cube = box_at(Vec3::new(20.0, 20.0, 4.0), Vec3::new(0.0, 0.0, 0.0));
        let at = |pct: f32| {
            let s = PrintSettings {
                infill_overlap: pct,
                ..PrintSettings::default()
            };
            reach(&slice(&cube, &s, 0.42, &|_| {}), 0, PathKind::SolidInfill)
        };
        // 50 % reaches 0.5 x 0.42 = 0.21 mm further out than 0 %.
        let more = at(50.0) - at(0.0);
        assert!((more - 0.21).abs() < 0.03, "{more}");
    }

    #[test]
    fn test_elephant_foot_draws_in_only_the_first_layer_on_the_plate() {
        let cube = box_at(Vec3::new(20.0, 20.0, 4.0), Vec3::new(0.0, 0.0, 0.0));
        let plain = slice(&cube, &PrintSettings::default(), 0.42, &|_| {});
        let s = PrintSettings {
            elephant_foot: 0.3,
            ..PrintSettings::default()
        };
        let foot = slice(&cube, &s, 0.42, &|_| {});
        let drawn_in = reach(&plain, 0, PathKind::OuterWall) - reach(&foot, 0, PathKind::OuterWall);
        assert!((drawn_in - 0.3).abs() < 0.01, "{drawn_in}");
        assert_eq!(
            reach(&plain, 1, PathKind::OuterWall),
            reach(&foot, 1, PathKind::OuterWall)
        );
        // On a raft the first model layer isn't squashed onto the plate.
        let raft = PrintSettings {
            adhesion: Adhesion::Raft,
            ..s.clone()
        };
        let raft_plain = PrintSettings {
            adhesion: Adhesion::Raft,
            ..PrintSettings::default()
        };
        let a = slice(&cube, &raft, 0.42, &|_| {});
        let b = slice(&cube, &raft_plain, 0.42, &|_| {});
        assert_eq!(
            reach(&a, RAFT_LAYERS, PathKind::OuterWall),
            reach(&b, RAFT_LAYERS, PathKind::OuterWall)
        );
    }

    #[test]
    fn test_hole_compensation_grows_holes() {
        use crate::model::SampleShape;
        // The sample tube: 14 mm outside radius, 9 mm hole.
        let tube = PlacedObject::new("tube", SampleShape::Tube.mesh()).world_triangles();
        let hole_wall = |s: &PrintSettings| {
            let r = slice(&tube, s, 0.42, &|_| {});
            r.layers[5]
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::OuterWall)
                .flat_map(|p| p.points.iter().map(|q| (q.x * q.x + q.y * q.y).sqrt()))
                .fold(f32::MAX, f32::min)
        };
        let s = PrintSettings {
            hole_compensation: 0.2,
            ..PrintSettings::default()
        };
        let grown = hole_wall(&s) - hole_wall(&PrintSettings::default());
        assert!((grown - 0.2).abs() < 0.02, "{grown}");
        // The outside is untouched.
        let r0 = slice(&tube, &PrintSettings::default(), 0.42, &|_| {});
        let r1 = slice(&tube, &s, 0.42, &|_| {});
        assert_eq!(
            reach(&r0, 5, PathKind::OuterWall),
            reach(&r1, 5, PathKind::OuterWall)
        );
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

    /// A cube of `size` mm standing with its centre at (x, 0), `height` tall.
    fn block_at(size: f32, height: f32, x: f32) -> Vec<Triangle> {
        let mut o = PlacedObject::new("b", Mesh::cube(Vec3::new(size, size, height)));
        o.x = x;
        o.world_triangles()
    }

    #[test]
    fn test_slice_groups_with_plate_settings_slices_as_before() {
        let mut all = block_at(10.0, 4.0, -15.0);
        all.extend(block_at(10.0, 6.0, 15.0));
        let plate = PrintSettings {
            supports: true,
            ..PrintSettings::default()
        };
        let plain = slice(&all, &plate, 0.42, &|_| {});
        let grouped = slice_groups(
            &all,
            &[(all.clone(), PartSettings::plain(plate.clone()))],
            &SupportVolumes::default(),
            &plate,
            0.42,
            &|_| {},
        );
        assert_eq!(format!("{grouped:?}"), format!("{plain:?}"));
    }

    /// A sphere 20 mm across resting on the plate.
    fn sphere() -> Vec<Triangle> {
        crate::modifier::Modifier {
            kind: crate::modifier::ModifierKind::Infill,
            shape: crate::modifier::ModifierShape::Sphere,
            size: Vec3::new(20.0, 20.0, 20.0),
            at: Vec3::default(),
            overrides: Default::default(),
        }
        .triangles(0.0, 0.0)
    }

    fn adaptive(quality: f32, smoothing: f32) -> PrintSettings {
        PrintSettings {
            adaptive_layers: true,
            adaptive_quality: quality,
            adaptive_smoothing: smoothing,
            ..PrintSettings::default()
        }
    }

    #[test]
    fn test_adaptive_layers_are_thinner_near_the_top_of_a_sphere() {
        let h = plate_layers(&sphere(), &adaptive(50.0, 0.0), 0.42);
        let at = |z: f32| h.iter().find(|(top, _)| *top >= z).unwrap().1;
        // Steep sides round the middle: thick layers; the gentle dome at the top: thin.
        assert!(at(10.0) > 0.28, "middle {}", at(10.0));
        assert!(at(19.8) < at(10.0) * 0.6, "top {}", at(19.8));
        assert!(at(0.6) < at(10.0), "the underside is gentle too");
        // Total height preserved: the last layer ends at the top of the sphere.
        let (top, _) = *h.last().unwrap();
        assert!((top - 20.0).abs() < 1e-3, "top {top}");
        // Heights add up, and stay within half to one and a half layer heights (with a
        // little room for the final stretch).
        let mut z = 0.0;
        for &(t, lh) in &h {
            z += lh;
            assert!((z - t).abs() < 1e-3);
        }
        assert!(
            h[1..].iter().all(|(_, lh)| (0.095..=0.31).contains(lh)),
            "{h:?}"
        );
    }

    #[test]
    fn test_adaptive_layers_quality_smoothing_and_off() {
        let ball = sphere();
        let count = |s: &PrintSettings| plate_layers(&ball, s, 0.42).len();
        assert!(count(&adaptive(100.0, 0.0)) > count(&adaptive(0.0, 0.0)) + 10);
        // Quality 0: about the thickest layers everywhere.
        assert!(count(&adaptive(0.0, 0.0)) <= 70);
        // Smoothing spreads the thin layers: more of them.
        let thin = |s: &PrintSettings| {
            plate_layers(&ball, s, 0.42)
                .iter()
                .filter(|(_, h)| *h < 0.2)
                .count()
        };
        assert!(thin(&adaptive(50.0, 3.0)) > thin(&adaptive(50.0, 0.0)));
        // Off: the fixed heights.
        let fixed = PrintSettings::default();
        assert_eq!(
            plate_layers(&ball, &fixed, 0.42),
            layer_heights(20.0, &fixed)
        );
        // A box has only upright walls and flat faces: the thickest layers.
        let cube = block_at(20.0, 9.0, 0.0);
        let h = plate_layers(&cube, &adaptive(100.0, 1.0), 0.42);
        assert!(
            h[1..].iter().all(|(_, lh)| (lh - 0.3).abs() < 0.02),
            "{h:?}"
        );
        assert!((h.last().unwrap().0 - 9.0).abs() < 1e-3);
    }

    #[test]
    fn test_adaptive_layers_on_a_raft_and_shared_by_grouped_slices() {
        let ball = sphere();
        let raft = PrintSettings {
            adhesion: Adhesion::Raft,
            ..adaptive(50.0, 1.0)
        };
        let h = plate_layers(&ball, &raft, 0.42);
        let bottom = raft_height(&raft);
        // Raft layers keep the normal height; the model's top is where it is lifted to.
        assert!(h
            .iter()
            .skip(1)
            .take_while(|(t, _)| *t <= bottom + 1e-4)
            .all(|(_, lh)| (lh - raft.layer_height).abs() < 1e-4));
        assert!((h.last().unwrap().0 - (20.0 + bottom)).abs() < 1e-3);
        // A part with settings of its own slices on the plate's variable layers.
        let plate = adaptive(50.0, 1.0);
        let own = PrintSettings {
            walls: 4,
            ..plate.clone()
        };
        let none = SupportVolumes::default();
        let r = slice_groups(
            &ball,
            &[(ball.clone(), PartSettings::plain(own))],
            &none,
            &plate,
            0.42,
            &|_| {},
        );
        let zs: Vec<(f32, f32)> = r.layers.iter().map(|l| (l.z, l.height)).collect();
        assert_eq!(zs, plate_layers(&ball, &plate, 0.42));
        assert!(r.layers[r.layers.len() / 2]
            .paths
            .iter()
            .any(|p| p.kind == PathKind::InnerWall));
    }

    /// Length of all sparse infill in `paths`.
    fn infill_length(paths: &[ToolPath]) -> f32 {
        paths
            .iter()
            .filter(|p| p.kind == PathKind::SparseInfill)
            .map(|p| {
                p.points
                    .windows(2)
                    .map(|w| w[0].distance(w[1]))
                    .sum::<f32>()
            })
            .sum()
    }

    #[test]
    fn test_infill_patterns_stay_inside_and_match_their_density() {
        // A 40 mm square at 20 %: line length x line width should be 20 % of 1600 mm2.
        let square = vec![vec![
            Point2::new(-20.0, -20.0),
            Point2::new(20.0, -20.0),
            Point2::new(20.0, 20.0),
            Point2::new(-20.0, 20.0),
        ]];
        let near = |_: Point2| true;
        // Lightning lays only what tops above need, not a density (tested below).
        let patterns = InfillPattern::ALL
            .into_iter()
            .filter(|p| *p != InfillPattern::Lightning);
        for pattern in patterns {
            for (layer, z) in [(10, 2.1), (11, 2.3), (40, 8.1)] {
                let at = InfillAt {
                    layer,
                    z,
                    near_surface: &near,
                    lightning: &[],
                };
                let paths =
                    sparse_infill_paths(&square, &|_| false, &at, 0.42, 20.0, pattern, None);
                assert!(
                    paths
                        .iter()
                        .flat_map(|p| &p.points)
                        .all(|q| q.x.abs() <= 20.01 && q.y.abs() <= 20.01),
                    "{pattern:?} outside the square"
                );
                let density = infill_length(&paths) * 0.42 / 1600.0;
                assert!(
                    (density / 0.2 - 1.0).abs() < 0.1,
                    "{pattern:?} layer {layer}: {:.1} %",
                    density * 100.0
                );
            }
        }
    }

    #[test]
    fn test_cubic_lines_move_with_height_and_honeycomb_stays_put() {
        let square = vec![vec![
            Point2::new(-20.0, -20.0),
            Point2::new(20.0, -20.0),
            Point2::new(20.0, 20.0),
            Point2::new(-20.0, 20.0),
        ]];
        let near = |_: Point2| true;
        let lay = |pattern, z| {
            let at = InfillAt {
                layer: 10,
                z,
                near_surface: &near,
                lightning: &[],
            };
            format!(
                "{:?}",
                sparse_infill_paths(&square, &|_| false, &at, 0.42, 20.0, pattern, None)
            )
        };
        assert_ne!(
            lay(InfillPattern::Cubic, 2.0),
            lay(InfillPattern::Cubic, 2.6)
        );
        assert_eq!(
            lay(InfillPattern::Triangles, 2.0),
            lay(InfillPattern::Triangles, 2.6)
        );
        assert_eq!(
            lay(InfillPattern::Honeycomb, 2.0),
            lay(InfillPattern::Honeycomb, 2.6)
        );
    }

    fn of_kind(l: &Layer, kind: PathKind) -> Vec<&ToolPath> {
        l.paths.iter().filter(|p| p.kind == kind).collect()
    }

    #[test]
    fn test_monotonic_top_lines_are_laid_in_order_across() {
        let tris = block_at(20.0, 5.0, 0.0);
        let s = PrintSettings {
            top_pattern: SolidPattern::Monotonic,
            ..PrintSettings::default()
        };
        let r = slice(&tris, &s, 0.42, &|_| {});
        let i = r.layers.len() - 1;
        let lines = of_kind(&r.layers[i], PathKind::TopSurface);
        assert!(lines.len() > 20);
        // Across the lines: the solid lines run at 45 or 135 degrees by layer.
        let angle = if i % 2 == 0 { 45f32 } else { 135f32 }.to_radians();
        let across = |p: Point2| -p.x * angle.sin() + p.y * angle.cos();
        let rows: Vec<f32> = lines.iter().map(|l| across(l.points[0])).collect();
        assert!(rows.windows(2).all(|w| w[1] >= w[0] - 0.01), "{rows:?}");
        // Each row the other way from the last.
        let dir = |l: &ToolPath| {
            let (a, b) = (l.points[0], l.points[l.points.len() - 1]);
            (b.x - a.x) * angle.cos() + (b.y - a.y) * angle.sin() > 0.0
        };
        assert!(lines.windows(2).all(|w| dir(w[0]) != dir(w[1])));
    }

    #[test]
    fn test_concentric_top_loops_fill_a_ring_without_overlapping() {
        // A tube (outer radius 14, hole radius 9): its top is a ring between the walls.
        let tube =
            PlacedObject::new("tube", crate::model::SampleShape::Tube.mesh()).world_triangles();
        let s = PrintSettings {
            top_pattern: SolidPattern::Concentric,
            ..PrintSettings::default()
        };
        let r = slice(&tube, &s, 0.42, &|_| {});
        let top = of_kind(r.layers.last().unwrap(), PathKind::TopSurface);
        assert!(top.len() >= 4, "{} loops", top.len());
        let points: Vec<(usize, Point2)> = top
            .iter()
            .enumerate()
            .flat_map(|(k, l)| l.points.iter().map(move |p| (k, *p)))
            .collect();
        for (_, p) in &points {
            let rad = p.x.hypot(p.y);
            assert!((9.5..13.6).contains(&rad), "a point at radius {rad}");
        }
        // Loops from the outer edge and from the hole meet without overlapping: points
        // of two different loops are never closer than half a line width (corners of
        // a loop lie a whole width from the next).
        for (a, p) in &points {
            for (b, q) in &points {
                if a != b {
                    assert!(p.distance(*q) > 0.2, "{p:?} and {q:?}");
                }
            }
        }
    }

    #[test]
    fn test_concentric_rings_round_a_hole_stay_in_the_region() {
        // A 20 mm square with a hole reaching to within a hair of one side (as an
        // engraved letter makes): the hole's rings grow outwards and must stop at the
        // square's edge.
        let square = |x0: f32, y0: f32, x1: f32, y1: f32| {
            vec![
                Point2::new(x0, y0),
                Point2::new(x1, y0),
                Point2::new(x1, y1),
                Point2::new(x0, y1),
            ]
        };
        let mut hole = square(4.0, -2.0, 9.95, 2.0);
        hole.reverse();
        let region = vec![square(-10.0, -10.0, 10.0, 10.0), hole];
        let paths = concentric_paths(&region, 0.42, &|_| true, PathKind::SolidInfill);
        for p in paths.iter().flat_map(|p| &p.points) {
            assert!(
                p.x.abs() <= 10.0 && p.y.abs() <= 10.0,
                "{p:?} outside the square"
            );
            assert!(
                !(p.x > 4.05 && p.x < 9.9 && p.y.abs() < 1.95),
                "{p:?} in the hole"
            );
        }
        // About one line width of line per 0.42 mm2 of area (376 mm2), not runaway rings.
        let length: f32 = paths
            .iter()
            .map(|p| {
                let mut pts = p.points.clone();
                if p.closed {
                    pts.push(pts[0]);
                }
                pts.windows(2).map(|w| w[0].distance(w[1])).sum::<f32>()
            })
            .sum();
        let filled = length * 0.42 / 376.2;
        assert!((0.75..1.15).contains(&filled), "{filled}");
    }

    #[test]
    fn test_concentric_rings_keep_going_past_chamfered_corners() {
        // A 20 mm square with 1 mm chamfers at its corners (as the XYZ cube has): the
        // short edges must not stop the rings early.
        let c = 1.0;
        let outline = vec![
            Point2::new(-10.0 + c, -10.0),
            Point2::new(10.0 - c, -10.0),
            Point2::new(10.0, -10.0 + c),
            Point2::new(10.0, 10.0 - c),
            Point2::new(10.0 - c, 10.0),
            Point2::new(-10.0 + c, 10.0),
            Point2::new(-10.0, 10.0 - c),
            Point2::new(-10.0, -10.0 + c),
        ];
        let paths = concentric_paths(&[outline], 0.42, &|_| true, PathKind::SolidInfill);
        let rings = paths.iter().filter(|p| p.closed).count();
        assert!(rings >= 20, "{rings} rings");
    }

    #[test]
    fn test_concentric_bottom_is_nested_loops() {
        let tris = block_at(20.0, 5.0, 0.0);
        let s = PrintSettings {
            solid_pattern: SolidPattern::Concentric,
            ..PrintSettings::default()
        };
        let r = slice(&tris, &s, 0.42, &|_| {});
        let loops = of_kind(&r.layers[1], PathKind::SolidInfill);
        // Nested rings about every line width in, then a few lines in the middle.
        let rings: Vec<&&ToolPath> = loops.iter().filter(|l| l.closed).collect();
        assert!(rings.len() > 18, "{} rings", rings.len());
        let areas: Vec<f32> = rings.iter().map(|l| area(&l.points).abs()).collect();
        assert!(areas.windows(2).all(|w| w[1] < w[0]), "{areas:?}");
    }

    #[test]
    fn test_lightning_infill_is_a_tree_under_the_top_only() {
        // A 20 mm cube: the only thing to hold up is its top.
        let tris = block_at(20.0, 20.0, 0.0);
        let with = |pattern| {
            let s = PrintSettings {
                infill_pattern: pattern,
                ..PrintSettings::default()
            };
            slice(&tris, &s, 0.42, &|_| {})
        };
        let (grid, light) = (with(InfillPattern::Grid), with(InfillPattern::Lightning));
        let n = light.layers.len();
        let sparse = |l: &Layer| -> Vec<ToolPath> {
            l.paths
                .iter()
                .filter(|p| p.kind == PathKind::SparseInfill)
                .cloned()
                .collect()
        };
        // Nothing in the lower half: the trees reach the walls well before.
        assert!(light.layers[..n / 2].iter().all(|l| sparse(l).is_empty()));
        // Just under the top skin (4 top layers): a tree.
        let under = &light.layers[n - 6];
        let branches = sparse(under);
        assert!(branches.len() > 3, "{} branches", branches.len());
        // Every branch ends at the walls (the infill area's edge is about 9.2 mm out).
        for b in &branches {
            let out = |p: &Point2| p.x.abs().max(p.y.abs());
            let far = out(&b.points[0]).max(out(b.points.last().unwrap()));
            assert!(far > 8.5, "a branch ends at {far} mm");
        }
        // Far less plastic than a grid.
        let total = |r: &SliceResult| {
            r.layers
                .iter()
                .map(|l| infill_length(&l.paths))
                .sum::<f32>()
        };
        let ratio = total(&light) / total(&grid);
        assert!(ratio < 0.4, "{ratio}");
        assert!(ratio > 0.0);
    }

    #[test]
    fn test_adaptive_cubic_is_thinner_deep_inside() {
        // A 40 mm cube: adaptive cubic uses less infill than cubic, at least half of it,
        // and is as dense as cubic next to the top.
        let tris = block_at(40.0, 40.0, 0.0);
        let with = |pattern| {
            let s = PrintSettings {
                infill_pattern: pattern,
                ..PrintSettings::default()
            };
            slice(&tris, &s, 0.42, &|_| {})
        };
        let (cubic, adaptive) = (
            with(InfillPattern::Cubic),
            with(InfillPattern::AdaptiveCubic),
        );
        let total = |r: &SliceResult| {
            r.layers
                .iter()
                .map(|l| infill_length(&l.paths))
                .sum::<f32>()
        };
        let ratio = total(&adaptive) / total(&cubic);
        assert!((0.5..0.85).contains(&ratio), "{ratio}");
        // The middle layer: deep inside it is half as dense.
        let mid = cubic.layers.len() / 2;
        let middle =
            infill_length(&adaptive.layers[mid].paths) / infill_length(&cubic.layers[mid].paths);
        assert!(middle < 0.8, "{middle}");
        // Just under the top surface (within 4 mm): the same as cubic.
        let near_top = cubic.layers.len() - 8;
        assert_eq!(
            infill_length(&adaptive.layers[near_top].paths),
            infill_length(&cubic.layers[near_top].paths)
        );
    }

    #[test]
    fn test_gyroid_infill_waves_inside_the_walls_and_changes_with_height() {
        let tris = block_at(20.0, 10.0, 0.0);
        let s = PrintSettings {
            infill_pattern: InfillPattern::Gyroid,
            ..PrintSettings::default()
        };
        let r = slice(&tris, &s, 0.42, &|_| {});
        let infill = |i: usize| -> Vec<&ToolPath> {
            r.layers[i]
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::SparseInfill)
                .collect()
        };
        let (a, b) = (infill(20), infill(23));
        assert!(!a.is_empty() && !b.is_empty());
        // Curves (many points per line), inside the cube's walls (10 mm half-width).
        assert!(a.iter().any(|p| p.points.len() > 10));
        assert!(a
            .iter()
            .flat_map(|p| &p.points)
            .all(|q| q.x.abs() < 10.0 && q.y.abs() < 10.0));
        assert_ne!(format!("{a:?}"), format!("{b:?}"));
    }

    #[test]
    fn test_box_modifier_with_full_infill_is_solid_only_inside_the_box() {
        // A 30 mm cube, and a 10 x 10 mm box modifier through its middle (x and y from
        // -5 to 5) with 100 % infill.
        let tris = block_at(30.0, 30.0, 0.0);
        let modifier = InfillModifier {
            triangles: block_at(10.0, 30.0, 0.0),
            infill_percent: Some(100.0),
            infill_pattern: None,
        };
        let settings = PrintSettings::default();
        let r = slice_modified(
            &tris,
            &[modifier],
            &SupportVolumes::default(),
            None,
            &settings,
            0.42,
            &|_| {},
        );
        let plain = slice(&tris, &settings, 0.42, &|_| {});
        assert_eq!(r.layers.len(), plain.layers.len());
        // A middle layer (well away from the top and bottom surfaces).
        let mid = &r.layers[r.layers.len() / 2];
        let points = |kind: PathKind| -> Vec<Point2> {
            mid.paths
                .iter()
                .filter(|p| p.kind == kind)
                .flat_map(|p| p.points.iter().copied())
                .collect()
        };
        let solid = points(PathKind::SolidInfill);
        assert!(!solid.is_empty(), "solid infill inside the box");
        let inside = |p: &Point2| p.x.abs() <= 5.6 && p.y.abs() <= 5.6;
        assert!(solid.iter().all(inside), "solid infill only inside the box");
        // The solid lines fill it: the box is 10 mm across, lines are 0.42 mm apart.
        let solid_lines = mid
            .paths
            .iter()
            .filter(|p| p.kind == PathKind::SolidInfill)
            .count();
        assert!(solid_lines >= 20, "{solid_lines} lines");
        // Sparse infill only outside the box (allowing for the half-cell steps).
        let sparse = points(PathKind::SparseInfill);
        assert!(!sparse.is_empty());
        let deep_inside = |p: &Point2| p.x.abs() < 4.4 && p.y.abs() < 4.4;
        assert!(
            !sparse.iter().any(deep_inside),
            "no sparse infill inside the box"
        );
        // The walls are the cube's own.
        let walls = |r: &SliceResult| {
            let l = &r.layers[r.layers.len() / 2];
            format!(
                "{:?}",
                l.paths
                    .iter()
                    .filter(|p| matches!(p.kind, PathKind::OuterWall | PathKind::InnerWall))
                    .collect::<Vec<_>>()
            )
        };
        assert_eq!(walls(&r), walls(&plain));
        // With no modifiers, nothing changes.
        let none = slice_modified(
            &tris,
            &[],
            &SupportVolumes::default(),
            None,
            &settings,
            0.42,
            &|_| {},
        );
        assert_eq!(format!("{none:?}"), format!("{plain:?}"));
    }

    #[test]
    fn test_slice_groups_height_range_changes_infill_exactly_at_its_layers() {
        // A 10 mm box; from 2 mm up to 4 mm it has no infill and four walls.
        for adhesion in [Adhesion::Skirt, Adhesion::Raft] {
            let tris = block_at(20.0, 10.0, 0.0);
            let plate = PrintSettings {
                adhesion,
                ..PrintSettings::default()
            };
            let range = PrintSettings {
                infill_percent: 0.0,
                walls: 4,
                ..plate.clone()
            };
            let ps = PartSettings {
                base: plate.clone(),
                ranges: vec![(2.0, 4.0, range)],
                modifiers: Vec::new(),
            };
            let none = SupportVolumes::default();
            let r = slice_groups(&tris, &[(tris.clone(), ps)], &none, &plate, 0.42, &|_| {});
            let plain = slice(&tris, &plate, 0.42, &|_| {});
            assert_eq!(r.layers.len(), plain.layers.len());
            let lift = if adhesion == Adhesion::Raft {
                raft_height(&plate) + RAFT_AIR_GAP_MM
            } else {
                0.0
            };
            let count =
                |l: &Layer, kind: PathKind| l.paths.iter().filter(|p| p.kind == kind).count();
            let mut in_range = 0;
            for (l, p) in r.layers.iter().zip(&plain.layers) {
                let middle = l.z - l.height / 2.0 - lift;
                if (2.0..4.0).contains(&middle) {
                    in_range += 1;
                    assert_eq!(
                        count(l, PathKind::SparseInfill),
                        0,
                        "{adhesion:?} z {}",
                        l.z
                    );
                    assert_eq!(count(l, PathKind::InnerWall), 3, "{adhesion:?} z {}", l.z);
                } else {
                    // Outside the range the layer is exactly the box's own.
                    assert_eq!(
                        format!("{:?}", l.paths),
                        format!("{:?}", p.paths),
                        "z {}",
                        l.z
                    );
                }
            }
            assert_eq!(in_range, 10, "{adhesion:?}: 2 mm of 0.2 mm layers");
        }
    }

    #[test]
    fn test_slice_groups_two_objects_with_different_infill_slice_differently() {
        let (a, b) = (block_at(10.0, 4.0, -15.0), block_at(10.0, 6.0, 15.0));
        let mut all = a.clone();
        all.extend(b.clone());
        let plate = PrintSettings::default();
        let empty = PrintSettings {
            infill_percent: 0.0,
            walls: 3,
            ..plate.clone()
        };
        let r = slice_groups(
            &all,
            &[
                (a, PartSettings::plain(empty)),
                (b, PartSettings::plain(plate.clone())),
            ],
            &SupportVolumes::default(),
            &plate,
            0.42,
            &|_| {},
        );
        // As many layers as the taller box needs.
        assert_eq!(
            r.layers.len(),
            slice(&all, &plate, 0.42, &|_| {}).layers.len()
        );
        let xs = |kind: PathKind| -> Vec<f32> {
            r.layers
                .iter()
                .flat_map(|l| &l.paths)
                .filter(|p| p.kind == kind)
                .flat_map(|p| p.points.iter().map(|q| q.x))
                .collect()
        };
        // Sparse infill only in the right-hand box, which keeps the plate's 25 %.
        let sparse = xs(PathKind::SparseInfill);
        assert!(!sparse.is_empty());
        assert!(
            sparse.iter().all(|&x| x > 5.0),
            "sparse infill in the left box"
        );
        // The left box has three walls: three loops on a middle layer, the right two.
        let mid = &r.layers[10];
        let loops = |left: bool| {
            mid.paths
                .iter()
                .filter(|p| matches!(p.kind, PathKind::OuterWall | PathKind::InnerWall))
                .filter(|p| (p.points[0].x < 0.0) == left)
                .count()
        };
        assert_eq!((loops(true), loops(false)), (3, 2));
        // One skirt round both boxes, on the first layer.
        let skirt = xs(PathKind::Skirt);
        let (lo, hi) = skirt
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)));
        assert!(lo < -20.0 && hi > 20.0, "{lo} {hi}");
        assert_eq!(
            r.layers[0]
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::Skirt)
                .count(),
            SKIRT_LOOPS
        );
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
        // A point on an edge near a corner doesn't stick out once the loop has shrunk past
        // it (it used to stay where it was along the edge).
        let thin = vec![
            Point2::new(-10.0, -0.5),
            Point2::new(10.0, -0.5),
            Point2::new(10.0, 0.45),
            Point2::new(10.0, 0.5),
            Point2::new(-10.0, 0.5),
            Point2::new(-10.0, -0.45),
        ];
        let shrunk = offset_left(&thin, 0.4).unwrap();
        assert_eq!(shrunk.len(), 4);
        assert!(shrunk.iter().all(|p| p.y.abs() < 0.11), "{shrunk:?}");
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

    /// Every point of every support line in `r`.
    fn support_points(r: &SliceResult) -> Vec<Point2> {
        r.layers
            .iter()
            .flat_map(|l| &l.paths)
            .filter(|p| p.kind == PathKind::Support)
            .flat_map(|p| p.points.iter().copied())
            .collect()
    }

    #[test]
    fn test_support_blocker_removes_supports_in_its_footprint() {
        let tris = slab_on_post();
        let s = PrintSettings {
            supports: true,
            ..PrintSettings::default()
        };
        // A blocker over the slab's left half (x below 0), floor to above the slab.
        let mut o = PlacedObject::new("blocker", Mesh::cube(Vec3::new(20.0, 40.0, 20.0)));
        o.x = -10.0;
        let volumes = SupportVolumes {
            blockers: vec![o.world_triangles()],
            enforcers: Vec::new(),
        };
        let free = support_points(&slice(&tris, &s, 0.42, &|_| {}));
        assert!(
            free.iter().any(|p| p.x < -5.0),
            "supports on the left without it"
        );
        let blocked = slice_modified(&tris, &[], &volumes, None, &s, 0.42, &|_| {});
        let pts = support_points(&blocked);
        assert!(
            pts.iter().any(|p| p.x > 5.0),
            "the right half is still supported"
        );
        assert!(
            !pts.iter().any(|p| p.x < -1.5),
            "no supports in the blocker's footprint"
        );
    }

    #[test]
    fn test_support_enforcer_supports_a_shallow_overhang() {
        // A cone widening upwards on a 5 mm post: its walls lean 30 degrees from upright,
        // too little for supports at the default angle, and supports are off.
        let mut tris =
            PlacedObject::new("post", Mesh::cylinder(4.0, 4.0, 5.0, 48)).world_triangles();
        tris.extend(
            Mesh::cylinder(4.0, 9.8, 10.0, 48)
                .triangles
                .iter()
                .map(|t| t.map(|v| v + Vec3::new(0.0, 0.0, 5.0))),
        );
        let s = PrintSettings::default();
        assert!(!s.supports);
        assert!(support_points(&slice(&tris, &s, 0.42, &|_| {})).is_empty());
        // An enforcer over the half with x above 0.
        let mut o = PlacedObject::new("enforcer", Mesh::cube(Vec3::new(12.0, 24.0, 12.0)));
        o.x = 6.0;
        let volumes = SupportVolumes {
            blockers: Vec::new(),
            enforcers: vec![o.world_triangles()],
        };
        let pts = support_points(&slice_modified(
            &tris,
            &[],
            &volumes,
            None,
            &s,
            0.42,
            &|_| {},
        ));
        // Supports under the flaring wall (beyond the post), and only inside the enforcer.
        assert!(
            pts.iter().any(|p| p.x.hypot(p.y) > 6.0),
            "the enforcer supports the wall"
        );
        assert!(pts.iter().all(|p| p.x > -1.5), "only inside the enforcer");
    }

    fn with_supports(change: impl Fn(&mut PrintSettings)) -> PrintSettings {
        let mut s = PrintSettings {
            supports: true,
            ..PrintSettings::default()
        };
        change(&mut s);
        s
    }

    /// The layers with support lines, and each one's support length.
    fn support_layers(r: &SliceResult) -> Vec<(usize, f32)> {
        r.layers
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let len: f32 = l
                    .paths
                    .iter()
                    .filter(|p| p.kind == PathKind::Support)
                    .map(|p| {
                        p.points
                            .windows(2)
                            .map(|w| w[0].distance(w[1]))
                            .sum::<f32>()
                    })
                    .sum();
                (i, len)
            })
            .filter(|(_, len)| *len > 0.0)
            .collect()
    }

    fn bridge_test_model() -> Vec<Triangle> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/models/bridge-overhang-test.stl");
        PlacedObject::new("bridge", Mesh::load(&path).unwrap()).world_triangles()
    }

    fn tree() -> PrintSettings {
        with_supports(|s| s.support_style = SupportStyle::Tree)
    }

    /// A fin 2 mm tall: a wedge 4 mm wide at x = 0 narrowing to a point at x = 20.
    fn wedge_fin() -> Vec<Triangle> {
        let (a, b, c) = (
            Vec3::new(0.0, -2.0, 0.0),
            Vec3::new(20.0, 0.0, 0.0),
            Vec3::new(0.0, 2.0, 0.0),
        );
        let up = Vec3::new(0.0, 0.0, 2.0);
        let (a2, b2, c2) = (a + up, b + up, c + up);
        vec![
            [a, c, b],
            [a2, b2, c2],
            [a, b, b2],
            [a, b2, a2],
            [b, c, c2],
            [b, c2, b2],
            [c, a, a2],
            [c, a2, c2],
        ]
    }

    #[test]
    fn test_variable_walls_fill_a_tapered_fin_continuously() {
        let fin = wedge_fin();
        let s = PrintSettings {
            variable_walls: true,
            ..PrintSettings::default()
        };
        let r = slice(&fin, &s, 0.42, &|_| {});
        let mid = &r.layers[r.layers.len() / 2];
        let walls: Vec<&ToolPath> = mid
            .paths
            .iter()
            .filter(|p| matches!(p.kind, PathKind::OuterWall | PathKind::InnerWall))
            .collect();
        assert!(
            walls.iter().any(|p| p.width.is_some()),
            "centre lines on the thin end"
        );
        for p in &walls {
            let w = p.width.unwrap_or(0.42);
            assert!((0.2..=0.64).contains(&w), "width {w}");
        }
        // A line crosses every 0.25 mm along the fin, to where it is half a line wide.
        for k in 1..76 {
            let x = k as f32 * 0.25;
            assert!(
                walls.iter().any(|p| p
                    .points
                    .windows(2)
                    .any(|w| (w[0].x - x) * (w[1].x - x) <= 0.0 && w[0].x != w[1].x)),
                "nothing at x = {x}"
            );
        }
        // No gap fill: the centre lines do its job.
        assert!(!mid.paths.iter().any(|p| p.kind == PathKind::GapFill));
        // Off (the default): the old walls and gap fill.
        let plain = slice(&fin, &PrintSettings::default(), 0.42, &|_| {});
        assert!(plain.layers[plain.layers.len() / 2]
            .paths
            .iter()
            .all(|p| p.kind != PathKind::OuterWall || p.width.is_none()));
    }

    #[test]
    fn test_tree_supports_keep_clear_of_the_model_sides() {
        let tris = bridge_test_model();
        let r = slice(&tris, &tree(), 0.42, &|_| {});
        let mut points = 0;
        for l in &r.layers {
            let outline = layer_outline(&tris, l.z - l.height / 2.0);
            for p in l
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::Support)
                .flat_map(|p| &p.points)
            {
                points += 1;
                // Outside the model, and the XY gap (1 mm, less a little) clear of it.
                let inside = outline.iter().filter(|o| contains(o, *p)).count() % 2 == 1;
                assert!(!inside, "support inside the model at z {}: {p:?}", l.z);
                let near = outline
                    .iter()
                    .map(|o| distance_to_loop(*p, o))
                    .fold(f32::MAX, f32::min);
                assert!(near > 0.6, "support {near} mm from the model at z {}", l.z);
            }
        }
        assert!(points > 1000, "{points} support points");
    }

    #[test]
    fn test_tree_supports_use_less_than_grid_supports() {
        let tris = bridge_test_model();
        let volume = |s: &PrintSettings| -> f32 {
            slice(&tris, s, 0.42, &|_| {})
                .layers
                .iter()
                .map(|l| {
                    let len: f32 = l
                        .paths
                        .iter()
                        .filter(|p| p.kind == PathKind::Support)
                        .map(|p| {
                            let mut pts = p.points.clone();
                            if p.closed {
                                pts.push(pts[0]);
                            }
                            pts.windows(2).map(|w| w[0].distance(w[1])).sum::<f32>()
                        })
                        .sum();
                    len * 0.42 * l.height
                })
                .sum()
        };
        let (grid, tree) = (volume(&with_supports(|_| {})), volume(&tree()));
        assert!(
            tree > 0.0 && tree < grid,
            "tree {tree} mm3, grid {grid} mm3"
        );
    }

    #[test]
    fn test_tree_branches_join_on_the_way_down() {
        // Under the wide slab, many tips start; far fewer branches reach low down.
        let r = slice(&slab_on_post(), &tree(), 0.42, &|_| {});
        let branches = |i: usize| {
            // Each branch's outermost ring is the first of its rings: count centres.
            let mut centres: Vec<(i32, i32)> = r.layers[i]
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::Support && p.closed)
                .map(|p| {
                    let n = p.points.len() as f32;
                    let (x, y) = p
                        .points
                        .iter()
                        .fold((0.0, 0.0), |a, q| (a.0 + q.x, a.1 + q.y));
                    ((x / n * 5.0).round() as i32, (y / n * 5.0).round() as i32)
                })
                .collect();
            centres.sort_unstable();
            centres.dedup();
            centres.len()
        };
        let top = support_layers(&r).last().unwrap().0;
        let (high, low) = (branches(top), branches(3));
        assert!(high > 50, "{high} tips");
        assert!(low * 3 < high, "{low} branches low down from {high} tips");
    }

    #[test]
    fn test_support_top_gap_leaves_the_layers_under_the_overhang_empty() {
        // The slab's underside is at 10 mm.
        let tris = slab_on_post();
        let top_of = |s: &PrintSettings| {
            let r = slice(&tris, s, 0.42, &|_| {});
            let (i, _) = *support_layers(&r).last().unwrap();
            r.layers[i].z
        };
        // Auto: one layer gap.
        assert!((top_of(&with_supports(|_| {})) - 9.8).abs() < 1e-3);
        // 1 mm: nothing above 9 mm.
        let gapped = top_of(&with_supports(|s| s.support_top_gap = Some(1.0)));
        assert!(gapped <= 9.0 + 1e-3 && gapped > 8.7, "{gapped}");
    }

    #[test]
    fn test_support_interface_layers_are_dense() {
        let tris = slab_on_post();
        let s = with_supports(|s| s.support_interface_layers = 3);
        let r = slice(&tris, &s, 0.42, &|_| {});
        let layers = support_layers(&r);
        let len = |i: usize| {
            layers
                .iter()
                .find(|(j, _)| *j == i)
                .map_or(0.0, |(_, l)| *l)
        };
        let top = layers.last().unwrap().0;
        // The top three layers are interface: several times the support of a normal
        // layer (lines a line width apart, not 2 mm).
        let normal = len(top - 10);
        for i in top - 2..=top {
            assert!(len(i) > normal * 3.0, "layer {i}: {} vs {normal}", len(i));
        }
        assert!(
            len(top - 3) < normal * 1.5,
            "the fourth layer down is normal"
        );
        // Without interface layers the top is normal too.
        let plain = support_layers(&slice(&tris, &with_supports(|_| {}), 0.42, &|_| {}));
        let (ptop, plen) = *plain.last().unwrap();
        assert_eq!(ptop, top);
        assert!(plen < normal * 1.5);
    }

    #[test]
    fn test_support_bottom_gap_keeps_off_the_model_below() {
        // The slab on a post, the post on a 3 mm block: supports stand on the block.
        let mut tris = slab_on_post();
        tris.iter_mut().flatten().for_each(|v| v.z += 3.0);
        tris.extend(
            PlacedObject::new("block", Mesh::cube(Vec3::new(40.0, 40.0, 3.0))).world_triangles(),
        );
        let lowest = |s: &PrintSettings| {
            let r = slice(&tris, s, 0.42, &|_| {});
            let (i, _) = support_layers(&r)[0];
            r.layers[i].z - r.layers[i].height
        };
        let base = |s: &mut PrintSettings| s.support_raft = false;
        let touching = lowest(&with_supports(base));
        assert!((touching - 3.0).abs() < 1e-3, "{touching}");
        let gapped = lowest(&with_supports(|s| {
            base(s);
            s.support_bottom_gap = 0.6;
        }));
        assert!((3.6 - 1e-3..3.9).contains(&gapped), "{gapped}");
    }

    #[test]
    fn test_support_xy_gap_and_line_spacing() {
        let tris = slab_on_post();
        let points = |s: &PrintSettings| -> Vec<Point2> {
            let r = slice(&tris, s, 0.42, &|_| {});
            r.layers[20]
                .paths
                .iter()
                .filter(|p| p.kind == PathKind::Support)
                .flat_map(|p| p.points.iter().copied())
                .collect()
        };
        // The post is 4 mm square at the middle: with a 3 mm gap no support comes nearer
        // than about 3 mm (2 mm half-width plus the gap, less half a cell).
        let far = points(&with_supports(|s| s.support_xy_gap = 3.0));
        assert!(
            far.iter().all(|p| p.x.abs().max(p.y.abs()) > 4.4),
            "near the post"
        );
        let near = points(&with_supports(|_| {}));
        assert!(near.iter().any(|p| p.x.abs().max(p.y.abs()) < 4.4));
        // Lines 4 mm apart: about half as many as 2 mm.
        let count = |s: &PrintSettings| points(s).len() / 2;
        let (two, four) = (
            count(&with_supports(|_| {})),
            count(&with_supports(|s| s.support_spacing = 4.0)),
        );
        assert!(
            (four as f32 / two as f32 - 0.5).abs() < 0.15,
            "{two} vs {four}"
        );
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
