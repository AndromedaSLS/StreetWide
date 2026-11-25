//! The crowd prototype's lanes: read from the scratch file that the
//! conversion script writes from the ingest's OSM cache, then kept clear of
//! the building shells, given their heights and the room beside them.
//!
//! This stands in for the ingest step that the navigation-data RFC would
//! add ("lanes matched to building shells by the ingest"): a lane point
//! closer than [`CLEAR_M`] to a wall is moved sideways, by at most
//! [`MAX_SHIFT_M`]; a lane that cannot be cleared so is dropped, unless it
//! ends at an entrance or passes through a building passage, which are
//! kept and marked. Client code: floats are fine here.

use std::time::Instant;

use super::population::{self as pop, Edge, Lanes, Place, Spot, Vert, flag, kind};
use super::walls::{Ground, Walls};

/// Lanes keep this far from walls (m): a body's radius and a margin.
pub const CLEAR_M: f64 = 0.35;
/// The furthest a lane is moved sideways (m).
pub const MAX_SHIFT_M: f64 = 1.0;
/// Walls are cut at this height above the ground (m).
pub const WALL_CUT_M: f64 = 0.5;
/// How fast a shift tapers off along the lane (m per m).
const TAPER: f64 = 0.75;
/// Room beside a lane is measured out to this far by kind (m).
fn half_width(k: u8) -> f64 {
    match k {
        kind::SIDEWALK | kind::FOOTWAY | kind::PLATFORM => 1.0,
        kind::CROSSING | kind::LIVING => 1.5,
        kind::STEPS => 0.8,
        kind::PEDESTRIAN | kind::SQUARE => 3.0,
        kind::LANE => 1.2,
        _ => 0.3,
    }
}

/// The file as written by the conversion.
pub struct Raw {
    nodes: Vec<([f64; 2], u8)>,
    edges: Vec<(u32, u32, u8, u8, u8, i32)>,
    squares: Vec<i64>,
    places: Vec<(u8, u32, i64, i64)>,
    spots: Vec<(u8, u32, i64)>,
}

fn section<'a>(lines: &mut impl Iterator<Item = &'a str>, name: &str) -> Result<usize, String> {
    let line = lines.next().ok_or(format!("missing section {name}"))?;
    let (n, count) = line
        .split_once(' ')
        .ok_or(format!("bad section line {line}"))?;
    if n != name {
        return Err(format!("expected {name}, found {line}"));
    }
    count
        .trim()
        .parse()
        .map_err(|_| format!("bad count in {line}"))
}

fn fields<const N: usize>(line: Option<&str>) -> Result<[i64; N], String> {
    let line = line.ok_or("file ends early")?;
    let v: Vec<i64> = line
        .split_whitespace()
        .map(|t| t.parse().map_err(|_| format!("bad number in {line}")))
        .collect::<Result<_, _>>()?;
    v.try_into()
        .map_err(|_| format!("expected {N} numbers in {line}"))
}

pub fn read(text: &str) -> Result<Raw, String> {
    let mut lines = text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty());
    let n = section(&mut lines, "nodes")?;
    let mut nodes = Vec::with_capacity(n);
    for _ in 0..n {
        let [x, z, f] = fields::<3>(lines.next())?;
        nodes.push(([x as f64 / 1000.0, z as f64 / 1000.0], f as u8));
    }
    let n = section(&mut lines, "edges")?;
    let mut edges = Vec::with_capacity(n);
    for _ in 0..n {
        let [a, b, k, c, f, q] = fields::<6>(lines.next())?;
        if a as usize >= nodes.len() || b as usize >= nodes.len() {
            return Err("edge with a missing node".into());
        }
        edges.push((a as u32, b as u32, k as u8, c as u8, f as u8, q as i32));
    }
    let n = section(&mut lines, "squares")?;
    let mut squares = Vec::with_capacity(n);
    for _ in 0..n {
        let [area] = fields::<1>(lines.next())?;
        squares.push(area);
    }
    let n = section(&mut lines, "places")?;
    let mut places = Vec::with_capacity(n);
    for _ in 0..n {
        let [k, node, w, a] = fields::<4>(lines.next())?;
        places.push((k as u8, node as u32, w, a));
    }
    let n = section(&mut lines, "spots")?;
    let mut spots = Vec::with_capacity(n);
    for _ in 0..n {
        let [k, node, a] = fields::<3>(lines.next())?;
        spots.push((k as u8, node as u32, a));
    }
    Ok(Raw {
        nodes,
        edges,
        squares,
        places,
        spots,
    })
}

/// What clearing the lanes did.
#[derive(Clone, Debug, Default)]
pub struct BakeStats {
    pub shells: usize,
    pub edges_in: usize,
    pub km_in: f64,
    /// Edges with a point closer than CLEAR_M to a wall, or inside a shell,
    /// before clearing (entrances and passages left out).
    pub edges_near_walls: usize,
    pub edges_inside_shells: usize,
    pub nodes_moved: usize,
    pub nodes_blocked: usize,
    /// Why lanes were dropped: an end that could not be cleared, a wall
    /// across the lane, walls on both sides, a shift over the limit, or a
    /// shift needed at an end.
    pub dropped_why: [usize; 5],
    pub edges_shifted: usize,
    pub edges_dropped: usize,
    pub km_dropped: f64,
    pub max_shift_m: f64,
    /// After clearing: lane points more than 0.1 m inside a shell, outside
    /// marked entrances and passages, sampled every 0.5 m.
    pub points_inside_after: usize,
    pub edges_entrance: usize,
    pub edges_passage: usize,
    pub places_dropped: usize,
    pub spots_dropped: usize,
    pub ms: f64,
}

fn mm(v: f64) -> i64 {
    (v * 1000.0).round() as i64
}

/// Clears the lanes and builds the function's input.
pub fn bake(raw: &Raw, walls: &Walls, ground: &Ground) -> (Lanes, BakeStats) {
    let start = Instant::now();
    let mut stats = BakeStats {
        shells: walls.shells,
        edges_in: raw.edges.len(),
        ..BakeStats::default()
    };
    let mut scratch = Vec::new();
    let height = |p: [f64; 2]| ground.height(p[0], p[1]);
    // Nodes that may sit in a shell: doors, ends of paths to doors,
    // passages and spots.
    let mut protect = vec![false; raw.nodes.len()];
    for &(a, b, k, _, f, _) in &raw.edges {
        if f & flag::PASSAGE != 0 {
            protect[a as usize] = true;
            protect[b as usize] = true;
        }
        if f & flag::ENTRANCE_A != 0 {
            protect[a as usize] = true;
        }
        if f & flag::ENTRANCE_B != 0 {
            protect[b as usize] = true;
        }
        if k == kind::ACCESS {
            // The far end (door or spot) of an access edge.
            protect[b as usize] = true;
        }
    }
    // Move nodes off walls.
    let mut pos: Vec<[f64; 2]> = raw.nodes.iter().map(|n| n.0).collect();
    let mut node_y: Vec<f64> = pos.iter().map(|&p| height(p).unwrap_or(0.0)).collect();
    let mut blocked = vec![false; pos.len()];
    for i in 0..pos.len() {
        if protect[i] {
            continue;
        }
        let start_p = pos[i];
        for _ in 0..4 {
            let h = node_y[i] + WALL_CUT_M;
            let Some((d, n)) = walls.signed_distance(pos[i][0], pos[i][1], h, 2.5, &mut scratch)
            else {
                break;
            };
            if d >= CLEAR_M {
                break;
            }
            let m = CLEAR_M - d + 0.01;
            pos[i] = [pos[i][0] + n[0] * m, pos[i][1] + n[1] * m];
        }
        let moved = ((pos[i][0] - start_p[0]).powi(2) + (pos[i][1] - start_p[1]).powi(2)).sqrt();
        let still = walls
            .signed_distance(
                pos[i][0],
                pos[i][1],
                node_y[i] + WALL_CUT_M,
                2.5,
                &mut scratch,
            )
            .is_some_and(|(d, _)| d < CLEAR_M - 0.05);
        if moved > MAX_SHIFT_M || still {
            blocked[i] = true;
            stats.nodes_blocked += 1;
            pos[i] = start_p;
        } else {
            if moved > 1e-6 {
                stats.nodes_moved += 1;
                stats.max_shift_m = stats.max_shift_m.max(moved);
            }
            node_y[i] = height(pos[i]).unwrap_or(node_y[i]);
        }
    }
    // Edges: shift sideways where needed, or drop.
    let mut keep: Vec<Option<(Vec<[f64; 3]>, bool)>> = Vec::with_capacity(raw.edges.len());
    for &(a, b, k, _, f, _) in &raw.edges {
        let (pa, pb) = (pos[a as usize], pos[b as usize]);
        let len = ((pb[0] - pa[0]).powi(2) + (pb[1] - pa[1]).powi(2)).sqrt();
        stats.km_in += len / 1000.0;
        if f & flag::PASSAGE != 0 {
            stats.edges_passage += 1;
        }
        if f & (flag::ENTRANCE_A | flag::ENTRANCE_B) != 0 {
            stats.edges_entrance += 1;
        }
        if len < 1e-3 {
            keep.push(Some((
                vec![
                    [pa[0], node_y[a as usize], pa[1]],
                    [pb[0], node_y[b as usize], pb[1]],
                ],
                false,
            )));
            continue;
        }
        if (blocked[a as usize] && !protect[a as usize])
            || (blocked[b as usize] && !protect[b as usize])
        {
            stats.dropped_why[0] += 1;
            stats.edges_dropped += 1;
            stats.km_dropped += len / 1000.0;
            keep.push(None);
            continue;
        }
        let d = [(pb[0] - pa[0]) / len, (pb[1] - pa[1]) / len];
        let right = [-d[1], d[0]];
        let point = |s: f64| [pa[0] + d[0] * s, pa[1] + d[1] * s];
        let free = k == kind::ACCESS || f & flag::PASSAGE != 0;
        let in_entrance = |s: f64| {
            (f & flag::ENTRANCE_A != 0 && s < pop::ENTRANCE_RANGE as f64 / 1000.0)
                || (f & flag::ENTRANCE_B != 0 && s > len - pop::ENTRANCE_RANGE as f64 / 1000.0)
        };
        let ya = node_y[a as usize];
        let yb = node_y[b as usize];
        let y_at = |s: f64| ya + (yb - ya) * s / len;
        // Constraints from samples every 0.5 m: shift right by at least
        // `lo`, or left by at least `hi` (negative).
        let n = (len / 0.5).ceil().max(1.0) as usize;
        let (mut lo, mut hi) = (Vec::new(), Vec::new());
        let (mut near, mut inside, mut infeasible) = (false, false, false);
        if !free {
            for j in 0..=n {
                let s = len * j as f64 / n as f64;
                if in_entrance(s) {
                    continue;
                }
                let p = point(s);
                let h = height(p).unwrap_or(y_at(s)) + WALL_CUT_M;
                let Some((dist, nrm)) = walls.signed_distance(p[0], p[1], h, 2.5, &mut scratch)
                else {
                    continue;
                };
                if dist >= CLEAR_M {
                    continue;
                }
                near = true;
                inside |= dist < -0.1;
                let along = nrm[0] * right[0] + nrm[1] * right[1];
                if along.abs() < 0.3 {
                    infeasible = true;
                    stats.dropped_why[1] += 1;
                    break;
                }
                let need = (CLEAR_M - dist + 0.02) / along;
                if need > 0.0 {
                    lo.push((s, need));
                } else {
                    hi.push((s, need));
                }
            }
        }
        stats.edges_near_walls += usize::from(near);
        stats.edges_inside_shells += usize::from(inside);
        // The shift along the lane: the tapered envelope of the needs.
        let shift = |s: f64| {
            let up = lo
                .iter()
                .map(|&(si, v)| v - (s - si).abs() * TAPER)
                .fold(0.0, f64::max);
            let down = hi
                .iter()
                .map(|&(si, v)| v + (s - si).abs() * TAPER)
                .fold(0.0, f64::min);
            (up, down)
        };
        let protected_end = |s: f64| in_entrance(s);
        let mut verts = Vec::new();
        let mut shifted = false;
        if !infeasible {
            let parts = (len
                / if lo.is_empty() && hi.is_empty() {
                    2.0
                } else {
                    1.0
                })
            .ceil()
            .max(1.0) as usize;
            for j in 0..=parts {
                let s = len * j as f64 / parts as f64;
                // The largest need within half a step either side.
                let (mut up, mut down) = (0.0f64, 0.0f64);
                for ds in [-0.5, 0.0, 0.5] {
                    let (u, w) = shift((s + ds).clamp(0.0, len));
                    up = up.max(u);
                    down = down.min(w);
                }
                if up > 0.0 && down < 0.0 {
                    infeasible = true;
                    stats.dropped_why[2] += 1;
                    break;
                }
                let v = if up > 0.0 { up } else { down };
                if v.abs() > MAX_SHIFT_M {
                    infeasible = true;
                    stats.dropped_why[3] += 1;
                    break;
                }
                // Ends stay at their nodes (cleared above).
                if (j == 0 || j == parts) && v.abs() > 0.1 && !protected_end(s) {
                    infeasible = true;
                    stats.dropped_why[4] += 1;
                    break;
                }
                let v = if j == 0 || j == parts { 0.0 } else { v };
                if v.abs() > 1e-6 {
                    shifted = true;
                    stats.max_shift_m = stats.max_shift_m.max(v.abs());
                }
                let p = point(s);
                let q = [p[0] + right[0] * v, p[1] + right[1] * v];
                let y = height(q).unwrap_or(y_at(s));
                verts.push([q[0], y, q[1]]);
            }
        }
        if infeasible {
            stats.edges_dropped += 1;
            stats.km_dropped += len / 1000.0;
            keep.push(None);
            continue;
        }
        stats.edges_shifted += usize::from(shifted);
        // Doors and spots: their far end keeps its own height.
        if let Some(last) = verts.last_mut()
            && k == kind::ACCESS
        {
            last[1] = node_y[b as usize];
        }
        keep.push(Some((verts, shifted)));
    }
    // Renumber the kept edges; places and spots follow them.
    let mut new_index = vec![-1i64; raw.edges.len()];
    let mut lanes = Lanes {
        nodes: pos
            .iter()
            .zip(&node_y)
            .map(|(p, &y)| [mm(p[0]), mm(y), mm(p[1])])
            .collect(),
        node_clear: pos
            .iter()
            .zip(&node_y)
            .map(|(p, &y)| {
                let max = pop::NODE_CLEAR_MAX as f64 / 1000.0;
                let d = walls
                    .signed_distance(p[0], p[1], y + WALL_CUT_M, max, &mut scratch)
                    .map_or(max, |(d, _)| d.clamp(0.0, max));
                mm(d)
            })
            .collect(),
        square_area_m2: raw.squares.clone(),
        ..Lanes::default()
    };
    for (i, (e, kept)) in raw.edges.iter().zip(&keep).enumerate() {
        let Some((verts, _)) = kept else { continue };
        let &(a, b, k, c, f, q) = e;
        new_index[i] = lanes.edges.len() as i64;
        let first = lanes.verts.len() as u32;
        // Room beside the lane every 0.5 m (walls can jut out between its
        // points), then at each point the smallest within 2 m either side.
        let mut s_at = Vec::with_capacity(verts.len());
        let mut s = 0.0;
        for (j, v) in verts.iter().enumerate() {
            if j > 0 {
                let u = verts[j - 1];
                s += ((v[0] - u[0]).powi(2) + (v[2] - u[2]).powi(2)).sqrt();
            }
            s_at.push(s);
        }
        let total = s;
        let cap = half_width(k);
        let mut samples: Vec<(f64, f64, f64)> = Vec::new();
        if k != kind::ACCESS {
            let n = (total / 0.5).ceil().max(1.0) as usize;
            for i in 0..=n {
                let si = total * i as f64 / n as f64;
                let j = s_at.partition_point(|&x| x <= si).clamp(1, verts.len() - 1);
                let (u, v) = (verts[j - 1], verts[j]);
                let seg = (s_at[j] - s_at[j - 1]).max(1e-9);
                let t = ((si - s_at[j - 1]) / seg).clamp(0.0, 1.0);
                let p = [
                    u[0] + (v[0] - u[0]) * t,
                    u[1] + (v[1] - u[1]) * t,
                    u[2] + (v[2] - u[2]) * t,
                ];
                let (dx, dz) = (v[0] - u[0], v[2] - u[2]);
                let l = (dx * dx + dz * dz).sqrt().max(1e-9);
                let right = [-dz / l, dx / l];
                let h = p[1] + WALL_CUT_M;
                samples.push((
                    si,
                    walls.ray(p[0], p[2], h, right, cap, &mut scratch),
                    walls.ray(p[0], p[2], h, [-right[0], -right[1]], cap, &mut scratch),
                ));
            }
        }
        for (j, v) in verts.iter().enumerate() {
            let sj = s_at[j];
            let (mut r, mut l) = (cap, cap);
            for p in &samples {
                if (p.0 - sj).abs() <= 2.0 {
                    r = r.min(p.1);
                    l = l.min(p.2);
                }
            }
            lanes.verts.push(Vert {
                x: mm(v[0]),
                y: mm(v[1]),
                z: mm(v[2]),
                s: mm(sj),
                right: mm(r),
                left: mm(l),
            });
        }
        let len = lanes.verts.last().map_or(0, |v| v.s).max(1);
        lanes.edges.push(Edge {
            a,
            b,
            kind: k,
            class: c,
            flags: f,
            square: q,
            len,
            first_vert: first,
            verts: lanes.verts.len() as u32 - first,
        });
    }
    // Places and spots whose lanes are gone are dropped.
    let walkable_at = |node: u32, lanes: &Lanes| {
        lanes
            .edges
            .iter()
            .any(|e| e.kind != kind::ACCESS && (e.a == node || e.b == node))
    };
    for &(k, node, w, access) in &raw.places {
        let access = if access >= 0 {
            match new_index[access as usize] {
                -1 => {
                    stats.places_dropped += 1;
                    continue;
                }
                i => Some(i as u32),
            }
        } else {
            None
        };
        if !walkable_at(node, &lanes) {
            stats.places_dropped += 1;
            continue;
        }
        lanes.places.push(Place {
            kind: k,
            node,
            weight: w,
            access,
        });
    }
    for &(k, node, access) in &raw.spots {
        let i = if access >= 0 {
            new_index[access as usize]
        } else {
            -1
        };
        if i < 0 {
            stats.spots_dropped += 1;
            continue;
        }
        let e = lanes.edges[i as usize];
        let lane = if e.a == node { e.b } else { e.a };
        if !walkable_at(lane, &lanes) {
            stats.spots_dropped += 1;
            continue;
        }
        lanes.spots.push(Spot {
            kind: k,
            node,
            access: i as u32,
        });
    }
    // Check: lane points still inside a shell (every 0.5 m), outside
    // entrances and passages.
    for e in &lanes.edges {
        if e.kind == kind::ACCESS || e.flags & flag::PASSAGE != 0 {
            continue;
        }
        let vs = &lanes.verts[e.first_vert as usize..(e.first_vert + e.verts) as usize];
        let steps = (e.len / 500).max(1);
        for j in 0..=steps {
            let s = e.len * j / steps;
            if (e.flags & flag::ENTRANCE_A != 0 && s < pop::ENTRANCE_RANGE)
                || (e.flags & flag::ENTRANCE_B != 0 && s > e.len - pop::ENTRANCE_RANGE)
            {
                continue;
            }
            let i = vs.partition_point(|v| v.s <= s).clamp(1, vs.len() - 1);
            let (u, v) = (vs[i - 1], vs[i]);
            let t = (s - u.s) as f64 / (v.s - u.s).max(1) as f64;
            let x = (u.x as f64 + (v.x - u.x) as f64 * t) / 1000.0;
            let z = (u.z as f64 + (v.z - u.z) as f64 * t) / 1000.0;
            let y = (u.y as f64 + (v.y - u.y) as f64 * t) / 1000.0;
            if walls
                .signed_distance(x, z, y + WALL_CUT_M, 1.0, &mut scratch)
                .is_some_and(|(d, _)| d < -0.1)
            {
                stats.points_inside_after += 1;
            }
        }
    }
    stats.ms = start.elapsed().as_secs_f64() * 1000.0;
    (lanes, stats)
}
