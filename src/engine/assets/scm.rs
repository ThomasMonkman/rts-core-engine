//! `.scm` mesh decoder: the skeleton (bones, for the sim) and the geometry
//! (vertices and triangles, for the renderer).
//!
//! Layout (version 5, little-endian, verified on stock meshes):
//! ```text
//! "MODL" i32 version  i32 boneOffset  i32 weightedBones  i32 vertexOffset
//! i32 extraVertexOffset  i32 vertexCount  i32 indexOffset  i32 indexCount
//! i32 infoOffset  i32 infoCount  i32 totalBones
//! bone (108 bytes) x totalBones at boneOffset:
//!   f32[16] inverse rest pose  f32[3] local position  f32[4] local rotation (w,x,y,z)
//!   i32 nameOffset (absolute, NUL-terminated)  i32 parent (-1 = root)  i32 0  i32 0
//! vertex (68 bytes) x vertexCount at vertexOffset:
//!   f32[3] position  f32[3] normal  f32[3] tangent  f32[3] binormal
//!   f32[2] uv0  f32[2] uv1  u8[4] bone indices
//! u16 x indexCount at indexOffset: triangle list
//! ```

use std::io;

use crate::engine::sim::math::{Fx, FxQuat, FxVec3};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Bone {
    pub name: String,
    pub parent: Option<u32>,
    /// Rest pose relative to the parent, in mesh units (fixed-point for the sim).
    pub position: FxVec3,
    pub rotation: FxQuat,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Skeleton {
    pub bones: Vec<Bone>,
    /// Rest pose of each bone in mesh space (parent chain composed), computed
    /// once so bone positions are one rotation, not a chain walk.
    pub model: Vec<(FxVec3, FxQuat)>,
}

impl Skeleton {
    pub fn new(bones: Vec<Bone>) -> Self {
        let model = (0..bones.len())
            .map(|i| {
                // Compose root -> bone. Usually parents precede children, but
                // walk the chain so any order works (cycle-guarded).
                let mut chain = Vec::new();
                let mut cur = Some(i as u32);
                while let Some(b) = cur.and_then(|c| bones.get(c as usize)) {
                    chain.push(b);
                    cur = b.parent;
                    if chain.len() > bones.len() {
                        break;
                    }
                }
                let (mut pos, mut rot) = (FxVec3::default(), FxQuat::IDENTITY);
                for b in chain.iter().rev() {
                    pos = pos + rot.rotate(b.position);
                    rot = rot.mul(b.rotation);
                }
                (pos, rot)
            })
            .collect();
        Self { bones, model }
    }

    /// Bone index by name (case-insensitive, as moho).
    pub fn find(&self, name: &str) -> Option<u32> {
        self.bones
            .iter()
            .position(|b| b.name.eq_ignore_ascii_case(name))
            .map(|i| i as u32)
    }
}

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

fn i32_at(b: &[u8], o: usize) -> io::Result<i32> {
    b.get(o..o + 4)
        .map(|s| i32::from_le_bytes(s.try_into().unwrap()))
        .ok_or_else(|| invalid("scm truncated"))
}

fn f32_at(b: &[u8], o: usize) -> io::Result<f32> {
    Ok(f32::from_bits(i32_at(b, o)? as u32))
}

fn fx(v: f32) -> Fx {
    Fx::saturating_from_num(v)
}

pub fn read_skeleton(b: &[u8]) -> io::Result<Skeleton> {
    if b.get(0..4) != Some(b"MODL") {
        return Err(invalid("not an scm (bad magic)"));
    }
    let version = i32_at(b, 4)?;
    if version != 5 {
        return Err(invalid(format!("unsupported scm version {version}")));
    }
    let bone_offset = i32_at(b, 8)? as usize;
    let total = i32_at(b, 44)?.max(0) as usize;
    let mut bones = Vec::with_capacity(total);
    for i in 0..total {
        let o = bone_offset + i * 108;
        let p = o + 64;
        let r = p + 12;
        let name_off = i32_at(b, r + 16)? as usize;
        let parent = i32_at(b, r + 20)?;
        let tail = b.get(name_off..).ok_or_else(|| invalid("bad bone name offset"))?;
        let len = tail.iter().position(|&c| c == 0).ok_or_else(|| invalid("unterminated bone name"))?;
        bones.push(Bone {
            name: String::from_utf8_lossy(&tail[..len]).into_owned(),
            parent: u32::try_from(parent).ok(),
            position: FxVec3 { x: fx(f32_at(b, p)?), y: fx(f32_at(b, p + 4)?), z: fx(f32_at(b, p + 8)?) },
            rotation: FxQuat {
                w: fx(f32_at(b, r)?),
                x: fx(f32_at(b, r + 4)?),
                y: fx(f32_at(b, r + 8)?),
                z: fx(f32_at(b, r + 12)?),
            },
        });
    }
    Ok(Skeleton::new(bones))
}

/// A mesh's geometry for rendering (mesh units, `f32`: never sim state).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Tangent frame for normal maps (FA's `NormalMappedVS`).
    pub tangents: Vec<[f32; 3]>,
    pub binormals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Second texture coordinates (normal maps read these).
    pub uvs1: Vec<[f32; 2]>,
    /// Triangle list.
    pub indices: Vec<u16>,
    /// The bone each vertex follows (FA skins rigidly: first bone index only).
    pub bone: Vec<u8>,
    pub bones: Vec<RenderBone>,
}

/// A bone's rest pose relative to its parent (`f32`, render-only).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderBone {
    /// Lowercased (clips bind to bones by name).
    pub name: String,
    pub parent: Option<u32>,
    pub position: [f32; 3],
    /// `[w, x, y, z]`, as stored.
    pub rotation: [f32; 4],
}

/// Vertex record of a version 5 `.scm`: position, normal, tangent,
/// binormal (f32x3 each), uv0, uv1 (f32x2), bone indices (u8x4) = 68 bytes.
const VERTEX_SIZE: usize = 68;

pub fn read_mesh(b: &[u8]) -> io::Result<MeshData> {
    if b.get(0..4) != Some(b"MODL") {
        return Err(invalid("not an scm (bad magic)"));
    }
    let version = i32_at(b, 4)?;
    if version != 5 {
        return Err(invalid(format!("unsupported scm version {version}")));
    }
    let vertex_offset = i32_at(b, 16)?.max(0) as usize;
    let vertex_count = i32_at(b, 24)?.max(0) as usize;
    let index_offset = i32_at(b, 28)?.max(0) as usize;
    let index_count = i32_at(b, 32)?.max(0) as usize;
    let mut m = MeshData {
        positions: Vec::with_capacity(vertex_count),
        normals: Vec::with_capacity(vertex_count),
        tangents: Vec::with_capacity(vertex_count),
        binormals: Vec::with_capacity(vertex_count),
        uvs: Vec::with_capacity(vertex_count),
        uvs1: Vec::with_capacity(vertex_count),
        indices: Vec::with_capacity(index_count),
        bone: Vec::with_capacity(vertex_count),
        bones: Vec::new(),
    };
    for i in 0..vertex_count {
        let o = vertex_offset + i * VERTEX_SIZE;
        let f = |k: usize| f32_at(b, o + k * 4);
        m.positions.push([f(0)?, f(1)?, f(2)?]);
        m.normals.push([f(3)?, f(4)?, f(5)?]);
        m.tangents.push([f(6)?, f(7)?, f(8)?]);
        m.binormals.push([f(9)?, f(10)?, f(11)?]);
        m.uvs.push([f(12)?, f(13)?]);
        m.uvs1.push([f(14)?, f(15)?]);
        m.bone.push(*b.get(o + 64).ok_or_else(|| invalid("scm vertices truncated"))?);
    }
    m.bones = read_bones(b)?;
    let total = m.bones.len();
    if let Some(&bad) = m.bone.iter().find(|&&v| v as usize >= total.max(1)) {
        return Err(invalid(format!("scm vertex bone {bad} out of range")));
    }
    for i in 0..index_count {
        let o = index_offset + i * 2;
        let v = b.get(o..o + 2).ok_or_else(|| invalid("scm indices truncated"))?;
        let idx = u16::from_le_bytes([v[0], v[1]]);
        if idx as usize >= vertex_count {
            return Err(invalid("scm index out of range"));
        }
        m.indices.push(idx);
    }
    Ok(m)
}

/// An `.scm`'s bones alone (see [`read_mesh`]).
pub fn read_bones(b: &[u8]) -> io::Result<Vec<RenderBone>> {
    if b.get(0..4) != Some(b"MODL") {
        return Err(invalid("not an scm (bad magic)"));
    }
    let bone_offset = i32_at(b, 8)?.max(0) as usize;
    let total = i32_at(b, 44)?.max(0) as usize;
    let mut bones = Vec::with_capacity(total);
    for i in 0..total {
        let p = bone_offset + i * 108 + 64;
        let r = p + 12;
        let name_off = i32_at(b, r + 16)? as usize;
        let tail = b.get(name_off..).ok_or_else(|| invalid("bad bone name offset"))?;
        let len = tail.iter().position(|&c| c == 0).ok_or_else(|| invalid("unterminated bone name"))?;
        bones.push(RenderBone {
            name: String::from_utf8_lossy(&tail[..len]).to_lowercase(),
            parent: u32::try_from(i32_at(b, r + 20)?).ok(),
            position: [f32_at(b, p)?, f32_at(b, p + 4)?, f32_at(b, p + 8)?],
            rotation: [f32_at(b, r)?, f32_at(b, r + 4)?, f32_at(b, r + 8)?, f32_at(b, r + 12)?],
        });
    }
    Ok(bones)
}

impl MeshData {
    /// Makes a far level of detail's mesh follow its LOD0's bones (`base`).
    ///
    /// A unit is posed by bone index into its LOD0 skeleton, and a far
    /// level's `.scm` lists fewer bones in another order (the Fatboy's LOD1:
    /// 16 of LOD0's 82; nearly every far level in the game differs), so
    /// posing it by those indices moves the wrong parts. Each vertex is
    /// rebound to the LOD0 bone of the same name, or of its nearest named
    /// ancestor, or the root (a far level's root has its own name,
    /// `lod1_<unit>`), and the mesh takes the LOD0 bones: both meshes are
    /// modelled in the same space, so LOD0's skinning matrices fit.
    pub fn rebind(&mut self, base: Vec<RenderBone>) {
        if base.is_empty() || base.len() > 256 {
            return;
        }
        let map: Vec<u8> = (0..self.bones.len())
            .map(|i| {
                let mut at = Some(i as u32);
                for _ in 0..self.bones.len() {
                    let Some(bone) = at.and_then(|j| self.bones.get(j as usize)) else { break };
                    if let Some(k) = base.iter().position(|b| b.name == bone.name) {
                        return k as u8;
                    }
                    at = bone.parent;
                }
                0
            })
            .collect();
        for v in &mut self.bone {
            *v = map.get(*v as usize).copied().unwrap_or(0);
        }
        self.bones = base;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bone(name: &str, parent: Option<u32>) -> RenderBone {
        RenderBone { name: name.into(), parent, ..Default::default() }
    }

    fn mesh(bone: Vec<u8>, bones: Vec<RenderBone>) -> MeshData {
        // (The other vertex data doesn't matter here.)
        MeshData { positions: Vec::new(), normals: Vec::new(), tangents: Vec::new(), binormals: Vec::new(), uvs: Vec::new(), uvs1: Vec::new(), indices: Vec::new(), bone, bones }
    }

    /// A far level's vertices follow the LOD0 bone of the same name; a bone
    /// LOD0 lacks follows its nearest named ancestor, and the level's own
    /// root (`lod1_<unit>`) the LOD0 root.
    #[test]
    fn a_far_level_is_rebound_to_its_lod0_bones_by_name() {
        let base = vec![bone("tank", None), bone("hull", Some(0)), bone("turret", Some(1)), bone("barrel", Some(2)), bone("track", Some(0))];
        let mut level = mesh(vec![0, 1, 2, 3, 4], vec![bone("lod1_tank", None), bone("track", Some(0)), bone("turret", Some(0)), bone("muzzle", Some(2)), bone("antenna", Some(0))]);
        level.rebind(base.clone());
        assert_eq!(level.bone, vec![0, 4, 2, 2, 0], "root, track, turret, muzzle -> turret, antenna -> root");
        assert_eq!(level.bones, base);
        // Nothing to follow: left as it was.
        let mut alone = mesh(vec![1], vec![bone("a", None), bone("b", Some(0))]);
        alone.rebind(Vec::new());
        assert_eq!((alone.bone, alone.bones.len()), (vec![1], 2));
    }
}
