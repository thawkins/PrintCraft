//! From placed triangles to a print file: the steps the window and the command line
//! share, so the two cannot drift apart.

use crate::gcode::{self, GcodeOutput};
use crate::geometry::Bounds;
use crate::model::Triangle;
use crate::profiles::{Filament, PrintSettings, Printer};
use crate::slicer::{self, SliceResult};
use image::RgbaImage;

/// Line width = nozzle × 1.05 (0.42 mm for a 0.4 mm nozzle). Estimated.
pub fn line_width(printer: &Printer) -> f32 {
    printer.nozzle_diameter * 1.05
}

/// Whether a box (in plate coordinates, centred on the origin) fits on the plate and
/// under the maximum height. A hundredth of a millimetre of slack absorbs rounding from
/// moving and turning a part that exactly fills the plate.
pub fn fits_on_plate(b: &Bounds, printer: &Printer) -> bool {
    const SLACK_MM: f32 = 0.01;
    let (hx, hy) = (
        printer.bed_x / 2.0 + SLACK_MM,
        printer.bed_y / 2.0 + SLACK_MM,
    );
    b.min.x >= -hx && b.max.x <= hx && b.min.y >= -hy && b.max.y <= hy && b.max.z <= printer.max_z
}

pub struct Sliced {
    pub result: SliceResult,
    pub gcode: GcodeOutput,
    /// The 300×300 picture written into the print file.
    pub picture: Option<RgbaImage>,
}

/// Slices the plate, writes the G-code and adds the thumbnail pictures to it.
/// `progress` hears 0..1 while slicing.
pub fn slice_plate(
    triangles: &[Triangle],
    printer: &Printer,
    filament: &Filament,
    settings: &PrintSettings,
    progress: &dyn Fn(f32),
) -> Sliced {
    let result = slicer::slice(triangles, settings, line_width(printer), progress);
    let mut gcode = gcode::generate(&result, printer, filament, settings);
    let (text, picture) = crate::thumbnail::add_to_gcode(&gcode.text, triangles);
    gcode.text = text;
    Sliced {
        result,
        gcode,
        picture,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slicing_a_plate_gives_layers_g_code_and_a_picture() {
        use crate::model::{Mesh, PlacedObject};
        let printer = Printer::presets().remove(0);
        let tris =
            PlacedObject::new("cube", Mesh::cube(Vec3::new(10.0, 10.0, 2.0))).world_triangles();
        let out = slice_plate(
            &tris,
            &printer,
            &Filament::presets()[0],
            &PrintSettings::default(),
            &|_| {},
        );
        assert_eq!(out.result.layers.len(), 10);
        assert!((out.result.line_width - line_width(&printer)).abs() < 1e-6);
        assert!(out.gcode.text.contains("; thumbnail begin"));
        assert!(out.picture.is_some());
    }

    #[test]
    fn line_width_is_a_little_wider_than_the_nozzle() {
        let mut printer = Printer::presets().remove(0);
        printer.nozzle_diameter = 0.4;
        assert!((line_width(&printer) - 0.42).abs() < 1e-6);
    }
    use crate::geometry::Vec3;

    #[test]
    fn a_part_exactly_filling_the_plate_fits_with_rounding_slack() {
        let printer = Printer::presets().remove(0);
        let (hx, hy) = (printer.bed_x / 2.0, printer.bed_y / 2.0);
        let exact = Bounds {
            min: Vec3::new(-hx - 0.005, -hy, 0.0),
            max: Vec3::new(hx, hy, printer.max_z),
        };
        assert!(fits_on_plate(&exact, &printer));
        let over = Bounds {
            max: Vec3::new(hx + 0.5, hy, 1.0),
            ..exact
        };
        assert!(!fits_on_plate(&over, &printer));
        let tall = Bounds {
            max: Vec3::new(hx, hy, printer.max_z + 1.0),
            ..exact
        };
        assert!(!fits_on_plate(&tall, &printer));
    }
}
