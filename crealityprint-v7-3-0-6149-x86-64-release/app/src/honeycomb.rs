//! Honeycomb infill: zigzag lines whose flat stretches meet their neighbours' flats, so
//! together they draw hexagons. The hexagons sit on a fixed grid, so the cells line up
//! from layer to layer into upright hexagonal tubes. The shared flats are laid twice
//! (once by each line), which the line spacing allows for.

use crate::geometry::Point2;

/// The honeycomb lines over the box `lo`..`hi` for the density of straight lines
/// `spacing` mm apart, cut to where `inside` is true; pieces shorter than `min_len`
/// are left out.
///
/// A line covers a band `h` tall: flats of length `a` at its bottom and top joined by
/// diagonals of length `a`, so it is `4a` long per `3a` across. Laying that every `h`
/// gives `4 / (3h)` of line per square mm, the same as straight lines `spacing` apart
/// when `h = 4 * spacing / 3`; and hexagons need `h = a * sqrt(3) / 2`.
pub fn honeycomb_lines(
    lo: Point2,
    hi: Point2,
    spacing: f32,
    min_len: f32,
    inside: &dyn Fn(Point2) -> bool,
) -> Vec<Vec<Point2>> {
    let h = spacing * 4.0 / 3.0;
    let a = 2.0 * h / 3f32.sqrt();
    let period = 3.0 * a;
    let x0 = (lo.x / period).floor() * period - period;
    let first = (lo.y / h).floor() as i64 - 1;
    let last = (hi.y / h).ceil() as i64 + 1;
    let mut out = Vec::new();
    for r in first..=last {
        let base = r as f32 * h;
        // Even lines start low, odd lines high: each line's top flats are the next
        // line's bottom flats.
        let (y_a, y_b) = if r.rem_euclid(2) == 0 {
            (base, base + h)
        } else {
            (base + h, base)
        };
        let mut line = Vec::new();
        let mut x = x0;
        while x <= hi.x + period {
            line.push(Point2::new(x, y_a));
            line.push(Point2::new(x + a, y_a));
            line.push(Point2::new(x + 1.5 * a, y_b));
            line.push(Point2::new(x + 2.5 * a, y_b));
            x += period;
        }
        // Every other line runs back, so the nozzle snakes from one to the next.
        if r.rem_euclid(2) == 1 {
            line.reverse();
        }
        out.extend(
            crate::gyroid::clip(&line, inside)
                .into_iter()
                .filter(|piece| crate::gyroid::length(piece) >= min_len),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_honeycomb_lines_make_hexagons_that_line_up() {
        let inside = |p: Point2| p.x.abs() < 20.0 && p.y.abs() < 20.0;
        let lines = honeycomb_lines(
            Point2::new(-20.0, -20.0),
            Point2::new(20.0, 20.0),
            1.68,
            0.42,
            &inside,
        );
        assert!(!lines.is_empty());
        assert!(lines
            .iter()
            .flatten()
            .all(|p| p.x.abs() <= 20.01 && p.y.abs() <= 20.01));
        // The segments come in three directions only: 0, 60 and 120 degrees.
        for l in &lines {
            for w in l.windows(2) {
                let d = (w[1].y - w[0].y)
                    .atan2(w[1].x - w[0].x)
                    .to_degrees()
                    .rem_euclid(180.0);
                let near = |t: f32| (d - t).abs() < 0.5;
                assert!(near(0.0) || near(60.0) || near(120.0) || near(180.0), "{d}");
            }
        }
        // The same box twice gives the same lines (a fixed grid, so layers line up).
        let again = honeycomb_lines(
            Point2::new(-20.0, -20.0),
            Point2::new(20.0, 20.0),
            1.68,
            0.42,
            &inside,
        );
        assert_eq!(lines, again);
    }
}
