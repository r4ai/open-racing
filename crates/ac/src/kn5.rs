//! KN5 model files: embedded textures, materials and a node tree of meshes.
//!
//! Layout (all little-endian, strings are an i32 byte length followed by bytes):
//! - header: magic `sc6969`, i32 version, one more i32 when version > 5
//! - textures: count, then per texture i32 kind, name, i32 size, bytes
//! - materials: count, then per material name, shader, u8 blend mode, and for
//!   version > 4 u8 alpha tested and i32 depth mode; properties (name, f32 value,
//!   9 more f32); samplers (name, i32 slot, texture name)
//! - one root node, recursively: i32 class, name, i32 child count, u8 active, the
//!   class payload, then the children

use glam::{DMat4, DVec3};

use crate::Error;
use crate::reader::Reader;

const MAGIC: &[u8] = b"sc6969";

/// Bytes per vertex of a static mesh: position, normal, uv, tangent.
const VERTEX_SIZE: usize = 44;
/// Bytes per vertex of a skinned mesh: static layout plus 4 weights and 4 bone indices.
const SKINNED_VERTEX_SIZE: usize = 76;

#[derive(Clone, Debug)]
pub struct Texture {
    pub name: String,
    /// DDS or PNG file contents.
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct Material {
    pub shader: String,
    /// 0 = opaque, 1 = alpha blend, 2 = alpha to coverage.
    pub blend_mode: u8,
    pub alpha_tested: bool,
    pub properties: Vec<(String, f32)>,
    /// (sampler, texture name), e.g. ("txDiffuse", "asphalt.dds").
    pub samplers: Vec<(String, String)>,
}

impl Material {
    pub fn property(&self, name: &str) -> Option<f32> {
        self.properties
            .iter()
            .find(|(n, _)| n == name)
            .map(|&(_, v)| v)
    }

    pub fn texture(&self, sampler: &str) -> Option<&str> {
        self.samplers
            .iter()
            .find(|(s, _)| s == sampler)
            .map(|(_, t)| t.as_str())
    }
}

/// A static mesh with vertices already transformed to the file's world space.
#[derive(Clone, Debug)]
pub struct Mesh {
    pub name: String,
    pub visible: bool,
    pub renderable: bool,
    pub cast_shadows: bool,
    pub material: usize,
    pub positions: Vec<DVec3>,
    pub normals: Vec<DVec3>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// Empty node, used by tracks as markers such as `AC_START_0`.
#[derive(Clone, Debug)]
pub struct Dummy {
    pub name: String,
    pub position: DVec3,
}

#[derive(Clone, Debug, Default)]
pub struct Kn5 {
    pub version: i32,
    pub textures: Vec<Texture>,
    pub materials: Vec<Material>,
    pub meshes: Vec<Mesh>,
    pub dummies: Vec<Dummy>,
}

pub fn parse(buf: &[u8]) -> Result<Kn5, Error> {
    if !buf.starts_with(MAGIC) {
        return Err(Error::Format(
            "not a KN5 file (it may be encrypted or protected, which is not supported)".into(),
        ));
    }
    let mut r = Reader::new(&buf[MAGIC.len()..]);
    let mut kn5 = Kn5 {
        version: r.i32()?,
        ..Default::default()
    };
    if !(1..=6).contains(&kn5.version) {
        return Err(Error::Format(format!(
            "unsupported KN5 version {}",
            kn5.version
        )));
    }
    if kn5.version > 5 {
        r.i32()?;
    }

    for _ in 0..r.count(12)? {
        let _kind = r.i32()?;
        let name = r.string()?;
        let size = r.count(1)?;
        kn5.textures.push(Texture {
            name,
            data: r.bytes(size)?.to_vec(),
        });
    }

    for _ in 0..r.count(12)? {
        let _name = r.string()?;
        let mut m = Material {
            shader: r.string()?,
            ..Default::default()
        };
        m.blend_mode = r.u8()?;
        if kn5.version > 4 {
            m.alpha_tested = r.u8()? != 0;
            let _depth_mode = r.i32()?;
        }
        for _ in 0..r.count(44)? {
            let name = r.string()?;
            let value = r.f32()?;
            r.skip(36)?;
            m.properties.push((name, value));
        }
        for _ in 0..r.count(12)? {
            let sampler = r.string()?;
            let _slot = r.i32()?;
            m.samplers.push((sampler, r.string()?));
        }
        kn5.materials.push(m);
    }

    node(&mut r, &mut kn5, DMat4::IDENTITY, 0)?;
    if r.remaining() > 0 {
        // Every byte of a plain file belongs to the node tree; leftovers mean a
        // variant this reader does not understand, such as a protected file.
        return Err(Error::Format(
            "unsupported KN5 variant (it may be encrypted or protected, which is not supported)"
                .into(),
        ));
    }
    Ok(kn5)
}

fn node(r: &mut Reader, kn5: &mut Kn5, parent: DMat4, depth: usize) -> Result<(), Error> {
    if depth > 256 {
        return Err(Error::Format("node tree too deep".into()));
    }
    let class = r.i32()?;
    let name = r.string()?;
    let children = r.count(4)?;
    let _active = r.u8()?;
    let mut world = parent;
    match class {
        1 => {
            // Row-vector matrix with the translation in the last row, which is the
            // column-major layout glam expects for column vectors.
            let m: [f32; 16] = r.f32s()?;
            world = parent * DMat4::from_cols_array(&m.map(f64::from));
            kn5.dummies.push(Dummy {
                name,
                position: world.w_axis.truncate(),
            });
        }
        2 => mesh(r, kn5, name, parent)?,
        3 => skinned(r)?,
        other => return Err(Error::Format(format!("unknown KN5 node class {other}"))),
    }
    for _ in 0..children {
        node(r, kn5, world, depth + 1)?;
    }
    Ok(())
}

fn mesh(r: &mut Reader, kn5: &mut Kn5, name: String, world: DMat4) -> Result<(), Error> {
    let cast_shadows = r.u8()? != 0;
    let visible = r.u8()? != 0;
    let _transparent = r.u8()?;
    let vertex_count = r.count(VERTEX_SIZE)?;
    let (mut positions, mut normals, mut uvs) = (
        Vec::with_capacity(vertex_count),
        Vec::with_capacity(vertex_count),
        Vec::with_capacity(vertex_count),
    );
    for _ in 0..vertex_count {
        let [px, py, pz, nx, ny, nz, u, v, _, _, _] = r.f32s::<11>()?;
        positions.push(world.transform_point3(DVec3::new(px.into(), py.into(), pz.into())));
        normals.push(
            world
                .transform_vector3(DVec3::new(nx.into(), ny.into(), nz.into()))
                .normalize_or_zero(),
        );
        uvs.push([u, v]);
    }
    let index_count = r.count(2)?;
    let mut indices = Vec::with_capacity(index_count);
    for _ in 0..index_count {
        let i = r.u16()? as u32;
        if i as usize >= vertex_count {
            return Err(Error::Format(format!(
                "mesh {name}: index {i} out of range"
            )));
        }
        indices.push(i);
    }
    indices.truncate(indices.len() / 3 * 3);
    let material = r.i32()?;
    let _layer = r.i32()?;
    let _lod_in_out: [f32; 2] = r.f32s()?;
    let _bounding_sphere: [f32; 4] = r.f32s()?;
    let renderable = r.u8()? != 0;
    kn5.meshes.push(Mesh {
        name,
        visible,
        renderable,
        cast_shadows,
        material: material.max(0) as usize,
        positions,
        normals,
        uvs,
        indices,
    });
    Ok(())
}

/// Skinned meshes (flags, trees) are skipped; tracks rarely use them.
fn skinned(r: &mut Reader) -> Result<(), Error> {
    r.skip(3)?;
    for _ in 0..r.count(68)? {
        r.string()?;
        r.skip(64)?;
    }
    let vertices = r.count(SKINNED_VERTEX_SIZE)?;
    r.skip(vertices * SKINNED_VERTEX_SIZE)?;
    let indices = r.count(2)?;
    r.skip(indices * 2)?;
    // Material id, layer, LOD in/out.
    r.skip(16)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::reader::write::Writer;

    /// A version-6 file with one texture, one material and a dummy holding a mesh
    /// triangle, all translated by (10, 0, 0).
    pub fn sample() -> Vec<u8> {
        let mut w = Writer::default();
        w.raw(MAGIC).i32(6).i32(0);
        w.i32(1).i32(1).string("road.dds").i32(4).raw(b"DDS ");
        w.i32(1)
            .string("mat")
            .string("ksPerPixel")
            .u8(0)
            .u8(1)
            .i32(0);
        w.i32(1).string("ksAlphaRef").f32(0.5).f32s(&[0.0; 9]);
        w.i32(1).string("txDiffuse").i32(0).string("road.dds");
        // Root dummy, translation in the last row.
        w.i32(1).string("root").i32(2).u8(1);
        w.f32s(&[
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 10.0, 0.0, 0.0, 1.0,
        ]);
        // Child 1: mesh.
        w.i32(2).string("1ROAD_01").i32(0).u8(1);
        w.u8(1).u8(1).u8(0);
        w.i32(3);
        for p in [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]] {
            w.f32s(&p)
                .f32s(&[0.0, 1.0, 0.0])
                .f32s(&[0.5, 0.5])
                .f32s(&[1.0, 0.0, 0.0]);
        }
        w.i32(3).u16(0).u16(2).u16(1);
        w.i32(0)
            .i32(0)
            .f32s(&[0.0, 1000.0])
            .f32s(&[0.0, 0.0, 0.0, 2.0])
            .u8(1);
        // Child 2: marker dummy at (1, 2, 3) relative to the root.
        w.i32(1).string("AC_START_0").i32(0).u8(1);
        w.f32s(&[
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 2.0, 3.0, 1.0,
        ]);
        w.0
    }

    #[test]
    fn parses_synthetic_file() {
        let kn5 = parse(&sample()).unwrap();
        assert_eq!(kn5.version, 6);
        assert_eq!(kn5.textures[0].name, "road.dds");
        assert_eq!(kn5.textures[0].data, b"DDS ");
        let m = &kn5.materials[0];
        assert!(m.alpha_tested);
        assert_eq!(m.property("ksAlphaRef"), Some(0.5));
        assert_eq!(m.texture("txDiffuse"), Some("road.dds"));

        let mesh = &kn5.meshes[0];
        assert_eq!(mesh.name, "1ROAD_01");
        assert_eq!(mesh.positions[1], DVec3::new(11.0, 0.0, 0.0));
        assert_eq!(mesh.indices, [0, 2, 1]);
        assert!(mesh.renderable);
        let start = kn5.dummies.iter().find(|d| d.name == "AC_START_0").unwrap();
        assert_eq!(start.position, DVec3::new(11.0, 2.0, 3.0));
    }

    #[test]
    fn rejects_other_files() {
        assert!(parse(b"not a model").is_err());
        let mut truncated = sample();
        truncated.truncate(truncated.len() - 10);
        assert!(parse(&truncated).is_err());
        let mut padded = sample();
        padded.extend_from_slice(&[0; 16]);
        assert!(parse(&padded).is_err());
    }
}
