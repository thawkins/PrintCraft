//! Selection marks and handles for the selected object, in the style of the screenshots
//! the person shared (reference/picture-4..6): an outline box with corner brackets and,
//! depending on the tool, X/Y move arrows (red/green), orange corner handles for scaling,
//! or red/green/blue rings for rotating. Drawn over the 3D view; the mouse picks a handle
//! when it is within a few pixels of it.

use crate::geometry::{Bounds, Vec3};
use crate::view3d::Camera;
use eframe::egui::{self, Color32, Pos2, Rect, Shape, Stroke};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Move,
    Scale,
    Rotate,
}

impl Tool {
    pub const ALL: [Tool; 3] = [Self::Move, Self::Scale, Self::Rotate];

    pub fn label(self) -> &'static str {
        match self {
            Self::Move => "Move",
            Self::Scale => "Scale",
            Self::Rotate => "Rotate",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Move => "M",
            Self::Scale => "S",
            Self::Rotate => "R",
        }
    }
}

/// One grabbable part of the marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    MoveX,
    MoveY,
    /// One of the 8 corners of the outline box (uniform scale).
    ScaleCorner(usize),
    RotateX,
    RotateY,
    RotateZ,
}

pub const RED: Color32 = Color32::from_rgb(220, 60, 60);
pub const GREEN: Color32 = Color32::from_rgb(70, 190, 80);
pub const BLUE: Color32 = Color32::from_rgb(60, 110, 235);
pub const ORANGE: Color32 = Color32::from_rgb(240, 140, 60);
const OUTLINE: Color32 = Color32::from_rgb(225, 230, 235);
/// How close (pixels) the mouse must be to grab a handle.
const GRAB_PX: f32 = 9.0;
const RING_SAMPLES: usize = 96;

fn color_of(h: Handle) -> Color32 {
    match h {
        Handle::MoveX | Handle::RotateX => RED,
        Handle::MoveY | Handle::RotateY => GREEN,
        Handle::RotateZ => BLUE,
        Handle::ScaleCorner(_) => ORANGE,
    }
}

/// The 8 corners of a box, bottom four first.
pub fn corners(b: &Bounds) -> [Vec3; 8] {
    let (a, c) = (b.min, b.max);
    [
        Vec3::new(a.x, a.y, a.z),
        Vec3::new(c.x, a.y, a.z),
        Vec3::new(c.x, c.y, a.z),
        Vec3::new(a.x, c.y, a.z),
        Vec3::new(a.x, a.y, c.z),
        Vec3::new(c.x, a.y, c.z),
        Vec3::new(c.x, c.y, c.z),
        Vec3::new(a.x, c.y, c.z),
    ]
}

/// Handles never shrink below these sizes on screen, however small the model or far away.
const MIN_ARROW_PX: f32 = 90.0;
const MIN_RING_PX: f32 = 75.0;

/// Move arrows start at the box centre and reach a little past the box. `px` is how many
/// millimetres one pixel spans there (`Camera::world_per_pixel`).
pub fn arrow_length(b: &Bounds, px: f32) -> f32 {
    let s = b.size();
    (s.x.max(s.y).max(s.z) * 0.5 + 12.0).max(MIN_ARROW_PX * px)
}

/// Rings enclose the whole box.
pub fn ring_radius(b: &Bounds, px: f32) -> f32 {
    (b.size().length() * 0.5 + 6.0).max(MIN_RING_PX * px)
}

fn arrow_tip(b: &Bounds, h: Handle, px: f32) -> Vec3 {
    let c = b.center();
    let l = arrow_length(b, px);
    match h {
        Handle::MoveX => c + Vec3::new(l, 0.0, 0.0),
        _ => c + Vec3::new(0.0, l, 0.0),
    }
}

/// Points around a rotation ring, in plate coordinates.
pub fn ring_points(b: &Bounds, h: Handle, px: f32) -> Vec<Vec3> {
    let c = b.center();
    let r = ring_radius(b, px);
    (0..=RING_SAMPLES)
        .map(|i| {
            let a = i as f32 / RING_SAMPLES as f32 * std::f32::consts::TAU;
            let (s, co) = a.sin_cos();
            c + match h {
                Handle::RotateX => Vec3::new(0.0, co * r, s * r),
                Handle::RotateY => Vec3::new(co * r, 0.0, s * r),
                _ => Vec3::new(co * r, s * r, 0.0),
            }
        })
        .collect()
}

/// The handles a tool offers.
pub fn handles(tool: Tool) -> Vec<Handle> {
    match tool {
        Tool::Move => vec![Handle::MoveX, Handle::MoveY],
        Tool::Scale => (0..8).map(Handle::ScaleCorner).collect(),
        Tool::Rotate => vec![Handle::RotateX, Handle::RotateY, Handle::RotateZ],
    }
}

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t).distance(p)
}

fn project(cam: &Camera, rect: Rect, p: Vec3) -> Option<Pos2> {
    cam.project(p, rect).map(|q| q.0)
}

/// The handle under the mouse, nearest first.
pub fn pick(cam: &Camera, rect: Rect, b: &Bounds, tool: Tool, at: Pos2) -> Option<Handle> {
    let c = project(cam, rect, b.center())?;
    let px = cam.world_per_pixel(b.center(), rect);
    let mut best: Option<(f32, Handle)> = None;
    let mut consider = |d: f32, h: Handle| {
        if d <= GRAB_PX && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, h));
        }
    };
    for h in handles(tool) {
        match h {
            Handle::MoveX | Handle::MoveY => {
                if let Some(t) = project(cam, rect, arrow_tip(b, h, px)) {
                    // Along the shaft, or a little more generous at the head.
                    consider(
                        dist_to_segment(at, c, t).min((t.distance(at) - 4.0).max(0.0)),
                        h,
                    );
                }
            }
            Handle::ScaleCorner(i) => {
                if let Some(p) = project(cam, rect, corners(b)[i]) {
                    consider(p.distance(at), h);
                }
            }
            _ => {
                let pts: Vec<Pos2> = ring_points(b, h, px)
                    .into_iter()
                    .filter_map(|p| project(cam, rect, p))
                    .collect();
                let d = pts
                    .windows(2)
                    .map(|w| dist_to_segment(at, w[0], w[1]))
                    .fold(f32::MAX, f32::min);
                consider(d, h);
            }
        }
    }
    best.map(|b| b.1)
}

/// Draws the selection marks and the current tool's handles.
pub fn draw(
    painter: &egui::Painter,
    cam: &Camera,
    rect: Rect,
    b: &Bounds,
    tool: Tool,
    hovered: Option<Handle>,
    active: Option<Handle>,
) {
    let cs = corners(b);
    let p: Vec<Option<Pos2>> = cs.iter().map(|c| project(cam, rect, *c)).collect();
    // Outline box: the full box faintly, plus corner brackets (as in the screenshots).
    const EDGES: [(usize, usize); 12] = [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0),
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];
    let faint = Stroke::new(1.0, OUTLINE.gamma_multiply(0.35));
    let bracket = Stroke::new(1.8, OUTLINE);
    for (a, c) in EDGES {
        if let (Some(pa), Some(pc)) = (p[a], p[c]) {
            painter.line_segment([pa, pc], faint);
            let d = pc - pa;
            let k = (14.0 / d.length().max(1.0)).min(0.35);
            painter.line_segment([pa, pa + d * k], bracket);
            painter.line_segment([pc, pc - d * k], bracket);
        }
    }
    let emphasis = |h: Handle| {
        if active == Some(h) {
            2.2
        } else if hovered == Some(h) {
            1.6
        } else {
            1.0
        }
    };
    let Some(c) = project(cam, rect, b.center()) else {
        return;
    };
    let px = cam.world_per_pixel(b.center(), rect);
    for h in handles(tool) {
        let color = color_of(h);
        let e = emphasis(h);
        let col = if e > 1.0 {
            color.lerp_to_gamma(Color32::WHITE, 0.25)
        } else {
            color
        };
        match h {
            Handle::MoveX | Handle::MoveY => {
                let Some(t) = project(cam, rect, arrow_tip(b, h, px)) else {
                    continue;
                };
                painter.line_segment([c, t], Stroke::new(2.0 * e, col));
                // Arrow head: a triangle pointing along the shaft, and a small cube near it.
                let dir = (t - c).normalized();
                let side = egui::vec2(-dir.y, dir.x);
                let head = 11.0 + 3.0 * e;
                painter.add(Shape::convex_polygon(
                    vec![
                        t + dir * head,
                        t + side * head * 0.45,
                        t - side * head * 0.45,
                    ],
                    col,
                    Stroke::NONE,
                ));
                let cube =
                    egui::Rect::from_center_size(t - dir * 10.0, egui::vec2(9.0, 9.0) * e.min(1.4));
                painter.rect_filled(cube, 1.0, col);
            }
            Handle::ScaleCorner(i) => {
                let Some(pc) = p[i] else { continue };
                painter.rect_filled(
                    egui::Rect::from_center_size(pc, egui::vec2(11.0, 11.0) * e.min(1.4)),
                    2.0,
                    col,
                );
                painter.rect_stroke(
                    egui::Rect::from_center_size(pc, egui::vec2(11.0, 11.0) * e.min(1.4)),
                    2.0,
                    Stroke::new(1.0, Color32::from_black_alpha(140)),
                    egui::StrokeKind::Outside,
                );
            }
            _ => {
                let pts: Vec<Pos2> = ring_points(b, h, px)
                    .into_iter()
                    .filter_map(|q| project(cam, rect, q))
                    .collect();
                painter.add(Shape::line(pts, Stroke::new(1.6 * e, col)));
            }
        }
    }
    // Centre lines, as the reference shows for the move tool.
    if tool == Tool::Move {
        let base = Vec3::new(b.center().x, b.center().y, b.min.z);
        if let Some(pb) = project(cam, rect, base) {
            painter.line_segment([c, pb], Stroke::new(1.0, BLUE.gamma_multiply(0.8)));
        }
    }
}

/// Signed angle (radians) from the box centre to the mouse, measured on screen the
/// mathematical way (counter-clockwise positive).
pub fn screen_angle(center: Pos2, at: Pos2) -> f32 {
    (-(at.y - center.y)).atan2(at.x - center.x)
}

/// How much (degrees) a ring drag turns the object about its axis. A turn about an axis
/// looks counter-clockwise when the axis points towards the camera.
pub fn ring_turn(cam: &Camera, h: Handle, start_angle: f32, now_angle: f32) -> f32 {
    let mut d = now_angle - start_angle;
    while d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    }
    while d < -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    let axis = match h {
        Handle::RotateX => Vec3::new(1.0, 0.0, 0.0),
        Handle::RotateY => Vec3::new(0.0, 1.0, 0.0),
        _ => Vec3::new(0.0, 0.0, 1.0),
    };
    let sign = if axis.dot(cam.toward_eye()) >= 0.0 {
        1.0
    } else {
        -1.0
    };
    d.to_degrees() * sign
}

/// Uniform scale from dragging a corner: how far the mouse is from the centre now,
/// compared with when the drag started.
pub fn scale_ratio(center: Pos2, start: Pos2, now: Pos2) -> f32 {
    let s = center.distance(start).max(4.0);
    (center.distance(now) / s).clamp(0.05, 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube_bounds() -> Bounds {
        Bounds {
            min: Vec3::new(-10.0, -10.0, 0.0),
            max: Vec3::new(10.0, 10.0, 20.0),
        }
    }

    fn view() -> (Camera, Rect) {
        (
            Camera::for_plate(220.0, 220.0),
            Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 800.0)),
        )
    }

    #[test]
    fn each_tool_grabs_its_own_handles() {
        let (cam, rect) = view();
        let b = cube_bounds();
        let px = cam.world_per_pixel(b.center(), rect);
        let tip = cam
            .project(arrow_tip(&b, Handle::MoveX, px), rect)
            .unwrap()
            .0;
        assert_eq!(pick(&cam, rect, &b, Tool::Move, tip), Some(Handle::MoveX));
        let ytip = cam
            .project(arrow_tip(&b, Handle::MoveY, px), rect)
            .unwrap()
            .0;
        assert_eq!(pick(&cam, rect, &b, Tool::Move, ytip), Some(Handle::MoveY));
        let corner = cam.project(corners(&b)[6], rect).unwrap().0;
        assert_eq!(
            pick(&cam, rect, &b, Tool::Scale, corner),
            Some(Handle::ScaleCorner(6))
        );
        assert_eq!(
            pick(
                &cam,
                rect,
                &b,
                Tool::Move,
                corner + egui::vec2(200.0, 200.0)
            ),
            None
        );
        // A point on the blue ring picks rotate-Z (45° round, away from where the
        // rings cross each other).
        let on_z = cam
            .project(ring_points(&b, Handle::RotateZ, px)[RING_SAMPLES / 8], rect)
            .unwrap()
            .0;
        assert_eq!(
            pick(&cam, rect, &b, Tool::Rotate, on_z),
            Some(Handle::RotateZ)
        );
    }

    #[test]
    fn dragging_a_ring_turns_the_right_way() {
        let (cam, _) = view();
        // The camera looks down from above, so +Z points towards it: a
        // counter-clockwise drag on screen is a positive turn about Z.
        assert!(ring_turn(&cam, Handle::RotateZ, 0.0, 0.5) > 0.0);
        assert!(
            (ring_turn(&cam, Handle::RotateZ, 3.0, -3.0)
                - (-6.0 + std::f32::consts::TAU).to_degrees())
            .abs()
                < 1e-3,
            "wraps around"
        );
    }

    #[test]
    fn corner_drags_scale_evenly() {
        let c = Pos2::new(100.0, 100.0);
        assert!(
            (scale_ratio(c, Pos2::new(150.0, 100.0), Pos2::new(200.0, 100.0)) - 2.0).abs() < 1e-4
        );
        assert!(
            (scale_ratio(c, Pos2::new(150.0, 100.0), Pos2::new(125.0, 100.0)) - 0.5).abs() < 1e-4
        );
    }
}
