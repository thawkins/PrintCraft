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
    pub const INFILL_PERCENT: RangeInclusive<f32> = 0.0..=100.0;
    pub const SUPPORT_ANGLE: RangeInclusive<f32> = 30.0..=80.0;
    pub const PRINT_SPEED: RangeInclusive<f32> = 10.0..=500.0;
    pub const FIRST_LAYER_SPEED: RangeInclusive<f32> = 5.0..=100.0;
    pub const TRAVEL_SPEED: RangeInclusive<f32> = 20.0..=600.0;
    pub const MIN_LAYER_TIME: RangeInclusive<f32> = 0.0..=120.0;
    pub const Z_HOP: RangeInclusive<f32> = 0.0..=2.0;
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
}

/// Filament for the purge lines: about what common start routines for this size of
/// printer push (two lines with roughly 15 mm each). Estimated.
pub const DEFAULT_PURGE_MM: f32 = 30.0;

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
        };
        vec![
            f("PLA", 210, 60, 1.24, 100, 20.0),
            f("PETG", 240, 80, 1.27, 50, 22.0),
            f("ABS", 250, 100, 1.04, 0, 22.0),
            f("TPU (flexible)", 225, 50, 1.21, 60, 30.0),
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adhesion {
    None,
    Skirt,
    Brim,
}

impl Adhesion {
    pub const ALL: [Adhesion; 3] = [Self::None, Self::Skirt, Self::Brim];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Skirt => "Skirt",
            Self::Brim => "Brim",
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
}

impl InfillPattern {
    pub const ALL: [InfillPattern; 3] = [Self::Lines, Self::Grid, Self::Triangles];

    pub fn label(self) -> &'static str {
        match self {
            Self::Lines => "Lines",
            Self::Grid => "Grid",
            Self::Triangles => "Triangles",
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
    pub supports: bool,
    /// Overhangs steeper than this (degrees from vertical) get supports.
    pub support_angle: f32,
    /// Supports only stand on the plate, never on the model.
    pub supports_on_plate_only: bool,
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
            infill_percent: 15.0,
            print_speed: 60.0,
            first_layer_speed: 25.0,
            travel_speed: 150.0,
            supports: false,
            support_angle: 50.0,
            supports_on_plate_only: false,
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
