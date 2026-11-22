//! The StreetWide glb profile: one byte form per mesh, strict reading.

mod common;

use op_world::glb::{self, GlbMesh, Material, Node, Primitive, mm_to_f32};
use op_world::sample::parse_mesh_specs;

fn fixture_meshes() -> Vec<(String, GlbMesh)> {
    let specs = parse_mesh_specs(&std::fs::read(common::format_dir().join("meshes.json")).unwrap())
        .unwrap();
    specs
        .into_iter()
        .map(|(k, s)| (k, s.to_mesh().unwrap()))
        .collect()
}

fn house_bytes() -> Vec<u8> {
    let (_, house) = fixture_meshes()
        .into_iter()
        .find(|(k, _)| k == "house")
        .unwrap();
    glb::write(&house).unwrap()
}

#[test]
fn write_then_read_round_trips() {
    for (name, mesh) in fixture_meshes() {
        let bytes = glb::write(&mesh).unwrap();
        assert_eq!(glb::read(&bytes).unwrap(), mesh, "{name}");
        assert_eq!(
            glb::write(&mesh).unwrap(),
            bytes,
            "{name}: writer is not deterministic"
        );
    }
}

#[test]
fn committed_meshes_are_canonical() {
    let dir = common::format_dir().join("world");
    let glbs: Vec<String> = common::files_under(&dir)
        .into_iter()
        .filter(|f| f.ends_with(".glb"))
        .collect();
    assert_eq!(glbs.len(), 2);
    for f in glbs {
        glb::read(&std::fs::read(dir.join(&f)).unwrap()).unwrap_or_else(|e| panic!("{f}: {e}"));
    }
}

/// Replaces the JSON chunk of a glb and fixes up the lengths.
fn with_json(glb_bytes: &[u8], json: &[u8]) -> Vec<u8> {
    let json_len = u32::from_le_bytes(glb_bytes[12..16].try_into().unwrap()) as usize;
    let bin = &glb_bytes[20 + json_len..];
    let mut padded = json.to_vec();
    while !padded.len().is_multiple_of(4) {
        padded.push(b' ');
    }
    let mut out = glb_bytes[..12].to_vec();
    out.extend_from_slice(&(padded.len() as u32).to_le_bytes());
    out.extend_from_slice(&glb_bytes[16..20]);
    out.extend_from_slice(&padded);
    out.extend_from_slice(bin);
    let total = out.len() as u32;
    out[8..12].copy_from_slice(&total.to_le_bytes());
    out
}

fn json_of(glb_bytes: &[u8]) -> serde_json::Value {
    let json_len = u32::from_le_bytes(glb_bytes[12..16].try_into().unwrap()) as usize;
    serde_json::from_slice(&glb_bytes[20..20 + json_len]).unwrap()
}

#[test]
fn corrupted_files_are_rejected() {
    let good = house_bytes();
    let json_len = u32::from_le_bytes(good[12..16].try_into().unwrap()) as usize;
    let bin_start = 20 + json_len + 8;

    let mut cases: Vec<(&str, Vec<u8>)> = Vec::new();
    cases.push(("truncated", good[..good.len() - 4].to_vec()));
    let mut appended = good.clone();
    appended.extend_from_slice(&[0, 0, 0, 0]);
    cases.push(("trailing data", appended));
    let mut magic = good.clone();
    magic[0] = b'x';
    cases.push(("bad magic", magic));
    let mut unsnapped = good.clone();
    unsnapped[bin_start] ^= 1; // lowest mantissa bit of the first position
    cases.push(("unsnapped position", unsnapped));

    // Same content, pretty-printed JSON: valid glTF but not canonical.
    let pretty = serde_json::to_vec_pretty(&json_of(&good)).unwrap();
    cases.push(("pretty JSON", with_json(&good, &pretty)));

    let mut doc = json_of(&good);
    doc["materials"][0]["name"] = "marble".into();
    cases.push((
        "material outside palette",
        with_json(&good, &serde_json::to_vec(&doc).unwrap()),
    ));

    let mut doc = json_of(&good);
    doc["buffers"][0]["uri"] = "data:application/octet-stream;base64,".into();
    cases.push((
        "external buffer uri",
        with_json(&good, &serde_json::to_vec(&doc).unwrap()),
    ));

    let mut doc = json_of(&good);
    doc["accessors"][0]["count"] = 1_000_000.into();
    cases.push((
        "count beyond buffer",
        with_json(&good, &serde_json::to_vec(&doc).unwrap()),
    ));

    for (name, bytes) in cases {
        assert!(glb::read(&bytes).is_err(), "{name} should be rejected");
    }
}

fn triangle(material: Material) -> Primitive {
    Primitive {
        material,
        positions: vec![
            [0.0, 0.0, 0.0],
            [mm_to_f32(1000), 0.0, 0.0],
            [0.0, 0.0, mm_to_f32(1000)],
        ],
        normals: vec![[0.0, 1.0, 0.0]; 3],
        indices: vec![0, 2, 1],
    }
}

#[test]
fn writer_rejects_meshes_outside_the_profile() {
    let node = |name: &str, primitives| GlbMesh {
        nodes: vec![Node {
            name: name.into(),
            primitives,
        }],
    };
    assert!(glb::write(&node("osm:way:1", vec![triangle(Material::Road)])).is_ok());

    let mut unsnapped = triangle(Material::Road);
    unsnapped.positions[1][0] = 1.0005;
    let mut out_of_range = triangle(Material::Road);
    out_of_range.indices[2] = 3;
    let mut ragged = triangle(Material::Road);
    ragged.indices.push(0);
    let bad: Vec<(&str, GlbMesh)> = vec![
        ("no nodes", GlbMesh::default()),
        ("no primitives", node("osm:way:1", vec![])),
        (
            "name is not a feature id",
            node("my house", vec![triangle(Material::Road)]),
        ),
        ("unsnapped", node("osm:way:1", vec![unsnapped])),
        ("index out of range", node("osm:way:1", vec![out_of_range])),
        (
            "index count not a multiple of 3",
            node("osm:way:1", vec![ragged]),
        ),
        (
            "materials out of palette order",
            node(
                "osm:way:1",
                vec![triangle(Material::Wall), triangle(Material::Roof)],
            ),
        ),
        (
            "duplicate material",
            node(
                "osm:way:1",
                vec![triangle(Material::Road), triangle(Material::Road)],
            ),
        ),
    ];
    for (name, mesh) in bad {
        assert!(glb::write(&mesh).is_err(), "{name} should be rejected");
    }
}
