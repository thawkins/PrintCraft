//! Waiting for the nozzle and plate to reach temperature, but only ever while they heat
//! up. A nozzle or plate that is already hotter than the job needs (after a previous
//! print, say) is set to the new target and the print carries on without waiting for it
//! to cool.
//!
//! * Marlin and Prusa firmware: `M109 S<t>` / `M190 S<t>` already wait only while
//!   heating (the `R` form waits both ways), so those are used as they are.
//! * Klipper (Ender-3 V3 KE, Moonraker printers): `M109` / `M190` wait in both
//!   directions there, so the target is set with `M104` / `M140` and Klipper's documented
//!   `TEMPERATURE_WAIT SENSOR=… MINIMUM=…` waits only for heating up.

/// Klipper waits until this close below the target (heaters hover around it).
pub const WAIT_MARGIN_C: u32 = 1;
/// Hotter than any hobby nozzle; a larger number in someone else's file is a typo.
const MAX_PLAUSIBLE_C: f32 = 500.0;

/// Set the nozzle target and wait while it heats up (not while it cools).
pub fn wait_nozzle(t: u32, klipper: bool) -> String {
    wait("M104", "M109", "extruder", "nozzle", t, klipper)
}

/// Set the plate target and wait while it heats up (not while it cools).
pub fn wait_bed(t: u32, klipper: bool) -> String {
    wait("M140", "M190", "heater_bed", "plate", t, klipper)
}

fn wait(set: &str, marlin_wait: &str, sensor: &str, what: &str, t: u32, klipper: bool) -> String {
    if t == 0 {
        return format!("{set} S0 ; {what} heater off\n");
    }
    if klipper {
        format!(
            "{set} S{t} ; {what} to {t} °C\nTEMPERATURE_WAIT SENSOR={sensor} MINIMUM={} ; wait only while the {what} heats up\n",
            t.saturating_sub(WAIT_MARGIN_C)
        )
    } else {
        format!("{marlin_wait} S{t} ; wait only while the {what} heats up to {t} °C\n")
    }
}

/// For Klipper printers: turns every `M109 S…` / `M190 S…` in a file into "set, then wait
/// only while heating". Other lines (and `R` waits, which ask for both ways on purpose)
/// are left alone. Running it twice changes nothing more.
pub fn heat_up_only(gcode: &str) -> String {
    let mut out = String::with_capacity(gcode.len() + 256);
    for line in gcode.split_inclusive('\n') {
        let code = line.split(';').next().unwrap_or("").trim();
        let target = |prefix: &str| {
            let rest = code.strip_prefix(prefix)?;
            let rest = rest.strip_prefix(' ')?;
            rest.split_whitespace()
                .find_map(|w| w.strip_prefix('S'))
                .and_then(|v| v.parse::<f32>().ok())
                .map(|v| v.round() as u32)
        };
        if let Some(t) = target("M109") {
            out.push_str(&wait_nozzle(t, true));
        } else if let Some(t) = target("M190") {
            out.push_str(&wait_bed(t, true));
        } else {
            out.push_str(line);
            if !line.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    if !gcode.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    out
}

/// The nozzle temperature a file heats to before printing: from its first wait
/// (`M109 S…`, or Klipper's `TEMPERATURE_WAIT SENSOR=extruder MINIMUM=…`), else its first
/// non-zero `M104 S…`.
pub fn nozzle_target(gcode: &str) -> Option<u32> {
    let codes = || {
        gcode
            .lines()
            .map(|l| l.split(';').next().unwrap_or("").trim())
    };
    let s_value = |rest: &str| {
        rest.split_whitespace()
            .find_map(|w| w.strip_prefix('S').or_else(|| w.strip_prefix('R')))
            .and_then(|v| v.parse::<f32>().ok())
            .map(|t| t.round() as u32)
            .filter(|t| *t > 0)
    };
    codes()
        .find_map(|c| {
            if let Some(rest) = c.strip_prefix("M109") {
                return s_value(rest);
            }
            let rest = c.strip_prefix("TEMPERATURE_WAIT")?;
            if !rest.contains("SENSOR=extruder") {
                return None;
            }
            rest.split_whitespace()
                .find_map(|w| w.strip_prefix("MINIMUM="))
                .and_then(|v| v.parse::<f32>().ok())
                .filter(|v| (0.0..=MAX_PLAUSIBLE_C).contains(v))
                .map(|v| v.round() as u32 + WAIT_MARGIN_C)
        })
        .or_else(|| codes().find_map(|c| s_value(c.strip_prefix("M104")?)))
}

/// True for lines that wait for the nozzle to heat (either firmware's form).
pub fn is_nozzle_wait(line: &str) -> bool {
    let c = line.trim_start();
    c.starts_with("M109") || (c.starts_with("TEMPERATURE_WAIT") && c.contains("SENSOR=extruder"))
}

/// True for lines that wait for the plate to heat (either firmware's form).
pub fn is_bed_wait(line: &str) -> bool {
    let c = line.trim_start();
    c.starts_with("M190") || (c.starts_with("TEMPERATURE_WAIT") && c.contains("SENSOR=heater_bed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absurd_wait_temperature_is_ignored_not_wrapped() {
        let g = "TEMPERATURE_WAIT SENSOR=extruder MINIMUM=1e10\n";
        assert_eq!(nozzle_target(g), None);
    }

    #[test]
    fn klipper_waits_only_while_heating() {
        let n = wait_nozzle(210, true);
        assert!(n.starts_with("M104 S210"));
        assert!(n.contains("TEMPERATURE_WAIT SENSOR=extruder MINIMUM=209"));
        assert!(!n.contains("MAXIMUM"), "never waits for cooling");
        let b = wait_bed(60, true);
        assert!(
            b.starts_with("M140 S60")
                && b.contains("TEMPERATURE_WAIT SENSOR=heater_bed MINIMUM=59")
        );
        assert!(!b.contains("M190"));
    }

    #[test]
    fn marlin_keeps_its_heat_only_waits() {
        assert!(wait_nozzle(210, false).starts_with("M109 S210"));
        assert!(wait_bed(60, false).starts_with("M190 S60"));
        assert!(!wait_nozzle(210, false).contains("R"));
        assert_eq!(wait_nozzle(0, true), "M104 S0 ; nozzle heater off\n");
    }

    #[test]
    fn converts_existing_files_for_klipper() {
        let g = "M140 S60\nM190 S60 ; wait for bed\nM109 S215 ; wait\nM109 R200\nG1 X1";
        let out = heat_up_only(g);
        assert!(!out.contains("M190 S60") && !out.contains("M109 S215"));
        assert!(out.contains("TEMPERATURE_WAIT SENSOR=heater_bed MINIMUM=59"));
        assert!(out.contains("TEMPERATURE_WAIT SENSOR=extruder MINIMUM=214"));
        assert!(
            out.contains("M109 R200"),
            "an explicit both-ways wait is respected"
        );
        assert!(out.ends_with("G1 X1"));
        assert_eq!(heat_up_only(&out), out, "running it twice changes nothing");
    }

    #[test]
    fn finds_the_nozzle_target_in_either_form() {
        assert_eq!(nozzle_target("M104 S200\nM109 S215\n"), Some(215));
        assert_eq!(nozzle_target(&wait_nozzle(230, true)), Some(230));
        assert_eq!(nozzle_target("M104 S0\nM104 S205\n"), Some(205));
        assert_eq!(nozzle_target("G1 X1\n"), None);
    }
}
