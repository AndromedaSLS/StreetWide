//! The StreetWide glb profile 0.1 (RFC 0001, section 6): reference writer
//! and strict reader.
//!
//! A mesh has exactly one valid byte representation: the one [`write`]
//! produces. [`read`] parses a file and then checks that writing the parsed
//! mesh reproduces the input byte for byte.

use serde_json::Value;

use crate::cjson::write_string;
use crate::manifest::BoundsMm;
use crate::rules::is_feature_id;
use crate::{Error, Result};

/// Value of `asset.generator` in every profile-0.1 glb.
pub const GENERATOR: &str = "StreetWide world format 0.1";
/// Largest glb accepted by [`read`].
pub const MAX_GLB_BYTES: usize = 64 * 1024 * 1024;

const GLB_MAGIC: u32 = 0x4654_6C67; // "glTF"
const GLB_VERSION: u32 = 2;
const CHUNK_JSON: u32 = 0x4E4F_534A; // "JSON"
const CHUNK_BIN: u32 = 0x004E_4942; // "BIN\0"
const ARRAY_BUFFER: i64 = 34962;
const ELEMENT_ARRAY_BUFFER: i64 = 34963;
const COMPONENT_FLOAT: i64 = 5126;
const COMPONENT_U32: i64 = 5125;
const MODE_TRIANGLES: i64 = 4;

/// Palette materials, in palette order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Material {
    Roof,
    Wall,
    Ground,
    Terrain,
    Road,
    Path,
    Bridge,
    Water,
}

impl Material {
    pub const PALETTE: [Material; 8] = [
        Material::Roof,
        Material::Wall,
        Material::Ground,
        Material::Terrain,
        Material::Road,
        Material::Path,
        Material::Bridge,
        Material::Water,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Material::Roof => "roof",
            Material::Wall => "wall",
            Material::Ground => "ground",
            Material::Terrain => "terrain",
            Material::Road => "road",
            Material::Path => "path",
            Material::Bridge => "bridge",
            Material::Water => "water",
        }
    }

    pub fn from_name(name: &str) -> Option<Material> {
        Self::PALETTE.into_iter().find(|m| m.name() == name)
    }

    /// Linear RGBA base colour, in thousandths.
    pub fn base_color_milli(self) -> [i64; 4] {
        match self {
            Material::Roof => [600, 250, 180, 1000],
            Material::Wall => [850, 820, 760, 1000],
            Material::Ground => [500, 480, 450, 1000],
            Material::Terrain => [420, 450, 380, 1000],
            Material::Road => [300, 300, 310, 1000],
            Material::Path => [600, 580, 550, 1000],
            Material::Bridge => [550, 540, 520, 1000],
            Material::Water => [200, 350, 450, 1000],
        }
    }
}

/// Triangles with one palette material.
#[derive(Clone, PartialEq, Debug)]
pub struct Primitive {
    pub material: Material,
    /// Millimetre-snapped positions in the tile frame (see [`mm_to_f32`]).
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

/// A named part of a mesh (usually one feature).
#[derive(Clone, PartialEq, Debug)]
pub struct Node {
    /// Feature id of what the node shows.
    pub name: String,
    /// At most one primitive per material, in palette order.
    pub primitives: Vec<Primitive>,
}

/// The content of a profile-0.1 glb.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct GlbMesh {
    pub nodes: Vec<Node>,
}

/// The f32 for an integer number of millimetres: the f32 nearest to the f64
/// `mm / 1000`.
pub fn mm_to_f32(mm: i64) -> f32 {
    (mm as f64 / 1000.0) as f32
}

/// Snaps metres to the nearest millimetre (ties away from zero).
pub fn snap_m(metres: f64) -> f32 {
    mm_to_f32((metres * 1000.0).round() as i64)
}

/// The millimetre value of a snapped coordinate, or `None` if `v` is not
/// exactly `mm_to_f32(k)` for some integer `k` (including `-0.0`).
pub fn f32_to_mm(v: f32) -> Option<i64> {
    if !v.is_finite() {
        return None;
    }
    let mm = (f64::from(v) * 1000.0).round();
    if mm.abs() > 9.0e15 {
        return None;
    }
    let mm = mm as i64;
    (mm_to_f32(mm).to_bits() == v.to_bits()).then_some(mm)
}

impl GlbMesh {
    /// Checks the profile rules that do not depend on the byte layout.
    pub fn validate(&self) -> Result<()> {
        let err = |m: String| Err(Error::Glb(m));
        if self.nodes.is_empty() {
            return err("a mesh needs at least one node".into());
        }
        for node in &self.nodes {
            if !is_feature_id(&node.name) {
                return err(format!("node name {:?} is not a feature id", node.name));
            }
            if node.primitives.is_empty() {
                return err(format!("node {:?} has no primitives", node.name));
            }
            if !node
                .primitives
                .windows(2)
                .all(|w| w[0].material < w[1].material)
            {
                return err(format!(
                    "node {:?}: primitives must have distinct materials in palette order",
                    node.name
                ));
            }
            for p in &node.primitives {
                let n = p.positions.len();
                if n == 0 || u32::try_from(n).is_err() {
                    return err(format!("node {:?}: bad vertex count {n}", node.name));
                }
                if p.normals.len() != n {
                    return err(format!(
                        "node {:?}: normals and positions differ in count",
                        node.name
                    ));
                }
                if p.indices.is_empty() || !p.indices.len().is_multiple_of(3) {
                    return err(format!(
                        "node {:?}: index count must be a positive multiple of 3",
                        node.name
                    ));
                }
                if let Some(i) = p.indices.iter().find(|&&i| i as usize >= n) {
                    return err(format!(
                        "node {:?}: index {i} out of range ({n} vertices)",
                        node.name
                    ));
                }
                if p.positions
                    .iter()
                    .flatten()
                    .any(|&v| f32_to_mm(v).is_none())
                {
                    return err(format!(
                        "node {:?}: positions must be snapped to millimetres",
                        node.name
                    ));
                }
                if p.normals.iter().flatten().any(|v| !v.is_finite()) {
                    return err(format!("node {:?}: normals must be finite", node.name));
                }
            }
        }
        Ok(())
    }

    /// Bounds of all positions in millimetres.
    pub fn bounds_mm(&self) -> Option<BoundsMm> {
        self.nodes
            .iter()
            .flat_map(|n| &n.primitives)
            .filter_map(|p| primitive_bounds_mm(&p.positions))
            .reduce(|a, b| a.union(&b))
    }

    /// Number of triangles.
    pub fn triangle_count(&self) -> usize {
        self.nodes
            .iter()
            .flat_map(|n| &n.primitives)
            .map(|p| p.indices.len() / 3)
            .sum()
    }
}

fn primitive_bounds_mm(positions: &[[f32; 3]]) -> Option<BoundsMm> {
    let mut it = positions
        .iter()
        .map(|p| p.map(|v| f32_to_mm(v).unwrap_or(0)));
    let first = it.next()?;
    let mut b = BoundsMm {
        min: first,
        max: first,
    };
    for p in it {
        for i in 0..3 {
            b.min[i] = b.min[i].min(p[i]);
            b.max[i] = b.max[i].max(p[i]);
        }
    }
    Some(b)
}

// ---------------------------------------------------------------- writer --

/// Minimal JSON tree for the glb JSON chunk. Objects are written with keys
/// sorted by byte value; `Milli` numbers are written in shortest decimal
/// form (`12.345`, `-0.5`, `7`).
enum J {
    Int(i64),
    Milli(i64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(&'static str, J)>),
}

impl J {
    fn write(&self, out: &mut Vec<u8>) -> Result<()> {
        match self {
            J::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
            J::Milli(k) => write_milli(*k, out),
            J::Str(s) => write_string(s, out)?,
            J::Arr(items) => {
                out.push(b'[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    item.write(out)?;
                }
                out.push(b']');
            }
            J::Obj(fields) => {
                let mut sorted: Vec<&(&str, J)> = fields.iter().collect();
                sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
                out.push(b'{');
                for (i, (k, v)) in sorted.into_iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    write_string(k, out)?;
                    out.push(b':');
                    v.write(out)?;
                }
                out.push(b'}');
            }
        }
        Ok(())
    }
}

fn write_milli(k: i64, out: &mut Vec<u8>) {
    let abs = k.unsigned_abs();
    let (whole, frac) = (abs / 1000, abs % 1000);
    if k < 0 {
        out.push(b'-');
    }
    out.extend_from_slice(whole.to_string().as_bytes());
    if frac != 0 {
        let digits = format!("{frac:03}");
        out.push(b'.');
        out.extend_from_slice(digits.trim_end_matches('0').as_bytes());
    }
}

fn idx(i: usize) -> J {
    J::Int(i as i64)
}

/// Writes a mesh as a profile-0.1 glb.
pub fn write(mesh: &GlbMesh) -> Result<Vec<u8>> {
    mesh.validate()?;
    let used: Vec<Material> = Material::PALETTE
        .into_iter()
        .filter(|m| {
            mesh.nodes
                .iter()
                .flat_map(|n| &n.primitives)
                .any(|p| p.material == *m)
        })
        .collect();

    let mut bin: Vec<u8> = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut meshes = Vec::new();
    let mut nodes = Vec::new();

    let mut push_view = |bin: &mut Vec<u8>, bytes: Vec<u8>, target: i64| -> usize {
        let view = views.len();
        views.push(J::Obj(vec![
            ("buffer", J::Int(0)),
            ("byteLength", idx(bytes.len())),
            ("byteOffset", idx(bin.len())),
            ("target", J::Int(target)),
        ]));
        bin.extend_from_slice(&bytes);
        view
    };

    for (node_index, node) in mesh.nodes.iter().enumerate() {
        let mut prims = Vec::new();
        for p in &node.primitives {
            let bounds = primitive_bounds_mm(&p.positions)
                .ok_or_else(|| Error::Glb("primitive without positions".into()))?;
            let position_accessor = accessors.len();

            let view = push_view(&mut bin, f32x3_bytes(&p.positions), ARRAY_BUFFER);
            accessors.push(J::Obj(vec![
                ("bufferView", idx(view)),
                ("componentType", J::Int(COMPONENT_FLOAT)),
                ("count", idx(p.positions.len())),
                (
                    "max",
                    J::Arr(bounds.max.iter().map(|&k| J::Milli(k)).collect()),
                ),
                (
                    "min",
                    J::Arr(bounds.min.iter().map(|&k| J::Milli(k)).collect()),
                ),
                ("type", J::Str("VEC3".into())),
            ]));

            let view = push_view(&mut bin, f32x3_bytes(&p.normals), ARRAY_BUFFER);
            accessors.push(J::Obj(vec![
                ("bufferView", idx(view)),
                ("componentType", J::Int(COMPONENT_FLOAT)),
                ("count", idx(p.normals.len())),
                ("type", J::Str("VEC3".into())),
            ]));

            let index_bytes: Vec<u8> = p.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
            let view = push_view(&mut bin, index_bytes, ELEMENT_ARRAY_BUFFER);
            accessors.push(J::Obj(vec![
                ("bufferView", idx(view)),
                ("componentType", J::Int(COMPONENT_U32)),
                ("count", idx(p.indices.len())),
                ("type", J::Str("SCALAR".into())),
            ]));

            let material = used
                .iter()
                .position(|m| *m == p.material)
                .ok_or_else(|| Error::Glb("material missing from palette list".into()))?;
            prims.push(J::Obj(vec![
                (
                    "attributes",
                    J::Obj(vec![
                        ("NORMAL", idx(position_accessor + 1)),
                        ("POSITION", idx(position_accessor)),
                    ]),
                ),
                ("indices", idx(position_accessor + 2)),
                ("material", idx(material)),
                ("mode", J::Int(MODE_TRIANGLES)),
            ]));
        }
        meshes.push(J::Obj(vec![("primitives", J::Arr(prims))]));
        nodes.push(J::Obj(vec![
            ("mesh", idx(node_index)),
            ("name", J::Str(node.name.clone())),
        ]));
    }

    let materials = used
        .iter()
        .map(|m| {
            J::Obj(vec![
                ("name", J::Str(m.name().into())),
                (
                    "pbrMetallicRoughness",
                    J::Obj(vec![
                        (
                            "baseColorFactor",
                            J::Arr(m.base_color_milli().iter().map(|&k| J::Milli(k)).collect()),
                        ),
                        ("metallicFactor", J::Int(0)),
                        ("roughnessFactor", J::Int(1)),
                    ]),
                ),
            ])
        })
        .collect();

    let doc = J::Obj(vec![
        ("accessors", J::Arr(accessors)),
        (
            "asset",
            J::Obj(vec![
                ("generator", J::Str(GENERATOR.into())),
                ("version", J::Str("2.0".into())),
            ]),
        ),
        ("bufferViews", J::Arr(views)),
        (
            "buffers",
            J::Arr(vec![J::Obj(vec![("byteLength", idx(bin.len()))])]),
        ),
        ("materials", J::Arr(materials)),
        ("meshes", J::Arr(meshes)),
        ("nodes", J::Arr(nodes)),
        ("scene", J::Int(0)),
        (
            "scenes",
            J::Arr(vec![J::Obj(vec![(
                "nodes",
                J::Arr((0..mesh.nodes.len()).map(idx).collect()),
            )])]),
        ),
    ]);

    let mut json = Vec::new();
    doc.write(&mut json)?;
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    // All binary data is made of 4-byte values, so the BIN chunk is already
    // aligned; no padding is ever needed.
    debug_assert!(bin.len().is_multiple_of(4));

    let total = 12 + 8 + json.len() + 8 + bin.len();
    let total_u32 =
        u32::try_from(total).map_err(|_| Error::Glb("mesh too large for a glb".into()))?;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&GLB_MAGIC.to_le_bytes());
    out.extend_from_slice(&GLB_VERSION.to_le_bytes());
    out.extend_from_slice(&total_u32.to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_JSON.to_le_bytes());
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
    out.extend_from_slice(&bin);
    Ok(out)
}

fn f32x3_bytes(values: &[[f32; 3]]) -> Vec<u8> {
    values
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect()
}

// ---------------------------------------------------------------- reader --

/// Parses a profile-0.1 glb and checks it is in canonical form.
pub fn read(bytes: &[u8]) -> Result<GlbMesh> {
    let mesh = parse(bytes)?;
    if write(&mesh)? != bytes {
        return Err(Error::Glb(
            "not in the canonical StreetWide glb form (re-writing it changes the bytes)".into(),
        ));
    }
    Ok(mesh)
}

fn glb_err<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::Glb(msg.into()))
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32> {
    match bytes.get(at..at + 4) {
        Some(b) => Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        None => glb_err("truncated file"),
    }
}

fn parse(bytes: &[u8]) -> Result<GlbMesh> {
    if bytes.len() > MAX_GLB_BYTES {
        return glb_err(format!("file larger than {MAX_GLB_BYTES} bytes"));
    }
    if u32_at(bytes, 0)? != GLB_MAGIC || u32_at(bytes, 4)? != GLB_VERSION {
        return glb_err("not a glTF 2.0 binary file");
    }
    if u32_at(bytes, 8)? as usize != bytes.len() {
        return glb_err("header length does not match file size");
    }
    let json_len = u32_at(bytes, 12)? as usize;
    if u32_at(bytes, 16)? != CHUNK_JSON {
        return glb_err("first chunk must be JSON");
    }
    let json = bytes
        .get(20..20 + json_len)
        .ok_or_else(|| Error::Glb("truncated JSON chunk".into()))?;
    let bin_header = 20 + json_len;
    let bin_len = u32_at(bytes, bin_header)? as usize;
    if u32_at(bytes, bin_header + 4)? != CHUNK_BIN {
        return glb_err("second chunk must be BIN");
    }
    let bin = bytes
        .get(bin_header + 8..bin_header + 8 + bin_len)
        .ok_or_else(|| Error::Glb("truncated BIN chunk".into()))?;
    if bin_header + 8 + bin_len != bytes.len() {
        return glb_err("unexpected data after the BIN chunk");
    }

    let doc: Value = serde_json::from_slice(json)?;
    let doc = Doc { doc: &doc, bin };

    let materials: Vec<Material> = doc
        .array("materials")?
        .iter()
        .map(|m| {
            m.get("name")
                .and_then(Value::as_str)
                .and_then(Material::from_name)
                .ok_or_else(|| Error::Glb("material is not in the palette".into()))
        })
        .collect::<Result<_>>()?;

    let mut nodes = Vec::new();
    for node in doc.array("nodes")? {
        let name = node
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Glb("node without a name".into()))?
            .to_string();
        let mesh = doc.index_into("meshes", node.get("mesh"))?;
        let mut primitives = Vec::new();
        for prim in mesh
            .get("primitives")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Glb("mesh without primitives".into()))?
        {
            let attrs = prim
                .get("attributes")
                .ok_or_else(|| Error::Glb("primitive without attributes".into()))?;
            let material = as_index(prim.get("material"))
                .and_then(|i| materials.get(i).copied())
                .ok_or_else(|| Error::Glb("primitive with a bad material".into()))?;
            primitives.push(Primitive {
                material,
                positions: doc.f32x3(attrs.get("POSITION"))?,
                normals: doc.f32x3(attrs.get("NORMAL"))?,
                indices: doc.u32s(prim.get("indices"))?,
            });
        }
        nodes.push(Node { name, primitives });
    }
    Ok(GlbMesh { nodes })
}

fn as_index(v: Option<&Value>) -> Option<usize> {
    v.and_then(Value::as_u64)
        .and_then(|i| usize::try_from(i).ok())
}

struct Doc<'a> {
    doc: &'a Value,
    bin: &'a [u8],
}

impl<'a> Doc<'a> {
    fn array(&self, key: &str) -> Result<&'a Vec<Value>> {
        self.doc
            .get(key)
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Glb(format!("missing {key}")))
    }

    fn index_into(&self, key: &str, index: Option<&Value>) -> Result<&'a Value> {
        as_index(index)
            .and_then(|i| self.array(key).ok()?.get(i))
            .ok_or_else(|| Error::Glb(format!("bad reference into {key}")))
    }

    /// The bytes behind an accessor, after checking its type.
    fn accessor_bytes(
        &self,
        accessor: Option<&Value>,
        component: i64,
        ty: &str,
        size: usize,
    ) -> Result<(usize, &'a [u8])> {
        let acc = self.index_into("accessors", accessor)?;
        if acc.get("componentType").and_then(Value::as_i64) != Some(component)
            || acc.get("type").and_then(Value::as_str) != Some(ty)
        {
            return glb_err(format!(
                "accessor must be {ty} of component type {component}"
            ));
        }
        let count = as_index(acc.get("count"))
            .ok_or_else(|| Error::Glb("accessor without count".into()))?;
        let view = self.index_into("bufferViews", acc.get("bufferView"))?;
        let offset = as_index(view.get("byteOffset"))
            .ok_or_else(|| Error::Glb("buffer view without byteOffset".into()))?;
        let length = as_index(view.get("byteLength"))
            .ok_or_else(|| Error::Glb("buffer view without byteLength".into()))?;
        if count.checked_mul(size) != Some(length) {
            return glb_err("accessor count does not match its buffer view");
        }
        let end = offset
            .checked_add(length)
            .ok_or_else(|| Error::Glb("buffer view overflows".into()))?;
        let data = self
            .bin
            .get(offset..end)
            .ok_or_else(|| Error::Glb("buffer view outside the BIN chunk".into()))?;
        Ok((count, data))
    }

    fn f32x3(&self, accessor: Option<&Value>) -> Result<Vec<[f32; 3]>> {
        let (_, data) = self.accessor_bytes(accessor, COMPONENT_FLOAT, "VEC3", 12)?;
        let (chunks, _) = data.as_chunks::<12>();
        Ok(chunks
            .iter()
            .map(|c| {
                let f = |o: usize| f32::from_le_bytes([c[o], c[o + 1], c[o + 2], c[o + 3]]);
                [f(0), f(4), f(8)]
            })
            .collect())
    }

    fn u32s(&self, accessor: Option<&Value>) -> Result<Vec<u32>> {
        let (_, data) = self.accessor_bytes(accessor, COMPONENT_U32, "SCALAR", 4)?;
        let (chunks, _) = data.as_chunks::<4>();
        Ok(chunks.iter().map(|c| u32::from_le_bytes(*c)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn milli_format() {
        let cases = [
            (0, "0"),
            (7000, "7"),
            (12345, "12.345"),
            (-500, "-0.5"),
            (-1, "-0.001"),
            (1230, "1.23"),
            (-1000, "-1"),
        ];
        for (k, s) in cases {
            let mut out = Vec::new();
            write_milli(k, &mut out);
            assert_eq!(String::from_utf8(out).unwrap(), s);
        }
    }

    #[test]
    fn snapping() {
        assert_eq!(f32_to_mm(mm_to_f32(12345)), Some(12345));
        assert_eq!(f32_to_mm(mm_to_f32(-7)), Some(-7));
        assert_eq!(f32_to_mm(0.0005), None);
        assert_eq!(f32_to_mm(-0.0), None);
        assert_eq!(f32_to_mm(f32::NAN), None);
        assert_eq!(snap_m(1.2344), mm_to_f32(1234));
    }
}
