//! The 3D view: an orbiting camera, the build plate, shaded models and the sliced toolpaths.
//! Drawn with egui's painter (flat-shaded triangles sorted back to front).

use crate::geometry::Vec3;
use crate::model::Triangle;
use crate::slicer::{PathKind, SliceResult};
use crate::theme;
use eframe::egui::{self, Color32, Pos2, Rect, Shape, Stroke};

#[derive(Clone, Debug)]
pub struct Camera {
    /// Degrees around the vertical axis; 0 looks at the plate from the front.
    pub yaw: f32,
    /// Degrees above the plate.
    pub pitch: f32,
    pub distance: f32,
    pub target: Vec3,
}

const FOV_DEG: f32 = 40.0;

impl Camera {
    pub fn for_plate(bed_x: f32, bed_y: f32) -> Self {
        Self {
            yaw: 0.0,
            pitch: 38.0,
            distance: bed_x.max(bed_y) * 1.7,
            target: Vec3::new(0.0, 0.0, 15.0),
        }
    }

    fn basis(&self) -> (Vec3, Vec3, Vec3, Vec3) {
        let (sy, cy) = self.yaw.to_radians().sin_cos();
        let (sp, cp) = self.pitch.to_radians().sin_cos();
        let offset = Vec3::new(sy * cp, -cy * cp, sp) * self.distance;
        let eye = self.target + offset;
        let forward = (offset * -1.0).normalized();
        let right = forward.cross(Vec3::new(0.0, 0.0, 1.0)).normalized();
        let up = right.cross(forward);
        (eye, forward, right, up)
    }

    fn focal(rect: Rect) -> f32 {
        rect.height() / (2.0 * (FOV_DEG.to_radians() / 2.0).tan())
    }

    /// Screen position and depth of a point, or None when it is behind the camera.
    /// Projecting many points? Build a `View` once and use `View::project`.
    pub fn project(&self, p: Vec3, rect: Rect) -> Option<(Pos2, f32)> {
        View::new(self, rect).project(p)
    }

    /// Where the mouse ray meets the plate (z = 0).
    pub fn plate_point(&self, screen: Pos2, rect: Rect) -> Option<Vec3> {
        let (eye, forward, right, up) = self.basis();
        let f = Self::focal(rect);
        let c = rect.center();
        let dir = forward + right * ((screen.x - c.x) / f) + up * (-(screen.y - c.y) / f);
        if dir.z.abs() < 1e-6 {
            return None;
        }
        let t = -eye.z / dir.z;
        (t > 0.0).then(|| eye + dir * t)
    }

    pub fn orbit(&mut self, delta: egui::Vec2) {
        self.yaw -= delta.x * 0.4;
        self.pitch = (self.pitch + delta.y * 0.4).clamp(5.0, 89.0);
    }

    pub fn pan(&mut self, delta: egui::Vec2, rect: Rect) {
        let (_, _, right, up) = self.basis();
        let scale = self.distance / Self::focal(rect);
        self.target = self.target + right * (-delta.x * scale) + up * (delta.y * scale);
    }

    pub fn zoom(&mut self, scroll: f32) {
        self.distance = (self.distance * (-scroll * 0.0015).exp()).clamp(40.0, 2000.0);
    }

    /// How many millimetres one screen pixel spans at the depth of `p`.
    pub fn world_per_pixel(&self, p: Vec3, rect: Rect) -> f32 {
        let (eye, forward, _, _) = self.basis();
        (p - eye).dot(forward).max(1.0) / Self::focal(rect)
    }

    /// Unit vector from the point looked at towards the camera.
    pub fn toward_eye(&self) -> Vec3 {
        let (_, forward, _, _) = self.basis();
        forward * -1.0
    }

    fn light_dir(&self) -> Vec3 {
        let (_, forward, right, up) = self.basis();
        (forward * -1.0 + up * 0.6 + right * 0.3).normalized()
    }
}

/// A camera's view basis and screen mapping for one rect, worked out once so that
/// projecting many points (a whole mesh or toolpath) does not redo the trigonometry.
struct View {
    eye: Vec3,
    forward: Vec3,
    right: Vec3,
    up: Vec3,
    focal: f32,
    center: Pos2,
}

impl View {
    fn new(cam: &Camera, rect: Rect) -> Self {
        let (eye, forward, right, up) = cam.basis();
        Self {
            eye,
            forward,
            right,
            up,
            focal: Camera::focal(rect),
            center: rect.center(),
        }
    }

    /// Same result as `Camera::project` for the camera and rect this view was built from.
    fn project(&self, p: Vec3) -> Option<(Pos2, f32)> {
        let v = p - self.eye;
        let depth = v.dot(self.forward);
        if depth < 1.0 {
            return None;
        }
        let f = self.focal / depth;
        let c = self.center;
        Some((
            Pos2::new(c.x + v.dot(self.right) * f, c.y - v.dot(self.up) * f),
            depth,
        ))
    }
}

/// Grey plate with a 10 mm grid and the outline of the printable volume.
pub fn draw_plate(
    painter: &egui::Painter,
    cam: &Camera,
    rect: Rect,
    bed_x: f32,
    bed_y: f32,
    max_z: f32,
) {
    let view = View::new(cam, rect);
    let (hx, hy) = (bed_x / 2.0, bed_y / 2.0);
    let corners = [
        Vec3::new(-hx, -hy, 0.0),
        Vec3::new(hx, -hy, 0.0),
        Vec3::new(hx, hy, 0.0),
        Vec3::new(-hx, hy, 0.0),
    ];
    let pts: Vec<Pos2> = corners
        .iter()
        .filter_map(|c| view.project(*c).map(|p| p.0))
        .collect();
    if pts.len() == 4 {
        painter.add(Shape::convex_polygon(pts, theme::PLATE, Stroke::NONE));
    }
    let line = |a: Vec3, b: Vec3, stroke: Stroke| {
        if let (Some(pa), Some(pb)) = (view.project(a), view.project(b)) {
            painter.line_segment([pa.0, pb.0], stroke);
        }
    };
    let minor = Stroke::new(1.0, theme::PLATE_GRID);
    let major = Stroke::new(1.2, theme::PLATE_GRID_MAJOR);
    let mut x = -(hx / 10.0).floor() * 10.0;
    while x <= hx {
        let s = if (x.round() as i32) % 50 == 0 {
            major
        } else {
            minor
        };
        line(Vec3::new(x, -hy, 0.0), Vec3::new(x, hy, 0.0), s);
        x += 10.0;
    }
    let mut y = -(hy / 10.0).floor() * 10.0;
    while y <= hy {
        let s = if (y.round() as i32) % 50 == 0 {
            major
        } else {
            minor
        };
        line(Vec3::new(-hx, y, 0.0), Vec3::new(hx, y, 0.0), s);
        y += 10.0;
    }
    let edge = Stroke::new(1.5, theme::PLATE_EDGE);
    let volume = Stroke::new(1.0, theme::VOLUME_EDGE);
    for i in 0..4 {
        let (a, b) = (corners[i], corners[(i + 1) % 4]);
        line(a, b, edge);
        let lift = Vec3::new(0.0, 0.0, max_z);
        line(a + lift, b + lift, volume);
        line(a, a + lift, volume);
    }
    // Front marker so people know which side faces them.
    if let Some((p, _)) = view.project(Vec3::new(0.0, -hy - 6.0, 0.0)) {
        painter.text(
            p,
            egui::Align2::CENTER_CENTER,
            "front",
            egui::FontId::proportional(12.0),
            theme::TEXT_DIM,
        );
    }
}

/// One object to draw: its plate triangles and its colour.
pub struct DrawObject<'a> {
    pub triangles: &'a [Triangle],
    pub color: Color32,
}

/// Flat-shaded triangles of all objects, sorted back to front.
pub fn draw_objects(painter: &egui::Painter, cam: &Camera, rect: Rect, objects: &[DrawObject]) {
    let light = cam.light_dir();
    let view = View::new(cam, rect);
    let eye = view.eye;
    let mut faces: Vec<(f32, [Pos2; 3], Color32)> = Vec::new();
    for obj in objects {
        for t in obj.triangles {
            let n = (t[1] - t[0]).cross(t[2] - t[0]).normalized();
            if n.dot(t[0] - eye) > 0.0 {
                continue;
            }
            let (Some(a), Some(b), Some(c)) =
                (view.project(t[0]), view.project(t[1]), view.project(t[2]))
            else {
                continue;
            };
            let shade = 0.35 + 0.65 * n.dot(light).max(0.0);
            let col = Color32::from_rgb(
                (obj.color.r() as f32 * shade) as u8,
                (obj.color.g() as f32 * shade) as u8,
                (obj.color.b() as f32 * shade) as u8,
            );
            faces.push(((a.1 + b.1 + c.1) / 3.0, [a.0, b.0, c.0], col));
        }
    }
    faces.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut mesh = egui::Mesh::default();
    for (_, p, col) in faces {
        let base = mesh.vertices.len() as u32;
        for v in p {
            mesh.colored_vertex(v, col);
        }
        mesh.add_triangle(base, base + 1, base + 2);
        if mesh.vertices.len() > 60_000 {
            painter.add(Shape::mesh(std::mem::take(&mut mesh)));
        }
    }
    painter.add(Shape::mesh(mesh));
}

/// Index of the object under the mouse, nearest first.
pub fn pick_object(cam: &Camera, rect: Rect, objects: &[&[Triangle]], at: Pos2) -> Option<usize> {
    let view = View::new(cam, rect);
    let mut best: Option<(f32, usize)> = None;
    for (i, tris) in objects.iter().enumerate() {
        for t in tris.iter() {
            let (Some(a), Some(b), Some(c)) =
                (view.project(t[0]), view.project(t[1]), view.project(t[2]))
            else {
                continue;
            };
            if inside_triangle(at, a.0, b.0, c.0) {
                let d = (a.1 + b.1 + c.1) / 3.0;
                if best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, i));
                }
            }
        }
    }
    best.map(|b| b.1)
}

fn inside_triangle(p: Pos2, a: Pos2, b: Pos2, c: Pos2) -> bool {
    let s = |p1: Pos2, p2: Pos2, p3: Pos2| {
        (p1.x - p3.x) * (p2.y - p3.y) - (p2.x - p3.x) * (p1.y - p3.y)
    };
    let (d1, d2, d3) = (s(p, a, b), s(p, b, c), s(p, c, a));
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    // All three zero means the triangle has no area on screen: it covers nothing.
    (neg || pos) && !(neg && pos)
}

/// Toolpaths of layers 0..=`upto`; the top one drawn brighter and thicker.
pub fn draw_toolpaths(
    painter: &egui::Painter,
    cam: &Camera,
    rect: Rect,
    result: &SliceResult,
    upto: usize,
    visible: &dyn Fn(PathKind) -> bool,
) {
    let total: usize = result
        .layers
        .iter()
        .take(upto + 1)
        .map(|l| l.paths.len())
        .sum();
    let view = View::new(cam, rect);
    // Very busy prints: thin out older layers so the view stays smooth.
    let stride = (total / 40_000).max(1);
    for (li, layer) in result.layers.iter().enumerate().take(upto + 1) {
        let current = li == upto;
        if !current && li % stride != 0 {
            continue;
        }
        for path in &layer.paths {
            if !visible(path.kind) {
                continue;
            }
            let mut pts: Vec<Pos2> = path
                .points
                .iter()
                .filter_map(|p| view.project(Vec3::new(p.x, p.y, layer.z)).map(|q| q.0))
                .collect();
            if path.closed && !pts.is_empty() {
                pts.push(pts[0]);
            }
            if pts.len() < 2 {
                continue;
            }
            let base = theme::path_color(path.kind);
            let (width, color) = if current {
                (2.0, base)
            } else {
                (1.0, base.gamma_multiply(0.75))
            };
            painter.add(Shape::line(pts, Stroke::new(width, color)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_triangle_squashed_to_a_point_covers_nothing() {
        let a = Pos2::new(5.0, 5.0);
        assert!(!inside_triangle(Pos2::new(100.0, 40.0), a, a, a));
        assert!(!inside_triangle(a, a, a, a));
    }

    #[test]
    fn a_real_triangle_still_covers_its_inside_and_edges() {
        let (a, b, c) = (
            Pos2::new(0.0, 0.0),
            Pos2::new(10.0, 0.0),
            Pos2::new(0.0, 10.0),
        );
        assert!(inside_triangle(Pos2::new(2.0, 2.0), a, b, c));
        assert!(inside_triangle(Pos2::new(5.0, 0.0), a, b, c), "on an edge");
        assert!(!inside_triangle(Pos2::new(9.0, 9.0), a, b, c));
    }
}
