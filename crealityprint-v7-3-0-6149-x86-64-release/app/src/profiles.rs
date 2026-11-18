//! Printer, filament and print-quality settings with a few ready-made presets.
//!
//! Every number here is an estimate from common 3D-printing practice (clean-room build:
//! the original program was not studied). See MECHANICS.md.

/// The allowed range of each setting a person can change. The settings panel offers
/// exactly these, and a project file is held to them when it is opened.
pub mod limits {
    use std::ops::RangeInclusive;

    pub const PLATE_MM: RangeInclusive<f32> = 50.0..=1000.0;
    pub const RETRACT_MM: RangeInclusive<f32> = 0.0..=10.0;
    pub const PURGE_MM: RangeInclusive<f32> = 0.0..=100.0;
    pub const NOZZLE_TEMP: RangeInclusive<u32> = 150..=300;
    pub const BED_TEMP: RangeInclusive<u32> = 0..=120;
    pub const FAN_PERCENT: RangeInclusive<u32> = 0..=100;
    pub const LAYER_HEIGHT: RangeInclusive<f32> = 0.04..=0.4;
    pub const FIRST_LAYER_HEIGHT: RangeInclusive<f32> = 0.08..=0.4;
    pub const WALLS: RangeInclusive<u32> = 1..=10;
    pub const SOLID_LAYERS: RangeInclusive<u32> = 0..=20;
    pub const FUZZY_TOP_MM: RangeInclusive<f32> = 0.02..=0.3;
    pub const ADAPTIVE_QUALITY: RangeInclusive<f32> = 0.0..=100.0;
    pub const ADAPTIVE_SMOOTHING: RangeInclusive<f32> = 0.0..=10.0;
    pub const INFILL_PERCENT: RangeInclusive<f32> = 0.0..=100.0;
    pub const SUPPORT_ANGLE: RangeInclusive<f32> = 30.0..=80.0;
    pub const SUPPORT_GAP: RangeInclusive<f32> = 0.0..=2.0;
    pub const SUPPORT_XY_GAP: RangeInclusive<f32> = 1.0..=5.0;
    pub const SUPPORT_SPACING: RangeInclusive<f32> = 1.0..=10.0;
    pub const SUPPORT_INTERFACE: RangeInclusive<u32> = 0..=10;
    pub const TREE_TIP: RangeInclusive<f32> = 0.4..=3.0;
    pub const TREE_BRANCH: RangeInclusive<f32> = 1.0..=10.0;
    // Printing never goes slower than gcode::MIN_PRINT_SPEED (15 mm/s), so the panel
    // doesn't offer slower speeds that would have no effect.
    pub const PRINT_SPEED: RangeInclusive<f32> = 15.0..=500.0;
    pub const FIRST_LAYER_SPEED: RangeInclusive<f32> = 15.0..=100.0;
    pub const TRAVEL_SPEED: RangeInclusive<f32> = 20.0..=600.0;
    pub const MIN_LAYER_TIME: RangeInclusive<f32> = 0.0..=120.0;
    pub const RETRACT_MIN_TRAVEL: RangeInclusive<f32> = 0.0..=20.0;
    pub const WIPE_MM: RangeInclusive<f32> = 0.0..=10.0;
    pub const TOOL_CHANGE_RETRACT: RangeInclusive<f32> = 0.0..=30.0;
    pub const STANDBY_DROP: RangeInclusive<u32> = 0..=100;
    pub const NOZZLE_SPACING: RangeInclusive<f32> = 5.0..=200.0;
    pub const SHIELD_DISTANCE: RangeInclusive<f32> = 1.0..=20.0;
    pub const Z_HOP: RangeInclusive<f32> = 0.0..=2.0;
    pub const ACCELERATION: RangeInclusive<f32> = 100.0..=30000.0;
    pub const BRIDGE_FLOW: RangeInclusive<f32> = 50.0..=150.0;
    pub const FLOW_PERCENT: RangeInclusive<f32> = 50.0..=150.0;
    pub const INFILL_OVERLAP: RangeInclusive<f32> = 0.0..=50.0;
    pub const ELEPHANT_FOOT: RangeInclusive<f32> = 0.0..=1.0;
    pub const HOLE_COMPENSATION: RangeInclusive<f32> = 0.0..=1.0;
}

#[derive(Clone, Debug, PartialEq)]
pub struct Printer {
    pub name: String,
    pub bed_x: f32,
    pub bed_y: f32,
    pub max_z: f32,
    pub nozzle_diameter: f32,
    pub retract_length: f32,
    pub retract_speed: f32,
    /// Filament (mm) pushed through the purge lines at the left edge before printing.
    pub purge_mm: f32,
    /// Runs Klipper firmware (heat waits are written Klipper's way; see heat.rs).
    pub klipper: bool,
    /// A MakerBot-protocol printer (Sailfish: FlashForge Creator Pro, Replicator): print
    /// files are X3G, translated from the G-code (src/x3g).
    pub x3g: bool,
    /// Export and send to PrusaLink as binary G-code (`.bgcode`, src/bgcode.rs). Other
    /// connections still get plain G-code; never with X3G.
    pub binary_gcode: bool,
    /// Nozzles: 1, or 2 (two nozzles on one carriage, like the Creator Pro). With 1, no
    /// dual-nozzle setting shows and the G-code is unchanged.
    pub extruders: u8,
    /// Retract before travels longer than this (mm). Shorter travels stay over the part.
    pub retract_min_travel: f32,
    /// Also retract before every layer change.
    pub retract_layer_change: bool,
    /// Wipe: while retracting, move back this far (mm) along the line just printed, to
    /// wipe the ooze off on the part. 0 = off.
    pub wipe_mm: f32,
    /// Two nozzles: how far the nozzle being set aside is pulled back (mm), so it doesn't
    /// ooze while the other prints. It is pushed forward by the same amount when it is
    /// used again.
    pub tool_change_retract: f32,
    /// Two nozzles: the idle nozzle waits this many °C below its printing temperature (0 =
    /// keep it hot). It is heated back, with a wait, before it prints again.
    pub standby_drop: u32,
    /// Two nozzles: the primary (default) nozzle, 0 = right (T0), 1 = left (T1). New parts,
    /// supports, the skirt and one-nozzle prints use it, and it has the main filament.
    pub primary_nozzle: u8,
    /// Two nozzles: the distance between them along X (mm), the extruder offset. A Ditto
    /// copy prints this far to the side, so a Ditto job must be narrower than it.
    pub nozzle_spacing: f32,
    /// The printer's own acceleration (mm/s^2), as its firmware is set up. Used for the
    /// time estimate, and written back where a print setting's acceleration is "Auto".
    pub acceleration: f32,
    /// Custom G-code run instead of PrintCraft's start (heat, home, purge). Empty: the
    /// built-in start.
    pub start_gcode: String,
    /// Custom G-code run instead of PrintCraft's end (retract, lift, park, heaters and
    /// motors off). Empty: the built-in end.
    pub end_gcode: String,
    /// Custom G-code added at every layer change, before the move up. Empty: nothing.
    pub layer_change_gcode: String,
    /// Custom G-code added at every tool change, just before the new nozzle is selected.
    /// Empty: nothing.
    pub tool_change_gcode: String,
}

impl Printer {
    /// The nozzle used by default on a two-nozzle printer (`primary_nozzle`), for new
    /// parts, supports, the skirt and one-nozzle prints. Tool 0 on a one-nozzle printer.
    pub fn primary_tool(&self) -> u8 {
        if self.extruders >= 2 {
            self.primary_nozzle.min(1)
        } else {
            0
        }
    }
}

/// The default primary nozzle of a two-nozzle printer profile: T0, the right nozzle, which
/// is also the firmware's tool 0 and the nozzle a one-nozzle print uses. Each printer
/// profile can change it (`Printer::primary_nozzle`).
pub const PRIMARY_TOOL: u8 = 0;
/// The default distance between two nozzles on one carriage: GPX's `r1d` toolhead
/// offset (33 mm) for the Creator Pro / Replicator Dual family.
pub const DEFAULT_NOZZLE_SPACING_MM: f32 = 33.0;
/// The default travel length above which a travel retracts.
pub const DEFAULT_RETRACT_MIN_TRAVEL_MM: f32 = 2.0;
/// The default tool-change retraction on two-nozzle printers. Estimated: a common figure
/// for one carriage with two nozzles.
pub const DEFAULT_TOOL_CHANGE_RETRACT_MM: f32 = 8.0;
/// The default standby drop for the idle nozzle. Estimated.
pub const DEFAULT_STANDBY_DROP_C: u32 = 15;

/// The job's two-nozzle choices, used only on a printer with `extruders: 2`.
#[derive(Clone, Debug, PartialEq)]
pub struct DualNozzle {
    /// The filament in the nozzle that isn't the primary; None: the same as the
    /// primary's.
    pub second_filament: Option<Filament>,
    /// The nozzle that prints supports (0 = right, 1 = left).
    pub support_tool: u8,
    /// Ditto printing: the printer's firmware prints a copy beside the part with the
    /// second nozzle. It is switched on in the printer's own menu; the app heats the second
    /// nozzle and keeps the plate narrow enough for the copy.
    pub ditto: bool,
    /// A prime tower: after each tool change the incoming nozzle primes on it before
    /// printing the part.
    pub prime_tower: bool,
    /// Where the prime tower's centre stands, in mm from the plate's front-left corner;
    /// None: near the back-right corner.
    pub tower_at: Option<[f32; 2]>,
    /// An ooze shield: a one-line wall around the print, for the idle nozzle to wipe on.
    pub ooze_shield: bool,
    /// How far the ooze shield stands outside everything else the print lays down (mm).
    pub shield_distance: f32,
}

/// The default ooze shield distance. Estimated: close enough to catch the idle nozzle's
/// ooze, far enough to clear the print's own lines.
pub const DEFAULT_SHIELD_DISTANCE_MM: f32 = 3.0;

impl Default for DualNozzle {
    fn default() -> Self {
        DualNozzle {
            second_filament: None,
            support_tool: PRIMARY_TOOL,
            ditto: false,
            prime_tower: true,
            tower_at: None,
            ooze_shield: false,
            shield_distance: DEFAULT_SHIELD_DISTANCE_MM,
        }
    }
}

/// Filament for the purge lines: about what common start routines for this size of
/// printer push (two lines with roughly 15 mm each). Estimated.
pub const DEFAULT_PURGE_MM: f32 = 30.0;

/// A printer preset's acceleration (mm/s^2): estimated from the makers' firmware
/// defaults as commonly reported, not measured.
pub fn preset_acceleration(name: &str) -> f32 {
    if name.contains("Ender-3 V3 KE") {
        8000.0
    } else if name.contains("MK4S") || name.contains("Core One") || name.contains("XL") {
        4000.0
    } else if name.contains("MK3S") || name.contains("MINI") {
        1250.0
    } else {
        // Generic printers and the Sailfish Creator Pro.
        DEFAULT_ACCELERATION
    }
}

/// The acceleration (mm/s^2) of a printer that doesn't say: a cautious bedslinger value.
pub const DEFAULT_ACCELERATION: f32 = 1000.0;

impl Printer {
    pub fn presets() -> Vec<Printer> {
        let p = |name: &str, x: f32, y: f32, z: f32, retract: f32| Printer {
            name: name.into(),
            bed_x: x,
            bed_y: y,
            max_z: z,
            nozzle_diameter: 0.4,
            retract_length: retract,
            retract_speed: 40.0,
            purge_mm: DEFAULT_PURGE_MM,
            // The KE runs Klipper (Creality OS); the others here run Marlin or Prusa firmware.
            klipper: name.contains("Ender-3 V3 KE"),
            x3g: name.contains("Sailfish"),
            binary_gcode: false,
            extruders: if name.contains("Sailfish") { 2 } else { 1 },
            retract_min_travel: DEFAULT_RETRACT_MIN_TRAVEL_MM,
            retract_layer_change: false,
            wipe_mm: 0.0,
            tool_change_retract: DEFAULT_TOOL_CHANGE_RETRACT_MM,
            standby_drop: DEFAULT_STANDBY_DROP_C,
            primary_nozzle: PRIMARY_TOOL,
            nozzle_spacing: DEFAULT_NOZZLE_SPACING_MM,
            acceleration: preset_acceleration(name),
            start_gcode: String::new(),
            end_gcode: String::new(),
            layer_change_gcode: String::new(),
            tool_change_gcode: String::new(),
        };
        // Build volumes of named printers are their makers' published specifications;
        // retraction values are common starting points (estimated).
        vec![
            p("Compact printer 220 × 220", 220.0, 220.0, 250.0, 0.8),
            p("Standard printer 235 × 235", 235.0, 235.0, 250.0, 0.8),
            p("Large printer 300 × 300", 300.0, 300.0, 400.0, 1.0),
            p("Mini printer 180 × 180", 180.0, 180.0, 180.0, 0.8),
            p("Prusa MK4S", 250.0, 210.0, 220.0, 0.7),
            p("Prusa Core One", 250.0, 220.0, 270.0, 0.7),
            p("Prusa MK3S+", 250.0, 210.0, 210.0, 0.8),
            p("Prusa MINI+", 180.0, 180.0, 180.0, 3.2),
            p("Prusa XL (one tool)", 360.0, 360.0, 360.0, 0.7),
            p("Creality Ender-3 V3 KE", 220.0, 220.0, 240.0, 0.8),
            // GPX's axis lengths for this family (Replicator 1 Dual, which GPX uses for the
            // Creator Pro); the early Creator Pro 2 is a clone. Retraction estimated.
            p(
                "FlashForge Creator Pro (Sailfish)",
                227.0,
                148.0,
                150.0,
                1.0,
            ),
        ]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Filament {
    pub name: String,
    pub nozzle_temp: u32,
    pub bed_temp: u32,
    pub diameter: f32,
    /// Grams per cubic centimetre.
    pub density: f32,
    /// Part-cooling fan from layer 2 onwards, 0-100 %.
    pub fan_percent: u32,
    /// Price per kilogram, for the cost estimate.
    pub price_per_kg: f32,
    /// Flow ratio (extrusion multiplier): plastic pushed for every printed line, as a
    /// share of the computed amount (%). Calibrates for filament and extruder.
    pub flow_percent: f32,
}

impl Filament {
    pub fn presets() -> Vec<Filament> {
        let f = |name: &str, nozzle, bed, density, fan, price| Filament {
            name: name.into(),
            nozzle_temp: nozzle,
            bed_temp: bed,
            diameter: 1.75,
            density,
            fan_percent: fan,
            price_per_kg: price,
            flow_percent: 100.0,
        };
        vec![
            f("PLA", 210, 60, 1.24, 100, 20.0),
            f("PETG", 240, 80, 1.27, 50, 22.0),
            f("ABS", 250, 100, 1.04, 0, 22.0),
            f("TPU (flexible)", 225, 50, 1.21, 60, 30.0),
            f("ASA", 250, 100, 1.07, 20, 25.0),
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adhesion {
    None,
    Skirt,
    Brim,
    /// A few layers of lines under the whole print; the model sits on top of them.
    Raft,
}

impl Adhesion {
    pub const ALL: [Adhesion; 4] = [Self::None, Self::Skirt, Self::Brim, Self::Raft];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Skirt => "Skirt",
            Self::Brim => "Brim",
            Self::Raft => "Raft",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InfillPattern {
    /// Parallel lines, turning 90° every layer.
    Lines,
    /// Two crossing sets of lines on every layer.
    Grid,
    /// Three sets of lines 60° apart: stiff in every direction.
    Triangles,
    /// Waving lines that follow a gyroid surface, changing from layer to layer: about
    /// equally strong in every direction, with no crossings within a layer.
    Gyroid,
    /// Zigzag lines that make hexagons, stacked into upright hexagonal tubes.
    Honeycomb,
    /// Three sets of lines 60° apart that slide with height, building a lattice of
    /// cubes standing on a corner: strong in every direction, no stacked crossings.
    Cubic,
    /// Cubic, at half the density deep inside the part (more than 4 mm from its
    /// walls, top and bottom), saving plastic where it isn't needed.
    AdaptiveCubic,
    /// Branching trees only under top surfaces, hollow elsewhere: the least plastic,
    /// for parts that only need to look right.
    Lightning,
}

impl InfillPattern {
    pub const ALL: [InfillPattern; 8] = [
        Self::Lines,
        Self::Grid,
        Self::Triangles,
        Self::Gyroid,
        Self::Honeycomb,
        Self::Cubic,
        Self::AdaptiveCubic,
        Self::Lightning,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Lines => "Lines",
            Self::Grid => "Grid",
            Self::Triangles => "Triangles",
            Self::Gyroid => "Gyroid",
            Self::Honeycomb => "Honeycomb",
            Self::Cubic => "Cubic",
            Self::AdaptiveCubic => "Adaptive cubic",
            Self::Lightning => "Lightning",
        }
    }
}

/// How supports are built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupportStyle {
    /// Straight columns of lines under every overhang.
    Grid,
    /// Thin branches from each overhang that lean together and join on the way down,
    /// keeping clear of the model: less plastic, easier to remove.
    Tree,
}

impl SupportStyle {
    pub const ALL: [SupportStyle; 2] = [Self::Grid, Self::Tree];

    pub fn label(self) -> &'static str {
        match self {
            Self::Grid => "Grid",
            Self::Tree => "Tree",
        }
    }
}

/// How solid layers (tops, bottoms, solid infill) are filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SolidPattern {
    /// Straight lines side by side, laid in the quickest order.
    Lines,
    /// The same lines, always laid in order across the surface, so each lies against
    /// its neighbour the same way: a more even finish.
    Monotonic,
    /// Loops following the edge, working inwards.
    Concentric,
}

impl SolidPattern {
    pub const ALL: [SolidPattern; 3] = [Self::Lines, Self::Monotonic, Self::Concentric];

    pub fn label(self) -> &'static str {
        match self {
            Self::Lines => "Lines",
            Self::Monotonic => "Monotonic",
            Self::Concentric => "Concentric",
        }
    }
}

/// Where each wall loop starts (and leaves its small "seam" mark).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeamPosition {
    /// Each layer starts as close as possible to the layer below's start: one neat line.
    Aligned,
    /// Wherever the nozzle already is: fastest.
    Nearest,
    /// Lined up at the back of the model, out of sight.
    Rear,
    /// Scattered, so no line shows.
    Random,
}

impl SeamPosition {
    pub const ALL: [SeamPosition; 4] = [Self::Aligned, Self::Nearest, Self::Rear, Self::Random];

    pub fn label(self) -> &'static str {
        match self {
            Self::Aligned => "Aligned",
            Self::Nearest => "Nearest",
            Self::Rear => "Rear",
            Self::Random => "Random",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PrintSettings {
    pub layer_height: f32,
    pub first_layer_height: f32,
    pub walls: u32,
    pub top_layers: u32,
    pub bottom_layers: u32,
    pub infill_percent: f32,
    pub print_speed: f32,
    pub first_layer_speed: f32,
    pub travel_speed: f32,
    /// Speeds for single kinds of line (Advanced); each is "Auto" until set.
    pub speeds: FeatureSpeeds,
    /// Accelerations for some kinds of move (Advanced); each is "Auto" until set.
    pub accels: FeatureAccels,
    /// Plastic laid on bridge lines, as a share of a normal line (%).
    pub bridge_flow: f32,
    /// The part fan while printing bridge lines (%).
    pub bridge_fan: u32,
    /// Route travels along the inside of the outer wall instead of out over air, and
    /// retract only for travels that must leave the part.
    pub avoid_crossing_walls: bool,
    /// How far infill line ends reach into the innermost wall's band, as a share of a
    /// line width (%). At 15 % the infill bead overlaps that wall by about 0.65 of a line.
    pub infill_overlap: f32,
    /// The first layer is drawn in by this much (mm) to offset the squash that widens it
    /// ("elephant's foot"). Not with a raft, which takes the squash instead.
    pub elephant_foot: f32,
    /// Holes are made this much (mm) larger all round, as printed holes come out small.
    pub hole_compensation: f32,
    pub supports: bool,
    /// Overhangs steeper than this (degrees from vertical) get supports.
    pub support_angle: f32,
    /// Supports only stand on the plate, never on the model.
    pub supports_on_plate_only: bool,
    /// Support columns that reach the plate stand on a small raft of their own, so they
    /// stick to the bed (not needed with a full raft, which already covers them).
    pub support_raft: bool,
    /// Gap (mm) between the top of a support and the overhang it holds; `None` is one
    /// layer.
    pub support_top_gap: Option<f32>,
    /// Gap (mm) between the model and a support standing on it.
    pub support_bottom_gap: f32,
    /// How far (mm) support keeps from the model's sides (in 1 mm steps).
    pub support_xy_gap: f32,
    /// Distance (mm) between support lines (in 1 mm steps).
    pub support_spacing: f32,
    /// Dense layers at the top of each support, for a cleaner underside.
    pub support_interface_layers: u32,
    pub support_style: SupportStyle,
    /// Tree supports: the branches' width (mm) where they touch the overhang.
    pub tree_tip_diameter: f32,
    /// Tree supports: the width (mm) branches grow to on the way down.
    pub tree_branch_diameter: f32,
    pub adhesion: Adhesion,
    pub infill_pattern: InfillPattern,
    pub seam: SeamPosition,
    /// A second, nearly dry pass over top surfaces to smooth them.
    pub ironing: bool,
    /// Small random wobble on outer walls for a textured finish.
    pub fuzzy_skin: bool,
    /// Fuzzy top: the nozzle randomly lifts by up to `fuzzy_top_mm` while laying the
    /// visible top surface, for a textured, matte top.
    pub fuzzy_top: bool,
    pub fuzzy_top_mm: f32,
    /// Spiral vase: one continuous outer wall, no top, no infill above the bottom.
    pub vase_mode: bool,
    /// Lift the nozzle by this much (mm) on each retraction; 0 = off.
    pub z_hop: f32,
    /// Slow down so each layer takes at least this long (seconds) to cool.
    pub min_layer_time: f32,
    /// Variable layer height: thinner layers where the surface slopes gently, thicker
    /// ones where it is steep, worked out from the whole plate.
    pub adaptive_layers: bool,
    /// How thin variable layers get where the surface needs it (%): 0 = always the
    /// thickest, 100 = as fine as possible.
    pub adaptive_quality: f32,
    /// How far (mm) thin variable layers spread, so heights don't jump.
    pub adaptive_smoothing: f32,
    /// How the visible top surface is filled.
    pub top_pattern: SolidPattern,
    /// How bottoms and other solid layers are filled.
    pub solid_pattern: SolidPattern,
    /// Variable-width walls (experimental): thin parts get one line as wide as the
    /// space instead of walls that don't fit.
    pub variable_walls: bool,
    /// Name each object in the G-code (Klipper `EXCLUDE_OBJECT`, M486 elsewhere), so one
    /// object can be cancelled while the others go on printing.
    pub label_objects: bool,
    /// Write runs of short segments that lie on a circle as G2/G3 arcs (not for X3G).
    pub arc_fitting: bool,
}

/// Infill reaches 15 % of a line width into the innermost wall, to bond to it.
pub const DEFAULT_INFILL_OVERLAP: f32 = 15.0;

/// Bridge flow (%): a little thinner than a normal line, so it is light enough to hang
/// straight. Estimated from common slicer defaults.
pub const DEFAULT_BRIDGE_FLOW: f32 = 95.0;
/// Bridge fan (%): full cooling, so the line sets before it sags.
pub const DEFAULT_BRIDGE_FAN: u32 = 100;

/// The kinds of line that can be given their own speed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpeedFeature {
    OuterWall,
    InnerWall,
    SparseInfill,
    SolidInfill,
    TopSurface,
    Support,
    Bridge,
}

impl SpeedFeature {
    pub const ALL: [Self; 7] = [
        Self::OuterWall,
        Self::InnerWall,
        Self::SparseInfill,
        Self::SolidInfill,
        Self::TopSurface,
        Self::Support,
        Self::Bridge,
    ];

    /// The outer wall's "Auto" speed is this share of the print speed: a slower outer
    /// wall gives a cleaner visible surface.
    pub const OUTER_WALL_FACTOR: f32 = 0.5;

    /// A bridge's "Auto" speed is this share of the print speed: slower lines sag less
    /// over air. Estimated from common slicer defaults.
    pub const BRIDGE_FACTOR: f32 = 0.5;

    /// The name of this speed in the settings panel.
    pub fn label(self) -> &'static str {
        match self {
            Self::OuterWall => "Outer wall",
            Self::InnerWall => "Inner wall",
            Self::SparseInfill => "Sparse infill",
            Self::SolidInfill => "Solid infill",
            Self::TopSurface => "Top surface",
            Self::Support => "Support",
            Self::Bridge => "Bridge",
        }
    }

    /// The speed (mm/s) used when this feature is left on "Auto".
    pub fn auto_speed(self, print_speed: f32) -> f32 {
        match self {
            Self::OuterWall => print_speed * Self::OUTER_WALL_FACTOR,
            Self::Bridge => print_speed * Self::BRIDGE_FACTOR,
            Self::InnerWall
            | Self::SparseInfill
            | Self::SolidInfill
            | Self::TopSurface
            | Self::Support => print_speed,
        }
    }
}

/// A speed per `SpeedFeature`. `None` is "Auto": the speed follows the print speed, so a
/// quality preset's speed still applies to it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FeatureSpeeds([Option<f32>; SpeedFeature::ALL.len()]);

impl FeatureSpeeds {
    /// The speed set for `f`, or `None` when it is on "Auto".
    pub fn get(&self, f: SpeedFeature) -> Option<f32> {
        self.0[f as usize]
    }

    /// Sets `f` to a speed (mm/s), or back to "Auto" with `None`.
    pub fn set(&mut self, f: SpeedFeature, speed: Option<f32>) {
        self.0[f as usize] = speed;
    }

    /// The speed (mm/s) `f` prints at with this print speed.
    pub fn speed(&self, f: SpeedFeature, print_speed: f32) -> f32 {
        self.get(f).unwrap_or_else(|| f.auto_speed(print_speed))
    }
}

/// The kinds of move that can be given their own acceleration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccelFeature {
    OuterWall,
    /// Sparse and solid infill.
    Infill,
    Travel,
    /// Every printing move on the first layer.
    FirstLayer,
}

impl AccelFeature {
    pub const ALL: [Self; 4] = [
        Self::OuterWall,
        Self::Infill,
        Self::Travel,
        Self::FirstLayer,
    ];

    /// The name of this acceleration in the settings panel.
    pub fn label(self) -> &'static str {
        match self {
            Self::OuterWall => "Outer wall accel.",
            Self::Infill => "Infill accel.",
            Self::Travel => "Travel accel.",
            Self::FirstLayer => "First layer accel.",
        }
    }
}

/// An acceleration (mm/s^2) per `AccelFeature`. `None` is "Auto": the printer's own
/// acceleration, and nothing is written to the G-code while every one is on Auto.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FeatureAccels([Option<f32>; AccelFeature::ALL.len()]);

impl FeatureAccels {
    /// The acceleration set for `f`, or `None` when it is on "Auto".
    pub fn get(&self, f: AccelFeature) -> Option<f32> {
        self.0[f as usize]
    }

    /// Sets `f` to an acceleration (mm/s^2), or back to "Auto" with `None`.
    pub fn set(&mut self, f: AccelFeature, accel: Option<f32>) {
        self.0[f as usize] = accel;
    }

    /// True when at least one acceleration is set (not "Auto").
    pub fn any_set(&self) -> bool {
        self.0.iter().any(Option::is_some)
    }

    /// The acceleration `f` uses on a printer whose own acceleration is `machine`.
    pub fn accel(&self, f: AccelFeature, machine: f32) -> f32 {
        self.get(f).unwrap_or(machine)
    }
}

/// The quick quality choices shown at the top of the print settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Fine,
    Standard,
    Draft,
}

impl Quality {
    pub const ALL: [Quality; 3] = [Self::Fine, Self::Standard, Self::Draft];

    pub fn label(self) -> &'static str {
        match self {
            Self::Fine => "Fine (0.12 mm)",
            Self::Standard => "Standard (0.20 mm)",
            Self::Draft => "Draft (0.28 mm)",
        }
    }

    pub fn parse(text: &str) -> Option<Quality> {
        match text.to_ascii_lowercase().as_str() {
            "fine" => Some(Self::Fine),
            "standard" => Some(Self::Standard),
            "draft" => Some(Self::Draft),
            _ => None,
        }
    }

    /// Changes only what a quality preset is about, keeping every other choice.
    pub fn apply(self, s: &mut PrintSettings) {
        let p = self.settings();
        s.layer_height = p.layer_height;
        s.first_layer_height = p.first_layer_height;
        s.walls = p.walls;
        s.top_layers = p.top_layers;
        s.bottom_layers = p.bottom_layers;
        s.infill_percent = p.infill_percent;
        s.print_speed = p.print_speed;
    }

    /// True when `s` has this preset's quality values.
    pub fn matches(self, s: &PrintSettings) -> bool {
        let mut t = s.clone();
        self.apply(&mut t);
        &t == s
    }

    pub fn settings(self) -> PrintSettings {
        let base = PrintSettings {
            layer_height: 0.2,
            first_layer_height: 0.2,
            walls: 2,
            top_layers: 4,
            bottom_layers: 3,
            // The person asked for 25 % as the default density.
            infill_percent: 25.0,
            print_speed: 60.0,
            first_layer_speed: 25.0,
            travel_speed: 150.0,
            speeds: FeatureSpeeds::default(),
            accels: FeatureAccels::default(),
            bridge_flow: DEFAULT_BRIDGE_FLOW,
            bridge_fan: DEFAULT_BRIDGE_FAN,
            avoid_crossing_walls: false,
            infill_overlap: DEFAULT_INFILL_OVERLAP,
            elephant_foot: 0.0,
            hole_compensation: 0.0,
            supports: false,
            support_angle: 50.0,
            supports_on_plate_only: false,
            support_raft: true,
            support_top_gap: None,
            support_bottom_gap: 0.0,
            support_xy_gap: 1.0,
            support_spacing: 2.0,
            support_interface_layers: 0,
            support_style: SupportStyle::Grid,
            tree_tip_diameter: 0.8,
            tree_branch_diameter: 2.0,
            adhesion: Adhesion::Skirt,
            infill_pattern: InfillPattern::Grid,
            // The original's main window shows "Seam position: Aligned" (reference/picture-2.png).
            seam: SeamPosition::Aligned,
            ironing: false,
            fuzzy_skin: false,
            fuzzy_top: false,
            fuzzy_top_mm: 0.1,
            vase_mode: false,
            z_hop: 0.0,
            min_layer_time: 15.0,
            adaptive_layers: false,
            adaptive_quality: 50.0,
            adaptive_smoothing: 1.0,
            top_pattern: SolidPattern::Lines,
            solid_pattern: SolidPattern::Lines,
            variable_walls: false,
            label_objects: false,
            arc_fitting: false,
        };
        match self {
            Self::Fine => PrintSettings {
                layer_height: 0.12,
                walls: 3,
                top_layers: 6,
                bottom_layers: 5,
                print_speed: 50.0,
                ..base
            },
            Self::Standard => base,
            Self::Draft => PrintSettings {
                layer_height: 0.28,
                first_layer_height: 0.28,
                top_layers: 3,
                bottom_layers: 3,
                infill_percent: 10.0,
                print_speed: 80.0,
                ..base
            },
        }
    }
}

impl Default for PrintSettings {
    fn default() -> Self {
        Quality::Standard.settings()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_quality_preset_is_recognised_after_applying_it() {
        for q in Quality::ALL {
            let mut s = PrintSettings {
                seam: SeamPosition::Random,
                ..PrintSettings::default()
            };
            q.apply(&mut s);
            assert!(q.matches(&s), "{q:?}");
            assert_eq!(s.seam, SeamPosition::Random, "other choices are kept");
            let word = q.label().split_whitespace().next().unwrap();
            assert_eq!(Quality::parse(word), Some(q));
        }
        assert_eq!(Quality::parse("ultra"), None);
    }

    #[test]
    fn presets_have_unique_names_and_stay_within_the_panel_limits() {
        let names = |v: Vec<String>| {
            let mut sorted = v.clone();
            sorted.sort();
            sorted.dedup();
            sorted.len() == v.len()
        };
        assert!(names(
            Printer::presets().into_iter().map(|p| p.name).collect()
        ));
        assert!(names(
            Filament::presets().into_iter().map(|f| f.name).collect()
        ));
        for p in Printer::presets() {
            assert!(limits::PLATE_MM.contains(&p.bed_x) && limits::PLATE_MM.contains(&p.bed_y));
            assert!(limits::PLATE_MM.contains(&p.max_z), "{}", p.name);
        }
        for f in Filament::presets() {
            assert!(limits::NOZZLE_TEMP.contains(&f.nozzle_temp), "{}", f.name);
            assert!(limits::BED_TEMP.contains(&f.bed_temp), "{}", f.name);
        }
        for s in Quality::ALL.map(Quality::settings) {
            assert!(limits::LAYER_HEIGHT.contains(&s.layer_height));
            assert!(limits::PRINT_SPEED.contains(&s.print_speed));
            assert!(limits::FIRST_LAYER_SPEED.contains(&s.first_layer_speed));
            assert!(limits::WALLS.contains(&s.walls));
        }
    }

    #[test]
    fn only_the_klipper_printer_preset_is_marked_klipper() {
        let klipper: Vec<String> = Printer::presets()
            .into_iter()
            .filter(|p| p.klipper)
            .map(|p| p.name)
            .collect();
        assert_eq!(klipper, ["Creality Ender-3 V3 KE"]);
    }

    #[test]
    fn test_feature_speeds_survive_a_quality_preset() {
        let mut s = PrintSettings::default();
        s.speeds.set(SpeedFeature::OuterWall, Some(22.0));
        Quality::Draft.apply(&mut s);
        assert_eq!(s.speeds.get(SpeedFeature::OuterWall), Some(22.0));
        // Auto speeds follow the preset's new print speed.
        assert_eq!(s.speeds.speed(SpeedFeature::InnerWall, s.print_speed), 80.0);
        assert!(Quality::Draft.matches(&s));
    }
}
