//! Split to objects: the separate pieces (shells) of a mesh. Triangles belong to one
//! shell when they share a corner, directly or through other triangles.
//!
//! A shell facing inwards (negative volume) is the inside wall of a hollow, not a part
//! of its own, so it stays with the smallest shell whose box holds it.

use crate::geometry::Bounds;
use crate::model::{Mesh, Triangle};
use std::collections::HashMap;

/// The shells of `triangles` as meshes, in the order their first triangles appear.
pub fn shells(triangles: &[Triangle]) -> Vec<Mesh> {
    let mut parent: Vec<usize> = (0..triangles.len()).collect();
    let mut first_at: HashMap<(u32, u32, u32), usize> = HashMap::new();
    for (i, t) in triangles.iter().enumerate() {
        for v in t {
            let key = (v.x.to_bits(), v.y.to_bits(), v.z.to_bits());
            let j = *first_at.entry(key).or_insert(i);
            union(&mut parent, i, j);
        }
    }
    // Group triangles by their root, in order of first appearance.
    let mut group_of: HashMap<usize, usize> = HashMap::new();
    let mut groups: Vec<Vec<Triangle>> = Vec::new();
    for (i, t) in triangles.iter().enumerate() {
        let root = find(&mut parent, i);
        let g = *group_of.entry(root).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[g].push(*t);
    }

    // Inward-facing shells join the smallest outward shell around them.
    let volume = |g: &[Triangle]| -> f64 {
        g.iter()
            .map(|t| f64::from(t[0].dot(t[1].cross(t[2]))))
            .sum()
    };
    let bounds: Vec<Bounds> = groups
        .iter()
        .map(|g| Bounds::of(g.iter().flatten().copied()))
        .collect();
    let outward: Vec<bool> = groups.iter().map(|g| volume(g) >= 0.0).collect();
    let holds = |a: &Bounds, b: &Bounds| {
        a.min.x <= b.min.x
            && a.min.y <= b.min.y
            && a.min.z <= b.min.z
            && a.max.x >= b.max.x
            && a.max.y >= b.max.y
            && a.max.z >= b.max.z
    };
    let size = |b: &Bounds| {
        let s = b.size();
        s.x * s.y * s.z
    };
    let mut into: Vec<usize> = (0..groups.len()).collect();
    for i in (0..groups.len()).filter(|&i| !outward[i]) {
        if let Some(j) = (0..groups.len())
            .filter(|&j| outward[j] && holds(&bounds[j], &bounds[i]))
            .min_by(|&a, &b| size(&bounds[a]).total_cmp(&size(&bounds[b])))
        {
            into[i] = j;
        }
    }
    let mut out: Vec<Option<Vec<Triangle>>> = vec![None; groups.len()];
    for (i, g) in groups.into_iter().enumerate() {
        out[into[i]].get_or_insert_with(Vec::new).extend(g);
    }
    out.into_iter()
        .flatten()
        .map(|triangles| Mesh { triangles })
        .collect()
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let (ra, rb) = (find(parent, a), find(parent, b));
    if ra != rb {
        parent[ra.max(rb)] = ra.min(rb);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Vec3;

    fn cube_at(size: f32, at: Vec3) -> Vec<Triangle> {
        Mesh::cube(Vec3::new(size, size, size))
            .triangles
            .iter()
            .map(|t| t.map(|v| v + at))
            .collect()
    }

    #[test]
    fn test_split_two_shell_mesh_gives_two_meshes() {
        let mut tris = cube_at(10.0, Vec3::default());
        tris.extend(cube_at(5.0, Vec3::new(30.0, 0.0, 0.0)));
        let parts = shells(&tris);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].triangles, cube_at(10.0, Vec3::default()));
        assert_eq!(parts[1].triangles.len(), 12);
        assert!((parts[1].bounds().center().x - 30.0).abs() < 1e-4);
    }

    #[test]
    fn test_split_one_shell_stays_whole() {
        let parts = shells(&cube_at(10.0, Vec3::default()));
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].triangles.len(), 12);
    }

    #[test]
    fn test_split_keeps_a_hollow_with_its_inside_wall() {
        // A hollow box: an outer cube and, inside it, a smaller cube facing inwards. A
        // separate cube beside it is still its own part.
        let mut tris = cube_at(20.0, Vec3::default());
        let inner: Vec<Triangle> = cube_at(10.0, Vec3::new(0.0, 0.0, 5.0))
            .into_iter()
            .map(|t| [t[0], t[2], t[1]])
            .collect();
        tris.extend(inner);
        tris.extend(cube_at(5.0, Vec3::new(40.0, 0.0, 0.0)));
        let parts = shells(&tris);
        assert_eq!(parts.len(), 2);
        assert_eq!(
            parts[0].triangles.len(),
            24,
            "outer and inner walls together"
        );
        assert_eq!(parts[1].triangles.len(), 12);
    }
}
