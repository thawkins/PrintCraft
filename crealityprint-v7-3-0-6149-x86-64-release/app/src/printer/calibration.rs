//! "Calibrate before printing": measure the plate (bed levelling) at the start of the job.
//!
//! Klipper printers reached through Moonraker use Klipper's documented
//! BED_MESH_CALIBRATE. Marlin and Prusa firmware (PrusaLink and USB printing) use the
//! standard G29. Printers on the Creality network service level by themselves, so nothing
//! is added for them (see device_ui `levels_itself`).
//!
//! The step goes in once both the plate **and the nozzle** have reached the job's
//! temperatures (after whichever of the two waits comes later), so the plate is measured
//! hot and the nozzle never cools down and heats up again
//! around the measurement. Afterwards the job temperature is set again, waiting only if the
//! nozzle has to heat back up (see heat.rs), in case the levelling routine changed it.

/// The levelling command for Klipper printers.
pub const KLIPPER_LEVEL: &str = "BED_MESH_CALIBRATE";
/// The levelling command for Marlin and Prusa firmware.
pub const MARLIN_LEVEL: &str = "G29";

/// The nozzle temperature the file heats to before printing, if any.
pub fn job_nozzle_temp(gcode: &str) -> Option<u32> {
    crate::heat::nozzle_target(gcode)
}

/// The calibration lines for one printer: home, level, then back at the job temperature
/// (waiting only if the nozzle needs to heat up again).
pub fn calibration_lines(klipper: bool, nozzle: Option<u32>) -> String {
    let level = if klipper { KLIPPER_LEVEL } else { MARLIN_LEVEL };
    let mut s = String::from("G28 ; home again now the plate and nozzle are hot\n");
    if let Some(t) = nozzle {
        s.push_str(&format!(
            "M104 S{t} ; keep the nozzle at the job temperature while measuring\n"
        ));
    }
    s.push_str(&format!(
        "{level} ; printer calibration: measure the plate\n"
    ));
    if let Some(t) = nozzle {
        s.push_str(&crate::heat::wait_nozzle(t, klipper));
    }
    s
}

/// The print file with the calibration step added (once) after the later of the nozzle-
/// and plate-heat waits, or after the first home move, or at the very start.
pub fn with_calibration(gcode: &str, klipper: bool) -> String {
    // Only commands count: a comment mentioning the step does not mean it runs.
    let already = gcode.lines().any(|l| {
        let code = l.split(';').next().unwrap_or("").trim();
        code.starts_with(MARLIN_LEVEL) || code.starts_with(KLIPPER_LEVEL)
    });
    if already {
        return gcode.to_string();
    }
    let lines = calibration_lines(klipper, job_nozzle_temp(gcode));
    let after = |test: &dyn Fn(&str) -> bool| {
        let mut offset = 0;
        for line in gcode.split_inclusive('\n') {
            offset += line.len();
            if test(line) {
                return Some(offset);
            }
        }
        None
    };
    let heated = match (
        after(&crate::heat::is_nozzle_wait),
        after(&crate::heat::is_bed_wait),
    ) {
        (Some(nozzle), Some(bed)) => Some(nozzle.max(bed)),
        (nozzle, bed) => nozzle.or(bed),
    };
    let at = heated
        .or_else(|| after(&|l: &str| l.trim_start().starts_with("G28")))
        .unwrap_or(0);
    let mut out = String::with_capacity(gcode.len() + lines.len() + 1);
    out.push_str(&gcode[..at]);
    if at > 0 && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&lines);
    out.push_str(&gcode[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_the_plate_too_when_it_heats_after_the_nozzle() {
        let out = with_calibration("M109 S215\nM190 S60\nG1 X1\n", true);
        let bed_wait = out.find("M190 S60").unwrap();
        let cal = out.find(KLIPPER_LEVEL).unwrap();
        assert!(cal > bed_wait, "{out}");
    }

    #[test]
    fn a_comment_naming_the_step_does_not_count_as_calibrating() {
        let g = "; BED_MESH_CALIBRATE disabled\nM190 S60\nM109 S210\nG1 X1\n";
        let out = with_calibration(g, true);
        assert_eq!(out.matches(KLIPPER_LEVEL).count(), 2, "{out}");
    }

    #[test]
    fn calibrates_with_the_nozzle_already_at_job_temperature() {
        let g = "M140 S60\nM104 S210\nG28 ; home\nM190 S60 ; wait for bed\nM109 S210 ; wait for nozzle\nG1 X1\n";
        let out = with_calibration(g, true);
        let bed = out.find("M190").unwrap();
        let nozzle_wait = out.find("M109 S210 ; wait for nozzle").unwrap();
        let cal = out.find("BED_MESH_CALIBRATE").unwrap();
        let print = out.find("G1 X1").unwrap();
        assert!(
            bed < nozzle_wait && nozzle_wait < cal && cal < print,
            "{out}"
        );
        // Never set to 0 °C; the job temperature is kept before and checked after.
        assert!(!out.contains("M104 S0") && !out.contains("M109 S0"));
        let after = &out[cal..print];
        assert!(
            after.contains("TEMPERATURE_WAIT SENSOR=extruder MINIMUM=209"),
            "{out}"
        );
        assert!(
            !after.contains("M109"),
            "Klipper waits only while heating: {out}"
        );
        assert!(out[nozzle_wait..cal].contains("M104 S210"), "{out}");
        assert_eq!(out.matches("BED_MESH_CALIBRATE").count(), 1);
    }

    #[test]
    fn reads_the_job_temperature() {
        assert_eq!(job_nozzle_temp("M104 S200\nM109 S215 ; wait\n"), Some(215));
        assert_eq!(job_nozzle_temp("M109 R240\n"), Some(240));
        assert_eq!(job_nozzle_temp("; M109 S1\nG1 X1\n"), None);
    }

    #[test]
    fn added_only_once() {
        let once = with_calibration("G28\nG1 X1\n", true);
        assert_eq!(with_calibration(&once, true), once);
    }

    #[test]
    fn falls_back_to_start_of_file() {
        assert!(with_calibration("G1 X1\n", true).starts_with("G28"));
    }

    #[test]
    fn our_own_print_file_gets_it_after_both_heat_waits() {
        use crate::geometry::Vec3;
        use crate::model::{Mesh, PlacedObject};
        use crate::profiles::{Filament, PrintSettings, Printer};
        let tris = PlacedObject::new("c", Mesh::cube(Vec3::new(10.0, 10.0, 2.0))).world_triangles();
        let s = PrintSettings::default();
        let r = crate::slicer::slice(&tris, &s, 0.42, &|_| {});
        let f = &Filament::presets()[0];
        let g = crate::gcode::generate(&r, &Printer::presets()[0], f, &s).text;
        let out = with_calibration(&g, true);
        let cal = out.find("BED_MESH_CALIBRATE").unwrap();
        assert!(out.find("M190").unwrap() < cal);
        assert!(out.find("M109").unwrap() < cal);
        assert!(cal < out.find(";LAYER:0").unwrap());
        assert!(out[cal..].contains(&format!("MINIMUM={}", f.nozzle_temp - 1)));
    }

    #[test]
    fn marlin_and_prusa_printers_get_g29() {
        let out = with_calibration("M190 S60\nM109 S210\n", false);
        assert!(out.contains("G29 ;") && !out.contains("BED_MESH"));
        assert_eq!(with_calibration(&out, false), out);
    }
}
