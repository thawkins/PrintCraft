//! From placed triangles to a print file: the steps the window and the command line
//! share, so the two cannot drift apart.

use crate::gcode::{self, GcodeOutput};
use crate::geometry::Bounds;
use crate::model::Triangle;
use crate::overrides::Overrides;
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
    slice_plate_with_tools(
        triangles,
        printer,
        filament,
        settings,
        &gcode::Tools::default(),
        progress,
    )
}

/// Like [`slice_plate`], for a printer with two nozzles (see `gcode::Tools`).
pub fn slice_plate_with_tools(
    triangles: &[Triangle],
    printer: &Printer,
    filament: &Filament,
    settings: &PrintSettings,
    tools: &gcode::Tools,
    progress: &dyn Fn(f32),
) -> Sliced {
    slice_plate_grouped(triangles, &[], printer, filament, settings, tools, progress)
}

/// The parts on the plate grouped by the settings they print with: `parts` are each
/// part's placed triangles and overrides. Parts with the same settings slice together.
/// Empty when no part has overrides (the whole plate prints with `plate`).
pub fn settings_groups(
    parts: &[(&[Triangle], &Overrides)],
    plate: &PrintSettings,
) -> Vec<(Vec<Triangle>, PrintSettings)> {
    if parts.iter().all(|(_, o)| o.is_empty()) {
        return Vec::new();
    }
    let mut groups: Vec<(Vec<Triangle>, PrintSettings)> = Vec::new();
    for (tris, overrides) in parts {
        let settings = overrides.apply(plate);
        match groups.iter_mut().find(|(_, s)| *s == settings) {
            Some((all, _)) => all.extend_from_slice(tris),
            None => groups.push((tris.to_vec(), settings)),
        }
    }
    groups
}

/// Like [`slice_plate_with_tools`], with parts that print with settings of their own:
/// `groups` from [`settings_groups`] (empty: everything prints with `settings`).
pub fn slice_plate_grouped(
    triangles: &[Triangle],
    groups: &[(Vec<Triangle>, PrintSettings)],
    printer: &Printer,
    filament: &Filament,
    settings: &PrintSettings,
    tools: &gcode::Tools,
    progress: &dyn Fn(f32),
) -> Sliced {
    let mut result =
        slicer::slice_groups(triangles, groups, settings, line_width(printer), progress);
    // The shield goes round what is printed, so it is added before the tower (which
    // stands apart).
    add_ooze_shield(&mut result, printer, tools);
    add_prime_tower(&mut result, printer, tools);
    let mut gcode = gcode::generate_with_tools(&result, printer, filament, settings, tools);
    let (text, picture) = crate::thumbnail::add_to_gcode(&gcode.text, triangles);
    gcode.text = text;
    Sliced {
        result,
        gcode,
        picture,
    }
}

/// Adds the ooze shield to every layer up to the last tool change: above it no nozzle sits
/// idle, so there is nothing to catch.
fn add_ooze_shield(result: &mut slicer::SliceResult, printer: &Printer, tools: &gcode::Tools) {
    let Some(distance) = tools
        .shield
        .filter(|_| printer.extruders >= 2 && !tools.ditto)
    else {
        return;
    };
    let Some(top) = gcode::last_tool_change(result, tools) else {
        return;
    };
    let Some(outline) = gcode::ooze_shield_outline(result, distance) else {
        return;
    };
    for layer in result.layers.iter_mut().take(top + 1) {
        layer.paths.push(slicer::ToolPath {
            kind: slicer::PathKind::OozeShield,
            points: outline.clone(),
            closed: true,
            width: None,
        });
    }
}

/// Adds the prime tower's lines to every layer up to the last tool change, so it is
/// printed (and previewed) under each change and never stands in the air.
fn add_prime_tower(result: &mut slicer::SliceResult, printer: &Printer, tools: &gcode::Tools) {
    let Some(centre) = tools
        .tower
        .filter(|_| printer.extruders >= 2 && !tools.ditto)
    else {
        return;
    };
    let Some(top) = gcode::last_tool_change(result, tools) else {
        return;
    };
    let width = result.line_width;
    for (li, layer) in result.layers.iter_mut().enumerate().take(top + 1) {
        layer
            .paths
            .extend(gcode::prime_tower_paths(centre, width, li));
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

    /// The two-nozzle Creator Pro preset.
    fn dual_printer() -> Printer {
        Printer::presets()
            .into_iter()
            .find(|p| p.extruders == 2)
            .expect("a two-nozzle preset")
    }

    /// Cubes 10 mm across at the given distances left/right of the plate's centre.
    fn cubes(printer: &Printer, xs: &[f32], tools: &gcode::Tools, supports: bool) -> String {
        use crate::model::{Mesh, PlacedObject};
        let mut tris = Vec::new();
        for &x in xs {
            let mut o = PlacedObject::new("cube", Mesh::cube(Vec3::new(10.0, 10.0, 1.0)));
            o.x = x;
            tris.extend(o.world_triangles());
        }
        let settings = PrintSettings {
            supports,
            ..PrintSettings::default()
        };
        slice_plate_with_tools(
            &tris,
            printer,
            &Filament::presets()[0],
            &settings,
            tools,
            &|_| {},
        )
        .gcode
        .text
    }

    /// Two cubes, 30 mm left and right of the plate's centre, on the Creator Pro preset.
    fn two_cubes(tools: &gcode::Tools, supports: bool) -> String {
        cubes(&dual_printer(), &[-30.0, 30.0], tools, supports)
    }

    /// The left cube on nozzle `left`, the right one on `right`, with the left nozzle (T1)
    /// set as the primary, so the right nozzle has the second filament.
    fn left_and_right(left: u8, right: u8) -> gcode::Tools {
        gcode::Tools {
            parts: vec![
                ([-35.0, -5.0, -25.0, 5.0], left),
                ([25.0, -5.0, 35.0, 5.0], right),
            ],
            primary: 1,
            support_tool: 1,
            second: Some(Filament::presets()[1].clone()),
            ..Default::default()
        }
    }

    #[test]
    fn test_tool_change_gcode_comes_before_each_tool_change() {
        let mut printer = dual_printer();
        printer.tool_change_gcode = "M117 changing".into();
        let text = cubes(&printer, &[-30.0, 30.0], &left_and_right(1, 0), false);
        let lines: Vec<&str> = text.lines().collect();
        // Tool changes during the print (after the first layer marker).
        let first_layer = lines.iter().position(|l| l.starts_with(";LAYER:")).unwrap();
        let changes: Vec<usize> = (first_layer..lines.len())
            .filter(|&i| lines[i] == "T0" || lines[i] == "T1")
            .collect();
        assert!(!changes.is_empty());
        for i in changes {
            assert!(
                lines[..i]
                    .iter()
                    .rev()
                    .take(4)
                    .any(|l| *l == "M117 changing"),
                "no custom code before line {i}: {}",
                lines[i]
            );
        }
    }

    #[test]
    fn test_tool_change_placeholders_name_both_nozzles() {
        let mut printer = dual_printer();
        printer.tool_change_gcode = "M117 {tool}>{next_tool}".into();
        let text = cubes(&printer, &[-30.0, 30.0], &left_and_right(1, 0), false);
        assert!(
            text.contains("M117 1>0\n") || text.contains("M117 0>1\n"),
            "no tool-change note"
        );
        assert!(!text.contains("M117 0>0") && !text.contains("M117 1>1"));
    }

    #[test]
    fn one_nozzle_printers_get_the_same_g_code_as_before() {
        use crate::model::{Mesh, PlacedObject};
        let printer = Printer::presets().remove(0);
        assert_eq!(printer.extruders, 1);
        let tris =
            PlacedObject::new("cube", Mesh::cube(Vec3::new(10.0, 10.0, 1.0))).world_triangles();
        let plain = slice_plate(
            &tris,
            &printer,
            &Filament::presets()[0],
            &PrintSettings::default(),
            &|_| {},
        );
        // Asking for the second nozzle changes nothing on a printer that has one.
        let asked = slice_plate_with_tools(
            &tris,
            &printer,
            &Filament::presets()[0],
            &PrintSettings::default(),
            &left_and_right(1, 1),
            &|_| {},
        );
        assert_eq!(plain.gcode.text, asked.gcode.text);
        assert!(!plain.gcode.text.lines().any(|l| l.starts_with('T')));
    }

    #[test]
    fn two_nozzles_print_each_part_with_its_own() {
        let printer = dual_printer();
        let g = two_cubes(&left_and_right(0, 1), false);
        let main = &Filament::presets()[0];
        let second = &Filament::presets()[1];
        // T1 (left, primary) has the main filament, T0 the second one.
        assert!(
            g.contains(&format!("M104 T1 S{}", main.nozzle_temp)),
            "primary heated"
        );
        assert!(
            g.contains(&format!("M104 T0 S{}", second.nozzle_temp)),
            "other heated"
        );
        assert!(
            g.contains(&format!("M109 T0 S{}", second.nozzle_temp)),
            "and waited for"
        );
        assert!(g.contains("T1 ; start with nozzle 1"), "the primary starts");
        assert!(
            g.contains("M104 T0 S0") && g.contains("M104 T1 S0"),
            "both off at the end"
        );
        let lines: Vec<&str> = g.lines().collect();
        let layers = g.matches(";LAYER:").count();
        let changes: Vec<usize> = (0..lines.len())
            .filter(|&i| lines[i] == "T0" || lines[i] == "T1")
            .collect();
        // Each layer prints the selected nozzle's part first: one change per layer.
        assert!(changes.len() >= layers - 1 && changes.len() <= layers);
        for &i in &changes {
            // Set aside: pulled back, then standby; afterwards heated back and primed by
            // exactly what it was pulled back.
            assert!(lines[i - 2].contains("set nozzle"), "{}", lines[i - 2]);
            assert!(lines[i - 1].ends_with("; standby"), "{}", lines[i - 1]);
            assert!(lines[i + 2].ends_with("; back to printing temperature"));
            assert_eq!(
                lines[i + 3],
                format!(
                    "G1 E{:.5} F{:.0} ; prime",
                    printer.tool_change_retract,
                    printer.retract_speed * 60.0
                )
            );
        }
        // Standby is the printing temperature less the drop.
        let standby_t0 = second.nozzle_temp - printer.standby_drop;
        assert!(g.contains(&format!("M104 T0 S{standby_t0} ; standby")));
        // The left cube's lines are printed by T0 and the right one's by T1.
        let mut tool = 9;
        let mut seen = [false; 2];
        for l in &lines {
            if let Some(t) = l.strip_prefix('T').and_then(|t| t.chars().next()) {
                tool = t.to_digit(10).unwrap_or(9);
            } else if l.starts_with("G1 X") && l.contains(" E") && !l.contains("; wipe") {
                let x: f32 = l[4..].split(' ').next().unwrap().parse().unwrap();
                let plate_x = x - printer.bed_x / 2.0;
                if plate_x < -20.0 && plate_x > -40.0 {
                    assert_eq!(tool, 0, "left part on T0: {l}");
                    seen[0] = true;
                } else if plate_x > 20.0 && plate_x < 40.0 {
                    assert_eq!(tool, 1, "right part on T1: {l}");
                    seen[1] = true;
                }
            }
        }
        assert_eq!(seen, [true, true]);
    }

    /// A single cube on the given printer's primary nozzle.
    fn plain_plate(printer: &Printer) -> String {
        let tools = gcode::Tools {
            parts: vec![([-5.0, -5.0, 5.0, 5.0], printer.primary_tool())],
            primary: printer.primary_tool(),
            support_tool: printer.primary_tool(),
            ..Default::default()
        };
        cubes(printer, &[0.0], &tools, false)
    }

    #[test]
    fn filament_use_is_worked_out_for_each_nozzle() {
        use crate::model::{Mesh, PlacedObject};
        let printer = dual_printer();
        let mut tris = Vec::new();
        for x in [-30.0, 30.0] {
            let mut o = PlacedObject::new("cube", Mesh::cube(Vec3::new(10.0, 10.0, 1.0)));
            o.x = x;
            tris.extend(o.world_triangles());
        }
        let main = Filament::presets()[0].clone();
        let second = Filament::presets()
            .into_iter()
            .find(|f| f.density != main.density)
            .expect("a filament of another density");
        let tools = gcode::Tools {
            second: Some(second.clone()),
            ..left_and_right(0, 1)
        };
        let e = slice_plate_with_tools(
            &tris,
            &printer,
            &main,
            &PrintSettings::default(),
            &tools,
            &|_| {},
        )
        .gcode
        .estimate;
        let n = e.per_nozzle.expect("both nozzles print");
        assert!(
            (n[0].0 + n[1].0 - e.filament_mm).abs() < 0.01,
            "{n:?} vs {}",
            e.filament_mm
        );
        // T1 is the primary here (main filament), T0 has the second one.
        let grams = |mm: f32, f: &Filament| {
            mm * std::f32::consts::PI * (f.diameter / 2.0).powi(2) / 1000.0 * f.density
        };
        assert!((n[1].1 - grams(n[1].0, &main)).abs() < 1e-3);
        assert!((n[0].1 - grams(n[0].0, &second)).abs() < 1e-3);
        assert!((e.grams - n[0].1 - n[1].1).abs() < 1e-3);
        // One-nozzle prints have no per-nozzle figures.
        let one = slice_plate(
            &tris,
            &Printer::presets().remove(0),
            &main,
            &PrintSettings::default(),
            &|_| {},
        );
        assert_eq!(one.gcode.estimate.per_nozzle, None);
    }

    #[test]
    fn the_incoming_nozzle_primes_on_the_tower_after_each_change() {
        use crate::model::{Mesh, PlacedObject};
        let printer = dual_printer();
        let centre = crate::geometry::Point2::new(80.0, 50.0);
        let tools = gcode::Tools {
            tower: Some(centre),
            ..left_and_right(0, 1)
        };
        let mut tris = Vec::new();
        for (x, h) in [(-30.0, 1.0), (30.0, 2.0)] {
            let mut o = PlacedObject::new("cube", Mesh::cube(Vec3::new(10.0, 10.0, h)));
            o.x = x;
            tris.extend(o.world_triangles());
        }
        let sliced = slice_plate_with_tools(
            &tris,
            &printer,
            &Filament::presets()[0],
            &PrintSettings::default(),
            &tools,
            &|_| {},
        );
        let tower_layers: Vec<usize> = sliced
            .result
            .layers
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                l.paths
                    .iter()
                    .any(|p| p.kind == crate::slicer::PathKind::PrimeTower)
            })
            .map(|(i, _)| i)
            .collect();
        // Both cubes share the first 1 mm (5 layers); above that only the taller one
        // prints, so the tower stops at the last layer with a change.
        let top = gcode::last_tool_change(&sliced.result, &tools).unwrap();
        assert_eq!(tower_layers, (0..=top).collect::<Vec<_>>());
        assert!(
            top < sliced.result.layers.len() - 1,
            "no tower above the last change"
        );
        // After every tool change the first extrusion is on the tower.
        let g = &sliced.gcode.text;
        let lines: Vec<&str> = g.lines().collect();
        let on_tower = |l: &str| {
            let mut w = l.split_whitespace();
            let (Some(x), Some(y)) = (w.nth(1), w.next()) else {
                return false;
            };
            let x: f32 = x[1..].parse().unwrap_or(f32::MAX) - printer.bed_x / 2.0;
            let y: f32 = y[1..].parse().unwrap_or(f32::MAX) - printer.bed_y / 2.0;
            (x - centre.x).abs() <= 6.5 && (y - centre.y).abs() <= 6.5
        };
        let mut changes = 0;
        for i in (0..lines.len()).filter(|&i| lines[i] == "T0" || lines[i] == "T1") {
            let first = lines[i..]
                .iter()
                .find(|l| l.starts_with("G1 X") && l.contains(" E"))
                .unwrap();
            assert!(on_tower(first), "after {}: {first}", lines[i]);
            changes += 1;
        }
        assert!(changes > 2);
    }

    #[test]
    fn the_ooze_shield_surrounds_the_print_up_to_the_last_tool_change() {
        use crate::model::{Mesh, PlacedObject};
        use crate::slicer::PathKind;
        let printer = dual_printer();
        let tools = gcode::Tools {
            shield: Some(3.0),
            ..left_and_right(0, 1)
        };
        let mut tris = Vec::new();
        for (x, h) in [(-30.0, 1.0), (30.0, 2.0)] {
            let mut o = PlacedObject::new("cube", Mesh::cube(Vec3::new(10.0, 10.0, h)));
            o.x = x;
            tris.extend(o.world_triangles());
        }
        let sliced = slice_plate_with_tools(
            &tris,
            &printer,
            &Filament::presets()[0],
            &PrintSettings::default(),
            &tools,
            &|_| {},
        );
        let layers = &sliced.result.layers;
        let top = gcode::last_tool_change(&sliced.result, &tools).unwrap();
        let shielded: Vec<usize> = (0..layers.len())
            .filter(|&i| {
                layers[i]
                    .paths
                    .iter()
                    .any(|p| p.kind == PathKind::OozeShield)
            })
            .collect();
        assert_eq!(shielded, (0..=top).collect::<Vec<_>>());
        // It lies outside every other line, by at least the distance from the hull of
        // them (a convex hull: so check against its bounds).
        let shield = layers[0]
            .paths
            .iter()
            .find(|p| p.kind == PathKind::OozeShield)
            .unwrap();
        let others: Vec<_> = layers
            .iter()
            .flat_map(|l| &l.paths)
            .filter(|p| p.kind != PathKind::OozeShield)
            .flat_map(|p| p.points.iter().copied())
            .collect();
        let (minx, maxx) = others
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.x), b.max(p.x)));
        let (sminx, smaxx) = shield
            .points
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.x), b.max(p.x)));
        assert!(
            sminx <= minx - 2.99 && smaxx >= maxx + 2.99,
            "{sminx} {minx} / {smaxx} {maxx}"
        );
        // Each layer that has it prints it first.
        let g = &sliced.gcode.text;
        for block in g.split(";LAYER:").skip(1).take(top + 1) {
            let first_type = block.lines().find(|l| l.starts_with(";TYPE:")).unwrap();
            assert_eq!(first_type, ";TYPE:SUPPORT", "the shield is first");
        }
    }

    #[test]
    fn one_nozzle_plates_get_no_tower() {
        let printer = dual_printer();
        let tools = gcode::Tools {
            parts: vec![([-5.0, -5.0, 5.0, 5.0], 0)],
            tower: Some(crate::geometry::Point2::new(80.0, 50.0)),
            ..Default::default()
        };
        let g = cubes(&printer, &[0.0], &tools, false);
        assert!(!g.lines().any(|l| l == "T0" || l == "T1"));
        let sliced_kinds = g.matches(";TYPE:SUPPORT").count();
        assert_eq!(sliced_kinds, 0, "no tower lines");
    }

    #[test]
    fn a_plain_plate_prints_with_the_right_nozzle_alone_by_default() {
        let printer = dual_printer();
        assert_eq!(printer.primary_tool(), 0, "T0 is the default primary");
        let g = plain_plate(&printer);
        assert!(g.contains("M104 T0 S") && g.contains("T0 ; start with nozzle 0"));
        assert!(
            !g.contains("M104 T1") && !g.contains("M109 T1"),
            "T1 isn't heated"
        );
        assert!(!g.lines().any(|l| l == "T0" || l == "T1"), "no tool change");
    }

    #[test]
    fn the_primary_nozzle_is_a_printer_setting() {
        let printer = Printer {
            primary_nozzle: 1,
            ..dual_printer()
        };
        assert_eq!(printer.primary_tool(), 1);
        let g = plain_plate(&printer);
        assert!(g.contains("M104 T1 S") && g.contains("T1 ; start with nozzle 1"));
        assert!(
            !g.contains("M104 T0") && !g.contains("M109 T0"),
            "T0 isn't heated"
        );
        // A one-nozzle printer has no primary to choose.
        let single = Printer {
            primary_nozzle: 1,
            ..Printer::presets().remove(0)
        };
        assert_eq!(single.primary_tool(), 0);
    }

    #[test]
    fn retraction_settings_change_where_and_how_it_retracts() {
        let base = Printer::presets().remove(0);
        let count = |g: &str, pat: &str| g.lines().filter(|l| l.contains(pat)).count();
        let plain = cubes(&base, &[-20.0, 20.0], &gcode::Tools::default(), false);
        let retracts = |g: &str| {
            g.lines()
                .filter(|l| l.starts_with("G1 E") && !l.contains("retract"))
                .count()
        };

        // A lower travel threshold retracts on more travels.
        let short = Printer {
            retract_min_travel: 0.2,
            ..base.clone()
        };
        let g = cubes(&short, &[-20.0, 20.0], &gcode::Tools::default(), false);
        assert!(
            retracts(&g) > retracts(&plain),
            "{} vs {}",
            retracts(&g),
            retracts(&plain)
        );

        // Retracting on layer changes: every layer after the first retracts before its
        // Z move.
        let layered = Printer {
            retract_layer_change: true,
            ..base.clone()
        };
        let g = cubes(&layered, &[0.0], &gcode::Tools::default(), false);
        let lines: Vec<&str> = g.lines().collect();
        for i in
            (0..lines.len()).filter(|&i| lines[i].starts_with(";LAYER:") && lines[i] != ";LAYER:0")
        {
            let z = (i..lines.len())
                .find(|&k| lines[k].starts_with("G1 Z"))
                .unwrap();
            assert!(
                lines[i..z].iter().any(|l| l.starts_with("G1 E")),
                "no retraction before {}",
                lines[i]
            );
        }

        // Wiping: the retraction is spread over a move back along the last line.
        let wiping = Printer {
            wipe_mm: 2.0,
            ..base.clone()
        };
        let g = cubes(&wiping, &[-20.0, 20.0], &gcode::Tools::default(), false);
        assert!(count(&g, "; wipe") > 0);
        assert_eq!(count(&plain, "; wipe"), 0, "off by default");
        // Each wipe pulls back exactly the retraction length in total.
        let lines: Vec<&str> = g.lines().collect();
        let e_of = |l: &str| -> f64 {
            l.split_whitespace()
                .find_map(|w| w.strip_prefix('E'))
                .unwrap()
                .parse()
                .unwrap()
        };
        let mut i = 1;
        let mut checked = 0;
        while i < lines.len() {
            if lines[i].ends_with("; wipe") && !lines[i - 1].ends_with("; wipe") {
                let before = (0..i).rev().find_map(|k| {
                    let l = lines[k];
                    (l.starts_with("G1 ") && l.contains(" E")).then(|| e_of(l))
                });
                let mut end = i;
                while lines[end + 1].ends_with("; wipe") {
                    end += 1;
                }
                let pulled = before.unwrap() - e_of(lines[end]);
                assert!(
                    (pulled - base.retract_length as f64).abs() < 1e-3,
                    "{pulled}"
                );
                checked += 1;
                i = end;
            }
            i += 1;
        }
        assert!(checked > 0);
    }

    #[test]
    fn ditto_heats_the_second_nozzle_and_never_switches() {
        let tools = gcode::Tools {
            ditto: true,
            ..Default::default()
        };
        let g = two_cubes(&tools, false);
        assert!(g.contains("M104 T1 S"), "second nozzle heated for the copy");
        assert!(g.contains("M104 T1 S0"));
        assert!(
            !g.lines().any(|l| l == "T1" || l.starts_with("T1 ")),
            "no tool change"
        );
    }

    #[test]
    fn a_two_nozzle_print_translates_to_x3g_with_tool_changes() {
        use crate::x3g::command::Action;
        use crate::x3g::{machine::Machine, translate};
        let g = two_cubes(&left_and_right(0, 1), false);
        let o = translate::Options::from_gcode(&g, "two").unwrap();
        let t = translate::translate(&g, &Machine::creator_pro(), &o).unwrap();
        let changes = t
            .actions
            .iter()
            .filter(|a| matches!(a, Action::ChangeTool { .. }))
            .count();
        assert!(changes > 2, "{changes} tool changes");
        let b_moves = t
            .actions
            .iter()
            .filter(|a| matches!(a, Action::Move { steps, .. } if steps[4] != 0))
            .count();
        assert!(
            b_moves > 50,
            "the second extruder (B) is driven: {b_moves} moves"
        );
        assert!(t.actions.contains(&Action::WaitForTool {
            tool: 1,
            timeout_s: u16::MAX
        }));
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

    #[test]
    fn test_settings_groups_part_with_overrides_slices_with_them() {
        use crate::model::{Mesh, PlacedObject};
        use crate::settings_registry::Value;
        let place = |x: f32, infill: Option<f32>| {
            let mut o = PlacedObject::new("box", Mesh::cube(Vec3::new(10.0, 10.0, 5.0)));
            o.x = x;
            if let Some(v) = infill {
                o.overrides.set("infill_percent", Value::Num(v));
            }
            o
        };
        let plate = PrintSettings::default();
        let objects = [
            place(-15.0, Some(0.0)),
            place(15.0, None),
            place(40.0, None),
        ];
        let placed: Vec<Vec<Triangle>> = objects.iter().map(|o| o.world_triangles()).collect();
        let parts: Vec<(&[Triangle], &Overrides)> = placed
            .iter()
            .zip(&objects)
            .map(|(t, o)| (t.as_slice(), &o.overrides))
            .collect();
        let groups = settings_groups(&parts, &plate);
        assert_eq!(groups.len(), 2, "the two plain boxes slice together");
        assert_eq!(groups[0].1.infill_percent, 0.0);
        assert_eq!(groups[1].0.len(), 24);
        let all: Vec<Triangle> = placed.concat();
        let printer = Printer::presets().remove(0);
        let filament = &Filament::presets()[0];
        let tools = gcode::Tools::default();
        let out = slice_plate_grouped(&all, &groups, &printer, filament, &plate, &tools, &|_| {});
        let sparse_left = out
            .result
            .layers
            .iter()
            .flat_map(|l| &l.paths)
            .filter(|p| p.kind == slicer::PathKind::SparseInfill)
            .any(|p| p.points.iter().any(|q| q.x < 0.0));
        assert!(!sparse_left, "the left box has no infill");
        // No overrides: no groups, and the plate slices exactly as before.
        let plain: Vec<(&[Triangle], &Overrides)> = parts
            .iter()
            .map(|(t, _)| (*t, &objects[1].overrides))
            .collect();
        assert!(settings_groups(&plain, &plate).is_empty());
    }
}
