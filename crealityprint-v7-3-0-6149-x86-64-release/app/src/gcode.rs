//! Writes sliced layers as G-code and estimates print time, filament and cost.

use crate::geometry::Point2;
use crate::motion;
use crate::profiles::{AccelFeature, Filament, PrintSettings, Printer, SeamPosition, SpeedFeature};
use crate::slicer::{Layer, PathKind, SliceResult, ToolPath};
use std::fmt::Write;

/// Extra time per layer for slowing down, turning and layer changes.
const LAYER_CHANGE_SECONDS: f32 = 1.0;
/// Heating up the bed and nozzle before printing.
const WARMUP_SECONDS: f32 = 120.0;
/// Fuzzy top: a new random height every this many millimetres along the top lines.
const FUZZY_TOP_STEP_MM: f32 = 0.5;
/// Ironing passes push out only this share of normal plastic, slowly.
const IRONING_FLOW: f32 = 0.15;
const IRONING_SPEED: f32 = 15.0;
/// No printing move is slower than this (mm/s): neither a speed set in the settings nor
/// the cooling slowdown can go below it. Ironing has its own fixed speed.
pub const MIN_PRINT_SPEED: f32 = 15.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Estimate {
    pub seconds: f32,
    pub filament_mm: f32,
    pub grams: f32,
    pub cost: f32,
    pub layers: usize,
    /// On a two-nozzle print, filament (mm) and weight (g) for each nozzle (T0, T1), each
    /// worked out with that nozzle's filament. None on a one-nozzle print.
    pub per_nozzle: Option<[(f32, f32); 2]>,
}

impl Estimate {
    pub fn time_text(&self) -> String {
        let total = self.seconds.round() as u64;
        let (h, m) = (total / 3600, (total % 3600) / 60);
        if h > 0 {
            format!("{h} h {m} min")
        } else {
            format!("{} min", m.max(1))
        }
    }
}

/// Filament (mm) needed for a line of this length, width and height.
pub fn filament_for(length: f32, width: f32, height: f32, filament_diameter: f32) -> f32 {
    let filament_area = std::f32::consts::PI * (filament_diameter / 2.0).powi(2);
    length * width * height / filament_area
}

/// The settings-panel speed that a kind of line prints at, if it has one of its own.
pub fn speed_feature(kind: PathKind) -> Option<SpeedFeature> {
    match kind {
        PathKind::OuterWall => Some(SpeedFeature::OuterWall),
        PathKind::InnerWall => Some(SpeedFeature::InnerWall),
        PathKind::SparseInfill => Some(SpeedFeature::SparseInfill),
        PathKind::SolidInfill => Some(SpeedFeature::SolidInfill),
        PathKind::TopSurface => Some(SpeedFeature::TopSurface),
        PathKind::Support => Some(SpeedFeature::Support),
        PathKind::Bridge => Some(SpeedFeature::Bridge),
        PathKind::Skirt
        | PathKind::Brim
        | PathKind::Ironing
        | PathKind::Raft
        | PathKind::PrimeTower
        | PathKind::OozeShield => None,
    }
}

/// The speed (mm/s) for a line of `kind` on layer `layer`, before cooling slow-down. The
/// first layer always uses the first layer speed.
fn speed_for(kind: PathKind, layer: usize, s: &PrintSettings) -> f32 {
    if layer == 0 {
        return s.first_layer_speed;
    }
    match speed_feature(kind) {
        Some(f) => s.speeds.speed(f, s.print_speed),
        None => s.print_speed,
    }
}

/// Splits a line into steps no longer than `step`, keeping every original corner.
pub fn resample(points: &[Point2], step: f32) -> Vec<Point2> {
    let mut out = Vec::with_capacity(points.len());
    for w in points.windows(2) {
        let len = w[0].distance(w[1]);
        let n = (len / step).ceil().max(1.0) as usize;
        for k in 0..n {
            let t = k as f32 / n as f32;
            out.push(Point2::new(
                w[0].x + (w[1].x - w[0].x) * t,
                w[0].y + (w[1].y - w[0].y) * t,
            ));
        }
    }
    if let Some(last) = points.last() {
        out.push(*last);
    }
    out
}

/// A repeatable "random" number from 0 to 1 for one step of one path on one layer, so
/// slicing the same plate twice gives the same file.
pub fn fuzzy_unit(layer: usize, path: usize, step: usize) -> f32 {
    let mut h = (layer as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (path as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (step as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 31;
    h = h.wrapping_mul(0xD6E8_FEB8_6659_FD93);
    h ^= h >> 32;
    (h % 10_001) as f32 / 10_000.0
}

fn path_length(points: &[Point2], closed: bool) -> f32 {
    let open: f32 = points.windows(2).map(|w| w[0].distance(w[1])).sum();
    match (closed, points.first(), points.last()) {
        (true, Some(a), Some(b)) => open + a.distance(*b),
        _ => open,
    }
}

/// The absolute extruder position (M82) and the filament pushed so far. Kept in f64:
/// E grows for the whole print, and an f32 at a 1 kg print's E (~330 m) only resolves
/// ~0.03 mm, which would swallow or double the short moves of fuzzy skin.
#[derive(Debug, Default)]
struct Extruder {
    e: f64,
    extruded: f64,
}

impl Extruder {
    /// Pushes `mm` of filament; returns the new absolute E.
    fn extrude(&mut self, mm: f32) -> f64 {
        self.e += mm as f64;
        self.extruded += mm as f64;
        self.e
    }

    fn retract(&mut self, mm: f32) -> f64 {
        self.e -= mm as f64;
        self.e
    }

    fn unretract(&mut self, mm: f32) -> f64 {
        self.e += mm as f64;
        self.e
    }
}

/// Index of the point a closed loop should start from.
#[cfg(test)]
pub fn seam_start(pts: &[Point2], nozzle: Point2, seam: SeamPosition, salt: usize) -> usize {
    seam_start_aligned(pts, nozzle, seam, salt, &[])
}

/// Like `seam_start`; `below` holds where the loops of the layer below started, which
/// Aligned seams follow (falling back to Rear on the first layer).
pub fn seam_start_aligned(
    pts: &[Point2],
    nozzle: Point2,
    seam: SeamPosition,
    salt: usize,
    below: &[Point2],
) -> usize {
    match seam {
        SeamPosition::Aligned if !below.is_empty() => (0..pts.len())
            .min_by(|&a, &b| {
                let d = |p: Point2| below.iter().map(|q| q.distance(p)).fold(f32::MAX, f32::min);
                d(pts[a]).total_cmp(&d(pts[b]))
            })
            .unwrap_or(0),
        SeamPosition::Aligned => seam_start_aligned(pts, nozzle, SeamPosition::Rear, salt, &[]),
        SeamPosition::Nearest => (0..pts.len())
            .min_by(|&a, &b| pts[a].distance(nozzle).total_cmp(&pts[b].distance(nozzle)))
            .unwrap_or(0),
        // The back of the plate is +Y; ties go to the point nearest the middle.
        SeamPosition::Rear => (0..pts.len())
            .max_by(|&a, &b| {
                (pts[a].y - pts[a].x.abs() * 1e-3).total_cmp(&(pts[b].y - pts[b].x.abs() * 1e-3))
            })
            .unwrap_or(0),
        SeamPosition::Random => {
            let mut h = (salt as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            h ^= h >> 29;
            (h % pts.len().max(1) as u64) as usize
        }
    }
}

/// Purge lines run from Y = 20 mm up to this far from the back edge (or 200 mm at most).
const PURGE_MARGIN_MM: f32 = 20.0;
const PURGE_MAX_LEN_MM: f32 = 200.0;
const PURGE_X_MM: f32 = 1.5;
/// The second line sits this far to the right of the first, so the two merge into one
/// wide strip that peels off easily.
const PURGE_LINE_GAP_MM: f32 = 0.4;
const PURGE_HEIGHT_MM: f32 = 0.3;
/// Filament pushed out in place at the start, to refill a nozzle that oozed while heating.
const PURGE_PRIME_MM: f32 = 2.0;
const PURGE_SPEED_MM_S: f32 = 25.0;

pub struct Purge {
    pub gcode: String,
    pub filament_mm: f32,
    pub seconds: f32,
    /// Where the nozzle ends up, in the printer's own coordinates.
    pub end: Point2,
}

/// The purge (prime) lines: a short push in place, then two lines up and back down the
/// left edge sharing the printer's purge amount, then a small lift so the nozzle doesn't
/// drag the strip. Coordinates are the printer's own (front-left is 0, 0).
pub fn purge_lines(p: &Printer) -> Purge {
    let len = (p.bed_y - PURGE_MARGIN_MM * 2.0).clamp(20.0, PURGE_MAX_LEN_MM);
    let (y0, y1) = (PURGE_MARGIN_MM, PURGE_MARGIN_MM + len);
    let (x0, x1) = (PURGE_X_MM, PURGE_X_MM + PURGE_LINE_GAP_MM);
    let total = p.purge_mm.max(0.0);
    let prime = PURGE_PRIME_MM.min(total);
    let per_line = (total - prime) / 2.0;
    let f = PURGE_SPEED_MM_S * 60.0;
    let mut g = String::new();
    let _ = writeln!(g, "; purge: {total:.0} mm of filament to clear the nozzle");
    let _ = writeln!(g, "G1 Z2.0 F3000");
    let _ = writeln!(g, "G1 X{x0:.1} Y{y0:.1} F5000");
    let _ = writeln!(g, "G1 Z{PURGE_HEIGHT_MM:.2} F600");
    let _ = writeln!(g, "G1 E{prime:.3} F300 ; push out what oozed while heating");
    let _ = writeln!(
        g,
        "G1 X{x0:.1} Y{y1:.1} E{:.3} F{f:.0} ; purge line 1",
        prime + per_line
    );
    let _ = writeln!(g, "G1 X{x1:.1} Y{y1:.1} F{f:.0}");
    let _ = writeln!(
        g,
        "G1 X{x1:.1} Y{y0:.1} E{:.3} F{f:.0} ; purge line 2",
        prime + per_line * 2.0
    );
    let _ = writeln!(g, "G1 Z2.0 F3000 ; lift clear of the purge strip");
    Purge {
        gcode: g,
        filament_mm: total,
        seconds: prime / 5.0 + 2.0 * len / PURGE_SPEED_MM_S + 3.0,
        end: Point2::new(x1, y0),
    }
}

/// The filament type as slicers name it ("PLA", "PETG", "ABS", "TPU").
fn filament_type(f: &Filament) -> String {
    f.name
        .split_whitespace()
        .next()
        .unwrap_or("PLA")
        .to_ascii_uppercase()
}

/// Header comments with the job's temperatures, in the forms Orca/PrusaSlicer-style
/// (`; key = value`) and Cura-style (`;KEY:value`) files carry.
pub fn header_block(f: &Filament, s: &PrintSettings, p: &Printer) -> String {
    let mut h = String::new();
    h.push_str("; HEADER_BLOCK_START\n");
    h.push_str(&format!(
        "; nozzle_temperature_initial_layer = {}\n",
        f.nozzle_temp
    ));
    h.push_str(&format!(
        "; bed_temperature_initial_layer_single = {}\n",
        f.bed_temp
    ));
    h.push_str(&format!("; filament_type = {}\n", filament_type(f)));
    h.push_str(&format!("; layer_height = {:.2}\n", s.layer_height));
    h.push_str(&format!("; nozzle_diameter = {:.2}\n", p.nozzle_diameter));
    h.push_str("; HEADER_BLOCK_END\n");
    h.push_str(&format!(
        ";EXTRUDER_TRAIN.0.INITIAL_TEMPERATURE:{}\n",
        f.nozzle_temp
    ));
    h.push_str(&format!(
        ";BUILD_PLATE.INITIAL_TEMPERATURE:{}\n",
        f.bed_temp
    ));
    h
}

/// The settings block at the end of the file, with the keys PrusaSlicer- and Orca-style
/// slicers use, so tools that read those (printer screens, Moonraker) find the job's
/// temperatures whichever wording they look for.
pub fn config_block(f: &Filament, s: &PrintSettings, p: &Printer) -> String {
    let rows = [
        (
            "nozzle_temperature_initial_layer",
            f.nozzle_temp.to_string(),
        ),
        ("nozzle_temperature", f.nozzle_temp.to_string()),
        ("first_layer_temperature", f.nozzle_temp.to_string()),
        ("temperature", f.nozzle_temp.to_string()),
        (
            "bed_temperature_initial_layer_single",
            f.bed_temp.to_string(),
        ),
        ("first_layer_bed_temperature", f.bed_temp.to_string()),
        ("bed_temperature", f.bed_temp.to_string()),
        ("hot_plate_temp_initial_layer", f.bed_temp.to_string()),
        ("hot_plate_temp", f.bed_temp.to_string()),
        ("filament_type", filament_type(f)),
        ("filament_diameter", format!("{:.2}", f.diameter)),
        ("layer_height", format!("{:.2}", s.layer_height)),
        ("first_layer_height", format!("{:.2}", s.first_layer_height)),
        ("nozzle_diameter", format!("{:.2}", p.nozzle_diameter)),
        ("printer_model", p.name.clone()),
    ];
    let mut c = String::from("; CONFIG_BLOCK_START\n");
    for (k, v) in rows {
        c.push_str(&format!("; {k} = {v}\n"));
    }
    c.push_str("; CONFIG_BLOCK_END\n");
    c
}

pub struct GcodeOutput {
    pub text: String,
    pub estimate: Estimate,
}

/// Which nozzle prints what, on a printer with two (`Printer::extruders`). With the
/// default (one nozzle) the G-code is exactly what a one-nozzle printer gets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tools {
    /// Each part's footprint on the plate (plate coordinates: min x, min y, max x, max y)
    /// and the nozzle that prints it (0 = first/right, 1 = second/left).
    pub parts: Vec<([f32; 4], u8)>,
    /// The nozzle that prints supports.
    pub support_tool: u8,
    /// The default nozzle: the skirt, brim and raft use it when a part does, and a plate
    /// with nothing assigned prints with it (`Printer::primary_tool`).
    pub primary: u8,
    /// The other nozzle's filament (when nothing is given, the primary's).
    pub second: Option<Filament>,
    /// Ditto printing (two nozzles on one carriage print the same part side by side; the
    /// firmware copies the moves): only the second nozzle's heating is added.
    pub ditto: bool,
    /// The prime tower's centre on the plate (plate coordinates), when there is one.
    pub tower: Option<Point2>,
    /// The ooze shield's distance (mm) outside everything else the print lays down, when
    /// there is one.
    pub shield: Option<f32>,
}

/// The ooze shield's outline: one loop `distance` mm outside the convex hull of every line
/// the print lays down (parts, supports, skirt, brim and raft; not the prime tower). The
/// same on every layer, so the wall never overhangs. None for an empty print.
pub fn ooze_shield_outline(result: &SliceResult, distance: f32) -> Option<Vec<Point2>> {
    let pts: Vec<Point2> = result
        .layers
        .iter()
        .flat_map(|l| &l.paths)
        .filter(|p| !matches!(p.kind, PathKind::PrimeTower | PathKind::OozeShield))
        .flat_map(|p| p.points.iter().copied())
        .collect();
    let hull = crate::slicer::convex_hull(pts);
    crate::slicer::offset_left(&hull, -distance.max(0.5))
}

/// The prime tower's side length (mm).
pub const PRIME_TOWER_MM: f32 = 12.0;

/// The prime tower's lines for layer `li`: a square outline and a zigzag fill, along X on
/// even layers and along Y on odd ones, `width` apart.
pub fn prime_tower_paths(centre: Point2, width: f32, li: usize) -> Vec<ToolPath> {
    let h = PRIME_TOWER_MM / 2.0;
    let corner = |dx: f32, dy: f32| Point2::new(centre.x + dx, centre.y + dy);
    let outline = ToolPath {
        kind: PathKind::PrimeTower,
        points: vec![corner(-h, -h), corner(h, -h), corner(h, h), corner(-h, h)],
        closed: true,
    };
    let inner = h - width;
    let mut fill = Vec::new();
    let mut k = 0;
    let mut t = -inner;
    while t <= inner + 1e-4 {
        let (a, b) = if li % 2 == 0 {
            (corner(-inner, t), corner(inner, t))
        } else {
            (corner(t, -inner), corner(t, inner))
        };
        if k % 2 == 0 {
            fill.extend([a, b]);
        } else {
            fill.extend([b, a]);
        }
        k += 1;
        t += width;
    }
    vec![
        outline,
        ToolPath {
            kind: PathKind::PrimeTower,
            points: fill,
            closed: false,
        },
    ]
}

/// The last layer on which the nozzles change, following the order the writer uses
/// (the selected nozzle's paths first, then the other's). None when they never change.
pub fn last_tool_change(result: &SliceResult, tools: &Tools) -> Option<usize> {
    let mut current = tools.first_tool();
    let mut last = None;
    for (li, layer) in result.layers.iter().enumerate() {
        let mut used = [false; 2];
        for p in layer.paths.iter().filter(|p| {
            p.points.len() >= 2 && !matches!(p.kind, PathKind::PrimeTower | PathKind::OozeShield)
        }) {
            used[tools.tool_for(p).min(1) as usize] = true;
        }
        let other = 1 - current;
        if used[other as usize] {
            last = Some(li);
            current = other;
        }
    }
    last
}

impl Tools {
    /// Which nozzles print something: each part's, the supports' (when on) and the
    /// skirt's.
    pub fn used(&self, supports: bool) -> [bool; 2] {
        let mut used = [false; 2];
        used[self.first_tool() as usize] = true;
        for (_, t) in &self.parts {
            used[(*t).min(1) as usize] = true;
        }
        if supports {
            used[self.support_tool.min(1) as usize] = true;
        }
        used
    }

    /// Whether the non-primary nozzle prints anything.
    #[cfg(test)]
    pub fn uses_second(&self, supports: bool) -> bool {
        self.used(supports)[(1 - self.primary.min(1)) as usize]
    }

    /// The nozzle for the skirt, brim and raft, and the one a print starts with: the
    /// primary, unless no part uses it.
    fn first_tool(&self) -> u8 {
        let other = 1 - self.primary.min(1);
        if !self.parts.is_empty() && self.parts.iter().all(|(_, t)| *t == other) {
            other
        } else {
            self.primary.min(1)
        }
    }

    /// The nozzle that prints `path`.
    pub fn tool_for(&self, path: &ToolPath) -> u8 {
        match path.kind {
            PathKind::Support => self.support_tool,
            PathKind::Skirt
            | PathKind::Brim
            | PathKind::Raft
            | PathKind::PrimeTower
            | PathKind::OozeShield => self.first_tool(),
            _ => {
                let p = path.points[0];
                self.parts
                    .iter()
                    .find(|(b, _)| {
                        p.x >= b[0] - 1.0
                            && p.y >= b[1] - 1.0
                            && p.x <= b[2] + 1.0
                            && p.y <= b[3] + 1.0
                    })
                    .map(|(_, t)| *t)
                    .unwrap_or_else(|| self.first_tool())
            }
        }
    }
}

/// The absolute E value the purge lines end at (their last `E` word).
fn purge_end_e(purge: &str) -> f64 {
    purge
        .lines()
        .rev()
        .filter_map(|l| l.split_whitespace().find_map(|w| w.strip_prefix('E')))
        .find_map(|e| e.parse::<f64>().ok())
        .unwrap_or(0.0)
}

/// Builds the whole G-code file for one nozzle. Plate coordinates are centred on the
/// plate; the printer's coordinates start at its front-left corner. (The app goes through
/// [`generate_with_tools`]; this is the one-nozzle shorthand the tests use.)
#[cfg(test)]
pub fn generate(
    result: &SliceResult,
    printer: &Printer,
    filament: &Filament,
    s: &PrintSettings,
) -> GcodeOutput {
    generate_with_tools(result, printer, filament, s, &Tools::default())
}

/// Like [`generate`], for a printer with two nozzles: each path goes to its nozzle (see
/// [`Tools`]), and each layer is printed nozzle by nozzle with a tool change between.
pub fn generate_with_tools(
    result: &SliceResult,
    printer: &Printer,
    filament: &Filament,
    s: &PrintSettings,
    tools: &Tools,
) -> GcodeOutput {
    let mut w = Writer::start(result, printer, filament, s, tools);
    // Where the wall loops of the previous layer started (for Aligned seams).
    let mut seams_below: Vec<Point2> = Vec::new();
    for (li, layer) in result.layers.iter().enumerate() {
        w.write_layer(li, layer, &mut seams_below);
    }
    w.finish(result)
}

/// Writes the file front to back, tracking where the nozzle is, the absolute E value
/// and the running time and filament totals.
struct Writer<'a> {
    g: String,
    printer: &'a Printer,
    filament: &'a Filament,
    s: &'a PrintSettings,
    /// Plate centre in printer coordinates (plate point + offset = printer point).
    ox: f32,
    oy: f32,
    line_width: f32,
    /// Travel feed rate in mm/min.
    travel_f: f32,
    /// Nozzle position in plate coordinates.
    pos: Point2,
    /// Absolute E and the filament extruded since the purge.
    ext: Extruder,
    /// Filament used by the purge lines.
    prime_e: f32,
    seconds: f32,
    tools: &'a Tools,
    /// A two-nozzle printer: nozzles are named in every heat command and selected
    /// explicitly.
    dual: bool,
    /// Which nozzles print something (heated, purged, switched off).
    used: [bool; 2],
    /// Both nozzles print, so layers change tools.
    two: bool,
    /// The nozzle selected now.
    tool: u8,
    /// Filament pulled back now (by a travel or layer-change retraction), pushed forward
    /// again before the next line.
    retracted: bool,
    /// How far each nozzle was pulled back when it was set aside (mm).
    parked: [f32; 2],
    /// The line just printed, for wiping back along it.
    last_path: Vec<Point2>,
    /// Filament pushed by each nozzle (mm), purge lines included.
    by_tool: [f64; 2],
    /// The acceleration last written for printing moves [0] and travels [1]; Klipper has
    /// one acceleration for both and uses [0] only. `None` until first written.
    accel_now: [Option<f32>; 2],
    /// The part fan now (%): off on the first layer, the filament's from the second, the
    /// bridge fan while printing bridges.
    fan_now: u32,
}

/// Time for a tool change: retract, switch, prime.
const TOOL_CHANGE_SECONDS: f32 = 5.0;

impl<'a> Writer<'a> {
    /// Header comments, heating, homing and the purge lines.
    fn start(
        result: &SliceResult,
        printer: &'a Printer,
        filament: &'a Filament,
        s: &'a PrintSettings,
        tools: &'a Tools,
    ) -> Self {
        let dual = printer.extruders >= 2;
        let used = if !dual {
            [true, false]
        } else if tools.ditto {
            [true, true]
        } else {
            tools.used(s.supports)
        };
        let two = dual && used[0] && used[1] && !tools.ditto;
        let start_tool = if dual { tools.first_tool() } else { 0 };
        // The primary nozzle has the main filament, the other one the second (in Ditto both
        // print the main one).
        let temp_of = |t: u8| -> u32 {
            match &tools.second {
                Some(f) if t != tools.primary && !tools.ditto => f.nozzle_temp,
                _ => filament.nozzle_temp,
            }
        };
        let mut g = String::with_capacity(1 << 20);
        let ox = printer.bed_x / 2.0;
        let oy = printer.bed_y / 2.0;

        let _ = writeln!(g, "; Made with PrintCraft (a simple slicer)");
        let _ = writeln!(g, "; printer: {}", printer.name);
        // Read back by the X3G translator, which needs the plate's centre.
        let _ = writeln!(g, "; plate: {:.1} x {:.1} mm", printer.bed_x, printer.bed_y);
        let _ = writeln!(
            g,
            "; filament: {} {}C / bed {}C",
            filament.name, filament.nozzle_temp, filament.bed_temp
        );
        let _ = writeln!(
            g,
            "; layer height {:.2} mm, {} walls, {:.0}% infill, supports {}",
            s.layer_height,
            s.walls,
            s.infill_percent,
            if s.supports { "on" } else { "off" }
        );
        let _ = writeln!(g, ";LAYER_COUNT:{}", result.layers.len());
        // The same count in the header wording other slicers use, which printer screens read.
        let _ = writeln!(g, "; total layer number: {}", result.layers.len());
        // The job's temperatures, in the header forms other slicers write. A printer that
        // prepares (levels) before running the file reads these to know the job temperature;
        // without them it may prepare at 0 °C and heat up again afterwards.
        g.push_str(&header_block(filament, s, printer));
        let _ = writeln!(g, "M140 S{} ; bed temperature", filament.bed_temp);
        if dual {
            for t in [0u8, 1].into_iter().filter(|t| used[*t as usize]) {
                let _ = writeln!(g, "M104 T{t} S{} ; nozzle {t} temperature", temp_of(t));
            }
        } else {
            let _ = writeln!(g, "M104 S{} ; nozzle temperature", filament.nozzle_temp);
        }
        let _ = writeln!(g, "G90 ; absolute positions");
        let _ = writeln!(g, "M82 ; absolute extrusion");
        let _ = writeln!(g, "G28 ; home all axes");
        // Wait only while heating up: a nozzle or plate that is already hotter carries on.
        g.push_str(&crate::heat::wait_bed(filament.bed_temp, printer.klipper));
        if dual {
            for t in [0u8, 1].into_iter().filter(|t| used[*t as usize]) {
                let _ = writeln!(g, "M109 T{t} S{} ; wait for nozzle {t}", temp_of(t));
            }
        } else {
            g.push_str(&crate::heat::wait_nozzle(
                filament.nozzle_temp,
                printer.klipper,
            ));
        }
        let _ = writeln!(g, "M107 ; fan off");
        let _ = writeln!(g, "G92 E0");
        if dual {
            let _ = writeln!(g, "T{start_tool} ; start with nozzle {start_tool}");
            let _ = writeln!(g, "G92 E0");
        }
        // Purge: clear the nozzle with two side-by-side lines along the left edge.
        let purge = purge_lines(printer);
        g.push_str(&purge.gcode);
        let _ = writeln!(g, "G92 E0");
        let mut prime_e = purge.filament_mm;
        let mut purge_seconds = purge.seconds;
        let mut parked = [0.0f32; 2];
        let mut by_tool = [0.0f64; 2];
        by_tool[start_tool as usize] = purge.filament_mm as f64;
        if two {
            // The other nozzle clears itself the same way, is set aside (pulled back and
            // left to cool to its standby temperature), and the first is selected again.
            let other = 1 - start_tool;
            let _ = writeln!(g, "T{other} ; purge nozzle {other} too");
            let _ = writeln!(g, "G92 E0");
            g.push_str(&purge.gcode);
            let park = printer.tool_change_retract.max(0.0);
            if park > 0.0 {
                let f = printer.retract_speed * 60.0;
                let _ = writeln!(
                    g,
                    "G1 E{:.5} F{f:.0} ; set nozzle {other} aside",
                    purge_end_e(&purge.gcode) - park as f64
                );
            }
            parked[other as usize] = park;
            if printer.standby_drop > 0 {
                let standby = temp_of(other).saturating_sub(printer.standby_drop);
                let _ = writeln!(g, "M104 T{other} S{standby} ; standby");
            }
            let _ = writeln!(g, "G92 E0");
            let _ = writeln!(g, "T{start_tool} ; back to nozzle {start_tool}");
            let _ = writeln!(g, "G92 E0");
            prime_e += purge.filament_mm;
            by_tool[other as usize] += purge.filament_mm as f64;
            purge_seconds += purge.seconds + 2.0 * TOOL_CHANGE_SECONDS;
        }

        Self {
            g,
            printer,
            filament,
            s,
            ox,
            oy,
            line_width: result.line_width,
            travel_f: s.travel_speed * 60.0,
            pos: Point2::new(purge.end.x - ox, purge.end.y - oy),
            ext: Extruder::default(),
            prime_e,
            seconds: WARMUP_SECONDS + purge_seconds,
            tools,
            dual,
            used,
            two,
            tool: start_tool,
            retracted: false,
            parked,
            last_path: Vec::new(),
            by_tool,
            accel_now: [None; 2],
            fan_now: 0,
        }
    }

    /// The filament in the selected nozzle: the primary has the main filament, the other
    /// nozzle the second one.
    fn filament_now(&self) -> &Filament {
        match &self.tools.second {
            Some(f) if self.dual && self.tool != self.tools.primary && !self.tools.ditto => f,
            _ => self.filament,
        }
    }

    /// The printing temperature of nozzle `t`.
    fn temp_of(&self, t: u8) -> u32 {
        match &self.tools.second {
            Some(f) if t != self.tools.primary && !self.tools.ditto => f.nozzle_temp,
            _ => self.filament.nozzle_temp,
        }
    }

    /// Pulls the filament back before a travel or layer change: in place, or, with a wipe
    /// set, while moving back along the line just printed.
    fn retract_now(&mut self) {
        let p = self.printer;
        if self.retracted || p.retract_length <= 0.0 {
            return;
        }
        let f = p.retract_speed * 60.0;
        let wipe = self.wipe_points();
        if wipe.is_empty() {
            let e = self.ext.retract(p.retract_length);
            let _ = writeln!(self.g, "G1 E{e:.5} F{f:.0}");
        } else {
            // Spread the retraction over the wipe, so it ends fully retracted.
            let total: f32 = wipe
                .iter()
                .scan(self.pos, |at, q| {
                    let d = at.distance(*q);
                    *at = *q;
                    Some(d)
                })
                .sum::<f32>()
                .max(1e-6);
            let mut at = self.pos;
            for q in wipe {
                let share = p.retract_length * at.distance(q) / total;
                let e = self.ext.retract(share);
                let _ = writeln!(
                    self.g,
                    "G1 X{:.3} Y{:.3} E{e:.5} F{:.0} ; wipe",
                    q.x + self.ox,
                    q.y + self.oy,
                    self.travel_f
                );
                self.seconds += at.distance(q) / self.s.travel_speed;
                at = q;
            }
            self.pos = at;
        }
        self.seconds += p.retract_length / p.retract_speed;
        self.retracted = true;
    }

    /// Where a wipe goes: back along the line just printed, up to the wipe length. Empty
    /// when wiping is off or there is no line to wipe along.
    fn wipe_points(&self) -> Vec<Point2> {
        let mut left = self.printer.wipe_mm;
        let mut out = Vec::new();
        if left <= 0.0 || self.last_path.len() < 2 {
            return out;
        }
        let mut at = self.pos;
        for q in self.last_path.iter().rev().skip(1) {
            let d = at.distance(*q);
            if d >= left {
                let t = left / d;
                out.push(Point2::new(
                    at.x + (q.x - at.x) * t,
                    at.y + (q.y - at.y) * t,
                ));
                return out;
            }
            left -= d;
            out.push(*q);
            at = *q;
        }
        out
    }

    /// Pushes the filament forward again after a retraction.
    fn unretract_now(&mut self) {
        if !self.retracted {
            return;
        }
        let p = self.printer;
        let e = self.ext.unretract(p.retract_length);
        let _ = writeln!(self.g, "G1 E{e:.5} F{:.0}", p.retract_speed * 60.0);
        self.seconds += p.retract_length / p.retract_speed;
        self.retracted = false;
    }

    /// Switches nozzles: the old one is pulled back by the tool-change retraction and
    /// left at its standby temperature; the new one is selected, heated back if it was on
    /// standby, and pushed forward by as much as it was pulled back when set aside.
    fn change_tool(&mut self, tool: u8) {
        let p = self.printer;
        let old = self.tool;
        let f = p.retract_speed * 60.0;
        let already = if self.retracted {
            p.retract_length
        } else {
            0.0
        };
        let park = p.tool_change_retract.max(already);
        if park > already {
            let e = self.ext.retract(park - already);
            let _ = writeln!(self.g, "G1 E{e:.5} F{f:.0} ; set nozzle {old} aside");
        }
        self.parked[old as usize] = park;
        if p.standby_drop > 0 {
            let standby = self.temp_of(old).saturating_sub(p.standby_drop);
            let _ = writeln!(self.g, "M104 T{old} S{standby} ; standby");
        }
        let _ = writeln!(self.g, "T{tool}");
        let _ = writeln!(self.g, "G92 E0");
        if p.standby_drop > 0 {
            let _ = writeln!(
                self.g,
                "M109 T{tool} S{} ; back to printing temperature",
                self.temp_of(tool)
            );
            // About a degree a second to heat back.
            self.seconds += p.standby_drop as f32;
        }
        let prime = self.parked[tool as usize];
        if prime > 0.0 {
            let _ = writeln!(self.g, "G1 E{prime:.5} F{f:.0} ; prime");
        }
        let _ = writeln!(self.g, "G92 E0");
        self.parked[tool as usize] = 0.0;
        self.ext.e = 0.0;
        self.retracted = false;
        self.tool = tool;
        self.seconds += TOOL_CHANGE_SECONDS;
    }

    /// One layer: the layer change, then every path. `seams_below` holds the wall seams
    /// of the layer below and is replaced by this layer's when it has any.
    fn write_layer(&mut self, li: usize, layer: &Layer, seams_below: &mut Vec<Point2>) {
        let spiral = self.begin_layer(li, layer);
        let slowdown = cooling_slowdown(layer, li, self.s);
        let seam = if spiral {
            SeamPosition::Nearest
        } else {
            self.s.seam
        };

        // The prime tower is printed by the nozzle that comes in at this layer's tool
        // change (or by the selected one when there is none), so it isn't in the order.
        let tower: Vec<usize> = (0..layer.paths.len())
            .filter(|&i| layer.paths[i].kind == PathKind::PrimeTower)
            .collect();
        // The ooze shield is printed first, by the nozzle already selected, so it adds no
        // tool change.
        let shield: Vec<usize> = (0..layer.paths.len())
            .filter(|&i| layer.paths[i].kind == PathKind::OozeShield)
            .collect();
        // One nozzle: every path in order. Two: the selected nozzle's paths first, then
        // the other's after a tool change, each group in the slicer's order.
        let order: Vec<(usize, u8)> = if self.two && !self.tools.ditto {
            let tool_of: Vec<u8> = layer
                .paths
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    if tower.contains(&i) || shield.contains(&i) {
                        u8::MAX
                    } else if p.points.is_empty() {
                        self.tool
                    } else {
                        self.tools.tool_for(p)
                    }
                })
                .collect();
            let other = 1 - self.tool;
            [self.tool, other]
                .into_iter()
                .flat_map(|t| {
                    tool_of
                        .iter()
                        .enumerate()
                        .filter(move |(_, &pt)| pt == t)
                        .map(move |(i, _)| (i, t))
                })
                .collect()
        } else {
            (0..layer.paths.len()).map(|i| (i, self.tool)).collect()
        };
        let mut look = LayerLook {
            li,
            seam,
            spiral,
            slowdown,
            last_kind: None,
            seams_here: Vec::new(),
        };
        for &pi in &shield {
            self.print_path(layer, pi, &mut look, seams_below);
        }
        // No tool change on this layer: the tower comes first, with the selected nozzle.
        let changes = order.iter().any(|(_, t)| *t != self.tool);
        let mut tower_done = tower.is_empty();
        if !changes && !tower_done {
            for &pi in &tower {
                self.print_path(layer, pi, &mut look, seams_below);
            }
            tower_done = true;
        }
        for (pi, tool) in order {
            if layer.paths[pi].points.len() < 2 {
                continue;
            }
            if tool != self.tool {
                self.change_tool(tool);
                look.last_kind = None;
                // The incoming nozzle primes on the tower before it prints the part.
                if !tower_done {
                    for &pi in &tower {
                        self.print_path(layer, pi, &mut look, seams_below);
                    }
                    tower_done = true;
                }
            }
            self.print_path(layer, pi, &mut look, seams_below);
        }
        let seams_here = std::mem::take(&mut look.seams_here);
        if !seams_here.is_empty() {
            *seams_below = seams_here;
        }
    }

    /// One path of a layer: its type marker when the type changes, a travel to its start
    /// and the extrusion.
    fn print_path(
        &mut self,
        layer: &Layer,
        pi: usize,
        look: &mut LayerLook,
        seams_below: &[Point2],
    ) {
        let path = &layer.paths[pi];
        if path.points.len() < 2 {
            return;
        }
        if look.last_kind != Some(path.kind) {
            let _ = writeln!(self.g, ";TYPE:{}", path.kind.gcode_type());
            look.last_kind = Some(path.kind);
        }
        let salt = look.li * 7919 + pi;
        let pts = ordered_points(
            path,
            self.pos,
            look.seam,
            salt,
            seams_below,
            &mut look.seams_here,
        );
        self.travel_to(pts[0], layer.z);
        self.set_fan_for(path.kind, look.li);
        let style = PathStyle {
            li: look.li,
            pi,
            kind: path.kind,
            spiral: look.spiral,
            slowdown: look.slowdown,
        };
        self.extrude(layer, &style, pts);
    }

    /// The layer marker, the Z move (skipped while spiralling), and the part fan from
    /// the second layer on. Returns whether this layer is part of the vase spiral.
    fn begin_layer(&mut self, li: usize, layer: &Layer) -> bool {
        let _ = writeln!(self.g, ";LAYER:{li}");
        // Vase mode: above the solid bottom, the wall climbs continuously instead.
        let spiral = self.s.vase_mode && li >= self.s.bottom_layers as usize && li > 0;
        if !spiral {
            if li > 0 && self.printer.retract_layer_change {
                self.retract_now();
            }
            let _ = writeln!(self.g, "G1 Z{:.3} F600", layer.z);
        }
        if li == 1 && self.filament.fan_percent > 0 {
            let _ = writeln!(
                self.g,
                "M106 S{} ; part fan",
                self.filament.fan_percent * 255 / 100
            );
            self.fan_now = self.filament.fan_percent;
        }
        self.seconds += LAYER_CHANGE_SECONDS;
        spiral
    }

    /// The acceleration (mm/s^2) for printing a line of `kind` on layer `li`: the print
    /// setting for it, or the printer's own when it is on Auto or has none.
    fn print_accel(&self, kind: PathKind, li: usize) -> f32 {
        let (a, machine) = (&self.s.accels, self.printer.acceleration);
        if li == 0 {
            return a.accel(AccelFeature::FirstLayer, machine);
        }
        match kind {
            PathKind::OuterWall => a.accel(AccelFeature::OuterWall, machine),
            PathKind::SparseInfill | PathKind::SolidInfill => {
                a.accel(AccelFeature::Infill, machine)
            }
            PathKind::Skirt
            | PathKind::Brim
            | PathKind::InnerWall
            | PathKind::Support
            | PathKind::Ironing
            | PathKind::TopSurface
            | PathKind::Bridge
            | PathKind::Raft
            | PathKind::PrimeTower
            | PathKind::OozeShield => machine,
        }
    }

    /// Bridges print with the bridge fan; everything else with the layer's normal fan
    /// (off on the first layer). Writes `M106` only when the fan changes, so a print
    /// without bridges gets no extra fan commands.
    fn set_fan_for(&mut self, kind: PathKind, li: usize) {
        let want = if kind == PathKind::Bridge {
            self.s.bridge_fan
        } else if li == 0 {
            0
        } else {
            self.filament.fan_percent
        };
        if want == self.fan_now {
            return;
        }
        let note = if kind == PathKind::Bridge {
            "bridge fan"
        } else {
            "part fan"
        };
        let _ = writeln!(self.g, "M106 S{} ; {note}", want * 255 / 100);
        self.fan_now = want;
    }

    /// Writes an acceleration change for printing moves (or travels with `travel`), only
    /// when it differs from the last one written. Nothing is written while every
    /// acceleration setting is on Auto, or for X3G printers (Sailfish has no such command).
    /// Marlin and Prusa firmware: `M204 P` (printing) and `M204 T` (travel). Klipper has
    /// one acceleration for all moves: `SET_VELOCITY_LIMIT ACCEL=`.
    fn set_accel(&mut self, accel: f32, travel: bool) {
        if !self.s.accels.any_set() || self.printer.x3g {
            return;
        }
        let slot = if self.printer.klipper {
            0
        } else {
            travel as usize
        };
        if self.accel_now[slot] == Some(accel) {
            return;
        }
        self.accel_now[slot] = Some(accel);
        if self.printer.klipper {
            let _ = writeln!(self.g, "SET_VELOCITY_LIMIT ACCEL={accel:.0}");
        } else {
            let which = if travel { "T" } else { "P" };
            let _ = writeln!(self.g, "M204 {which}{accel:.0}");
        }
    }

    /// Moves to `to` (retracting and z-hopping first on long travels), then undoes any
    /// retraction so the nozzle is ready to print at layer height `z`.
    fn travel_to(&mut self, to: Point2, z: f32) {
        let p = self.printer;
        let travel = self.pos.distance(to);
        let mut hopped = false;
        if travel > 0.01 {
            let retract = travel > p.retract_min_travel && p.retract_length > 0.0;
            if retract {
                self.retract_now();
                if self.s.z_hop > 0.0 {
                    let _ = writeln!(self.g, "G1 Z{:.3} F600 ; z-hop", z + self.s.z_hop);
                    self.seconds += 2.0 * self.s.z_hop / 10.0;
                    hopped = true;
                }
            }
            // A wipe may have moved the nozzle: travel from where it is now.
            let accel = self.s.accels.accel(AccelFeature::Travel, p.acceleration);
            self.set_accel(accel, true);
            let _ = writeln!(
                self.g,
                "G0 X{:.3} Y{:.3} F{:.0}",
                to.x + self.ox,
                to.y + self.oy,
                self.travel_f
            );
            self.seconds += motion::line_time(&[self.pos, to], self.s.travel_speed, accel);
        }
        if hopped {
            let _ = writeln!(self.g, "G1 Z{:.3} F600", z);
        }
        // The next line starts un-retracted (after a travel or layer-change retraction).
        self.unretract_now();
    }

    /// Prints along `pts`: one G1 per segment, with a rising Z on vase-spiral walls and
    /// a random lift on fuzzy top lines. Leaves the nozzle at the last point.
    fn extrude(&mut self, layer: &Layer, style: &PathStyle, mut pts: Vec<Point2>) {
        let s = self.s;
        let (flow, speed) = match style.kind {
            PathKind::Ironing => (IRONING_FLOW, IRONING_SPEED),
            PathKind::Bridge => (
                s.bridge_flow / 100.0,
                (speed_for(PathKind::Bridge, style.li, s) * style.slowdown).max(MIN_PRINT_SPEED),
            ),
            kind => (
                1.0,
                (speed_for(kind, style.li, s) * style.slowdown).max(MIN_PRINT_SPEED),
            ),
        };
        let accel = self.print_accel(style.kind, style.li);
        self.set_accel(accel, false);
        self.seconds += motion::line_time(&pts, speed, accel);
        let climb = style.spiral && style.kind == PathKind::OuterWall;
        // Fuzzy top: short steps, each lifted by a random 0..amount above the layer.
        let fuzzy = s.fuzzy_top && s.fuzzy_top_mm > 0.0 && style.kind == PathKind::TopSurface;
        if fuzzy {
            pts = resample(&pts, FUZZY_TOP_STEP_MM);
        }
        let total_len = path_length(&pts, false).max(1e-6);
        let mut done_len = 0.0;
        let mut first = true;
        for (wi, w) in pts.windows(2).enumerate() {
            let len = w[0].distance(w[1]);
            if len < 1e-4 {
                continue;
            }
            done_len += len;
            let de = filament_for(
                len,
                self.line_width,
                layer.height,
                self.filament_now().diameter,
            ) * flow;
            let e = self.ext.extrude(de);
            self.by_tool[self.tool as usize] += de as f64;
            let z = if climb {
                format!(
                    " Z{:.3}",
                    layer.z - layer.height + layer.height * done_len / total_len
                )
            } else if fuzzy {
                let lift = fuzzy_unit(style.li, style.pi, wi) * s.fuzzy_top_mm;
                format!(" Z{:.3}", layer.z + lift)
            } else {
                String::new()
            };
            let feed = if first {
                format!(" F{:.0}", speed * 60.0)
            } else {
                String::new()
            };
            first = false;
            let _ = writeln!(
                self.g,
                "G1 X{:.3} Y{:.3}{z} E{:.5}{feed}",
                w[1].x + self.ox,
                w[1].y + self.oy,
                e
            );
        }
        if fuzzy {
            // Back to the layer height before anything else moves.
            let _ = writeln!(self.g, "G1 Z{:.3} F600", layer.z);
        }
        self.pos = *pts.last().unwrap();
        self.last_path = pts;
    }

    /// The end sequence (retract, lift, present the print, heaters and motors off), the
    /// time and filament summary, and the settings block.
    fn finish(mut self, result: &SliceResult) -> GcodeOutput {
        let (printer, filament, g) = (self.printer, self.filament, &mut self.g);
        let top = result.layers.last().map(|l| l.z).unwrap_or(0.0);
        let _ = writeln!(g, ";END");
        let _ = writeln!(g, "G1 E{:.5} F2400 ; retract", self.ext.e - 2.0);
        let _ = writeln!(g, "G0 Z{:.2} F600", (top + 10.0).min(printer.max_z));
        let _ = writeln!(
            g,
            "G0 X5 Y{:.0} F{:.0} ; present the print",
            printer.bed_y - 10.0,
            self.travel_f
        );
        if self.dual {
            for t in [0u8, 1].into_iter().filter(|t| self.used[*t as usize]) {
                let _ = writeln!(g, "M104 T{t} S0");
            }
        } else {
            let _ = writeln!(g, "M104 S0");
        }
        let _ = writeln!(g, "M140 S0");
        let _ = writeln!(g, "M107");
        let _ = writeln!(g, "M84 ; motors off");

        let total_e = (self.prime_e as f64 + self.ext.extruded) as f32;
        let seconds = self.seconds;
        let volume_cm3 =
            total_e * std::f32::consts::PI * (filament.diameter / 2.0).powi(2) / 1000.0;
        let grams = volume_cm3 * filament.density;
        // A two-nozzle print: each nozzle's share with its own filament.
        let per_nozzle = self.two.then(|| {
            let second = self.tools.second.as_ref().unwrap_or(filament);
            std::array::from_fn(|t| {
                let f = if t as u8 == self.tools.primary || self.tools.ditto {
                    filament
                } else {
                    second
                };
                let mm = self.by_tool[t] as f32;
                let g = mm * std::f32::consts::PI * (f.diameter / 2.0).powi(2) / 1000.0 * f.density;
                (mm, g)
            })
        });
        let (grams, cost) = match per_nozzle {
            Some(n) => {
                let second = self.tools.second.as_ref().unwrap_or(filament);
                let price = |t: usize| {
                    if t as u8 == self.tools.primary {
                        filament.price_per_kg
                    } else {
                        second.price_per_kg
                    }
                };
                (
                    n[0].1 + n[1].1,
                    n[0].1 / 1000.0 * price(0) + n[1].1 / 1000.0 * price(1),
                )
            }
            None => (grams, grams / 1000.0 * filament.price_per_kg),
        };
        let estimate = Estimate {
            seconds,
            filament_mm: total_e,
            grams,
            cost,
            layers: result.layers.len(),
            per_nozzle,
        };
        let _ = writeln!(g, "; estimated time: {}", estimate.time_text());
        let _ = writeln!(
            g,
            "; filament used: {:.2} m, {:.1} g",
            total_e / 1000.0,
            grams
        );
        if let Some(n) = per_nozzle {
            for (t, (mm, grams)) in n.iter().enumerate() {
                let _ = writeln!(
                    g,
                    "; filament used by nozzle {t}: {:.2} m, {grams:.1} g",
                    mm / 1000.0
                );
            }
        }
        g.push_str(&config_block(filament, self.s, printer));
        GcodeOutput {
            text: self.g,
            estimate,
        }
    }
}

/// What stays the same for every path of a layer, and what the layer collects.
struct LayerLook {
    li: usize,
    seam: SeamPosition,
    spiral: bool,
    slowdown: f32,
    /// The path type last marked with `;TYPE:`.
    last_kind: Option<PathKind>,
    /// Where this layer's wall loops started.
    seams_here: Vec<Point2>,
}

/// What `Writer::extrude` needs to know about the path it is printing.
struct PathStyle {
    /// Layer and path index (they seed the fuzzy-top lifts).
    li: usize,
    pi: usize,
    kind: PathKind,
    /// The layer is part of the vase spiral.
    spiral: bool,
    /// Cooling factor from `cooling_slowdown`.
    slowdown: f32,
}

/// Cooling: the factor (at most 1) that stretches a layer which would otherwise finish
/// in less than the minimum layer time.
fn cooling_slowdown(layer: &Layer, li: usize, s: &PrintSettings) -> f32 {
    let layer_time: f32 = layer
        .paths
        .iter()
        .map(|p| path_length(&p.points, p.closed) / speed_for(p.kind, li, s))
        .sum();
    if s.min_layer_time > 0.0 && layer_time > 0.0 {
        (layer_time / s.min_layer_time).min(1.0)
    } else {
        1.0
    }
}

/// The points of `path` in printing order. A closed loop is rotated to start at its
/// seam and repeats that point at the end; wall seams are recorded in `seams_here`, and
/// Aligned seams follow `seams_below`.
fn ordered_points(
    path: &ToolPath,
    nozzle: Point2,
    seam: SeamPosition,
    salt: usize,
    seams_below: &[Point2],
    seams_here: &mut Vec<Point2>,
) -> Vec<Point2> {
    let mut pts = path.points.clone();
    if path.closed {
        let is_wall = matches!(path.kind, PathKind::OuterWall | PathKind::InnerWall);
        let below: &[Point2] = if is_wall { seams_below } else { &[] };
        let start = seam_start_aligned(&pts, nozzle, seam, salt, below);
        pts.rotate_left(start);
        if is_wall {
            seams_here.push(pts[0]);
        }
        pts.push(pts[0]);
    }
    pts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Vec3;
    use crate::model::{Mesh, PlacedObject, Triangle};
    use crate::profiles::SeamPosition;

    #[test]
    fn filament_formula_matches_volume() {
        // 100 mm of 0.4 x 0.2 line = 8 mm³ → 8 / 2.405 mm² ≈ 3.326 mm of 1.75 filament.
        let e = filament_for(100.0, 0.4, 0.2, 1.75);
        assert!((e - 3.326).abs() < 0.01);
    }

    #[test]
    fn extruder_keeps_short_moves_exact_late_in_a_long_print() {
        let mut ext = Extruder {
            e: 330_000.0,
            extruded: 330_000.0,
        };
        let before = ext.e;
        let after = ext.extrude(0.0175);
        assert!(
            ((after - before) - 0.0175).abs() < 1e-6,
            "{}",
            after - before
        );
        assert_eq!(ext.retract(0.8), after - 0.8f32 as f64);
    }

    /// Every Z written while printing a layer, by layer number.
    fn z_values_by_layer(text: &str) -> Vec<(usize, Vec<f32>)> {
        let mut out = Vec::new();
        for chunk in text.split(";LAYER:").skip(1) {
            let n: usize = chunk.lines().next().unwrap().trim().parse().unwrap();
            let zs = chunk
                .lines()
                .filter(|l| l.starts_with("G1 X") && l.contains(" E"))
                .filter_map(|l| {
                    l.split_whitespace()
                        .find(|w| w.starts_with('Z'))
                        .map(|w| w[1..].parse::<f32>().unwrap())
                })
                .collect();
            out.push((n, zs));
        }
        out
    }

    #[test]
    fn fuzzy_top_lifts_only_the_visible_top_by_up_to_its_amount() {
        let tris = PlacedObject::new("c", Mesh::cube(Vec3::new(20.0, 20.0, 4.0))).world_triangles();
        let plain = PrintSettings::default();
        let fuzzy = PrintSettings {
            fuzzy_top: true,
            ..PrintSettings::default()
        };
        assert_eq!(fuzzy.fuzzy_top_mm, 0.1);
        let p = &Printer::presets()[0];
        let f = &Filament::presets()[0];
        let r = crate::slicer::slice(&tris, &fuzzy, 0.42, &|_| {});
        let top_layer = r.layers.len() - 1;
        // Only the last layer has top-surface lines.
        for (i, l) in r.layers.iter().enumerate() {
            let has = l.paths.iter().any(|p| p.kind == PathKind::TopSurface);
            assert_eq!(has, i == top_layer, "layer {i}");
        }
        let out = generate(&r, p, f, &fuzzy).text;
        let top_z = r.layers[top_layer].z;
        for (n, zs) in z_values_by_layer(&out) {
            if n == top_layer {
                assert!(
                    zs.len() > 200,
                    "the top gets many small lifts: {}",
                    zs.len()
                );
                assert!(
                    zs.iter()
                        .all(|z| *z >= top_z - 1e-3 && *z <= top_z + 0.1 + 1e-3),
                    "{zs:?}"
                );
                let spread = zs.iter().cloned().fold(f32::MIN, f32::max)
                    - zs.iter().cloned().fold(f32::MAX, f32::min);
                assert!(spread > 0.05, "the heights really vary: {spread}");
            } else {
                assert!(zs.is_empty(), "layer {n} moves stay flat");
            }
        }
        // Back at the layer height after each fuzzy line, and the same file every time.
        assert!(out.contains(&format!("G1 Z{top_z:.3} F600")));
        assert_eq!(out, generate(&r, p, f, &fuzzy).text);
        // Off: no Z changes on the top layer.
        let r2 = crate::slicer::slice(&tris, &plain, 0.42, &|_| {});
        let out2 = generate(&r2, p, f, &plain).text;
        assert!(z_values_by_layer(&out2).iter().all(|(_, zs)| zs.is_empty()));
    }

    #[test]
    fn fuzzy_top_turns_ironing_off() {
        let tris = PlacedObject::new("c", Mesh::cube(Vec3::new(20.0, 20.0, 2.0))).world_triangles();
        let s = PrintSettings {
            ironing: true,
            fuzzy_top: true,
            ..PrintSettings::default()
        };
        let r = crate::slicer::slice(&tris, &s, 0.42, &|_| {});
        assert!(!r
            .layers
            .iter()
            .flat_map(|l| &l.paths)
            .any(|p| p.kind == PathKind::Ironing));
    }

    #[test]
    fn resample_keeps_corners_and_limits_step() {
        let pts = [
            Point2::new(0.0, 0.0),
            Point2::new(2.0, 0.0),
            Point2::new(2.0, 1.0),
        ];
        let r = resample(&pts, 0.5);
        assert_eq!(r.first(), Some(&pts[0]));
        assert_eq!(r.last(), Some(&pts[2]));
        assert!(r.contains(&pts[1]));
        assert!(r.windows(2).all(|w| w[0].distance(w[1]) <= 0.5 + 1e-4));
        assert!((0..1000).all(|i| (0.0..=1.0).contains(&fuzzy_unit(3, 7, i))));
    }

    #[test]
    fn heat_waits_never_wait_for_cooling() {
        let tris = PlacedObject::new("c", Mesh::cube(Vec3::new(10.0, 10.0, 2.0))).world_triangles();
        let s = PrintSettings::default();
        let r = crate::slicer::slice(&tris, &s, 0.42, &|_| {});
        let f = &Filament::presets()[0];
        // Klipper (the KE): set the targets, then wait only for heating up.
        let ke = Printer::presets()
            .into_iter()
            .find(|p| p.name.contains("KE"))
            .unwrap();
        assert!(ke.klipper);
        let out = generate(&r, &ke, f, &s).text;
        let start = &out[..out.find(";LAYER:0").unwrap()];
        let codes: Vec<&str> = start.lines().filter(|l| !l.starts_with(';')).collect();
        assert!(
            !codes
                .iter()
                .any(|l| l.starts_with("M109") || l.starts_with("M190")),
            "no both-ways waits"
        );
        assert!(start.contains("TEMPERATURE_WAIT SENSOR=heater_bed MINIMUM=59"));
        assert!(start.contains("TEMPERATURE_WAIT SENSOR=extruder MINIMUM=209"));
        assert!(!start.contains("MAXIMUM"));
        // Marlin / Prusa firmware: M109 S and M190 S already wait only while heating.
        let marlin = Printer::presets()
            .into_iter()
            .find(|p| p.name.contains("MK4S"))
            .unwrap();
        assert!(!marlin.klipper);
        let out = generate(&r, &marlin, f, &s).text;
        assert!(out.contains("M190 S60") && out.contains("M109 S210"));
        assert!(
            !out.contains("M109 R") && !out.contains("M190 R"),
            "the R form would also wait for cooling"
        );
    }

    #[test]
    fn purge_pushes_the_full_amount_in_two_lines_on_the_left() {
        let ke = Printer::presets()
            .into_iter()
            .find(|p| p.name.contains("KE"))
            .unwrap();
        assert_eq!(ke.purge_mm, 30.0);
        let p = purge_lines(&ke);
        let moves: Vec<&str> = p.gcode.lines().filter(|l| l.contains(" E")).collect();
        assert_eq!(moves.len(), 3, "a push in place and two lines: {}", p.gcode);
        // The last extrusion reaches the whole purge amount (absolute E).
        let last_e: f32 = moves[2]
            .split_whitespace()
            .find(|w| w.starts_with('E'))
            .unwrap()[1..]
            .parse()
            .unwrap();
        assert!((last_e - 30.0).abs() < 0.01);
        assert!((p.filament_mm - 30.0).abs() < 1e-3);
        // Both lines run along the left edge, 180 mm long on a 220 mm plate.
        for l in &moves[1..] {
            let x: f32 = l.split_whitespace().nth(1).unwrap()[1..].parse().unwrap();
            assert!(x < 3.0);
        }
        assert!(moves[1].contains("Y200.0") && moves[2].contains("Y20.0"));
        // About three times what the old single line pushed (about 9 mm).
        let old = filament_for(120.0, 0.42 * 1.5, 0.3, 1.75);
        assert!(p.filament_mm > old * 3.0);
        // The amount follows the printer setting.
        let mut more = ke.clone();
        more.purge_mm = 50.0;
        assert!((purge_lines(&more).filament_mm - 50.0).abs() < 1e-3);
    }

    #[test]
    fn job_temperatures_are_in_the_file_and_never_zero_before_printing() {
        let tris = PlacedObject::new("c", Mesh::cube(Vec3::new(10.0, 10.0, 2.0))).world_triangles();
        let s = PrintSettings::default();
        let r = crate::slicer::slice(&tris, &s, 0.42, &|_| {});
        let petg = Filament::presets()
            .into_iter()
            .find(|f| f.name == "PETG")
            .unwrap();
        let out = generate(&r, &Printer::presets()[0], &petg, &s).text;
        let first_layer = out.find(";LAYER:0").unwrap();
        let header = &out[..first_layer];
        for needle in [
            "; nozzle_temperature_initial_layer = 240",
            "; bed_temperature_initial_layer_single = 80",
            ";EXTRUDER_TRAIN.0.INITIAL_TEMPERATURE:240",
            ";BUILD_PLATE.INITIAL_TEMPERATURE:80",
            "; filament_type = PETG",
        ] {
            assert!(header.contains(needle), "missing {needle}");
        }
        let footer = &out[out.find("; CONFIG_BLOCK_START").unwrap()..];
        assert!(footer.contains("; first_layer_temperature = 240"));
        assert!(footer.contains("; first_layer_bed_temperature = 80"));
        // The nozzle is only ever set to the job temperature before the print starts.
        assert!(!header
            .lines()
            .any(|l| l.starts_with("M104 S0") || l.starts_with("M109 S0")));
        assert!(header.contains("M104 S240") && header.contains("M109 S240"));
    }

    #[test]
    fn cube_gcode_is_plausible() {
        let tris =
            PlacedObject::new("c", Mesh::cube(Vec3::new(20.0, 20.0, 20.0))).world_triangles();
        let s = PrintSettings::default();
        let r = crate::slicer::slice(&tris, &s, 0.42, &|_| {});
        let printer = &Printer::presets()[0];
        let out = generate(&r, printer, &Filament::presets()[0], &s);
        assert!(out.text.contains("M109 S210"));
        assert!(out.text.contains(";LAYER:99"));
        // A 20 mm cube at 15 % infill weighs a few grams and takes well under 2 hours.
        assert!(
            out.estimate.grams > 2.0 && out.estimate.grams < 9.0,
            "{}",
            out.estimate.grams
        );
        assert!(
            out.estimate.seconds > 600.0 && out.estimate.seconds < 7200.0,
            "{}",
            out.estimate.seconds
        );
        // Everything stays on the plate.
        for line in out
            .text
            .lines()
            .filter(|l| l.starts_with("G1 X") || l.starts_with("G0 X"))
        {
            let x: f32 = line.split_whitespace().nth(1).unwrap()[1..]
                .parse()
                .unwrap();
            assert!((0.0..=printer.bed_x).contains(&x));
        }
    }

    fn sliced_cube(s: &PrintSettings) -> (SliceResult, GcodeOutput) {
        let tris =
            PlacedObject::new("c", Mesh::cube(Vec3::new(20.0, 20.0, 10.0))).world_triangles();
        let r = crate::slicer::slice(&tris, s, 0.42, &|_| {});
        let out = generate(&r, &Printer::presets()[0], &Filament::presets()[0], s);
        (r, out)
    }

    #[test]
    fn ironing_only_on_the_top_layer() {
        let s = PrintSettings {
            ironing: true,
            ..PrintSettings::default()
        };
        let (r, out) = sliced_cube(&s);
        let ironed: Vec<usize> = (0..r.layers.len())
            .filter(|&i| {
                r.layers[i]
                    .paths
                    .iter()
                    .any(|p| p.kind == PathKind::Ironing)
            })
            .collect();
        assert_eq!(ironed, vec![r.layers.len() - 1]);
        assert!(out.text.contains(";TYPE:IRONING"));
    }

    #[test]
    fn vase_mode_climbs_in_one_spiral() {
        let s = PrintSettings {
            vase_mode: true,
            ..PrintSettings::default()
        };
        let (r, out) = sliced_cube(&s);
        let mid = &r.layers[20];
        assert!(mid.paths.iter().all(|p| p.kind == PathKind::OuterWall));
        assert_eq!(mid.paths.len(), 1);
        // Inside a spiral layer, moves carry a rising Z.
        let layer = out.text.split(";LAYER:20\n").nth(1).unwrap();
        assert!(
            layer
                .lines()
                .filter(|l| l.starts_with("G1 X") && l.contains(" Z"))
                .count()
                > 3
        );
    }

    #[test]
    fn aligned_seams_line_up_from_layer_to_layer() {
        // A cylinder: every loop has many points, so a careless seam would wander.
        let tris = PlacedObject::new("cyl", Mesh::cylinder(10.0, 10.0, 6.0, 64)).world_triangles();
        let s = PrintSettings::default();
        assert_eq!(s.seam, SeamPosition::Aligned, "Aligned is the default");
        let r = crate::slicer::slice(&tris, &s, 0.42, &|_| {});
        let out = generate(&r, &Printer::presets()[0], &Filament::presets()[0], &s);
        // The first move of every outer wall: all within a couple of millimetres.
        let starts: Vec<(f32, f32)> = out
            .text
            .split(";TYPE:WALL-OUTER\n")
            .skip(1)
            .filter_map(|chunk| {
                let line = chunk
                    .lines()
                    .find(|l| l.starts_with("G0 X") || l.starts_with("G1 X"))?;
                let mut it = line.split_whitespace().skip(1);
                let x: f32 = it.next()?[1..].parse().ok()?;
                let y: f32 = it.next()?[1..].parse().ok()?;
                Some((x, y))
            })
            .collect();
        assert!(starts.len() > 20);
        let (x0, y0) = starts[1];
        for (x, y) in &starts[1..] {
            assert!(
                ((x - x0).powi(2) + (y - y0).powi(2)).sqrt() < 2.0,
                "seam wandered to {x},{y} from {x0},{y0}"
            );
        }
    }

    #[test]
    fn rear_seam_starts_loops_at_the_back() {
        let s = PrintSettings {
            seam: SeamPosition::Rear,
            ..PrintSettings::default()
        };
        let pts = [
            Point2::new(0.0, 0.0),
            Point2::new(5.0, 0.0),
            Point2::new(5.0, 9.0),
            Point2::new(0.0, 4.0),
        ];
        assert_eq!(seam_start(&pts, Point2::new(0.0, 0.0), s.seam, 0), 2);
        assert_eq!(
            seam_start(&pts, Point2::new(0.0, 0.0), SeamPosition::Nearest, 0),
            0
        );
    }

    #[test]
    fn z_hop_lifts_on_retraction() {
        let s = PrintSettings {
            z_hop: 0.4,
            ..PrintSettings::default()
        };
        let (_, out) = sliced_cube(&s);
        assert!(out.text.contains("; z-hop"));
    }

    #[test]
    fn a_speed_set_below_the_minimum_prints_at_15_mm_s() {
        let tris = PlacedObject::new("c", Mesh::cube(Vec3::new(20.0, 20.0, 2.0))).world_triangles();
        let slow = PrintSettings {
            print_speed: 10.0,
            first_layer_speed: 5.0,
            min_layer_time: 0.0,
            ..PrintSettings::default()
        };
        let r = crate::slicer::slice(&tris, &slow, 0.42, &|_| {});
        let out = generate(&r, &Printer::presets()[0], &Filament::presets()[0], &slow);
        let feeds: Vec<f32> = out
            .text
            .lines()
            .filter(|l| l.starts_with("G1 X"))
            .filter_map(|l| l.rsplit_once(" F").and_then(|(_, f)| f.parse().ok()))
            .collect();
        assert!(!feeds.is_empty());
        assert!(
            feeds.iter().all(|f| *f >= MIN_PRINT_SPEED * 60.0),
            "{feeds:?}"
        );
    }

    #[test]
    fn short_layers_are_slowed_for_cooling() {
        let tris = PlacedObject::new("pin", Mesh::cylinder(2.0, 2.0, 10.0, 24)).world_triangles();
        let fast = PrintSettings {
            min_layer_time: 0.0,
            ..PrintSettings::default()
        };
        let cool = PrintSettings {
            min_layer_time: 15.0,
            ..PrintSettings::default()
        };
        let p = &Printer::presets()[0];
        let f = &Filament::presets()[0];
        let r = crate::slicer::slice(&tris, &cool, 0.42, &|_| {});
        let fast_out = generate(&r, p, f, &fast);
        let cool_out = generate(&r, p, f, &cool);
        assert!(cool_out.estimate.seconds > fast_out.estimate.seconds);
        // A tiny pin hits the slowest allowed speed (15 mm/s = F900), never below it.
        let slow_moves = |t: &str| {
            t.lines()
                .filter(|l| l.starts_with("G1 X") && l.ends_with(" F900"))
                .count()
        };
        assert!(slow_moves(&cool_out.text) > slow_moves(&fast_out.text) + 10);
        assert!(!cool_out
            .text
            .lines()
            .any(|l| l.starts_with("G1 X") && (l.ends_with(" F600") || l.ends_with(" F300"))));
    }

    #[test]
    fn test_feature_speeds_auto_follows_print_speed() {
        let s = PrintSettings::default();
        assert_eq!(speed_for(PathKind::OuterWall, 3, &s), s.print_speed * 0.5);
        for kind in [
            PathKind::InnerWall,
            PathKind::SparseInfill,
            PathKind::SolidInfill,
            PathKind::TopSurface,
            PathKind::Support,
            PathKind::Skirt,
        ] {
            assert_eq!(speed_for(kind, 3, &s), s.print_speed, "{kind:?}");
        }
    }

    #[test]
    fn test_feature_speeds_each_feature_uses_its_own_speed() {
        let mut s = PrintSettings::default();
        for (i, f) in SpeedFeature::ALL.into_iter().enumerate() {
            s.speeds.set(f, Some(20.0 + 10.0 * i as f32));
        }
        for (i, f) in SpeedFeature::ALL.into_iter().enumerate() {
            let kind = PathKind::ALL
                .into_iter()
                .find(|k| speed_feature(*k) == Some(f))
                .expect("a path kind for each feature");
            assert_eq!(speed_for(kind, 3, &s), 20.0 + 10.0 * i as f32, "{f:?}");
        }
        // Lines without a speed of their own, and the whole first layer, are unchanged.
        assert_eq!(speed_for(PathKind::Skirt, 3, &s), s.print_speed);
        assert_eq!(speed_for(PathKind::OuterWall, 0, &s), s.first_layer_speed);
    }

    /// The feed rates (mm/min) of extruding moves after the first layer, by `;TYPE:`.
    fn feeds_by_type(text: &str) -> std::collections::HashMap<String, Vec<f32>> {
        let mut by_type: std::collections::HashMap<String, Vec<f32>> = Default::default();
        let (mut layer, mut kind) = (0usize, String::new());
        for l in text.lines() {
            if let Some(n) = l.strip_prefix(";LAYER:") {
                layer = n.trim().parse().unwrap_or(0);
            } else if let Some(k) = l.strip_prefix(";TYPE:") {
                kind = k.to_string();
            } else if layer > 0 && l.starts_with("G1 X") && l.contains(" E") {
                if let Some(f) = l.rsplit_once(" F").and_then(|(_, f)| f.parse().ok()) {
                    by_type.entry(kind.clone()).or_default().push(f);
                }
            }
        }
        by_type
    }

    #[test]
    fn test_feature_speeds_reach_the_gcode() {
        let mut s = PrintSettings {
            min_layer_time: 0.0,
            ..PrintSettings::default()
        };
        s.speeds.set(SpeedFeature::OuterWall, Some(25.0));
        s.speeds.set(SpeedFeature::InnerWall, Some(45.0));
        s.speeds.set(SpeedFeature::SparseInfill, Some(90.0));
        let (_, out) = sliced_cube(&s);
        let feeds = feeds_by_type(&out.text);
        let all_at = |t: &str, f: f32| feeds[t].iter().all(|x| *x == f);
        assert!(all_at("WALL-OUTER", 1500.0), "{:?}", feeds["WALL-OUTER"]);
        assert!(all_at("WALL-INNER", 2700.0), "{:?}", feeds["WALL-INNER"]);
        assert!(all_at("FILL", 5400.0), "{:?}", feeds["FILL"]);
        // Solid infill was left on Auto: it still follows the print speed.
        assert!(all_at("SKIN", s.print_speed * 60.0), "{:?}", feeds["SKIN"]);
    }

    #[test]
    fn test_feature_speeds_default_matches_the_old_single_speed() {
        let s = PrintSettings {
            min_layer_time: 0.0,
            ..PrintSettings::default()
        };
        let (_, out) = sliced_cube(&s);
        let feeds = feeds_by_type(&out.text);
        let p = s.print_speed * 60.0;
        assert!(feeds["WALL-OUTER"].iter().all(|x| *x == p * 0.5));
        for t in ["WALL-INNER", "FILL", "SKIN"] {
            assert!(feeds[t].iter().all(|x| *x == p), "{t}: {:?}", feeds[t]);
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3 * b.abs().max(1.0)
    }

    #[test]
    fn test_motion_straight_line_is_a_trapezoid() {
        let line = [Point2::new(0.0, 0.0), Point2::new(100.0, 0.0)];
        // 1.25 mm to reach 50 mm/s at 1000 mm/s^2, the same to stop: 0.05 + 0.05 + 97.5 / 50.
        assert!(close(motion::line_time(&line, 50.0, 1000.0), 2.05));
        // Too short to reach cruise: up to sqrt(a * len) and back down.
        let short = [Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)];
        let peak = 1000.0f32.sqrt();
        assert!(close(
            motion::line_time(&short, 100.0, 1000.0),
            2.0 * peak / 1000.0
        ));
        // No acceleration: length / speed.
        assert!(close(motion::line_time(&line, 50.0, 0.0), 2.0));
    }

    #[test]
    fn test_motion_corners_cost_time_and_straight_joins_do_not() {
        let p = Point2::new;
        let straight = motion::line_time(&[p(0.0, 0.0), p(40.0, 0.0)], 60.0, 2000.0);
        let split = motion::line_time(&[p(0.0, 0.0), p(20.0, 0.0), p(40.0, 0.0)], 60.0, 2000.0);
        assert!(close(split, straight), "{split} vs {straight}");
        let corner = motion::line_time(&[p(0.0, 0.0), p(20.0, 0.0), p(20.0, 20.0)], 60.0, 2000.0);
        assert!(corner > straight * 1.02, "{corner} vs {straight}");
        // A gentle bend costs less than a square corner.
        let bend = motion::line_time(&[p(0.0, 0.0), p(20.0, 0.0), p(39.0, 6.0)], 60.0, 2000.0);
        assert!(bend < corner);
    }

    /// The acceleration commands in a G-code text, in order.
    fn accel_lines(text: &str) -> Vec<&str> {
        text.lines()
            .filter(|l| l.starts_with("M204") || l.starts_with("SET_VELOCITY_LIMIT"))
            .collect()
    }

    fn accel_settings() -> PrintSettings {
        let mut s = PrintSettings::default();
        s.accels.set(AccelFeature::OuterWall, Some(500.0));
        s.accels.set(AccelFeature::Infill, Some(3000.0));
        s.accels.set(AccelFeature::Travel, Some(2500.0));
        s
    }

    fn sliced_cube_on(s: &PrintSettings, printer: &Printer) -> GcodeOutput {
        let tris = PlacedObject::new("c", Mesh::cube(Vec3::new(20.0, 20.0, 4.0))).world_triangles();
        let r = crate::slicer::slice(&tris, s, 0.42, &|_| {});
        generate(&r, printer, &Filament::presets()[0], s)
    }

    #[test]
    fn test_accels_on_auto_write_nothing() {
        for printer in Printer::presets() {
            let out = sliced_cube_on(&PrintSettings::default(), &printer);
            assert!(accel_lines(&out.text).is_empty(), "{}", printer.name);
        }
    }

    #[test]
    fn test_accels_marlin_writes_m204_only_on_change() {
        let s = accel_settings();
        let printer = &Printer::presets()[0];
        assert!(!printer.klipper && !printer.x3g);
        let out = sliced_cube_on(&s, printer);
        let lines = accel_lines(&out.text);
        for want in ["M204 P500", "M204 P3000", "M204 T2500", "M204 P1000"] {
            assert!(lines.contains(&want), "{want} missing from {lines:?}");
        }
        assert!(lines.iter().all(|l| !l.starts_with("SET_VELOCITY_LIMIT")));
        // The travel acceleration never changes, so it is written once.
        assert_eq!(lines.iter().filter(|l| l.starts_with("M204 T")).count(), 1);
        // Printing changes alternate: never the same value twice in a row.
        let print: Vec<_> = lines.iter().filter(|l| l.starts_with("M204 P")).collect();
        assert!(print.windows(2).all(|w| w[0] != w[1]), "{print:?}");
        // Every printing move after the first layer runs at its feature's acceleration.
        let (mut layer, mut kind, mut accel) = (0usize, "", "");
        let mut checked = 0;
        for l in out.text.lines() {
            if let Some(n) = l.strip_prefix(";LAYER:") {
                layer = n.trim().parse().unwrap_or(0);
            } else if let Some(k) = l.strip_prefix(";TYPE:") {
                kind = k;
            } else if let Some(a) = l.strip_prefix("M204 P") {
                accel = a;
            } else if layer > 0 && l.starts_with("G1 X") && l.contains(" E") {
                let want = match kind {
                    "WALL-OUTER" => "500",
                    "FILL" | "SKIN" => "3000",
                    _ => "1000",
                };
                assert_eq!(accel, want, "layer {layer} {kind}: {l}");
                checked += 1;
            }
        }
        assert!(checked > 100);
    }

    #[test]
    fn test_accels_first_layer_uses_its_own_setting() {
        let mut s = accel_settings();
        s.accels.set(AccelFeature::FirstLayer, Some(700.0));
        let out = sliced_cube_on(&s, &Printer::presets()[0]);
        let first = &out.text[..out.text.find(";LAYER:1\n").unwrap()];
        let firsts = accel_lines(first);
        assert!(firsts.contains(&"M204 P700"), "{firsts:?}");
        assert!(!firsts.contains(&"M204 P500") && !firsts.contains(&"M204 P3000"));
    }

    #[test]
    fn test_accels_klipper_uses_set_velocity_limit_and_x3g_none() {
        let s = accel_settings();
        let presets = Printer::presets();
        let ke = presets.iter().find(|p| p.klipper).unwrap();
        let ke_out = sliced_cube_on(&s, ke);
        let lines = accel_lines(&ke_out.text);
        assert!(!lines.is_empty());
        assert!(lines
            .iter()
            .all(|l| l.starts_with("SET_VELOCITY_LIMIT ACCEL=")));
        assert!(lines.contains(&"SET_VELOCITY_LIMIT ACCEL=2500"));
        assert!(lines.windows(2).all(|w| w[0] != w[1]), "{lines:?}");
        let sailfish = presets.iter().find(|p| p.x3g).unwrap();
        assert!(accel_lines(&sliced_cube_on(&s, sailfish).text).is_empty());
    }

    #[test]
    fn test_accels_change_the_time_estimate() {
        let s = PrintSettings {
            min_layer_time: 0.0,
            ..PrintSettings::default()
        };
        let mut printer = Printer::presets()[0].clone();
        printer.acceleration = 500.0;
        let slow = sliced_cube_on(&s, &printer).estimate.seconds;
        printer.acceleration = 20000.0;
        let fast = sliced_cube_on(&s, &printer).estimate.seconds;
        assert!(slow > fast * 1.1, "{slow} vs {fast}");
        // Setting a low infill acceleration makes the same printer slower.
        let mut low = s.clone();
        low.accels.set(AccelFeature::Infill, Some(200.0));
        assert!(sliced_cube_on(&low, &printer).estimate.seconds > fast);
    }

    /// Two pillars 20 mm apart with a 2 mm deck across them, sliced and written with a
    /// filament whose fan is 50 %.
    fn bridge_gcode(s: &PrintSettings) -> GcodeOutput {
        let at = |size: Vec3, x: f32, z: f32| -> Vec<Triangle> {
            Mesh::cube(size)
                .triangles
                .iter()
                .map(|t| t.map(|v| v + Vec3::new(x, 0.0, z)))
                .collect()
        };
        let pillar = Vec3::new(10.0, 10.0, 10.0);
        let mut tris = at(pillar, -15.0, 0.0);
        tris.extend(at(pillar, 15.0, 0.0));
        tris.extend(at(Vec3::new(40.0, 10.0, 2.0), 0.0, 10.0));
        let r = crate::slicer::slice(&tris, s, 0.42, &|_| {});
        let filament = Filament {
            fan_percent: 50,
            ..Filament::presets()[0].clone()
        };
        generate(&r, &Printer::presets()[0], &filament, s)
    }

    /// Each extruding move between the bridge fan coming on and going off: (feed rate
    /// mm/min, filament per mm of line).
    fn bridge_moves(text: &str) -> Vec<(f32, f32)> {
        let (mut x, mut y, mut e, mut f) = (0f32, 0f32, 0f32, 0f32);
        let (mut on, mut out) = (false, Vec::new());
        for l in text.lines() {
            if l.ends_with("; bridge fan") {
                on = true;
            } else if l.ends_with("; part fan") {
                on = false;
            }
            if !l.starts_with("G1 ") && !l.starts_with("G0 ") {
                continue;
            }
            let (mut nx, mut ny, mut ne) = (x, y, e);
            for w in l.split(';').next().unwrap_or("").split_whitespace().skip(1) {
                let v: f32 = w[1..].parse().unwrap_or(0.0);
                match &w[..1] {
                    "X" => nx = v,
                    "Y" => ny = v,
                    "E" => ne = v,
                    "F" => f = v,
                    _ => {}
                }
            }
            let len = ((nx - x).powi(2) + (ny - y).powi(2)).sqrt();
            if on && ne > e && len > 1.0 {
                out.push((f, (ne - e) / len));
            }
            (x, y, e) = (nx, ny, ne);
        }
        out
    }

    #[test]
    fn test_bridge_prints_with_its_fan_speed_and_flow() {
        let s = PrintSettings {
            min_layer_time: 0.0,
            ..PrintSettings::default()
        };
        let out = bridge_gcode(&s);
        assert_eq!(out.text.matches("M106 S255 ; bridge fan").count(), 1);
        // The normal fan (50 % = 127) comes back after the bridge.
        let after = &out.text[out.text.find("; bridge fan").unwrap()..];
        assert!(after.contains("M106 S127 ; part fan"));
        let moves = bridge_moves(&out.text);
        assert!(moves.len() > 10, "{}", moves.len());
        // Auto bridge speed: half the print speed.
        let want_f = s.print_speed * SpeedFeature::BRIDGE_FACTOR * 60.0;
        assert!(moves.iter().all(|(f, _)| *f == want_f), "{moves:?}");
        // Bridge flow 95 %, then 60 %: filament per mm scales with it.
        let thin = PrintSettings {
            bridge_flow: 60.0,
            ..s.clone()
        };
        let thin_moves = bridge_moves(&bridge_gcode(&thin).text);
        let per_mm = |m: &[(f32, f32)]| m.iter().map(|x| x.1).sum::<f32>() / m.len() as f32;
        let ratio = per_mm(&thin_moves) / per_mm(&moves);
        assert!((ratio - 60.0 / 95.0).abs() < 0.01, "{ratio}");
    }

    #[test]
    fn test_bridge_fan_not_written_without_bridges_or_when_equal() {
        let out = sliced_cube(&PrintSettings::default()).1;
        assert!(!out.text.contains("bridge fan"));
        assert_eq!(out.text.matches("M106").count(), 1, "only the layer-2 fan");
        // A bridge fan equal to the normal fan writes nothing extra.
        let same = PrintSettings {
            bridge_fan: 50,
            ..PrintSettings::default()
        };
        assert!(!bridge_gcode(&same).text.contains("bridge fan"));
    }

    #[test]
    fn supports_go_to_the_support_nozzle_and_parts_to_their_own() {
        let path = |kind, x: f32| ToolPath {
            kind,
            points: vec![Point2::new(x, 0.0), Point2::new(x + 1.0, 0.0)],
            closed: false,
        };
        let tools = Tools {
            parts: vec![([-35.0, -5.0, -25.0, 5.0], 0), ([25.0, -5.0, 35.0, 5.0], 1)],
            support_tool: 1,
            ..Default::default()
        };
        assert_eq!(tools.tool_for(&path(PathKind::Support, -30.0)), 1);
        assert_eq!(tools.tool_for(&path(PathKind::OuterWall, -30.0)), 0);
        assert_eq!(tools.tool_for(&path(PathKind::SparseInfill, 30.0)), 1);
        assert_eq!(
            tools.tool_for(&path(PathKind::Skirt, 30.0)),
            0,
            "skirt on the first nozzle used"
        );
        assert!(tools.uses_second(false));
        let only_left = Tools {
            parts: vec![([-35.0, -5.0, -25.0, 5.0], 0)],
            support_tool: 1,
            ..Default::default()
        };
        assert!(!only_left.uses_second(false));
        assert!(
            only_left.uses_second(true),
            "supports alone need the second nozzle"
        );
    }
}
