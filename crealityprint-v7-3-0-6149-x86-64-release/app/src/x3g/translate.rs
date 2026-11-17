//! Turns the G-code PrintCraft writes into S3G commands for a MakerBot-protocol printer.
//! The same commands make an `.x3g` file ([`to_x3g`]) or are sent over USB one packet at
//! a time.
//!
//! Only the G-code PrintCraft itself writes has to be understood (see gcode.rs, heat.rs
//! and calibration.rs). A command the translator doesn't know is an error that names the
//! line, never a silent skip, so a new command in the slicer's output is noticed by the
//! test that translates a whole sliced plate.
//!
//! How moves are written follows MakerBot's specification and Sailfish's `Steppers.cc`
//! (command 155):
//! * X, Y and Z are absolute step counts in the machine's coordinates, which have 0, 0 at
//!   the bed's centre. PrintCraft's G-code has its origin at the front-left corner, so
//!   half the bed is taken off X and Y.
//! * The extruders are always relative. Their steps are negative for forward extrusion,
//!   as GPX writes them for this board family. Rounding remainders carry over to the next
//!   move, so no plastic is lost to rounding over a long print.
//! * The speed is given three ways: steps per second on the axis that moves furthest, the
//!   move's length in mm (the XYZ length, or the extrusion length for a move that only
//!   extrudes) and the feed rate in mm/s. The feed rate is capped so that no axis goes
//!   faster than the machine's limit.
//! * The nozzle offset of the second extruder is applied by the firmware, not here.

use super::command::{Action, A, B, XYZ};
use super::machine::Machine;

/// How long the printer waits for the nozzle or bed to heat. The largest value, as GPX
/// writes it: a shorter limit makes the printer carry on printing with a cold bed or
/// nozzle once it runs out (a big bed can take over ten minutes).
const HEAT_TIMEOUT_S: u16 = u16::MAX;

#[derive(Clone, Debug)]
pub struct Options {
    /// The bed's size in PrintCraft's coordinates (the printer profile), whose centre is
    /// the machine's 0, 0.
    pub bed_mm: (f64, f64),
    /// Shown on the printer's screen and stored as the build name.
    pub job_name: String,
    /// For the progress shown on the printer's screen; 0 when not known.
    pub total_layers: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Translation {
    pub actions: Vec<Action>,
    /// Things in the G-code that the printer can't do and were left out, once each.
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TranslateError {
    /// 1-based line number in the G-code.
    pub line_no: usize,
    pub line: String,
    pub why: String,
}

impl std::fmt::Display for TranslateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {} (\"{}\"): {}", self.line_no, self.line, self.why)
    }
}

impl Options {
    /// The options a sliced plate's own G-code gives: its plate size (the `; plate:`
    /// header PrintCraft writes) and its layer count (`;LAYER_COUNT:`).
    pub fn from_gcode(gcode: &str, job_name: &str) -> Result<Options, String> {
        let mut bed = None;
        let mut layers = 0;
        for line in gcode.lines().take(200) {
            if let Some(rest) = line.strip_prefix("; plate:") {
                let nums: Vec<f64> = rest
                    .split(|c: char| !(c.is_ascii_digit() || c == '.'))
                    .filter_map(|n| n.parse().ok())
                    .collect();
                if let [x, y, ..] = nums[..] {
                    bed = Some((x, y));
                }
            } else if let Some(n) = line.strip_prefix(";LAYER_COUNT:") {
                layers = n.trim().parse().unwrap_or(0);
            }
        }
        Ok(Options {
            bed_mm: bed.ok_or(
                "the print file doesn't say its plate size; slice it again with this \
                 version of PrintCraft",
            )?,
            job_name: job_name.to_string(),
            total_layers: layers,
        })
    }
}

/// The commands that home the printer (G28), without the build notices around a print.
pub fn home_actions(m: &Machine) -> Vec<Action> {
    let o = Options {
        bed_mm: (0.0, 0.0),
        job_name: String::new(),
        total_layers: 0,
    };
    translate("G28\n", m, &o)
        .map(|t| {
            t.actions
                .into_iter()
                .filter(|a| matches!(a, Action::Home { .. } | Action::RecallHome { .. }))
                .collect()
        })
        .unwrap_or_default()
}

/// One jog: move `axis` ('X', 'Y' or 'Z') by `mm` from wherever it is, at
/// `feed_mm_min` (capped at the axis's limit). Every axis is sent relative, so the
/// printer's position needn't be known. None for an unknown axis or a move under a step.
pub fn jog(m: &Machine, axis: char, mm: f64, feed_mm_min: f64) -> Option<Action> {
    let i = match axis.to_ascii_uppercase() {
        'X' => 0,
        'Y' => 1,
        'Z' => 2,
        _ => return None,
    };
    let steps = (mm * m.steps_per_mm[i]).round() as i32;
    if steps == 0 {
        return None;
    }
    let feed = feed_mm_min.min(m.max_feedrate[i]) / 60.0;
    let mut all = [0; 5];
    all[i] = steps;
    Some(Action::Move {
        steps: all,
        dda_rate: (feed * steps.unsigned_abs() as f64 / mm.abs())
            .round()
            .max(1.0) as u32,
        relative: super::command::ALL_AXES,
        distance_mm: mm.abs() as f32,
        feedrate: feed as f32,
    })
}

/// The commands as an `.x3g` file.
pub fn to_x3g(actions: &[Action]) -> Vec<u8> {
    actions.iter().flat_map(|a| a.encode()).collect()
}

struct State<'a> {
    m: &'a Machine,
    origin: (f64, f64),
    /// Machine coordinates in mm; None until homed or set with G92.
    pos: [Option<f64>; 3],
    /// Last absolute E value written in the G-code (for absolute extrusion).
    e: f64,
    absolute_xyz: bool,
    absolute_e: bool,
    /// mm/min.
    feed: f64,
    tool: u8,
    /// Fractional extruder steps not yet sent.
    excess: f64,
    percent: Option<u8>,
    out: Translation,
}

impl State<'_> {
    fn warn_once(&mut self, w: &str) {
        if !self.out.warnings.iter().any(|x| x == w) {
            self.out.warnings.push(w.to_string());
        }
    }

    fn steps(&self, axis: usize, mm: f64) -> i32 {
        (mm * self.m.steps_per_mm[axis]).round() as i32
    }

    fn position_steps(&self) -> [i32; 5] {
        let mut s = [0; 5];
        for (i, p) in self.pos.iter().enumerate() {
            s[i] = self.steps(i, p.unwrap_or(0.0));
        }
        s
    }
}

/// The value after a letter in a G-code line's words (`X12.5`).
fn word(words: &[(char, &str)], letter: char) -> Option<Result<f64, String>> {
    words.iter().find(|(c, _)| *c == letter).map(|(_, v)| {
        v.parse::<f64>()
            .map_err(|_| format!("{letter}{v} is not a number"))
    })
}

fn has(words: &[(char, &str)], letter: char) -> bool {
    words.iter().any(|(c, _)| *c == letter)
}

pub fn translate(gcode: &str, m: &Machine, o: &Options) -> Result<Translation, TranslateError> {
    let mut st = State {
        m,
        origin: (o.bed_mm.0 / 2.0, o.bed_mm.1 / 2.0),
        pos: [None; 3],
        e: 0.0,
        absolute_xyz: true,
        absolute_e: true,
        feed: 1200.0,
        tool: 0,
        excess: 0.0,
        percent: None,
        out: Translation::default(),
    };
    st.out
        .actions
        .push(Action::StreamVersion { bot_pid: m.bot_pid });
    st.out.actions.push(Action::BuildStart {
        name: o.job_name.clone(),
    });
    st.out.actions.extend(Action::message(&o.job_name));
    for (i, raw) in gcode.lines().enumerate() {
        let fail = |why: String| TranslateError {
            line_no: i + 1,
            line: raw.trim().to_string(),
            why,
        };
        if let Some(n) = raw.trim().strip_prefix(";LAYER:") {
            if let (Ok(layer), true) = (n.trim().parse::<u32>(), o.total_layers > 0) {
                let p = ((layer * 100) / o.total_layers).min(100) as u8;
                if st.percent != Some(p) {
                    st.percent = Some(p);
                    st.out.actions.push(Action::BuildPercent { percent: p });
                }
            }
            continue;
        }
        let code = raw.split(';').next().unwrap_or("").trim();
        if code.is_empty() {
            continue;
        }
        let mut parts = code.split_whitespace();
        let cmd = parts.next().unwrap_or("").to_ascii_uppercase();
        let words: Vec<(char, &str)> = parts
            .filter_map(|p| {
                let mut c = p.chars();
                let letter = c.next()?.to_ascii_uppercase();
                Some((letter, c.as_str()))
            })
            .collect();
        line(&mut st, &cmd, &words, code).map_err(fail)?;
    }
    st.out.actions.push(Action::BuildPercent { percent: 100 });
    st.out.actions.push(Action::BuildEnd);
    Ok(st.out)
}

/// One command. Err is the reason the line can't be translated.
fn line(st: &mut State, cmd: &str, words: &[(char, &str)], code: &str) -> Result<(), String> {
    let num = |letter: char| word(words, letter).transpose();
    match cmd {
        "G0" | "G1" => move_to(st, words),
        "G4" => {
            let ms = num('P')?.or(num('S')?.map(|s| s * 1000.0)).unwrap_or(0.0);
            if ms > 0.0 {
                st.out.actions.push(Action::Delay { ms: ms as u32 });
            }
            Ok(())
        }
        "G28" => {
            home(st, words);
            Ok(())
        }
        "G29" | "G32" => {
            st.warn_once(
                "Bed levelling (G29/G32) was left out: MakerBot-protocol printers level \
                 the bed from their own menu.",
            );
            Ok(())
        }
        "G90" => {
            st.absolute_xyz = true;
            st.absolute_e = true;
            Ok(())
        }
        "G91" => {
            st.absolute_xyz = false;
            st.absolute_e = false;
            Ok(())
        }
        "M82" => {
            st.absolute_e = true;
            Ok(())
        }
        "M83" => {
            st.absolute_e = false;
            Ok(())
        }
        "G92" => set_position(st, words),
        "M104" | "M109" => {
            let t = num('S')?.or(num('R')?).unwrap_or(0.0);
            let tool = match num('T')? {
                Some(t) => checked_tool(st, t)?,
                None => st.tool,
            };
            st.out.actions.push(Action::SetNozzleTemp {
                tool,
                celsius: t.round() as i16,
            });
            if cmd == "M109" && t > 0.0 {
                st.out.actions.push(Action::WaitForTool {
                    tool,
                    timeout_s: HEAT_TIMEOUT_S,
                });
            }
            Ok(())
        }
        "M140" | "M190" => {
            let t = num('S')?.or(num('R')?).unwrap_or(0.0);
            if !st.m.heated_bed {
                if t > 0.0 {
                    st.warn_once("This printer has no heated bed; bed temperatures were left out.");
                }
                return Ok(());
            }
            st.out.actions.push(Action::SetBedTemp {
                celsius: t.round() as i16,
            });
            if cmd == "M190" && t > 0.0 {
                st.out.actions.push(Action::WaitForBed {
                    timeout_s: HEAT_TIMEOUT_S,
                });
            }
            Ok(())
        }
        "M106" => {
            let s = num('S')?.unwrap_or(255.0);
            if s > 0.0 && s < 255.0 {
                st.warn_once(
                    "The part fan can only be switched on or off on this printer; \
                     fan speeds were rounded to on.",
                );
            }
            let tool = st.tool;
            st.out.actions.push(Action::Fan { tool, on: s > 0.0 });
            Ok(())
        }
        "M107" => {
            let tool = st.tool;
            st.out.actions.push(Action::Fan { tool, on: false });
            Ok(())
        }
        "M84" | "M18" => {
            st.out.actions.push(Action::EnableAxes {
                axes: XYZ | A | B,
                enable: false,
            });
            Ok(())
        }
        "M17" => {
            st.out.actions.push(Action::EnableAxes {
                axes: XYZ | A | B,
                enable: true,
            });
            Ok(())
        }
        "M117" => {
            let text = code.get(4..).unwrap_or("").trim();
            st.out.actions.extend(Action::message(text));
            Ok(())
        }
        c if c.len() >= 2 && c.starts_with('T') && c[1..].bytes().all(|b| b.is_ascii_digit()) => {
            let tool = checked_tool(st, c[1..].parse::<f64>().unwrap_or(0.0))?;
            if tool != st.tool {
                st.tool = tool;
                st.out.actions.push(Action::ChangeTool { tool });
            }
            Ok(())
        }
        other => Err(format!(
            "{other} has no MakerBot-protocol equivalent the app knows"
        )),
    }
}

fn checked_tool(st: &State, t: f64) -> Result<u8, String> {
    if t < 0.0 || t >= st.m.extruders as f64 || t.fract() != 0.0 {
        Err(format!(
            "tool {t} doesn't exist: this printer has {} nozzle(s)",
            st.m.extruders
        ))
    } else {
        Ok(t as u8)
    }
}

/// G28: X and Y (on this family) home to their maximum, Z to its minimum. Afterwards the
/// printer takes its stored home position (command 144, the `M132` of ReplicatorG).
fn home(st: &mut State, words: &[(char, &str)]) {
    let mut axes = 0u8;
    for (i, letter) in ['X', 'Y', 'Z'].into_iter().enumerate() {
        if has(words, letter) {
            axes |= 1 << i;
        }
    }
    if axes == 0 {
        axes = XYZ;
    }
    let m = st.m;
    for to_max in [true, false] {
        let group: Vec<usize> = (0..3)
            .filter(|&i| axes & (1 << i) != 0 && m.home_to_max[i] == to_max)
            .collect();
        if group.is_empty() {
            continue;
        }
        // As GPX works it out: the slowest homing speed of the group is the speed of the
        // combined move (so two axes each go 1/√2 as fast), timed on the axis with the most
        // steps per mm.
        let feed = group
            .iter()
            .map(|&i| m.home_feedrate[i])
            .fold(f64::INFINITY, f64::min);
        let most_steps_per_mm = group.iter().map(|&i| m.steps_per_mm[i]).fold(0.0, f64::max);
        let length = (group.len() as f64).sqrt();
        let step_us = (length / feed * 60_000_000.0 / most_steps_per_mm).round() as u32;
        let mask = group.iter().fold(0u8, |acc, &i| acc | (1 << i));
        st.out.actions.push(Action::Home {
            to_max,
            axes: mask,
            step_us,
            timeout_s: m.timeout_s,
        });
    }
    st.out.actions.push(Action::RecallHome { axes });
    for i in 0..3 {
        if axes & (1 << i) != 0 {
            st.pos[i] = Some(m.home_mm[i]);
        }
    }
}

/// G92: X, Y and Z set the machine's position (sent to the printer); E only resets the
/// app's own count, since the extruders are always sent relative.
fn set_position(st: &mut State, words: &[(char, &str)]) -> Result<(), String> {
    let mut xyz = false;
    for (i, letter) in ['X', 'Y', 'Z'].into_iter().enumerate() {
        if let Some(v) = word(words, letter).transpose()? {
            let offset = [st.origin.0, st.origin.1, 0.0][i];
            st.pos[i] = Some(v - offset);
            xyz = true;
        }
    }
    if let Some(e) = word(words, 'E').transpose()? {
        st.e = e;
    }
    if xyz {
        let steps = st.position_steps();
        st.out.actions.push(Action::SetPosition { steps });
    }
    Ok(())
}

fn move_to(st: &mut State, words: &[(char, &str)]) -> Result<(), String> {
    if let Some(f) = word(words, 'F').transpose()? {
        if f <= 0.0 {
            return Err("a feed rate must be above zero".into());
        }
        st.feed = f;
    }
    let mut target = st.pos;
    let mut moves_xyz = false;
    for (i, letter) in ['X', 'Y', 'Z'].into_iter().enumerate() {
        if let Some(v) = word(words, letter).transpose()? {
            moves_xyz = true;
            let offset = [st.origin.0, st.origin.1, 0.0][i];
            target[i] = Some(if st.absolute_xyz {
                v - offset
            } else {
                st.pos[i].ok_or("a relative move before the printer was homed (G28)")? + v
            });
        }
    }
    // Without a known starting point the move's length (and so its speed) is unknown.
    let given = ['X', 'Y', 'Z'].map(|l| has(words, l));
    if moves_xyz && (0..3).any(|i| given[i] && st.pos[i].is_none()) {
        return Err("a move before the printer was homed (G28)".into());
    }
    let de = match word(words, 'E').transpose()? {
        Some(e) if st.absolute_e => {
            let d = e - st.e;
            st.e = e;
            d
        }
        Some(e) => e,
        None => 0.0,
    };

    let m = st.m;
    let mut delta_mm = [0.0f64; 4];
    for i in 0..3 {
        delta_mm[i] = match (st.pos[i], target[i]) {
            (Some(a), Some(b)) => b - a,
            _ => 0.0,
        };
    }
    delta_mm[3] = de;
    let new_steps: [i32; 3] =
        std::array::from_fn(|i| target[i].map(|v| st.steps(i, v)).unwrap_or(0));
    let old_steps: [i32; 3] =
        std::array::from_fn(|i| st.pos[i].map(|v| st.steps(i, v)).unwrap_or(new_steps[i]));
    // Forward extrusion is negative steps on this board family.
    let a_exact = -de * m.steps_per_mm[3 + st.tool as usize] + st.excess;
    let a_steps = a_exact.round();
    st.excess = a_exact - a_steps;
    let a_steps = a_steps as i32;

    st.pos = target;
    let delta_steps = [
        (new_steps[0] - old_steps[0]).abs(),
        (new_steps[1] - old_steps[1]).abs(),
        (new_steps[2] - old_steps[2]).abs(),
        a_steps.abs(),
    ];
    let most = *delta_steps.iter().max().unwrap_or(&0);
    if most == 0 {
        return Ok(());
    }
    let xyz_len = (delta_mm[0].powi(2) + delta_mm[1].powi(2) + delta_mm[2].powi(2)).sqrt();
    let distance = if xyz_len > 0.0 { xyz_len } else { de.abs() };
    // Cap the feed rate so no axis goes over its limit.
    let mut feed = st.feed;
    for (i, d) in delta_mm.iter().enumerate() {
        let axis = if i == 3 { 3 + st.tool as usize } else { i };
        if d.abs() > 1e-9 {
            feed = feed.min(m.max_feedrate[axis] * distance / d.abs());
        }
    }
    let feed_mm_s = feed / 60.0;
    // A move far shorter than one step can still cross a step boundary; its step rate
    // would then come out far above what the motor can do. Cap it at the fastest rate of
    // the axis that steps most.
    let dominant = (0..4).max_by_key(|&i| delta_steps[i]).unwrap_or(0);
    let dominant_axis = if dominant == 3 {
        3 + st.tool as usize
    } else {
        dominant
    };
    let max_rate = m.max_feedrate[dominant_axis] / 60.0 * m.steps_per_mm[dominant_axis];
    let dda_rate = (feed_mm_s * most as f64 / distance)
        .min(max_rate)
        .round()
        .max(1.0) as u32;
    let mut steps = [new_steps[0], new_steps[1], new_steps[2], 0, 0];
    steps[3 + st.tool as usize] = a_steps;
    st.out.actions.push(Action::Move {
        steps,
        dda_rate,
        relative: A | B,
        distance_mm: distance as f32,
        feedrate: feed_mm_s as f32,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Options {
        Options {
            bed_mm: (220.0, 140.0),
            job_name: "cube".into(),
            total_layers: 4,
        }
    }

    fn moves(t: &Translation) -> Vec<(&[i32; 5], u32, f32, f32)> {
        t.actions
            .iter()
            .filter_map(|a| match a {
                Action::Move {
                    steps,
                    dda_rate,
                    distance_mm,
                    feedrate,
                    ..
                } => Some((steps, *dda_rate, *distance_mm, *feedrate)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn reads_the_plate_and_layers_from_the_gcode_header() {
        let g = "; printer: x\n; plate: 227.0 x 148.0 mm\n;LAYER_COUNT:12\nG28\n";
        let o = Options::from_gcode(g, "cube").unwrap();
        assert_eq!(o.bed_mm, (227.0, 148.0));
        assert_eq!(o.total_layers, 12);
        assert!(Options::from_gcode("G28\n", "cube")
            .unwrap_err()
            .contains("plate size"));
    }

    #[test]
    fn jogs_are_relative_and_homing_has_no_build_notices() {
        let m = Machine::creator_pro();
        let Some(Action::Move {
            steps,
            relative,
            feedrate,
            ..
        }) = jog(&m, 'z', -1.0, 600.0)
        else {
            panic!("no jog");
        };
        assert_eq!(steps, [0, 0, -400, 0, 0]);
        assert_eq!(relative, crate::x3g::command::ALL_AXES);
        assert_eq!(feedrate, 10.0);
        assert_eq!(jog(&m, 'Q', 1.0, 600.0), None);
        assert_eq!(jog(&m, 'X', 0.001, 600.0), None, "under one step");
        let home = home_actions(&m);
        assert_eq!(home.len(), 3);
        assert!(matches!(home[2], Action::RecallHome { .. }));
    }

    #[test]
    fn starts_and_ends_a_build() {
        let t = translate("G28\n", &Machine::creator_pro(), &opts()).unwrap();
        assert_eq!(t.actions[0], Action::StreamVersion { bot_pid: 0xD314 });
        assert_eq!(
            t.actions[1],
            Action::BuildStart {
                name: "cube".into()
            }
        );
        assert_eq!(t.actions.last(), Some(&Action::BuildEnd));
    }

    #[test]
    fn homes_xy_to_max_z_to_min_then_takes_the_stored_home() {
        let t = translate("G28\n", &Machine::creator_pro(), &opts()).unwrap();
        let homing: Vec<&Action> = t
            .actions
            .iter()
            .filter(|a| matches!(a, Action::Home { .. } | Action::RecallHome { .. }))
            .collect();
        assert!(matches!(
            homing[0],
            Action::Home {
                to_max: true,
                axes: 0b011,
                ..
            }
        ));
        assert!(matches!(
            homing[1],
            Action::Home {
                to_max: false,
                axes: 0b100,
                ..
            }
        ));
        assert_eq!(homing[2], &Action::RecallHome { axes: XYZ });
        // GPX's figures for the Creator Pro: X and Y together at 2500 mm/min take 361 µs
        // per step; Z alone at 1100 mm/min, 136 µs.
        if let Action::Home { step_us, .. } = homing[0] {
            assert_eq!(*step_us, 361);
        }
        if let Action::Home { step_us, .. } = homing[1] {
            assert_eq!(*step_us, 136);
        }
    }

    #[test]
    fn moves_are_in_steps_from_the_bed_centre() {
        let g = "G28\nG1 Z0.3 F600\nG1 X110 Y70 F3000\nG1 X120 Y70\n";
        let t = translate(g, &Machine::creator_pro(), &opts()).unwrap();
        let mv = moves(&t);
        // Z 0.3 mm at 400 steps/mm.
        assert_eq!(mv[0].0[2], 120);
        // The bed's centre (110, 70) is the machine's 0, 0.
        assert_eq!(&mv[1].0[..2], &[0, 0]);
        // 10 mm right: 941.17647 steps, rounded.
        assert_eq!(mv[2].0[0], 941);
        assert_eq!(mv[2].2, 10.0, "distance");
        assert_eq!(mv[2].3, 50.0, "3000 mm/min is 50 mm/s");
        // 50 mm/s × 941 steps / 10 mm.
        assert_eq!(mv[2].1, 4705);
    }

    #[test]
    fn extrusion_is_relative_negative_and_loses_nothing_to_rounding() {
        let mut g = String::from("G28\nG92 E0\nG1 X110 Y70 Z0.2 F3000\n");
        // 1000 tiny moves of 0.0101 mm of filament each: 0.97 steps each.
        for i in 1..=1000 {
            g.push_str(&format!(
                "G1 X{:.3} E{:.4}\n",
                110.0 + i as f64 * 0.05,
                i as f64 * 0.0101
            ));
        }
        let t = translate(&g, &Machine::creator_pro(), &opts()).unwrap();
        let total: i64 = moves(&t).iter().map(|m| m.0[3] as i64).sum();
        let expected = -(10.1 * 96.275_201_870_333_66f64);
        assert!(
            (total as f64 - expected).abs() <= 1.0,
            "{total} steps vs {expected}"
        );
        for a in &t.actions {
            if let Action::Move { relative, .. } = a {
                assert_eq!(*relative, A | B, "extruders relative, XYZ absolute");
            }
        }
    }

    #[test]
    fn a_retraction_alone_uses_the_extrusion_length() {
        let g = "G28\nG1 X110 Y70 Z0.2 F3000\nG92 E0\nG1 E-0.8 F2400\n";
        let t = translate(g, &Machine::creator_pro(), &opts()).unwrap();
        let last = *moves(&t).last().unwrap();
        assert_eq!(last.0[3], 77, "pulled back: positive steps");
        assert!((last.2 - 0.8).abs() < 1e-6);
        // 2400 mm/min is over the extruder's 1600 mm/min limit, so it is capped.
        assert!((last.3 - 1600.0 / 60.0).abs() < 1e-3, "{}", last.3);
    }

    #[test]
    fn heats_and_waits_only_when_asked_to_wait() {
        let g = "M104 S210\nM109 S215\nM140 S60\nM190 S60\nM104 S0\nM109 S0\n";
        let t = translate(g, &Machine::creator_pro(), &opts()).unwrap();
        let heat: Vec<&Action> = t
            .actions
            .iter()
            .filter(|a| {
                matches!(
                    a,
                    Action::SetNozzleTemp { .. }
                        | Action::WaitForTool { .. }
                        | Action::SetBedTemp { .. }
                        | Action::WaitForBed { .. }
                )
            })
            .collect();
        assert_eq!(
            heat,
            vec![
                &Action::SetNozzleTemp {
                    tool: 0,
                    celsius: 210
                },
                &Action::SetNozzleTemp {
                    tool: 0,
                    celsius: 215
                },
                &Action::WaitForTool {
                    tool: 0,
                    timeout_s: 65535
                },
                // M140 sets the bed; M190 sets it again and waits.
                &Action::SetBedTemp { celsius: 60 },
                &Action::SetBedTemp { celsius: 60 },
                &Action::WaitForBed { timeout_s: 65535 },
                &Action::SetNozzleTemp {
                    tool: 0,
                    celsius: 0
                },
                &Action::SetNozzleTemp {
                    tool: 0,
                    celsius: 0
                },
            ]
        );
    }

    #[test]
    fn progress_follows_the_layers() {
        let g = ";LAYER:0\n;LAYER:1\n;LAYER:2\n;LAYER:3\n";
        let t = translate(g, &Machine::creator_pro(), &opts()).unwrap();
        let p: Vec<u8> = t
            .actions
            .iter()
            .filter_map(|a| match a {
                Action::BuildPercent { percent } => Some(*percent),
                _ => None,
            })
            .collect();
        assert_eq!(p, vec![0, 25, 50, 75, 100]);
    }

    #[test]
    fn refuses_what_it_does_not_know_and_names_the_line() {
        let e = translate("G28\nM555 S1\n", &Machine::creator_pro(), &opts()).unwrap_err();
        assert_eq!(e.line_no, 2);
        assert!(e.to_string().contains("M555"), "{e}");
        let e = translate("G1 X10 F600\n", &Machine::creator_pro(), &opts()).unwrap_err();
        assert!(e.why.contains("homed"), "{e}");
        let e = translate("T2\n", &Machine::creator_pro(), &opts()).unwrap_err();
        assert!(e.why.contains("2 nozzle"), "{e}");
    }

    #[test]
    fn levelling_and_fan_speeds_are_left_out_with_a_warning() {
        let g = "G29\nM106 S128\nM107\n";
        let t = translate(g, &Machine::creator_pro(), &opts()).unwrap();
        assert_eq!(t.warnings.len(), 2, "{:?}", t.warnings);
        assert!(t.actions.contains(&Action::Fan { tool: 0, on: true }));
        assert!(t.actions.contains(&Action::Fan { tool: 0, on: false }));
    }

    /// Packets GPX 2.6.8 wrote for the same G-code (`gpx -I -r -F -m fcp`, with the G-code
    /// shifted to the bed's centre), kept as reference bytes. They pin the framing, the CRC
    /// and each command's layout to what a proven tool sends a Creator Pro.
    #[test]
    fn matches_packets_written_by_gpx() {
        let hex = |s: &str| -> Vec<u8> {
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
                .collect()
        };
        let framed = |a: &Action| crate::x3g::frame(&a.encode());
        let g = "M140 S60\nM104 S210\nG28\nM190 S60\nG1 Z2.0 F3000\n\
                 G1 X1.5 Y20.0 F5000\nG1 Z0.30 F600\nM107\n";
        let o = Options {
            bed_mm: (227.0, 148.0),
            job_name: "cube".into(),
            total_layers: 0,
        };
        let t = translate(g, &Machine::creator_pro(), &o).unwrap();
        let find = |pred: &dyn Fn(&Action) -> bool| -> Vec<u8> {
            framed(t.actions.iter().find(|a| pred(a)).unwrap())
        };
        assert_eq!(
            find(&|a| matches!(a, Action::SetBedTemp { .. })),
            hex("d50688001f023c00c6")
        );
        assert_eq!(
            find(&|a| matches!(a, Action::SetNozzleTemp { .. })),
            hex("d50688000302d20045")
        );
        assert_eq!(
            find(&|a| matches!(a, Action::Home { to_max: true, .. })),
            hex("d50884036901000014008f")
        );
        assert_eq!(
            find(&|a| matches!(a, Action::Home { to_max: false, .. })),
            hex("d50883048800000014003f")
        );
        assert_eq!(
            find(&|a| matches!(a, Action::WaitForBed { .. })),
            hex("d5068d006400ffff8a")
        );
        // GPX sends M107 unframed in its serial log as 88 00 0D 01 00.
        assert_eq!(
            find(&|a| matches!(a, Action::Fan { .. }))[2..7],
            hex("88000d0100")[..]
        );
        // The third move: down to Z 0.3 at the purge start, (-112, -54) mm from the centre.
        let third_move = t
            .actions
            .iter()
            .filter(|a| matches!(a, Action::Move { .. }))
            .nth(2)
            .unwrap();
        assert_eq!(
            framed(third_move),
            hex(concat!(
                "d5209bd3d6ffff26ecffff780000000000000000000000a00f0000",
                "189a99d93f8002fa"
            ))
        );
    }

    #[test]
    fn translates_a_whole_sliced_plate() {
        use crate::geometry::Vec3;
        use crate::model::{Mesh, PlacedObject};
        use crate::profiles::{Filament, PrintSettings, Printer};
        let mut printer = Printer::presets().remove(0);
        printer.bed_x = 227.0;
        printer.bed_y = 148.0;
        let mut obj = PlacedObject::new("cube", Mesh::cube(Vec3::new(10.0, 10.0, 2.0)));
        // Placed at the plate's centre (positions are measured from it).
        obj.x = 0.0;
        obj.y = 0.0;
        let out = crate::pipeline::slice_plate(
            &obj.world_triangles(),
            &printer,
            &Filament::presets()[0],
            &PrintSettings::default(),
            &|_| {},
        );
        // With and without the levelling step the app adds when sending a print.
        let levelled = crate::printer::calibration::with_calibration(
            &out.gcode.text,
            crate::printer::Firmware::Marlin,
        );
        for (calibrate, text) in [(false, &out.gcode.text), (true, &levelled)] {
            let o = Options {
                bed_mm: (227.0, 148.0),
                job_name: "cube".into(),
                total_layers: out.result.layers.len() as u32,
            };
            let t = translate(text, &Machine::creator_pro(), &o)
                .unwrap_or_else(|e| panic!("calibrate {calibrate}: {e}"));
            let mv = moves(&t);
            assert!(mv.len() > 100, "{} moves", mv.len());
            // Everything that lays down plastic (purge lines included) is on the bed, which
            // reaches half its size either side of the machine's 0, 0. Moves that only
            // travel may start from the home position, which is off the bed.
            for m in mv.iter().filter(|m| m.0[3] != 0 && m.2 > 0.0) {
                let (x, y) = (m.0[0] as f64 / 94.117647, m.0[1] as f64 / 94.117647);
                assert!(
                    x.abs() <= 113.5 && y.abs() <= 74.0,
                    "extruding off the bed at {x:.1}, {y:.1} mm"
                );
            }
            // The cube itself, 10 mm wide at the centre, prints within a few mm of 0, 0.
            let near_centre = mv
                .iter()
                .filter(|m| m.0[3] < 0 && (m.0[0] as f64 / 94.117647).abs() < 8.0)
                .count();
            assert!(
                near_centre > 50,
                "{near_centre} extruding moves near the centre"
            );
            let bytes = to_x3g(&t.actions);
            assert!(bytes.len() > 1000);
            assert_eq!(bytes[0], 157, "an X3G file starts with its stream version");
        }
    }
}
