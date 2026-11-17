//! 3D models: loading STL files, built-in sample shapes, and objects placed on the plate.

use crate::geometry::{Bounds, Vec3};
use std::path::Path;

pub type Triangle = [Vec3; 3];

/// A triangle mesh in millimetres.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub triangles: Vec<Triangle>,
}

impl Mesh {
    pub fn bounds(&self) -> Bounds {
        let mut b = Bounds::empty();
        for t in &self.triangles {
            for v in t {
                b.include(*v);
            }
        }
        b
    }

    /// Moves the mesh so its footprint is centred on the origin and it rests on z = 0.
    pub fn recentred(mut self) -> Self {
        let b = self.bounds();
        if !b.is_valid() {
            return self;
        }
        let c = b.center();
        let offset = Vec3::new(c.x, c.y, b.min.z);
        for t in &mut self.triangles {
            for v in t.iter_mut() {
                *v = *v - offset;
            }
        }
        self
    }

    /// Opens an STL, OBJ or 3MF file, judged by its extension.
    pub fn load(path: &Path) -> Result<Mesh, String> {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let mesh = match ext.as_str() {
            "obj" => {
                let data =
                    std::fs::read(path).map_err(|e| format!("Could not read the file: {e}"))?;
                crate::formats::parse_obj(&String::from_utf8_lossy(&data))
            }
            "3mf" => crate::formats::load_3mf(path),
            _ => Self::load_stl(path),
        }?;
        if !mesh.is_finite() {
            return Err("The model has corrupt coordinates (not a number or infinite).".into());
        }
        Ok(mesh)
    }

    /// True when every vertex coordinate is an ordinary number (no NaN or infinity).
    pub fn is_finite(&self) -> bool {
        self.triangles
            .iter()
            .flatten()
            .all(|v| v.x.is_finite() && v.y.is_finite() && v.z.is_finite())
    }

    pub fn load_stl(path: &Path) -> Result<Mesh, String> {
        let data = std::fs::read(path).map_err(|e| format!("Could not read the file: {e}"))?;
        Self::parse_stl(&data)
    }

    pub fn parse_stl(data: &[u8]) -> Result<Mesh, String> {
        let looks_binary = data.len() >= 84 && {
            let count = u32::from_le_bytes([data[80], data[81], data[82], data[83]]) as usize;
            84 + count * 50 == data.len()
        };
        let mesh = if looks_binary {
            Self::parse_binary_stl(data)?
        } else if data.starts_with(b"solid") {
            Self::parse_ascii_stl(data)?
        } else {
            Self::parse_binary_stl(data)?
        };
        if mesh.triangles.is_empty() {
            return Err("The file has no triangles in it.".into());
        }
        Ok(mesh)
    }

    fn parse_binary_stl(data: &[u8]) -> Result<Mesh, String> {
        if data.len() < 84 {
            return Err("The file is too short to be an STL model.".into());
        }
        let count = u32::from_le_bytes([data[80], data[81], data[82], data[83]]) as usize;
        if data.len() < 84 + count * 50 {
            return Err("The STL file is cut short.".into());
        }
        let f = |o: usize| f32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
        let triangles = (0..count)
            .map(|i| {
                let base = 84 + i * 50 + 12;
                let v = |k: usize| {
                    let o = base + k * 12;
                    Vec3::new(f(o), f(o + 4), f(o + 8))
                };
                [v(0), v(1), v(2)]
            })
            .collect();
        Ok(Mesh { triangles })
    }

    fn parse_ascii_stl(data: &[u8]) -> Result<Mesh, String> {
        let text = String::from_utf8_lossy(data);
        let mut verts = Vec::new();
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            if parts.next() == Some("vertex") {
                let nums: Vec<f32> = parts.filter_map(|p| p.parse().ok()).collect();
                if nums.len() == 3 {
                    verts.push(Vec3::new(nums[0], nums[1], nums[2]));
                }
            }
        }
        let triangles = verts.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
        Ok(Mesh { triangles })
    }

    /// Writes the mesh as a binary STL (used by tests and "save model").
    #[cfg(test)]
    pub fn to_binary_stl(&self) -> Vec<u8> {
        let mut out = vec![0u8; 80];
        out.extend_from_slice(&(self.triangles.len() as u32).to_le_bytes());
        for t in &self.triangles {
            let n = (t[1] - t[0]).cross(t[2] - t[0]).normalized();
            for v in std::iter::once(n).chain(t.iter().copied()) {
                for c in [v.x, v.y, v.z] {
                    out.extend_from_slice(&c.to_le_bytes());
                }
            }
            out.extend_from_slice(&[0, 0]);
        }
        out
    }

    pub fn cube(size: Vec3) -> Mesh {
        let (x, y, z) = (size.x / 2.0, size.y / 2.0, size.z);
        let p = [
            Vec3::new(-x, -y, 0.0),
            Vec3::new(x, -y, 0.0),
            Vec3::new(x, y, 0.0),
            Vec3::new(-x, y, 0.0),
            Vec3::new(-x, -y, z),
            Vec3::new(x, -y, z),
            Vec3::new(x, y, z),
            Vec3::new(-x, y, z),
        ];
        let quads = [
            [0, 3, 2, 1],
            [4, 5, 6, 7],
            [0, 1, 5, 4],
            [1, 2, 6, 5],
            [2, 3, 7, 6],
            [3, 0, 4, 7],
        ];
        let mut triangles = Vec::new();
        for q in quads {
            triangles.push([p[q[0]], p[q[1]], p[q[2]]]);
            triangles.push([p[q[0]], p[q[2]], p[q[3]]]);
        }
        Mesh { triangles }
    }

    /// A cylinder (or cone when the radii differ) standing on z = 0.
    pub fn cylinder(bottom_radius: f32, top_radius: f32, height: f32, sides: usize) -> Mesh {
        let ring = |r: f32, z: f32| -> Vec<Vec3> {
            (0..sides)
                .map(|i| {
                    let a = i as f32 / sides as f32 * std::f32::consts::TAU;
                    Vec3::new(r * a.cos(), r * a.sin(), z)
                })
                .collect()
        };
        let bottom = ring(bottom_radius, 0.0);
        let top = ring(top_radius, height);
        let cb = Vec3::new(0.0, 0.0, 0.0);
        let ct = Vec3::new(0.0, 0.0, height);
        let mut triangles = Vec::new();
        for i in 0..sides {
            let j = (i + 1) % sides;
            triangles.push([cb, bottom[j], bottom[i]]);
            triangles.push([ct, top[i], top[j]]);
            triangles.push([bottom[i], bottom[j], top[j]]);
            triangles.push([bottom[i], top[j], top[i]]);
        }
        Mesh { triangles }
    }
}

/// The built-in shapes offered in the "Add shape" menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleShape {
    Cube,
    Cylinder,
    Cone,
    Tube,
}

impl SampleShape {
    pub const ALL: [SampleShape; 4] = [Self::Cube, Self::Cylinder, Self::Cone, Self::Tube];

    pub fn label(self) -> &'static str {
        match self {
            Self::Cube => "Cube",
            Self::Cylinder => "Cylinder",
            Self::Cone => "Cone",
            Self::Tube => "Tube",
        }
    }

    pub fn mesh(self) -> Mesh {
        match self {
            Self::Cube => Mesh::cube(Vec3::new(20.0, 20.0, 20.0)),
            Self::Cylinder => Mesh::cylinder(12.0, 12.0, 30.0, 48),
            Self::Cone => Mesh::cylinder(15.0, 2.0, 30.0, 48),
            Self::Tube => tube(14.0, 9.0, 25.0, 48),
        }
    }
}

/// A hollow tube: shows off inner walls in the slice preview.
fn tube(outer: f32, inner: f32, height: f32, sides: usize) -> Mesh {
    let pt = |r: f32, i: usize, z: f32| {
        let a = i as f32 / sides as f32 * std::f32::consts::TAU;
        Vec3::new(r * a.cos(), r * a.sin(), z)
    };
    let mut triangles = Vec::new();
    for i in 0..sides {
        let j = (i + 1) % sides;
        let (ob0, ob1, ot0, ot1) = (
            pt(outer, i, 0.0),
            pt(outer, j, 0.0),
            pt(outer, i, height),
            pt(outer, j, height),
        );
        let (ib0, ib1, it0, it1) = (
            pt(inner, i, 0.0),
            pt(inner, j, 0.0),
            pt(inner, i, height),
            pt(inner, j, height),
        );
        triangles.extend([[ob0, ob1, ot1], [ob0, ot1, ot0]]);
        triangles.extend([[ib1, ib0, it0], [ib1, it0, it1]]);
        triangles.extend([[ot0, ot1, it1], [ot0, it1, it0]]);
        triangles.extend([[ob1, ob0, ib0], [ob1, ib0, ib1]]);
    }
    Mesh { triangles }
}

/// One model placed on the build plate.
#[derive(Clone, Debug)]
pub struct PlacedObject {
    pub name: String,
    /// The original mesh, centred on the origin and resting on z = 0.
    pub mesh: Mesh,
    /// Centre of the footprint on the plate, in millimetres.
    pub x: f32,
    pub y: f32,
    /// Rotation around each axis, in degrees.
    pub rotation: Vec3,
    /// Uniform scale, in percent.
    pub scale_percent: f32,
}

impl PlacedObject {
    pub fn new(name: impl Into<String>, mesh: Mesh) -> Self {
        Self {
            name: name.into(),
            mesh: mesh.recentred(),
            x: 0.0,
            y: 0.0,
            rotation: Vec3::default(),
            scale_percent: 100.0,
        }
    }

    fn rotate(&self, v: Vec3) -> Vec3 {
        let (sx, cx) = self.rotation.x.to_radians().sin_cos();
        let (sy, cy) = self.rotation.y.to_radians().sin_cos();
        let (sz, cz) = self.rotation.z.to_radians().sin_cos();
        let v = Vec3::new(v.x, v.y * cx - v.z * sx, v.y * sx + v.z * cx);
        let v = Vec3::new(v.x * cy + v.z * sy, v.y, -v.x * sy + v.z * cy);
        Vec3::new(v.x * cz - v.y * sz, v.x * sz + v.y * cz, v.z)
    }

    /// Triangles in plate coordinates; the object always rests on the plate (z = 0).
    pub fn world_triangles(&self) -> Vec<Triangle> {
        let s = self.scale_percent / 100.0;
        let mut tris: Vec<Triangle> = self
            .mesh
            .triangles
            .iter()
            .map(|t| t.map(|v| self.rotate(v * s)))
            .collect();
        let mut b = Bounds::empty();
        tris.iter().flatten().for_each(|v| b.include(*v));
        if b.is_valid() {
            let c = b.center();
            let shift = Vec3::new(self.x - c.x, self.y - c.y, -b.min.z);
            for t in &mut tris {
                for v in t.iter_mut() {
                    *v = *v + shift;
                }
            }
        }
        tris
    }

    /// Turns the model so its largest flat face rests on the plate. Faces pointing the
    /// same way (within about 1°) count together. Spin around the vertical is kept.
    pub fn lay_flat(&mut self) {
        let mut areas: std::collections::HashMap<(i32, i32, i32), (f32, Vec3)> = Default::default();
        for t in &self.mesh.triangles {
            let c = (t[1] - t[0]).cross(t[2] - t[0]);
            let area = c.length() * 0.5;
            if area < 1e-9 {
                continue;
            }
            let n = c.normalized();
            let key = (
                (n.x * 60.0).round() as i32,
                (n.y * 60.0).round() as i32,
                (n.z * 60.0).round() as i32,
            );
            let e = areas.entry(key).or_insert((0.0, Vec3::default()));
            e.0 += area;
            e.1 = e.1 + n * area;
        }
        let Some((_, (_, sum))) = areas.into_iter().max_by(|a, b| a.1 .0.total_cmp(&b.1 .0)) else {
            return;
        };
        let n = sum.normalized();
        // Turn about X until the normal has no Y part, then about Y until it points down.
        let a = n.y.atan2(n.z);
        let r = (n.y * n.y + n.z * n.z).sqrt();
        let b = (-n.x).atan2(r) + std::f32::consts::PI;
        let wrap = |deg: f32| {
            let d = deg % 360.0;
            if d > 180.0 {
                d - 360.0
            } else if d < -180.0 {
                d + 360.0
            } else {
                d
            }
        };
        self.rotation.x = wrap(a.to_degrees());
        self.rotation.y = wrap(b.to_degrees());
    }

    pub fn world_bounds(&self) -> Bounds {
        let mut b = Bounds::empty();
        self.world_triangles()
            .iter()
            .flatten()
            .for_each(|v| b.include(*v));
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_model_with_infinite_or_nan_coordinates() {
        let dir = std::env::temp_dir().join(format!("printcraft-finite-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, coord) in [("inf.stl", "1e39"), ("nan.stl", "NaN")] {
            let path = dir.join(name);
            let stl = format!(
                "solid t\nfacet normal 0 0 1\nouter loop\nvertex {coord} 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid t\n"
            );
            std::fs::write(&path, stl).unwrap();
            let err = Mesh::load(&path).unwrap_err();
            assert!(err.contains("corrupt"), "{name}: {err}");
        }
        assert!(Mesh::cube(Vec3::new(1.0, 1.0, 1.0)).is_finite());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn binary_stl_round_trips() {
        let cube = Mesh::cube(Vec3::new(10.0, 10.0, 10.0));
        let parsed = Mesh::parse_stl(&cube.to_binary_stl()).unwrap();
        assert_eq!(parsed.triangles.len(), 12);
    }

    #[test]
    fn reads_ascii_stl() {
        let text = "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid t\n";
        let m = Mesh::parse_stl(text.as_bytes()).unwrap();
        assert_eq!(m.triangles.len(), 1);
    }

    #[test]
    fn lay_flat_puts_biggest_face_down() {
        // A thin slab stood on its edge: after laying flat it should be 2 mm tall.
        let mut o = PlacedObject::new("slab", Mesh::cube(Vec3::new(40.0, 30.0, 2.0)));
        o.rotation = Vec3::new(90.0, 0.0, 30.0);
        assert!(o.world_bounds().size().z > 20.0);
        o.lay_flat();
        assert!(
            (o.world_bounds().size().z - 2.0).abs() < 0.01,
            "{:?}",
            o.world_bounds().size()
        );
        // A cone lies on its wide base, not its tip.
        let mut cone = PlacedObject::new("cone", SampleShape::Cone.mesh());
        cone.rotation = Vec3::new(0.0, 180.0, 0.0);
        cone.lay_flat();
        let tris = cone.world_triangles();
        let low = tris.iter().flatten().filter(|v| v.z < 0.01).count();
        assert!(low > 40, "base should touch the plate");
    }

    #[test]
    fn placed_object_rests_on_plate() {
        let mut o = PlacedObject::new("c", Mesh::cube(Vec3::new(10.0, 10.0, 10.0)));
        o.rotation = Vec3::new(45.0, 0.0, 0.0);
        o.x = 50.0;
        let b = o.world_bounds();
        assert!(b.min.z.abs() < 1e-4);
        assert!((b.center().x - 50.0).abs() < 1e-3);
    }
}
