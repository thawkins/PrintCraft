//! Sequential printing ("one object at a time"): each object is printed from bottom to
//! top before the next one starts. While a later object prints, the print head moves
//! low beside the ones already finished, so two things must hold:
//!
//! * Around the nozzle, the head (fan duct, heater block, carriage) reaches out to the
//!   printer's **clearance radius**: the objects' footprints must be at least that far
//!   apart, or printing one would knock the other over.
//! * Above the nozzle, the gantry (the X axis beam) sits at the printer's **clearance
//!   height**: every object except the last one printed must be lower than that, or the
//!   beam would hit it while the next object's first layers print. So objects are
//!   printed shortest first, and only the tallest may be taller.

use crate::geometry::Point2;

/// An object as the check sees it: its name, its footprint (convex outline on the
/// plate, as `PlacedObject::footprint_hull`) and its height (mm).
#[derive(Clone, Debug, PartialEq)]
pub struct Footprint {
    pub name: String,
    pub outline: Vec<Point2>,
    pub height: f32,
}

/// The order to print the objects in (indices into `objects`, shortest first), or why
/// they can't be printed one at a time with a head of `radius` and a gantry `height` mm
/// above the nozzle.
pub fn print_order(objects: &[Footprint], radius: f32, height: f32) -> Result<Vec<usize>, String> {
    for (i, a) in objects.iter().enumerate() {
        for b in &objects[i + 1..] {
            let gap = gap_between(&a.outline, &b.outline);
            if gap < radius {
                return Err(format!(
                    "\"{}\" and \"{}\" are {gap:.1} mm apart. Printing one object at a time \
                     needs at least {radius:.0} mm between them (the print head's clearance \
                     radius): move them apart, or turn sequential printing off.",
                    a.name, b.name
                ));
            }
        }
    }
    let mut order: Vec<usize> = (0..objects.len()).collect();
    order.sort_by(|&i, &j| objects[i].height.total_cmp(&objects[j].height));
    // Every object but the last is printed while the gantry passes over it.
    if let Some(&i) = order
        .iter()
        .rev()
        .skip(1)
        .find(|&&i| objects[i].height > height)
    {
        return Err(format!(
            "\"{}\" is {:.1} mm tall, above the printer's {height:.0} mm clearance height: \
             printing one object at a time, only the tallest object may be taller than that. \
             Print it on its own, or turn sequential printing off.",
            objects[i].name, objects[i].height
        ));
    }
    Ok(order)
}

/// The shortest distance (mm) between two outlines; 0 when they overlap.
fn gap_between(a: &[Point2], b: &[Point2]) -> f32 {
    let edges = |p: &[Point2]| -> Vec<(Point2, Point2)> {
        (0..p.len()).map(|k| (p[k], p[(k + 1) % p.len()])).collect()
    };
    let (ea, eb) = (edges(a), edges(b));
    let overlap = a.iter().any(|&p| crate::slicer::contains(b, p))
        || b.iter().any(|&p| crate::slicer::contains(a, p))
        || ea
            .iter()
            .any(|&(p, q)| eb.iter().any(|&(r, t)| segments_cross(p, q, r, t)));
    if overlap {
        return 0.0;
    }
    // Apart, the closest points are a corner of one and an edge of the other.
    let near = |points: &[Point2], edges: &[(Point2, Point2)]| {
        points
            .iter()
            .flat_map(|&p| {
                edges
                    .iter()
                    .map(move |&(q, r)| distance_to_segment(p, q, r))
            })
            .fold(f32::MAX, f32::min)
    };
    near(a, &eb).min(near(b, &ea))
}

fn distance_to_segment(p: Point2, a: Point2, b: Point2) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(Point2::new(a.x + t * dx, a.y + t * dy))
}

/// Whether the segments p-q and r-t cross each other (touching only does not count;
/// touching outlines are 0 mm apart by the distance check anyway).
fn segments_cross(p: Point2, q: Point2, r: Point2, t: Point2) -> bool {
    let side =
        |a: Point2, b: Point2, c: Point2| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    let (d1, d2) = (side(r, t, p), side(r, t, q));
    let (d3, d4) = (side(p, q, r), side(p, q, t));
    (d1 * d2 < 0.0) && (d3 * d4 < 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(name: &str, x: f32, size: f32, height: f32) -> Footprint {
        let h = size / 2.0;
        Footprint {
            name: name.into(),
            outline: vec![
                Point2::new(x - h, -h),
                Point2::new(x + h, -h),
                Point2::new(x + h, h),
                Point2::new(x - h, h),
            ],
            height,
        }
    }

    #[test]
    fn test_sequential_orders_shortest_first() {
        // 10 mm squares, 30 mm apart edge to edge.
        let objects = [
            square("tall", -20.0, 10.0, 40.0),
            square("short", 20.0, 10.0, 5.0),
            square("middle", 60.0, 10.0, 15.0),
        ];
        assert_eq!(print_order(&objects, 20.0, 20.0), Ok(vec![1, 2, 0]));
        // One object: nothing to check.
        assert_eq!(print_order(&objects[..1], 20.0, 20.0), Ok(vec![0]));
    }

    #[test]
    fn test_sequential_rejects_objects_too_close() {
        let objects = [
            square("left", -10.0, 10.0, 5.0),
            square("right", 10.0, 10.0, 5.0),
        ];
        // 10 mm apart: fine for a 10 mm head, not a 20 mm one.
        assert!(print_order(&objects, 10.0, 20.0).is_ok());
        let why = print_order(&objects, 20.0, 20.0).unwrap_err();
        assert!(
            why.contains("left") && why.contains("right") && why.contains("20"),
            "{why}"
        );
        // Overlapping footprints are too close too.
        let overlapping = [square("a", 0.0, 10.0, 5.0), square("b", 4.0, 10.0, 5.0)];
        assert!(print_order(&overlapping, 1.0, 20.0).is_err());
        // Corner to corner is measured as such: 10 mm squares whose corners are 10 mm
        // apart diagonally, though their centres are 24 mm apart in x and y.
        let corner = [
            square("a", 0.0, 10.0, 5.0),
            Footprint {
                outline: square("b", 0.0, 10.0, 5.0)
                    .outline
                    .iter()
                    .map(|p| Point2::new(p.x + 17.0711, p.y + 17.0711))
                    .collect(),
                ..square("b", 0.0, 10.0, 5.0)
            },
        ];
        assert!(print_order(&corner, 9.9, 20.0).is_ok());
        assert!(print_order(&corner, 10.1, 20.0).is_err());
    }

    #[test]
    fn test_sequential_rejects_two_objects_above_the_gantry() {
        let objects = [
            square("a", -20.0, 10.0, 25.0),
            square("b", 20.0, 10.0, 30.0),
        ];
        let why = print_order(&objects, 20.0, 20.0).unwrap_err();
        assert!(why.contains('a') && why.contains("20"), "{why}");
        // One tall object is fine: it prints last.
        let one_tall = [
            square("a", -20.0, 10.0, 25.0),
            square("b", 20.0, 10.0, 15.0),
        ];
        assert_eq!(print_order(&one_tall, 20.0, 20.0), Ok(vec![1, 0]));
    }
}
