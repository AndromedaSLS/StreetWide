//! The prototype's points file: trees and lamp positions in the region
//! frame, converted once from the city's open data by a script kept outside
//! the repository. It is a stand-in for RFC 0004's point layers, not world
//! data, and is read only with `--proto-points`.

use std::path::Path;

use serde_json::Value;

/// A file larger than this is refused (bytes).
const MAX_BYTES: u64 = 64 << 20;

/// Which tree model draws a tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    /// Broad crowns: lime, plane, maple, horse chestnut, ash.
    Broad,
    /// Small trees: cherry, pear, apple, hawthorn; globe maples.
    Small,
    /// Narrow upright cultivars ('Erecta', 'Fastigiata', 'Columnare').
    Columnar,
    /// Conifers: yew, pine, spruce, false cypress, larch.
    Conifer,
}

impl Group {
    pub const ALL: [Group; 4] = [Group::Broad, Group::Small, Group::Columnar, Group::Conifer];

    pub fn index(self) -> usize {
        self as usize
    }

    fn parse(s: &str) -> Option<Group> {
        Some(match s {
            "broad" => Group::Broad,
            "small" => Group::Small,
            "columnar" => Group::Columnar,
            "conifer" => Group::Conifer,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Tree {
    pub id: String,
    /// Region frame (m).
    pub x: f64,
    pub z: f64,
    /// swissALTI3D ground under the trunk, in the region frame (m): a check
    /// and a fallback; the client stands trees on its own terrain.
    pub ground: f64,
    pub genus: String,
    pub group: Group,
    pub evergreen: bool,
    pub height: f32,
    pub crown: f32,
    /// Distance from the trunk to the nearest building footprint (m).
    pub facade_m: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mount {
    Wall,
    Wire,
    Pole,
    /// Deep inside a building footprint: not drawn.
    Inside,
}

#[derive(Clone, Debug)]
pub struct Lamp {
    pub id: String,
    pub x: f64,
    pub z: f64,
    pub ground: f64,
    /// Facing, degrees clockwise from north, if the record has one.
    pub yaw_deg: Option<f32>,
    pub mount: Mount,
    /// Over the Limmat in the data (bridge lamps anchored at river level).
    pub water: bool,
    /// Inside a building footprint, and how deep (m).
    pub inside: bool,
    pub inside_depth: f32,
    /// Unit vector (x, z) towards the nearest facade, and its distance (m).
    pub facade_dir: Option<[f32; 2]>,
    pub facade_m: Option<f32>,
    /// Distance to the facade on the other side (m).
    pub opposite_m: Option<f32>,
    pub records: u32,
}

pub struct Points {
    pub trees: Vec<Tree>,
    pub lamps: Vec<Lamp>,
}

fn f(v: &Value, key: &str) -> Result<f64, String> {
    v[key]
        .as_f64()
        .filter(|x| x.is_finite())
        .ok_or(format!("{key} missing or not a number"))
}

fn opt_f32(v: &Value, key: &str) -> Option<f32> {
    v[key].as_f64().filter(|x| x.is_finite()).map(|x| x as f32)
}

fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_string()
}

pub fn load(path: &Path) -> Result<Points, String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > MAX_BYTES {
        return Err(format!("{len} bytes, more than the {MAX_BYTES} allowed"));
    }
    let doc: Value = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    if doc["format"] != "streetwide.proto-points" {
        return Err("not a prototype points file".into());
    }
    let mut trees = Vec::new();
    for (i, t) in doc["trees"].as_array().into_iter().flatten().enumerate() {
        let at = |e: String| format!("tree {i}: {e}");
        let group = Group::parse(t["group"].as_str().unwrap_or_default())
            .ok_or_else(|| at("unknown group".into()))?;
        trees.push(Tree {
            id: text(t, "id"),
            x: f(t, "x").map_err(at)?,
            z: f(t, "z").map_err(at)?,
            ground: f(t, "ground").map_err(at)?,
            genus: text(t, "genus"),
            group,
            evergreen: t["evergreen"].as_bool().unwrap_or(false),
            height: (f(t, "height").map_err(at)? as f32).clamp(1.5, 60.0),
            crown: (f(t, "crown").map_err(at)? as f32).clamp(1.0, 40.0),
            facade_m: opt_f32(t, "facade_m").unwrap_or(60.0).max(0.0),
        });
    }
    let mut lamps = Vec::new();
    for (i, l) in doc["lamps"].as_array().into_iter().flatten().enumerate() {
        let at = |e: String| format!("lamp {i}: {e}");
        let mount = match l["mount"].as_str() {
            Some("wall") => Mount::Wall,
            Some("wire") => Mount::Wire,
            Some("pole") => Mount::Pole,
            Some("inside") => Mount::Inside,
            _ => return Err(at("unknown mount".into())),
        };
        let facade_dir = l["facade_dir"].as_array().and_then(|a| match a[..] {
            [ref x, ref z] => {
                let (x, z) = (x.as_f64()? as f32, z.as_f64()? as f32);
                let n = (x * x + z * z).sqrt();
                (n > 0.5).then(|| [x / n, z / n])
            }
            _ => None,
        });
        lamps.push(Lamp {
            id: text(l, "id"),
            x: f(l, "x").map_err(at)?,
            z: f(l, "z").map_err(at)?,
            ground: f(l, "ground").map_err(at)?,
            yaw_deg: opt_f32(l, "yaw"),
            mount,
            water: l["water"].as_bool().unwrap_or(false),
            inside: l["inside"].as_bool().unwrap_or(false),
            inside_depth: opt_f32(l, "inside_depth").unwrap_or(0.0),
            facade_dir,
            facade_m: opt_f32(l, "facade_m"),
            opposite_m: opt_f32(l, "opposite_m"),
            records: l["records"].as_u64().unwrap_or(1) as u32,
        });
    }
    Ok(Points { trees, lamps })
}
