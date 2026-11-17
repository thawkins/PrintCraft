//! Writes sliced layers as G-code and estimates print time, filament and cost.

use crate::geometry::Point2;
use crate::profiles::{Filament, PrintSettings, Printer, SeamPosition};
use crate::slicer::{Layer, PathKind, SliceResult, ToolPath};
use std::fmt::Write;

/// Travel moves longer than this retract the filament first.
const RETRACT_MIN_TRAVEL_MM: f32 = 2.0;
/// Outer walls print slower than the main speed for a nicer surface.
const OUTER_WALL_SPEED_FACTOR: f32 = 0.5;
/// Extra time per layer for slowing down, turning and layer changes.
const LAYER_CHANGE_SECONDS: f32 = 1.0;
/// Real printers accelerate; plain length / speed under-counts by roughly this much.
const ACCELERATION_ALLOWANCE: f32 = 1.15;
/// Heating up the bed and nozzle before printing.
const WARMUP_SECONDS: f32 = 120.0;
/// Fuzzy top: a new random height every this many millimetres along the top lines.
const FUZZY_TOP_STEP_MM: f32 = 0.5;
/// Ironing passes push out only this share of normal plastic, slowly.
const IRONING_FLOW: f32 = 0.15;
const IRONING_SPEED: f32 = 15.0;
/// Cooling slowdown never goes below this speed (mm/s).
const MIN_PRINT_SPEED: f32 = 15.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Estimate {
    pub seconds: f32,
    pub filament_mm: f32,
    pub grams: f32,
    pub cost: f32,
    pub layers: usize,
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

fn speed_for(kind: PathKind, layer: usize, s: &PrintSettings) -> f32 {
    if layer == 0 {
        return s.first_layer_speed;
    }
    match kind {
        PathKind::OuterWall => s.print_speed * OUTER_WALL_SPEED_FACTOR,
        _ => s.print_speed,
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

/// Builds the whole G-code file. Plate coordinates are centred on the plate; the
/// printer's coordinates start at its front-left corner.
pub fn generate(
    result: &SliceResult,
    printer: &Printer,
    filament: &Filament,
    s: &PrintSettings,
) -> GcodeOutput {
    let mut w = Writer::start(result, printer, filament, s);
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
}

impl<'a> Writer<'a> {
    /// Header comments, heating, homing and the purge lines.
    fn start(
        result: &SliceResult,
        printer: &'a Printer,
        filament: &'a Filament,
        s: &'a PrintSettings,
    ) -> Self {
        let mut g = String::with_capacity(1 << 20);
        let ox = printer.bed_x / 2.0;
        let oy = printer.bed_y / 2.0;

        let _ = writeln!(g, "; Made with PrintCraft (a simple slicer)");
        let _ = writeln!(g, "; printer: {}", printer.name);
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
        let _ = writeln!(g, "M104 S{} ; nozzle temperature", filament.nozzle_temp);
        let _ = writeln!(g, "G90 ; absolute positions");
        let _ = writeln!(g, "M82 ; absolute extrusion");
        let _ = writeln!(g, "G28 ; home all axes");
        // Wait only while heating up: a nozzle or plate that is already hotter carries on.
        g.push_str(&crate::heat::wait_bed(filament.bed_temp, printer.klipper));
        g.push_str(&crate::heat::wait_nozzle(
            filament.nozzle_temp,
            printer.klipper,
        ));
        let _ = writeln!(g, "M107 ; fan off");
        let _ = writeln!(g, "G92 E0");
        // Purge: clear the nozzle with two side-by-side lines along the left edge.
        let purge = purge_lines(printer);
        g.push_str(&purge.gcode);
        let _ = writeln!(g, "G92 E0");

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
            prime_e: purge.filament_mm,
            seconds: WARMUP_SECONDS + purge.seconds,
        }
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

        let mut last_kind = None;
        let mut seams_here: Vec<Point2> = Vec::new();
        for (pi, path) in layer.paths.iter().enumerate() {
            if path.points.len() < 2 {
                continue;
            }
            if last_kind != Some(path.kind) {
                let _ = writeln!(self.g, ";TYPE:{}", path.kind.gcode_type());
                last_kind = Some(path.kind);
            }
            let salt = li * 7919 + pi;
            let pts = ordered_points(path, self.pos, seam, salt, seams_below, &mut seams_here);
            self.travel_to(pts[0], layer.z);
            let style = PathStyle {
                li,
                pi,
                kind: path.kind,
                spiral,
                slowdown,
            };
            self.extrude(layer, &style, pts);
        }
        if !seams_here.is_empty() {
            *seams_below = seams_here;
        }
    }

    /// The layer marker, the Z move (skipped while spiralling), and the part fan from
    /// the second layer on. Returns whether this layer is part of the vase spiral.
    fn begin_layer(&mut self, li: usize, layer: &Layer) -> bool {
        let _ = writeln!(self.g, ";LAYER:{li}");
        // Vase mode: above the solid bottom, the wall climbs continuously instead.
        let spiral = self.s.vase_mode && li >= self.s.bottom_layers as usize && li > 0;
        if !spiral {
            let _ = writeln!(self.g, "G1 Z{:.3} F600", layer.z);
        }
        if li == 1 && self.filament.fan_percent > 0 {
            let _ = writeln!(
                self.g,
                "M106 S{} ; part fan",
                self.filament.fan_percent * 255 / 100
            );
        }
        self.seconds += LAYER_CHANGE_SECONDS;
        spiral
    }

    /// Moves to `to` (retracting and z-hopping first on long travels), then undoes any
    /// retraction so the nozzle is ready to print at layer height `z`.
    fn travel_to(&mut self, to: Point2, z: f32) {
        let p = self.printer;
        let travel = self.pos.distance(to);
        // Retracted for this travel only: the path after it always starts un-retracted.
        let mut retracted = false;
        if travel > 0.01 {
            let retract = travel > RETRACT_MIN_TRAVEL_MM && p.retract_length > 0.0;
            if retract {
                let e = self.ext.retract(p.retract_length);
                let _ = writeln!(self.g, "G1 E{:.5} F{:.0}", e, p.retract_speed * 60.0);
                retracted = true;
                self.seconds += p.retract_length / p.retract_speed * 2.0;
                if self.s.z_hop > 0.0 {
                    let _ = writeln!(self.g, "G1 Z{:.3} F600 ; z-hop", z + self.s.z_hop);
                    self.seconds += 2.0 * self.s.z_hop / 10.0;
                }
            }
            let _ = writeln!(
                self.g,
                "G0 X{:.3} Y{:.3} F{:.0}",
                to.x + self.ox,
                to.y + self.oy,
                self.travel_f
            );
            self.seconds += travel / self.s.travel_speed;
        }
        if retracted {
            if self.s.z_hop > 0.0 {
                let _ = writeln!(self.g, "G1 Z{:.3} F600", z);
            }
            let e = self.ext.unretract(p.retract_length);
            let _ = writeln!(self.g, "G1 E{:.5} F{:.0}", e, p.retract_speed * 60.0);
        }
    }

    /// Prints along `pts`: one G1 per segment, with a rising Z on vase-spiral walls and
    /// a random lift on fuzzy top lines. Leaves the nozzle at the last point.
    fn extrude(&mut self, layer: &Layer, style: &PathStyle, mut pts: Vec<Point2>) {
        let s = self.s;
        let (flow, speed) = match style.kind {
            PathKind::Ironing => (IRONING_FLOW, IRONING_SPEED),
            kind => (
                1.0,
                (speed_for(kind, style.li, s) * style.slowdown).max(MIN_PRINT_SPEED),
            ),
        };
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
            let de =
                filament_for(len, self.line_width, layer.height, self.filament.diameter) * flow;
            let e = self.ext.extrude(de);
            self.seconds += len / speed;
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
        let _ = writeln!(g, "M104 S0");
        let _ = writeln!(g, "M140 S0");
        let _ = writeln!(g, "M107");
        let _ = writeln!(g, "M84 ; motors off");

        let total_e = (self.prime_e as f64 + self.ext.extruded) as f32;
        let seconds = self.seconds * ACCELERATION_ALLOWANCE;
        let volume_cm3 =
            total_e * std::f32::consts::PI * (filament.diameter / 2.0).powi(2) / 1000.0;
        let grams = volume_cm3 * filament.density;
        let estimate = Estimate {
            seconds,
            filament_mm: total_e,
            grams,
            cost: grams / 1000.0 * filament.price_per_kg,
            layers: result.layers.len(),
        };
        let _ = writeln!(g, "; estimated time: {}", estimate.time_text());
        let _ = writeln!(
            g,
            "; filament used: {:.2} m, {:.1} g",
            total_e / 1000.0,
            grams
        );
        g.push_str(&config_block(filament, self.s, printer));
        GcodeOutput {
            text: self.g,
            estimate,
        }
    }
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
    use crate::model::{Mesh, PlacedObject};
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
}
