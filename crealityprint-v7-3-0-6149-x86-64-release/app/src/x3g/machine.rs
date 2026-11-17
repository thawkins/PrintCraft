//! What the translator needs to know about a MakerBot-protocol printer: X3G moves are
//! in motor steps, so every axis's steps per mm must be right or the print comes out the
//! wrong size.
//!
//! The starting values are GPX's `r1d` machine (Replicator 1 Dual), which GPX also uses for
//! the FlashForge Creator Pro (`fcp`); early Creator Pro machines are clones of it. They
//! are facts read from GPX's `std_machines.h`, not GPX code. The values read from the
//! printer's own EEPROM (`MachineSettings`) replace them wherever the printer has one.

use super::query::MachineSettings;

/// Axis order everywhere: X, Y, Z, A (right extruder), B (left extruder).
#[derive(Clone, Debug, PartialEq)]
pub struct Machine {
    pub name: String,
    pub steps_per_mm: [f64; 5],
    /// mm/min.
    pub max_feedrate: [f64; 5],
    /// Homing speed for X, Y, Z, mm/min.
    pub home_feedrate: [f64; 3],
    /// Whether X, Y and Z home towards their maximum end.
    pub home_to_max: [bool; 3],
    /// Where the nozzle is after homing, in the machine's own coordinates (mm, with 0, 0
    /// at the bed's centre). Only used to work out the length (and so the speed) of the
    /// first move after homing; the printer itself takes its stored home position.
    pub home_mm: [f64; 3],
    pub extruders: u8,
    pub heated_bed: bool,
    /// USB product id of the board family, written at the start of an X3G file.
    pub bot_pid: u16,
    /// Seconds to wait for homing or heating before the printer gives up.
    pub timeout_s: u16,
}

/// GPX's figure for the extruders' steps per mm (`r1d`, `r2`).
const EXTRUDER_STEPS_PER_MM: f64 = 96.275_201_870_333_66;

impl Machine {
    /// The FlashForge Creator Pro family (early Creator Pro 2 included): two nozzles on
    /// one carriage, heated bed. GPX `r1d` values.
    pub fn creator_pro() -> Machine {
        Machine {
            name: "FlashForge Creator Pro (Sailfish)".into(),
            steps_per_mm: [
                94.117647,
                94.117647,
                400.0,
                EXTRUDER_STEPS_PER_MM,
                EXTRUDER_STEPS_PER_MM,
            ],
            max_feedrate: [18000.0, 18000.0, 1170.0, 1600.0, 1600.0],
            home_feedrate: [2500.0, 2500.0, 1100.0],
            home_to_max: [true, true, false],
            // Estimated: the ReplicatorG start code for this family set X 152, Y 75, Z 0
            // after homing. Replaced by the EEPROM's home position when it is known.
            home_mm: [152.0, 75.0, 0.0],
            extruders: 2,
            heated_bed: true,
            // GPX writes the Replicator 1 board's id (0xD314) for this family.
            bot_pid: 0xD314,
            // GPX's `r1d` timeout is 20 s for homing; heating gets longer (see translate.rs).
            timeout_s: 20,
        }
    }

    /// This machine with the values the printer's EEPROM holds put in place of the
    /// defaults. Values the EEPROM doesn't have (erased, or not read) keep the default.
    pub fn with_settings(mut self, s: &MachineSettings) -> Machine {
        for i in 0..5 {
            if let Some(spm) = s.steps_per_mm[i].filter(|v| *v > 0.0) {
                self.steps_per_mm[i] = spm;
            }
            if let Some(f) = s.max_feedrate[i].filter(|v| *v > 0) {
                self.max_feedrate[i] = f as f64 * 60.0;
            }
        }
        // An erased home-direction byte reads as 0 ("all to min"), which no real machine
        // of this family uses, so only a byte with some bit set is taken.
        if s.home_direction & 0b111 != 0 {
            for (i, to_max) in self.home_to_max.iter_mut().enumerate() {
                *to_max = s.home_direction & (1 << i) != 0;
            }
        }
        for i in 0..3 {
            if let Some(steps) = s.home_steps[i] {
                self.home_mm[i] = steps as f64 / self.steps_per_mm[i];
            }
        }
        if let Some(t) = s.tool_count.filter(|t| (1..=2).contains(t)) {
            self.extruders = t;
        }
        if let Some(bed) = s.heated_bed {
            self.heated_bed = bed;
        }
        if !s.name.is_empty() {
            self.name = format!("{} (from its settings)", s.name);
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_creator_pro_starts_from_the_replicator_dual_values() {
        let m = Machine::creator_pro();
        assert_eq!(m.steps_per_mm[0], 94.117647);
        assert_eq!(m.steps_per_mm[2], 400.0);
        assert_eq!(m.home_to_max, [true, true, false]);
        assert_eq!(m.extruders, 2);
        assert!(m.heated_bed);
    }

    #[test]
    fn the_printers_own_settings_take_priority() {
        let mut eeprom = vec![0xFF; 4096];
        eeprom[0x22..0x22 + 4].copy_from_slice(b"CP2\0");
        eeprom[0x42] = 1;
        eeprom[0x4C] = 0;
        eeprom[0x0C] = 0b011;
        eeprom[0x01A4..0x01A8].copy_from_slice(&88_888_889u32.to_le_bytes());
        // X home position 13 600 steps.
        eeprom[0x0E..0x12].copy_from_slice(&13_600i32.to_le_bytes());
        let s = MachineSettings::from_eeprom(&eeprom);
        let m = Machine::creator_pro().with_settings(&s);
        assert!((m.steps_per_mm[0] - 88.888889).abs() < 1e-9);
        assert_eq!(m.steps_per_mm[1], 94.117647, "Y wasn't in the EEPROM copy");
        assert!((m.home_mm[0] - 13_600.0 / 88.888889).abs() < 1e-6);
        assert_eq!(m.home_mm[1], 75.0);
        assert_eq!(m.extruders, 1);
        assert!(!m.heated_bed);
        assert_eq!(m.name, "CP2 (from its settings)");
    }
}
