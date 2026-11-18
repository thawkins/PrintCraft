//! Print-time maths for moves that speed up and slow down: how long a printer takes to
//! follow a line of points at a cruise speed with a given acceleration.
//!
//! The model is the usual firmware one (trapezoidal speed profiles with look-ahead):
//! * the line starts and ends at rest;
//! * at each corner the speed is limited by the corner's angle, using Klipper's
//!   square-corner-velocity rule (a 90 degree corner is taken at `SQUARE_CORNER_VELOCITY`;
//!   a straight continuation is not limited);
//! * a backward pass and a forward pass make every corner speed reachable from its
//!   neighbours with the acceleration, then each segment is timed as a trapezoid (or a
//!   triangle when it is too short to reach the cruise speed).
//!
//! This is an estimate: real firmware also has jerk limits, minimum cruise ratios and
//! buffer limits that are not modelled.

use crate::geometry::Point2;

/// Speed (mm/s) through a 90 degree corner: Klipper's default `square_corner_velocity`.
pub const SQUARE_CORNER_VELOCITY: f32 = 5.0;

/// Seconds to move `len` mm starting at `v0`, ending at `v1`, cruising at most at `v`,
/// accelerating and braking at `a` (mm/s^2). `v0` and `v1` must not exceed what `a` can
/// reach over `len` (the planner passes guarantee it).
pub fn segment_time(len: f32, v0: f32, v1: f32, v: f32, a: f32) -> f32 {
    if len <= 0.0 || v <= 0.0 {
        return 0.0;
    }
    if a <= 0.0 {
        return len / v;
    }
    let d_acc = (v * v - v0 * v0).max(0.0) / (2.0 * a);
    let d_dec = (v * v - v1 * v1).max(0.0) / (2.0 * a);
    if d_acc + d_dec <= len {
        (v - v0).max(0.0) / a + (v - v1).max(0.0) / a + (len - d_acc - d_dec) / v
    } else {
        // Too short to reach the cruise speed: speed up to a peak, then slow down.
        let peak = ((2.0 * a * len + v0 * v0 + v1 * v1) / 2.0).sqrt();
        (peak - v0).max(0.0) / a + (peak - v1).max(0.0) / a
    }
}

/// The highest speed (mm/s) through the corner between directions `d1` and `d2` (unit
/// vectors), by Klipper's junction-deviation formula.
fn corner_speed(d1: (f32, f32), d2: (f32, f32), a: f32) -> f32 {
    let deviation = SQUARE_CORNER_VELOCITY.powi(2) * (std::f32::consts::SQRT_2 - 1.0) / a;
    let cos_theta = -(d1.0 * d2.0 + d1.1 * d2.1);
    let sin_half = (0.5 * (1.0 - cos_theta)).max(0.0).sqrt();
    if sin_half >= 0.999_999 {
        return f32::INFINITY;
    }
    (a * deviation * sin_half / (1.0 - sin_half)).sqrt()
}

/// Seconds to follow `points` at cruise speed `v` (mm/s) with acceleration `a` (mm/s^2),
/// starting and ending at rest. With `a` <= 0 it is plain length / speed.
pub fn line_time(points: &[Point2], v: f32, a: f32) -> f32 {
    let segs: Vec<(f32, (f32, f32))> = points
        .windows(2)
        .map(|w| {
            let len = w[0].distance(w[1]);
            let dir = if len > 0.0 {
                ((w[1].x - w[0].x) / len, (w[1].y - w[0].y) / len)
            } else {
                (0.0, 0.0)
            };
            (len, dir)
        })
        .filter(|(len, _)| *len > 1e-6)
        .collect();
    if segs.is_empty() || v <= 0.0 {
        return 0.0;
    }
    if a <= 0.0 {
        return segs.iter().map(|(len, _)| len / v).sum();
    }
    // Speed at each junction: 0 at both ends, corner-limited inside.
    let mut vj: Vec<f32> = std::iter::once(0.0)
        .chain(
            segs.windows(2)
                .map(|w| corner_speed(w[0].1, w[1].1, a).min(v)),
        )
        .chain(std::iter::once(0.0))
        .collect();
    // Backward pass: each junction slow enough to brake for the next one.
    for i in (0..segs.len()).rev() {
        let reach = (vj[i + 1] * vj[i + 1] + 2.0 * a * segs[i].0).sqrt();
        vj[i] = vj[i].min(reach);
    }
    // Forward pass: each junction reachable from the one before.
    for i in 0..segs.len() {
        let reach = (vj[i] * vj[i] + 2.0 * a * segs[i].0).sqrt();
        vj[i + 1] = vj[i + 1].min(reach);
    }
    segs.iter()
        .enumerate()
        .map(|(i, (len, _))| segment_time(*len, vj[i], vj[i + 1], v, a))
        .sum()
}
