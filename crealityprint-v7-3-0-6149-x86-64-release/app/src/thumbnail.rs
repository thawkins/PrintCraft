//! Pictures of the part inside the print file, so the printer's screen (and Klipper /
//! Moonraker web pages) can show what is printing.
//!
//! This is the common slicer convention, not something specific to one printer: each
//! picture is a PNG, base64-encoded and written as G-code comments near the top:
//!
//! ```text
//! ; thumbnail begin 300x300 12345
//! ; iVBORw0KGgoAAAANSUhEUgAA...   (lines of 78 characters)
//! ; thumbnail end
//! ```
//!
//! Moonraker reads these blocks into the file's metadata, and printer screens that use
//! them pick the size they want. 300×300 and 96×96 are the sizes Klipper-based Creality
//! printers are usually set up for (estimated; see PRINTER_LINK.md).

// Kept at this path: callers and tests name `thumbnail::base64`; it lives in codec.
pub(crate) use crate::codec::base64;
use crate::geometry::{Bounds, Vec3};
use crate::model::Triangle;
use image::{Rgba, RgbaImage};

/// Picture sizes written into every print file, largest first.
pub const SIZES: [u32; 2] = [300, 96];
const LINE_CHARS: usize = 78;
/// The part's colour in the picture (the app's accent, slightly lighter).
const PART_COLOUR: [f32; 3] = [70.0, 200.0, 165.0];

/// Draws the triangles from above and in front, filling about 90 % of the picture, on a
/// transparent background. Drawn at twice the size and scaled down for smooth edges.
pub fn render(tris: &[Triangle], size: u32) -> RgbaImage {
    let big = size * 2;
    let mut img = RgbaImage::new(big, big);
    let mut depth = vec![f32::INFINITY; (big * big) as usize];
    let b = Bounds::of(tris.iter().flatten().copied());
    if !b.is_valid() {
        return RgbaImage::new(size, size);
    }
    // A fixed view: turned 30° and looking down 35°, like a product shot.
    let (sy, cy) = 30f32.to_radians().sin_cos();
    let (sp, cp) = 35f32.to_radians().sin_cos();
    let towards_eye = Vec3::new(sy * cp, -cy * cp, sp);
    let forward = towards_eye * -1.0;
    let right = forward.cross(Vec3::new(0.0, 0.0, 1.0)).normalized();
    let up = right.cross(forward);
    let light = (towards_eye + up * 0.6 + right * 0.3).normalized();
    let c = b.center();
    let to_view = |p: Vec3| {
        let d = p - c;
        (d.dot(right), d.dot(up), d.dot(forward))
    };
    let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for v in tris.iter().flatten() {
        let (x, y, _) = to_view(*v);
        x0 = x0.min(x);
        x1 = x1.max(x);
        y0 = y0.min(y);
        y1 = y1.max(y);
    }
    let scale = 0.9 * big as f32 / (x1 - x0).max(y1 - y0).max(1e-3);
    let (mx, my) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let half = big as f32 / 2.0;
    let to_pixel = |p: Vec3| {
        let (x, y, z) = to_view(p);
        (half + (x - mx) * scale, half - (y - my) * scale, z)
    };
    for t in tris {
        let n = (t[1] - t[0]).cross(t[2] - t[0]).normalized();
        if n.dot(forward) > 0.0 {
            continue; // facing away
        }
        let shade = 0.35 + 0.65 * n.dot(light).max(0.0);
        let colour = Rgba([
            (PART_COLOUR[0] * shade).min(255.0) as u8,
            (PART_COLOUR[1] * shade).min(255.0) as u8,
            (PART_COLOUR[2] * shade).min(255.0) as u8,
            255,
        ]);
        let p = [to_pixel(t[0]), to_pixel(t[1]), to_pixel(t[2])];
        fill_triangle(&mut img, &mut depth, big, p, colour);
    }
    image::imageops::resize(&img, size, size, image::imageops::FilterType::Triangle)
}

fn fill_triangle(
    img: &mut RgbaImage,
    depth: &mut [f32],
    size: u32,
    p: [(f32, f32, f32); 3],
    colour: Rgba<u8>,
) {
    let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
    if area.abs() < 1e-6 {
        return;
    }
    let min_x = p
        .iter()
        .map(|q| q.0)
        .fold(f32::MAX, f32::min)
        .floor()
        .max(0.0) as u32;
    let max_x = p
        .iter()
        .map(|q| q.0)
        .fold(f32::MIN, f32::max)
        .ceil()
        .min(size as f32 - 1.0);
    let min_y = p
        .iter()
        .map(|q| q.1)
        .fold(f32::MAX, f32::min)
        .floor()
        .max(0.0) as u32;
    let max_y = p
        .iter()
        .map(|q| q.1)
        .fold(f32::MIN, f32::max)
        .ceil()
        .min(size as f32 - 1.0);
    if max_x < 0.0 || max_y < 0.0 {
        return;
    }
    for y in min_y..=max_y as u32 {
        for x in min_x..=max_x as u32 {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let w0 = ((p[1].0 - px) * (p[2].1 - py) - (p[2].0 - px) * (p[1].1 - py)) / area;
            let w1 = ((p[2].0 - px) * (p[0].1 - py) - (p[0].0 - px) * (p[2].1 - py)) / area;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let z = w0 * p[0].2 + w1 * p[1].2 + w2 * p[2].2;
            let i = (y * size + x) as usize;
            if z < depth[i] {
                depth[i] = z;
                img.put_pixel(x, y, colour);
            }
        }
    }
}

pub fn png_bytes(img: &RgbaImage) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    let _ = img.write_to(&mut out, image::ImageFormat::Png);
    out.into_inner()
}

/// One `; thumbnail begin … ; thumbnail end` block for a PNG picture.
pub fn gcode_block(img: &RgbaImage) -> String {
    let data = base64(&png_bytes(img));
    let mut s = format!(
        ";\n; thumbnail begin {}x{} {}\n",
        img.width(),
        img.height(),
        data.len()
    );
    for line in data.as_bytes().chunks(LINE_CHARS) {
        s.push_str("; ");
        s.push_str(std::str::from_utf8(line).unwrap_or(""));
        s.push('\n');
    }
    s.push_str("; thumbnail end\n;\n");
    s
}

/// The print file with pictures of the plate added after its first line (the header
/// comment), plus the large picture for showing in the app.
pub fn add_to_gcode(gcode: &str, tris: &[Triangle]) -> (String, Option<RgbaImage>) {
    if tris.is_empty() || gcode.contains("; thumbnail begin") {
        return (gcode.to_string(), None);
    }
    let pictures: Vec<RgbaImage> = SIZES.iter().map(|s| render(tris, *s)).collect();
    let blocks: String = pictures.iter().map(gcode_block).collect();
    let at = gcode.find('\n').map(|i| i + 1).unwrap_or(0);
    let mut out = String::with_capacity(gcode.len() + blocks.len());
    out.push_str(&gcode[..at]);
    out.push_str(&blocks);
    out.push_str(&gcode[at..]);
    (out, pictures.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Mesh, PlacedObject};

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn renders_the_part_with_a_clear_background() {
        let tris =
            PlacedObject::new("c", Mesh::cube(Vec3::new(20.0, 20.0, 20.0))).world_triangles();
        let img = render(&tris, 96);
        assert_eq!((img.width(), img.height()), (96, 96));
        let solid = img.pixels().filter(|p| p.0[3] == 255).count();
        assert!(
            solid > 96 * 96 / 4,
            "the cube should fill a good part of the picture"
        );
        assert_eq!(img.get_pixel(0, 0).0[3], 0, "corners stay transparent");
    }

    #[test]
    fn gcode_gets_standard_thumbnail_blocks() {
        let tris = PlacedObject::new("c", Mesh::cube(Vec3::new(20.0, 20.0, 5.0))).world_triangles();
        let (g, big) = add_to_gcode("; header\nG28\n", &tris);
        assert!(big.is_some());
        assert!(g.starts_with("; header\n;\n; thumbnail begin 300x300 "));
        assert!(g.contains("; thumbnail begin 96x96 "));
        assert_eq!(g.matches("; thumbnail end").count(), 2);
        assert!(g.ends_with("G28\n"));
        // The stated length matches the data, and every data line is at most 78 characters.
        let block = g.split("; thumbnail begin 96x96 ").nth(1).unwrap();
        let len: usize = block.lines().next().unwrap().parse().unwrap();
        let data: String = block
            .lines()
            .skip(1)
            .take_while(|l| !l.starts_with("; thumbnail end"))
            .map(|l| {
                assert!(l.len() <= 2 + 78);
                l.trim_start_matches("; ")
            })
            .collect();
        assert_eq!(data.len(), len);
        assert!(data.starts_with("iVBORw0KGgo"), "a PNG");
        // Only added once.
        assert_eq!(add_to_gcode(&g, &tris).0, g);
    }
}
