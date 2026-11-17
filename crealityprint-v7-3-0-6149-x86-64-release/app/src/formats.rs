//! Reading OBJ and 3MF model files (STL lives in model.rs).

use crate::geometry::Vec3;
use crate::model::{Mesh, Triangle};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

/// Wavefront OBJ: `v x y z` vertices and `f a b c ...` faces (indices may be `a/b/c`
/// or negative, counting back from the end). Faces with more corners are fanned.
pub fn parse_obj(text: &str) -> Result<Mesh, String> {
    let mut verts: Vec<Vec3> = Vec::new();
    let mut triangles = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("v") => {
                let n: Vec<f32> = parts.take(3).filter_map(|p| p.parse().ok()).collect();
                if n.len() == 3 {
                    verts.push(Vec3::new(n[0], n[1], n[2]));
                }
            }
            Some("f") => {
                // A face with any bad corner is skipped whole: dropping just that corner
                // would fan the rest into a triangle the file never described.
                let Some(idx) = parts
                    .map(|p| {
                        let i: i64 = p.split('/').next()?.parse().ok()?;
                        let k = if i < 0 { verts.len() as i64 + i } else { i - 1 };
                        (k >= 0 && (k as usize) < verts.len()).then_some(k as usize)
                    })
                    .collect::<Option<Vec<usize>>>()
                else {
                    continue;
                };
                for k in 1..idx.len().saturating_sub(1) {
                    triangles.push([verts[idx[0]], verts[idx[k]], verts[idx[k + 1]]]);
                }
            }
            _ => {}
        }
    }
    if triangles.is_empty() {
        return Err("The OBJ file has no faces in it.".into());
    }
    Ok(Mesh { triangles })
}

pub fn load_3mf(path: &Path) -> Result<Mesh, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("Could not read the file: {e}"))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|_| "This 3MF file isn't a valid package.".to_string())?;
    // Every model part: slicers such as Bambu Studio and Orca keep each object's mesh in
    // its own part (3D/Objects/object_1.model) and point to it from the main model.
    let names: Vec<String> = zip.file_names().map(|s| s.to_string()).collect();
    let mut parts: HashMap<String, String> = HashMap::new();
    for name in names
        .iter()
        .filter(|n| n.to_ascii_lowercase().ends_with(".model"))
    {
        let mut xml = String::new();
        zip.by_name(name)
            .map_err(|e| e.to_string())?
            .read_to_string(&mut xml)
            .map_err(|e| format!("Could not read the model inside the 3MF: {e}"))?;
        parts.insert(part_key(name), xml);
    }
    let root = parts
        .keys()
        .find(|k| k.eq_ignore_ascii_case("3d/3dmodel.model"))
        .or_else(|| parts.keys().min())
        .cloned()
        .ok_or("The 3MF file has no model inside.")?;
    parse_3mf_package(&root, &parts)
}

/// Package part names compared without a leading "/" and without case.
fn part_key(name: &str) -> String {
    name.trim_start_matches('/').to_ascii_lowercase()
}

/// One XML tag: name (with a leading '/' for closing tags), attributes, self-closing.
struct Tag<'a> {
    name: &'a str,
    attrs: Vec<(&'a str, &'a str)>,
}

fn tags(xml: &str) -> impl Iterator<Item = Tag<'_>> {
    let mut rest = xml;
    std::iter::from_fn(move || loop {
        let start = rest.find('<')?;
        let end = rest[start..].find('>')? + start;
        let inner = rest[start + 1..end].trim_end_matches('/').trim();
        rest = &rest[end + 1..];
        if inner.starts_with('?') || inner.starts_with('!') {
            continue;
        }
        let (name, mut attr_text) = match inner.find(char::is_whitespace) {
            Some(i) => (&inner[..i], &inner[i..]),
            None => (inner, ""),
        };
        let mut attrs = Vec::new();
        while let Some(eq) = attr_text.find('=') {
            let key = attr_text[..eq].trim();
            let after = attr_text[eq + 1..].trim_start();
            // XML attribute values are quoted with ' or "; anything else is malformed.
            let Some(q) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                break;
            };
            let Some(close) = after[1..].find(q) else {
                break;
            };
            attrs.push((key, &after[1..close + 1]));
            attr_text = &after[close + 2..];
        }
        // Drop a namespace prefix such as "m:".
        let name = name.rsplit(':').next().unwrap_or(name);
        return Some(Tag { name, attrs });
    })
}

fn attr<'a>(t: &Tag<'a>, key: &str) -> Option<&'a str> {
    t.attrs.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// A 3MF transform: 12 numbers, row-vector convention (x' = x·m00 + y·m10 + z·m20 + m30).
type Transform = [f32; 12];
const IDENTITY: Transform = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];

fn parse_transform(text: Option<&str>) -> Transform {
    let nums: Vec<f32> = text
        .unwrap_or("")
        .split_whitespace()
        .filter_map(|n| n.parse().ok())
        .collect();
    nums.try_into().unwrap_or(IDENTITY)
}

fn apply(m: &Transform, v: Vec3) -> Vec3 {
    Vec3::new(
        v.x * m[0] + v.y * m[3] + v.z * m[6] + m[9],
        v.x * m[1] + v.y * m[4] + v.z * m[7] + m[10],
        v.x * m[2] + v.y * m[5] + v.z * m[8] + m[11],
    )
}

/// `b` after `a`.
fn compose(a: &Transform, b: &Transform) -> Transform {
    let p = |v: Vec3| apply(b, apply(a, v));
    let o = p(Vec3::new(0.0, 0.0, 0.0));
    let x = p(Vec3::new(1.0, 0.0, 0.0)) - o;
    let y = p(Vec3::new(0.0, 1.0, 0.0)) - o;
    let z = p(Vec3::new(0.0, 0.0, 1.0)) - o;
    [x.x, x.y, x.z, y.x, y.y, y.z, z.x, z.y, z.z, o.x, o.y, o.z]
}

/// What one model part holds: object meshes, components and build items.
#[derive(Default)]
struct ModelPart {
    /// Millimetres per model unit, from `<model unit="…">` (None when not given: mm).
    mm_per_unit: Option<f32>,
    meshes: HashMap<String, Vec<Triangle>>,
    /// Object id → its components: (part the child lives in, child object id, transform).
    components: HashMap<String, Vec<(Option<String>, String, Transform)>>,
    /// Build items: (object id, transform).
    items: Vec<(String, Transform)>,
}

/// An attribute that may carry a namespace prefix (`p:path`, `path`).
fn attr_any<'a>(t: &Tag<'a>, local: &str) -> Option<&'a str> {
    t.attrs
        .iter()
        .find(|(k, _)| *k == local || k.rsplit(':').next() == Some(local))
        .map(|(_, v)| *v)
}

/// The 3MF core spec's units, in millimetres.
fn mm_per_unit(unit: &str) -> Option<f32> {
    Some(match unit {
        "micron" => 0.001,
        "millimeter" => 1.0,
        "centimeter" => 10.0,
        "inch" => 25.4,
        "foot" => 304.8,
        "meter" => 1000.0,
        _ => return None,
    })
}

fn parse_part(xml: &str) -> ModelPart {
    let mut part = ModelPart::default();
    let mut current: Option<String> = None;
    let mut verts: Vec<Vec3> = Vec::new();
    for t in tags(xml) {
        match t.name {
            "model" => part.mm_per_unit = attr(&t, "unit").and_then(mm_per_unit),
            "object" => {
                current = attr(&t, "id").map(str::to_string);
                verts.clear();
            }
            "vertex" => {
                let f = |k| {
                    attr(&t, k)
                        .and_then(|v| v.parse::<f32>().ok())
                        .unwrap_or(0.0)
                };
                verts.push(Vec3::new(f("x"), f("y"), f("z")));
            }
            "triangle" => {
                let i = |k| attr(&t, k).and_then(|v| v.parse::<usize>().ok());
                if let (Some(a), Some(b), Some(c), Some(id)) = (i("v1"), i("v2"), i("v3"), &current)
                {
                    if a < verts.len() && b < verts.len() && c < verts.len() {
                        part.meshes
                            .entry(id.clone())
                            .or_default()
                            .push([verts[a], verts[b], verts[c]]);
                    }
                }
            }
            "component" => {
                if let (Some(id), Some(obj)) = (&current, attr(&t, "objectid")) {
                    // The 3MF Production extension's p:path points into another part.
                    let path = attr_any(&t, "path").map(part_key);
                    part.components.entry(id.clone()).or_default().push((
                        path,
                        obj.to_string(),
                        parse_transform(attr(&t, "transform")),
                    ));
                }
            }
            "item" => {
                // Items marked printable="0" are kept out of the print by the slicer that made them.
                let printable = attr(&t, "printable").is_none_or(|p| p != "0");
                if let (Some(obj), true) = (attr(&t, "objectid"), printable) {
                    part.items
                        .push((obj.to_string(), parse_transform(attr(&t, "transform"))));
                }
            }
            _ => {}
        }
    }
    part
}

/// Reads a whole 3MF package: the build items of the `root` part, following components
/// into other parts. `parts` maps part names (see `part_key`) to their XML.
pub fn parse_3mf_package(root: &str, parts: &HashMap<String, String>) -> Result<Mesh, String> {
    let parsed: HashMap<String, ModelPart> = parts
        .iter()
        .map(|(k, xml)| (k.clone(), parse_part(xml)))
        .collect();
    fn collect(
        part: &str,
        id: &str,
        m: &Transform,
        parsed: &HashMap<String, ModelPart>,
        depth: usize,
        out: &mut Vec<Triangle>,
    ) {
        if depth > 16 {
            return;
        }
        let Some(p) = parsed.get(part) else { return };
        if let Some(tris) = p.meshes.get(id) {
            out.extend(tris.iter().map(|t| t.map(|v| apply(m, v))));
        }
        for (child_part, child, cm) in p.components.get(id).into_iter().flatten() {
            let child_part = child_part.as_deref().unwrap_or(part);
            collect(child_part, child, &compose(cm, m), parsed, depth + 1, out);
        }
    }
    let root = part_key(root);
    let main = parsed.get(&root).ok_or("The 3MF file has no main model.")?;
    let mut triangles = Vec::new();
    if main.items.is_empty() {
        let mut ids: Vec<&String> = main.meshes.keys().chain(main.components.keys()).collect();
        ids.sort();
        ids.dedup();
        for id in ids {
            collect(&root, id, &IDENTITY, &parsed, 0, &mut triangles);
        }
    } else {
        for (id, m) in &main.items {
            collect(&root, id, m, &parsed, 0, &mut triangles);
        }
    }
    if triangles.is_empty() {
        return Err("The 3MF file has no triangles in it.".into());
    }
    // The main model's unit covers the whole package; slicing works in millimetres.
    if let Some(scale) = main.mm_per_unit.filter(|s| *s != 1.0) {
        for v in triangles.iter_mut().flatten() {
            *v = Vec3::new(v.x * scale, v.y * scale, v.z * scale);
        }
    }
    Ok(Mesh { triangles })
}

/// Reads a single 3MF model part on its own (no links to other parts).
#[cfg(test)]
pub fn parse_3mf_model(xml: &str) -> Result<Mesh, String> {
    let mut parts = HashMap::new();
    parts.insert("3d/3dmodel.model".to_string(), xml.to_string());
    parse_3mf_package("3D/3dmodel.model", &parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn honours_the_3mf_model_unit() {
        let model = |unit: &str| {
            format!(
                r#"<model unit="{unit}"><resources><object id="1"><mesh><vertices>
                <vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="2"/>
                </vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object>
                </resources><build><item objectid="1"/></build></model>"#
            )
        };
        let height = |unit: &str| parse_3mf_model(&model(unit)).unwrap().bounds().size().z;
        assert!((height("millimeter") - 2.0).abs() < 1e-5);
        assert!((height("inch") - 50.8).abs() < 1e-3);
        assert!((height("centimeter") - 20.0).abs() < 1e-4);
    }

    #[test]
    fn obj_face_with_a_bad_corner_is_skipped_not_refanned() {
        let obj = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 999 3\nf 1 2 4\n";
        let m = parse_obj(obj).unwrap();
        assert_eq!(m.triangles.len(), 1, "only the valid face survives");
        assert_eq!(m.triangles[0][2], Vec3::new(0.0, 1.0, 0.0));
    }

    #[test]
    fn unquoted_non_ascii_attribute_does_not_crash_the_3mf_parser() {
        let xml = "<model><resources><object id=\"1\"><mesh><vertices>\
            <vertex x=\u{e9}1 y=\"0\" z=\"0\"/></vertices></mesh></object></resources></model>";
        // Malformed input: any result is fine as long as it returns.
        let _ = parse_3mf_model(xml);
    }

    #[test]
    fn reads_obj_with_quads_and_slashes() {
        let obj = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1/1/1 2/2/2 3/3/3 4/4/4\nf -4 -3 -2\n";
        let m = parse_obj(obj).unwrap();
        assert_eq!(m.triangles.len(), 3);
    }

    #[test]
    fn reads_3mf_model_with_build_transform() {
        let xml = r#"<?xml version="1.0"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
 <resources>
  <object id="1" type="model"><mesh>
   <vertices><vertex x="0" y="0" z="0"/><vertex x="10" y="0" z="0"/><vertex x="0" y="10" z="0"/></vertices>
   <triangles><triangle v1="0" v2="1" v3="2"/></triangles>
  </mesh></object>
  <object id="2" type="model"><components><component objectid="1" transform="1 0 0 0 1 0 0 0 1 0 0 5"/></components></object>
 </resources>
 <build><item objectid="2" transform="1 0 0 0 1 0 0 0 1 100 0 0"/></build>
</model>"#;
        let m = parse_3mf_model(xml).unwrap();
        assert_eq!(m.triangles.len(), 1);
        let v = m.triangles[0][1];
        assert_eq!((v.x, v.y, v.z), (110.0, 0.0, 5.0));
    }

    #[test]
    fn follows_components_into_other_parts_like_bambu_and_orca_files() {
        // The layout Bambu Studio / Orca write: the main model only points (p:path) to the
        // object's mesh in its own part, and one build item is marked not printable.
        let main = r#"<model xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06" requiredextensions="p">
 <resources>
  <object id="2" type="model"><components>
   <component p:path="/3D/Objects/object_1.model" objectid="1" transform="1 0 0 0 1 0 0 0 1 0 0 0" />
  </components></object>
  <object id="3" type="model"><components>
   <component p:path="/3D/Objects/object_1.model" objectid="1" />
  </components></object>
 </resources>
 <build>
  <item objectid="2" transform="1 0 0 0 1 0 0 0 1 128 128 12.5" printable="1" />
  <item objectid="3" transform="1 0 0 0 1 0 0 0 1 0 0 0" printable="0" />
 </build>
</model>"#;
        let object = r#"<model><resources><object id="1" type="model"><mesh>
  <vertices><vertex x="0" y="0" z="-1"/><vertex x="1" y="0" z="-1"/><vertex x="0" y="1" z="1"/></vertices>
  <triangles><triangle v1="0" v2="1" v3="2"/></triangles>
</mesh></object></resources></model>"#;
        let mut parts = HashMap::new();
        parts.insert(part_key("3D/3dmodel.model"), main.to_string());
        parts.insert(part_key("3D/Objects/object_1.model"), object.to_string());
        let m = parse_3mf_package("3D/3dmodel.model", &parts).unwrap();
        assert_eq!(m.triangles.len(), 1, "only the printable item");
        let v = m.triangles[0][1];
        assert_eq!((v.x, v.y, v.z), (129.0, 128.0, 11.5));
        // Without the linked part there is nothing to load, and the message says so.
        parts.remove(&part_key("3D/Objects/object_1.model"));
        assert!(parse_3mf_package("3D/3dmodel.model", &parts).is_err());
    }

    /// The person's file that didn't open (app/assets/models/XYZ+.3mf, a Bambu Studio 3MF).
    #[test]
    fn opens_the_persons_bambu_studio_3mf() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/models/XYZ+.3mf");
        if !path.exists() {
            return;
        }
        let m = load_3mf(&path).unwrap();
        assert!(m.triangles.len() > 1000, "{} triangles", m.triangles.len());
        // The same cube as the person's XYZ.stl beside it: the same size.
        let size = m.bounds().size();
        let stl = path.with_file_name("XYZ.stl");
        if stl.exists() {
            let s = crate::model::Mesh::load_stl(&stl).unwrap().bounds().size();
            for (a, b) in [(size.x, s.x), (size.y, s.y), (size.z, s.z)] {
                assert!((a - b).abs() < 0.05, "3MF {size:?} vs STL {s:?}");
            }
        }
        assert!(size.z > 1.0, "{size:?}");
    }

    #[test]
    fn reads_a_real_zip_package() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("printcraft-3mf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.3mf");
        let mut w = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        w.start_file("3D/3dmodel.model", zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(br#"<model><resources><object id="1"><mesh><vertices><vertex x="0" y="0" z="0"/><vertex x="1" y="0" z="0"/><vertex x="0" y="1" z="1"/></vertices><triangles><triangle v1="0" v2="1" v3="2"/></triangles></mesh></object></resources><build><item objectid="1"/></build></model>"#).unwrap();
        w.finish().unwrap();
        let m = load_3mf(&path).unwrap();
        assert_eq!(m.triangles.len(), 1);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}
