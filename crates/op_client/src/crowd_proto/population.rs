//! The crowd prototype's population function (ADR-0021, option C2).
//!
//! Who is where is a pure function of the lanes, places and population
//! profiles ([`World`]), the session seed, the crowd's clock (a date and
//! time plus ticks, [`crowd_time`]), the population's version
//! ([`VERSION`]) and the character catalogue's size ([`Inputs`]). Nothing
//! from the camera decides who exists: the camera only chooses which part
//! of the population is worked out ([`Crowd::people`] takes a centre and a
//! radius), and the cache behind it only keeps trips already worked out, so
//! the answer never depends on what was asked before (tested).
//!
//! - **Trips** start and end only at places (entrances of shops and public
//!   buildings, stops, squares, the world's edge). A place sends groups out
//!   in slots of [`SLOT`] ticks at a rate that follows the profiles; each
//!   group walks a seeded random walk over the lanes, weighted by each
//!   lane's density, for a seeded distance, then on to the nearest place
//!   ahead, where the trip ends.
//! - **Visits:** people walk to spots in squares and at stops, stand there
//!   for a while in a ring, and walk on to a place.
//! - **Groups:** more than half the people walk in groups of 2 to 4,
//!   abreast where the lane is wide enough and staggered where it is not.
//! - **Avoidance built in** ([`Avoid::Lanes`]): people keep to the right of
//!   a lane, slower walkers further out, and any two closer than
//!   [`PUSH_RANGE`] step aside from each other, worked out in closed form
//!   from where both are at that tick (no state).
//!
//! Written without Bevy and without floating point (integer millimetres,
//! ticks and per-mille factors, a counter-based hash for randomness), so
//! that it could move to `op_sim` as the ADR recommends. A test checks
//! that this file names no float type.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use op_kernel::StateHasher;

/// The population's version: screenshots and benchmark reports record it.
pub const VERSION: &str = "crowd-proto-0.1";

pub const TICKS_PER_SECOND: i64 = 60;
const TICKS_PER_HOUR: i64 = 216_000;
/// Trips leave a place in slots of this many ticks.
pub const SLOT: i64 = 120;
/// At most one visit per spot in each stay slot (ticks; 12 minutes).
const STAY_SLOT: i64 = 43_200;
/// A body's radius: how far a walker's centre keeps from a wall (mm).
pub const BODY: i64 = 300;
/// Distance between walkers abreast in a group (mm).
const SPACING: i64 = 700;
/// Node clearances are measured out to this (mm).
pub const NODE_CLEAR_MAX: i64 = 3_000;
/// Turns at a node are rounded off over this distance each side (mm).
const BLEND: i64 = 1_500;
/// The last metres before a door, where feet may enter the shell (mm).
pub const ENTRANCE_RANGE: i64 = 3_000;
/// Groups gather into their ring at a spot over this distance (mm).
const STAY_BLEND: i64 = 4_000;
/// Built-in avoidance: two people closer than this step aside (mm).
pub const PUSH_RANGE: i64 = 800;
/// ... by at most this much in all (mm).
const PUSH_MAX: i64 = 600;
/// Rounds of calibrating the walk weights, and trips sampled in each.
const CAL_ROUNDS: usize = 8;
const CAL_TRIPS: u64 = 8_000;
/// A place's trips follow the profiles of the lanes this close (mm).
const MIX_RADIUS: i64 = 150_000;
/// Walk budgets (mm): trips walk this far before heading for a place.
const TRIP_BUDGET: (i64, i64) = (150_000, 700_000);
const LEG_BUDGET: (i64, i64) = (60_000, 300_000);
/// Unit vectors are scaled to this length.
const UNIT: i64 = 1024;

/// Lane kinds, as the conversion writes them.
pub mod kind {
    pub const SIDEWALK: u8 = 1;
    pub const FOOTWAY: u8 = 2;
    pub const CROSSING: u8 = 3;
    pub const STEPS: u8 = 4;
    pub const PEDESTRIAN: u8 = 5;
    pub const LIVING: u8 = 6;
    pub const LANE: u8 = 7;
    pub const SQUARE: u8 = 9;
    pub const ACCESS: u8 = 10;
    pub const PLATFORM: u8 = 11;
}

/// Edge flags.
pub mod flag {
    pub const PASSAGE: u8 = 1;
    pub const ENTRANCE_A: u8 = 2;
    pub const ENTRANCE_B: u8 = 4;
    pub const ACCESS: u8 = 8;
}

/// Place kinds.
pub mod place_kind {
    pub const SHOP: u8 = 1;
    pub const PUBLIC: u8 = 2;
    pub const STOP: u8 = 3;
    pub const SQUARE: u8 = 4;
    pub const EDGE: u8 = 5;
}

/// Spot kinds.
pub mod spot_kind {
    pub const SQUARE: u8 = 1;
    pub const STOP: u8 = 2;
    pub const BENCH: u8 = 3;
}

/// Profile classes of lanes and places.
pub const CLASSES: usize = 3;
pub const CITY: u8 = 0;
pub const RIVER: u8 = 1;
pub const SQUARE_CLASS: u8 = 2;

// ---------------------------------------------------------------------------
// Inputs

/// A lane's point (region frame, mm): `s` from the edge's start, and the
/// free room to the right and left of the lane's line, seen from its start
/// towards its end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vert {
    pub x: i64,
    pub y: i64,
    pub z: i64,
    pub s: i64,
    pub right: i64,
    pub left: i64,
}

/// A lane between two nodes, drawn by `verts` from `a` to `b`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub a: u32,
    pub b: u32,
    pub kind: u8,
    pub class: u8,
    pub flags: u8,
    /// The square it crosses, or -1.
    pub square: i32,
    pub len: i64,
    pub first_vert: u32,
    pub verts: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub kind: u8,
    pub node: u32,
    /// Relative trip rate, per mille.
    pub weight: i64,
    /// The edge from the lane to the door, for entrances.
    pub access: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spot {
    pub kind: u8,
    /// Where people stand or sit.
    pub node: u32,
    /// The edge from a lane node to the spot.
    pub access: u32,
}

/// The navigation data: lanes, places and spots (region frame, mm).
#[derive(Clone, Debug, Default)]
pub struct Lanes {
    pub nodes: Vec<[i64; 3]>,
    /// How far each node is from the nearest wall (mm, up to 3 m): turns
    /// there keep people within it.
    pub node_clear: Vec<i64>,
    pub edges: Vec<Edge>,
    pub verts: Vec<Vert>,
    pub square_area_m2: Vec<i64>,
    pub places: Vec<Place>,
    pub spots: Vec<Spot>,
}

/// The population profiles.
#[derive(Clone, Debug)]
pub struct Profiles {
    /// People by class (city, river front, square), day type (weekday,
    /// Saturday, Sunday) and hour, per mille of the class's weekday peak.
    pub hourly: [[[i64; 24]; 3]; CLASSES],
    /// Month factor by class, per mille of the year's mean.
    pub month: [[i64; 12]; CLASSES],
    /// People per km of lane by kind at the reference (the class's weekday
    /// peak in an average month). Set by hand.
    pub per_km: [i64; 12],
    /// People per hectare of a square at the reference. Set by hand.
    pub square_per_ha: i64,
    /// Walking speed on each kind, per mille of the walker's own.
    pub kind_speed: [i64; 12],
    /// Share of spots taken at the reference, per mille, by spot kind.
    pub occupancy: [i64; 4],
}

/// Normalises counts to per mille of `peak`.
fn per_mille<const N: usize>(v: [i64; N], peak: i64) -> [i64; N] {
    v.map(|x| x * 1000 / peak)
}

impl Profiles {
    /// Zurich's old town. Hourly shapes:
    /// - city: Bahnhofstrasse (Mitte), mean people per hour in 2025,
    ///   weekdays, Saturdays and Sundays (hystreet's counts on the city's
    ///   portal);
    /// - river front: the Limmatquai counter, device values per hour from
    ///   January to November 2024 (the city's CC0 counts);
    /// - square: the mean of the two, by hand.
    ///
    /// Month factors: the same series' mean per day by month (Bahnhofstrasse
    /// weekdays in 2025; the Limmatquai in 2024, December estimated as the
    /// mean of November and January, the counter having stopped). Densities
    /// per lane kind and occupancies are set by hand.
    pub fn zurich() -> Profiles {
        let city_peak = 3_890;
        let city = [
            per_mille(
                [
                    112, 27, 13, 10, 22, 127, 360, 974, 1_422, 1_397, 1_820, 2_584, 3_488, 3_430,
                    3_196, 3_146, 3_303, 3_890, 3_356, 2_090, 1_330, 1_105, 874, 519,
                ],
                city_peak,
            ),
            per_mille(
                [
                    610, 328, 252, 152, 73, 88, 114, 209, 458, 1_138, 2_246, 3_465, 4_283, 5_031,
                    5_812, 6_461, 6_494, 5_626, 4_110, 2_850, 2_412, 2_275, 2_147, 1_620,
                ],
                city_peak,
            ),
            per_mille(
                [
                    944, 480, 317, 205, 147, 116, 108, 139, 287, 459, 842, 1_324, 1_577, 1_772,
                    2_056, 2_383, 2_397, 2_195, 1_618, 1_163, 828, 613, 424, 195,
                ],
                city_peak,
            ),
        ];
        let river_peak = 422;
        let river = [
            per_mille(
                [
                    12, 4, 3, 2, 2, 4, 16, 46, 90, 139, 218, 326, 404, 416, 422, 386, 368, 392,
                    339, 231, 163, 144, 116, 61,
                ],
                river_peak,
            ),
            per_mille(
                [
                    70, 30, 17, 9, 6, 9, 11, 17, 58, 129, 265, 448, 587, 680, 737, 728, 668, 568,
                    402, 303, 251, 237, 239, 168,
                ],
                river_peak,
            ),
            per_mille(
                [
                    92, 43, 24, 11, 8, 9, 8, 10, 29, 68, 139, 252, 349, 436, 496, 513, 432, 377,
                    266, 216, 157, 120, 71, 31,
                ],
                river_peak,
            ),
        ];
        let mut square = [[0; 24]; 3];
        for d in 0..3 {
            for h in 0..24 {
                square[d][h] = (city[d][h] + river[d][h]) / 2;
            }
        }
        let city_month = per_mille(
            [
                33_763, 32_907, 36_082, 38_851, 37_673, 38_046, 38_752, 39_853, 37_871, 38_836,
                41_071, 49_282,
            ],
            38_582,
        );
        let river_month = per_mille(
            [
                3_050, 3_898, 4_129, 5_373, 5_684, 6_013, 5_323, 5_805, 5_084, 3_282, 3_519, 3_285,
            ],
            4_537,
        );
        let mut square_month = [0; 12];
        for m in 0..12 {
            square_month[m] = (city_month[m] + river_month[m]) / 2;
        }
        let mut per_km = [0; 12];
        per_km[kind::SIDEWALK as usize] = 80;
        per_km[kind::FOOTWAY as usize] = 50;
        per_km[kind::CROSSING as usize] = 60;
        per_km[kind::STEPS as usize] = 30;
        per_km[kind::PEDESTRIAN as usize] = 350;
        per_km[kind::LIVING as usize] = 120;
        per_km[kind::LANE as usize] = 50;
        per_km[kind::PLATFORM as usize] = 100;
        let mut kind_speed = [1000; 12];
        kind_speed[kind::STEPS as usize] = 500;
        kind_speed[kind::CROSSING as usize] = 1_050;
        kind_speed[kind::ACCESS as usize] = 700;
        kind_speed[kind::SQUARE as usize] = 950;
        Profiles {
            hourly: [city, river, square],
            month: [city_month, river_month, square_month],
            per_km,
            square_per_ha: 200,
            kind_speed,
            occupancy: [0, 400, 500, 500],
        }
    }
}

/// How people avoid each other in the function itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Avoid {
    /// On the lane's line, groups centred: no avoidance.
    None,
    /// Keep right (slower walkers further out) and step aside in closed
    /// form from anyone within [`PUSH_RANGE`].
    Lanes,
}

/// Everything else the population depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inputs {
    pub seed: u64,
    pub avoid: Avoid,
    /// Looks in the character catalogue for each season.
    pub looks: u32,
}

/// The crowd's clock: a Swiss civil date and time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub minute: u32,
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The month (1 to 12) of a day since 1970-01-01.
fn month_of(days: i64) -> usize {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    (if mp < 10 { mp + 3 } else { mp - 9 }) as usize
}

/// 0 weekday, 1 Saturday, 2 Sunday. Holidays are not known.
fn day_type(days: i64) -> usize {
    match (days + 3).rem_euclid(7) {
        5 => 1,
        6 => 2,
        _ => 0,
    }
}

/// The season (0 winter, 1 spring, 2 summer, 3 autumn) of a month.
pub fn season(month: u32) -> u32 {
    match month {
        12 | 1 | 2 => 0,
        3..=5 => 1,
        6..=8 => 2,
        _ => 3,
    }
}

/// The crowd's time: ticks since 1970-01-01 00:00 Swiss civil time.
pub fn crowd_time(clock: &Clock, tick: u64) -> i64 {
    let days = days_from_civil(clock.year, clock.month, clock.day);
    (days * 1_440 + i64::from(clock.minute)) * 3_600 + tick as i64
}

// ---------------------------------------------------------------------------
// Randomness: a counter-based hash (SplitMix64's finaliser).

fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn hash(parts: &[u64]) -> u64 {
    let mut h = 0x5354_5245_4554_5749;
    for &p in parts {
        h = mix(h ^ p);
    }
    h
}

/// Uniform in `0..n` (n below 2^32).
fn below(h: u64, n: u64) -> u64 {
    ((h >> 32) * n) >> 32
}

/// Uniform in `lo..hi`.
fn between(h: u64, lo: i64, hi: i64) -> i64 {
    lo + below(h, (hi - lo) as u64) as i64
}

/// Roughly normal, as the sum of twelve uniforms (Irwin-Hall).
fn normal(h: u64, mean: i64, sd: i64) -> i64 {
    let mut sum = 0i64;
    let mut x = h;
    for i in 0..3 {
        x = mix(x ^ i);
        for k in 0..4 {
            sum += ((x >> (16 * k)) & 0xFFFF) as i64;
        }
    }
    mean + (sum - 393_210) * sd / 65_536
}

fn version_tag() -> u64 {
    VERSION.bytes().fold(0xCBF2_9CE4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01B3)
    })
}

const TAG_TRIP: u64 = 1;
const TAG_VISIT: u64 = 2;
const TAG_CALIBRATE: u64 = 3;

// ---------------------------------------------------------------------------
// The world, with what the function derives from it once.

/// A directed edge: `edge * 2`, plus 1 when walked from `b` to `a`.
fn directed(edge: u32, fwd: bool) -> usize {
    edge as usize * 2 + usize::from(!fwd)
}

/// Lanes, places and profiles, with tables derived from them.
pub struct World {
    pub lanes: Lanes,
    pub profiles: Profiles,
    /// Edges at each node.
    adj: Vec<Vec<u32>>,
    /// People per km at the reference, in thousandths, per edge.
    dens: Vec<i64>,
    /// The place at a node, or -1.
    place_at: Vec<i32>,
    /// The mix of profile classes among the people expected within
    /// [`MIX_RADIUS`] of each place (per mille): its trips follow that mix.
    place_mix: Vec<[i64; CLASSES]>,
    /// Trips per slot at the reference, in millionths, per place.
    rate: Vec<i64>,
    /// From the end of a directed edge to the nearest place ahead: the
    /// distance (mm) and the next directed edge (-1 when there).
    to_place: Vec<(i64, i64)>,
    /// The lane node of each spot, and its class.
    spot_lane: Vec<u32>,
    spot_class: Vec<u8>,
    /// Bounds (min x, min z, max x, max z) per edge.
    bbox: Vec<[i64; 4]>,
    /// The direction (length [`UNIT`]) of the segment that ends at each
    /// lane point (the first point of an edge has its first segment's).
    seg_dir: Vec<[i64; 2]>,
    /// The longest a trip or visit can last (ticks).
    pub window: i64,
    /// The longest a walk to or from a spot can last (ticks).
    leg_window: i64,
    /// Each lane's walk weight is multiplied by this (per mille), found by
    /// sampling trips so that each lane gets its share of the people.
    mult: Vec<i64>,
    /// People at the reference on each class of lane, and the share of
    /// trips that each class's factor drives (both in millionths of the
    /// whole), so that the expected total follows the profiles at any time.
    class_people: [i64; CLASSES],
    class_trips: [i64; CLASSES],
    /// The longest route (mm).
    pub max_route: i64,
}

fn walkable(e: &Edge) -> bool {
    e.kind != kind::ACCESS
}

impl World {
    pub fn new(lanes: Lanes, profiles: Profiles) -> World {
        let n = lanes.nodes.len();
        let mut adj = vec![Vec::new(); n];
        for (i, e) in lanes.edges.iter().enumerate() {
            adj[e.a as usize].push(i as u32);
            if e.b != e.a {
                adj[e.b as usize].push(i as u32);
            }
        }
        // Density per edge: by kind, or a square's people over its lanes.
        let mut square_len = vec![0i64; lanes.square_area_m2.len()];
        for e in &lanes.edges {
            if e.kind == kind::SQUARE && e.square >= 0 {
                square_len[e.square as usize] += e.len;
            }
        }
        let dens: Vec<i64> = lanes
            .edges
            .iter()
            .map(|e| {
                if e.kind == kind::ACCESS {
                    0
                } else if e.kind == kind::SQUARE && e.square >= 0 {
                    let q = e.square as usize;
                    // people = per_ha * area / 10_000; per km = people / (len / 1e6)
                    profiles.square_per_ha * lanes.square_area_m2[q] * 100_000
                        / square_len[q].max(1)
                } else {
                    profiles.per_km[e.kind as usize] * 1000
                }
            })
            .collect();
        let mut place_at = vec![-1i32; n];
        for (i, p) in lanes.places.iter().enumerate() {
            if place_at[p.node as usize] < 0 {
                place_at[p.node as usize] = i as i32;
            }
        }
        let place_mix = lanes
            .places
            .iter()
            .map(|p| {
                let c = lanes.nodes[p.node as usize];
                let mut sum = [0i64; CLASSES];
                for (e, &d) in lanes.edges.iter().zip(&dens) {
                    let (a, b) = (lanes.nodes[e.a as usize], lanes.nodes[e.b as usize]);
                    let (mx, mz) = ((a[0] + b[0]) / 2 - c[0], (a[2] + b[2]) / 2 - c[2]);
                    if mx * mx + mz * mz <= MIX_RADIUS * MIX_RADIUS {
                        sum[e.class as usize] += d * e.len / 1_000_000;
                    }
                }
                let total: i64 = sum.iter().sum();
                if total <= 0 {
                    [1000, 0, 0]
                } else {
                    sum.map(|v| v * 1000 / total)
                }
            })
            .collect();
        let spot_lane: Vec<u32> = lanes
            .spots
            .iter()
            .map(|s| {
                let e = &lanes.edges[s.access as usize];
                if e.a == s.node { e.b } else { e.a }
            })
            .collect();
        let spot_class = lanes
            .spots
            .iter()
            .map(|s| lanes.edges[s.access as usize].class)
            .collect();
        let bbox = lanes
            .edges
            .iter()
            .map(|e| {
                let vs = &lanes.verts[e.first_vert as usize..(e.first_vert + e.verts) as usize];
                let mut b = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
                for v in vs {
                    b = [b[0].min(v.x), b[1].min(v.z), b[2].max(v.x), b[3].max(v.z)];
                }
                b
            })
            .collect();
        let mut seg_dir = vec![[UNIT, 0]; lanes.verts.len()];
        for e in &lanes.edges {
            let f = e.first_vert as usize;
            for j in 1..e.verts as usize {
                let (u, v) = (lanes.verts[f + j - 1], lanes.verts[f + j]);
                let (dx, dz) = (v.x - u.x, v.z - u.z);
                let l = (dx * dx + dz * dz).isqrt().max(1);
                seg_dir[f + j] = [dx * UNIT / l, dz * UNIT / l];
            }
            if e.verts > 1 {
                seg_dir[f] = seg_dir[f + 1];
            }
        }
        let mut world = World {
            seg_dir,
            lanes,
            profiles,
            adj,
            dens,
            place_at,
            place_mix,
            rate: Vec::new(),
            to_place: Vec::new(),
            spot_lane,
            spot_class,
            bbox,
            window: 0,
            leg_window: 0,
            max_route: 0,
            mult: Vec::new(),
            class_people: [0; CLASSES],
            class_trips: [0; CLASSES],
        };
        world.to_place = world.nearest_places();
        world.calibrate();
        world
    }

    /// Edges a walk may take from `node`, arriving by `came`.
    fn options(&self, node: u32, came: Option<u32>) -> impl Iterator<Item = u32> + '_ {
        let all = &self.adj[node as usize];
        let others = all
            .iter()
            .filter(|&&e| walkable(&self.lanes.edges[e as usize]) && Some(e) != came)
            .count();
        all.iter().copied().filter(move |&e| {
            walkable(&self.lanes.edges[e as usize]) && (Some(e) != came || others == 0)
        })
    }

    /// The node a directed edge arrives at.
    fn head(&self, d: usize) -> u32 {
        let e = &self.lanes.edges[d / 2];
        if d.is_multiple_of(2) { e.b } else { e.a }
    }

    /// For each directed edge, the distance from its end to the nearest
    /// place ahead without turning back (except at dead ends), and the next
    /// directed edge on the way: Dijkstra backwards from the places.
    fn nearest_places(&self) -> Vec<(i64, i64)> {
        let m = self.lanes.edges.len() * 2;
        let mut best = vec![(i64::MAX, -1i64); m];
        let mut heap = BinaryHeap::new();
        for d in 0..m {
            let e = &self.lanes.edges[d / 2];
            if walkable(e) && self.place_at[self.head(d) as usize] >= 0 {
                best[d] = (0, -1);
                heap.push(Reverse((0i64, d)));
            }
        }
        while let Some(Reverse((dist, d2))) = heap.pop() {
            if dist > best[d2].0 {
                continue;
            }
            // d2 leaves node `from`; any walkable edge arriving there may
            // continue with it.
            let e2 = d2 / 2;
            let from = if d2 % 2 == 0 {
                self.lanes.edges[e2].a
            } else {
                self.lanes.edges[e2].b
            };
            let len2 = self.lanes.edges[e2].len;
            for &e1 in &self.adj[from as usize] {
                let edge1 = &self.lanes.edges[e1 as usize];
                if !walkable(edge1) {
                    continue;
                }
                let d1 = directed(e1, edge1.b == from);
                if self.head(d1) != from {
                    continue;
                }
                // Turning straight back is allowed only at a dead end.
                if e1 as usize == e2 && self.options(from, Some(e1)).any(|o| o != e1) {
                    continue;
                }
                let cand = dist + len2;
                if self.place_at[from as usize] < 0 && cand < best[d1].0 {
                    best[d1] = (cand, d2 as i64);
                    heap.push(Reverse((cand, d1)));
                }
            }
        }
        best
    }

    /// Walk weights at a time: density times speed times the class factor.
    fn weight(&self, edge: u32, f: &[i64; CLASSES]) -> i64 {
        let e = &self.lanes.edges[edge as usize];
        self.dens[edge as usize] * self.profiles.kind_speed[e.kind as usize] / 1000
            * self.mult[edge as usize]
            / 1000
            * f[e.class as usize]
            / 1000
    }

    /// The profile factor of a class at a time, per mille of the reference:
    /// the hourly shape for the day type, interpolated between the middles
    /// of the hours, times the month factor.
    pub fn factor(&self, class: u8, t: i64) -> i64 {
        let x = t - TICKS_PER_HOUR / 2;
        let hour = x.div_euclid(TICKS_PER_HOUR);
        let frac = x.rem_euclid(TICKS_PER_HOUR);
        let v0 = self.hour_value(class, hour);
        let v1 = self.hour_value(class, hour + 1);
        v0 + (v1 - v0) * frac / TICKS_PER_HOUR
    }

    fn hour_value(&self, class: u8, abs_hour: i64) -> i64 {
        let day = abs_hour.div_euclid(24);
        let h = abs_hour.rem_euclid(24) as usize;
        let c = class as usize;
        self.profiles.hourly[c][day_type(day)][h] * self.profiles.month[c][month_of(day) - 1] / 1000
    }

    /// The factor (per mille) that brings the expected total of people to
    /// the profiles' total at a time: places send trips by the classes
    /// around them, the profiles count people by the classes of lanes.
    fn balance(&self, f: &[i64; CLASSES]) -> i64 {
        let want: i128 = (0..CLASSES)
            .map(|c| i128::from(self.class_people[c]) * i128::from(f[c]))
            .sum();
        let got: i128 = (0..CLASSES)
            .map(|c| i128::from(self.class_trips[c]) * i128::from(f[c]))
            .sum();
        if got <= 0 {
            1000
        } else {
            (want * 1000 / got) as i64
        }
    }

    fn factors(&self, t: i64) -> [i64; CLASSES] {
        [CITY, RIVER, SQUARE_CLASS].map(|c| self.factor(c, t))
    }

    /// A seeded walk from `start` (never on access edges): weighted by the
    /// lanes' densities until `budget` is walked, then on to the nearest
    /// place ahead. Returns the legs and the place where it ends.
    fn walk(
        &self,
        start: u32,
        budget: i64,
        key: u64,
        f: &[i64; CLASSES],
    ) -> (Vec<(u32, bool)>, i32) {
        let mut legs: Vec<(u32, bool)> = Vec::new();
        let mut node = start;
        let mut came: Option<u32> = None;
        let mut walked = 0i64;
        for step in 0..4_000u64 {
            if walked >= budget && self.place_at[node as usize] >= 0 && !legs.is_empty() {
                return (legs, self.place_at[node as usize]);
            }
            let next = if walked >= budget {
                match legs.last() {
                    Some(&(e, fwd)) => {
                        let (_, nd) = self.to_place[directed(e, fwd)];
                        if nd < 0 { None } else { Some(nd as usize / 2) }
                    }
                    None => self
                        .options(node, None)
                        .min_by_key(|&e| {
                            let fwd = self.lanes.edges[e as usize].a == node;
                            (self.to_place[directed(e, fwd)].0, e)
                        })
                        .map(|e| e as usize),
                }
            } else {
                let total: i64 = self.options(node, came).map(|e| self.weight(e, f)).sum();
                let h = hash(&[key, step]);
                if total <= 0 {
                    self.options(node, came).next().map(|e| e as usize)
                } else {
                    let mut r = below(h, total as u64) as i64;
                    let mut pick = None;
                    for e in self.options(node, came) {
                        let w = self.weight(e, f);
                        if r < w {
                            pick = Some(e as usize);
                            break;
                        }
                        r -= w;
                    }
                    pick
                }
            };
            let Some(e) = next else {
                break;
            };
            let edge = &self.lanes.edges[e];
            let fwd = edge.a == node;
            legs.push((e as u32, fwd));
            walked += edge.len;
            node = if fwd { edge.b } else { edge.a };
            came = Some(e as u32);
        }
        // No place reachable ahead: end where the walk stands (the
        // conversion keeps only parts of the graph with places, so this
        // does not happen in practice).
        let at = self.place_at[node as usize];
        (legs, at)
    }

    /// Estimates people times ticks per trip at the reference by sampling,
    /// then sets each place's rate so the whole population matches the
    /// profiles' density.
    fn calibrate(&mut self) {
        let f = [1000; CLASSES];
        let m = self.lanes.edges.len();
        self.mult = vec![1000; m];
        let target: Vec<i128> = self
            .lanes
            .edges
            .iter()
            .zip(&self.dens)
            .map(|(e, &d)| {
                if walkable(e) {
                    i128::from(d) * i128::from(e.len)
                } else {
                    0
                }
            })
            .collect();
        let total_target: i128 = target.iter().sum();
        let people_ref = total_target / 1_000_000_000;
        let inputs = Inputs {
            seed: 0,
            avoid: Avoid::Lanes,
            looks: 1,
        };
        let (mut per_trip, mut longest) = (1i128, 0i64);
        let mut weights = Vec::new();
        for round in 0..=CAL_ROUNDS {
            // Places send trips in proportion to the walk weights around
            // them.
            weights = self
                .lanes
                .places
                .iter()
                .map(|p| {
                    let w: i64 = self.options(p.node, None).map(|e| self.weight(e, &f)).sum();
                    p.weight * w / 1000
                })
                .collect();
            let total_w: i64 = weights.iter().sum();
            if total_w <= 0 {
                break;
            }
            let mut cumulative = Vec::with_capacity(weights.len());
            let mut acc = 0i64;
            for w in &weights {
                acc += w;
                cumulative.push(acc);
            }
            let mut occ = vec![0i128; m];
            let mut sum = 0i128;
            for i in 0..CAL_TRIPS {
                let h = hash(&[TAG_CALIBRATE, round as u64, i]);
                let r = below(h, total_w as u64) as i64;
                let p = cumulative.partition_point(|&c| c <= r);
                let trip = self.trip(
                    &inputs,
                    p as u32,
                    hash(&[TAG_CALIBRATE, round as u64, i, 1]),
                    0,
                    &f,
                );
                for g in trip.segs.iter().filter(|g| !g.stay) {
                    occ[g.edge as usize] += i128::from(trip.size) * i128::from(g.dur);
                }
                sum += i128::from(trip.size) * i128::from(trip.end_mt / 1000);
                longest = longest.max(trip.end_mt / 1000);
            }
            per_trip = (sum / i128::from(CAL_TRIPS)).max(1);
            if round == CAL_ROUNDS {
                break;
            }
            // Each lane's weight moves halfway (geometrically) towards
            // what would give it its share of the people.
            let total_occ: i128 = occ
                .iter()
                .zip(&target)
                .filter(|(_, t)| **t > 0)
                .map(|(o, _)| *o)
                .sum();
            for e in 0..m {
                if target[e] <= 0 {
                    continue;
                }
                let ratio = if occ[e] == 0 {
                    4_000
                } else {
                    (target[e] * total_occ * 1000 / (occ[e] * total_target).max(1))
                        .clamp(1, 100_000) as i64
                };
                let step = (ratio * 1000).isqrt();
                self.mult[e] = (self.mult[e] * step / 1000).clamp(20, 50_000);
            }
        }
        let total_w: i64 = weights.iter().sum();
        // How the expected total depends on the class factors.
        let mut by_class = [0i128; CLASSES];
        for (e, t) in self.lanes.edges.iter().zip(&target) {
            by_class[e.class as usize] += *t;
        }
        self.class_people = by_class.map(|v| (v * 1_000_000 / total_target.max(1)) as i64);
        let mut trips = [0i128; CLASSES];
        for (w, mix) in weights.iter().zip(&self.place_mix) {
            for c in 0..CLASSES {
                trips[c] += i128::from(*w) * i128::from(mix[c]);
            }
        }
        let all: i128 = trips.iter().sum();
        self.class_trips = trips.map(|v| (v * 1_000_000 / all.max(1)) as i64);
        // Trips per slot over all places = people / (people ticks per trip)
        // * SLOT; in millionths, shared by weight.
        self.rate = weights
            .iter()
            .map(|&w| {
                if total_w <= 0 {
                    0
                } else {
                    (people_ref * i128::from(SLOT) * 1_000_000 * i128::from(w)
                        / (per_trip * i128::from(total_w))) as i64
                }
            })
            .collect();
        // The longest walk: the largest budget, then the farthest place
        // ahead, at the slowest pace (700 mm/s on steps at half speed).
        let far = self
            .to_place
            .iter()
            .map(|&(d, _)| d)
            .filter(|&d| d < i64::MAX)
            .max()
            .unwrap_or(0);
        let slowest = 700 * self.profiles.kind_speed[kind::STEPS as usize] / 1000;
        let ticks = |len: i64| len * TICKS_PER_SECOND / slowest.max(1) + 600;
        self.max_route = TRIP_BUDGET.1 + far + 100_000;
        self.window = ticks(self.max_route).max(longest);
        self.leg_window = ticks(LEG_BUDGET.1 + far + 50_000);
    }

    /// Whether an edge's bounds come within `r` of `c`.
    fn edge_near(&self, e: u32, c: [i64; 2], r: i64) -> bool {
        let b = self.bbox[e as usize];
        let dx = (b[0] - c[0]).max(c[0] - b[2]).max(0);
        let dz = (b[1] - c[1]).max(c[1] - b[3]).max(0);
        dx * dx + dz * dz <= r * r
    }

    fn node_xz(&self, n: u32) -> [i64; 2] {
        let p = self.lanes.nodes[n as usize];
        [p[0], p[2]]
    }

    /// The target by lane kind (index 0: spots), in thousandths.
    pub fn target_by_kind(&self, t: i64, c: [i64; 2], r: i64) -> [i64; 12] {
        let f = self.factors(t);
        let mut out = [0i64; 12];
        for (i, e) in self.lanes.edges.iter().enumerate() {
            if !walkable(e) || !self.edge_near(i as u32, c, r) {
                continue;
            }
            let steps = (e.len / 500).max(1);
            let mut inside = 0i64;
            for k in 0..steps {
                let s = e.len * (2 * k + 1) / (2 * steps);
                let p = self.edge_point(i as u32, true, s);
                let (dx, dz) = (p.pos[0] - c[0], p.pos[2] - c[1]);
                if dx * dx + dz * dz <= r * r {
                    inside += 1;
                }
            }
            let len_in = e.len * inside / steps;
            out[e.kind as usize] +=
                (i128::from(self.dens[i]) * i128::from(len_in) * i128::from(f[e.class as usize])
                    / 1_000_000_000) as i64;
        }
        out[0] = self.target(t, c, r) - out.iter().sum::<i64>();
        out
    }

    /// The profiles' target: people expected within `r` of `c` at `t`
    /// (thousandths): lanes by length inside times density, and spots by
    /// their occupancy times the mean party.
    pub fn target(&self, t: i64, c: [i64; 2], r: i64) -> i64 {
        let f = self.factors(t);
        let mut milli = 0i128;
        for (i, e) in self.lanes.edges.iter().enumerate() {
            if !walkable(e) || !self.edge_near(i as u32, c, r) {
                continue;
            }
            // Length inside, sampled every 0.5 m.
            let steps = (e.len / 500).max(1);
            let mut inside = 0i64;
            for k in 0..steps {
                let s = e.len * (2 * k + 1) / (2 * steps);
                let p = self.edge_point(i as u32, true, s);
                let (dx, dz) = (p.pos[0] - c[0], p.pos[2] - c[1]);
                if dx * dx + dz * dz <= r * r {
                    inside += 1;
                }
            }
            let len_in = e.len * inside / steps;
            milli +=
                i128::from(self.dens[i]) * i128::from(len_in) * i128::from(f[e.class as usize])
                    / 1_000_000_000;
        }
        for (i, s) in self.lanes.spots.iter().enumerate() {
            let p = self.node_xz(s.node);
            let (dx, dz) = (p[0] - c[0], p[1] - c[1]);
            if dx * dx + dz * dz > r * r {
                continue;
            }
            let occ = self.occupancy(i, t);
            milli += i128::from(occ * party_mean_milli(s.kind) / 1000);
        }
        milli as i64
    }

    /// Share of a spot taken at a time (per mille).
    fn occupancy(&self, spot: usize, t: i64) -> i64 {
        let s = &self.lanes.spots[spot];
        let base = self.profiles.occupancy[s.kind as usize & 3];
        (base * self.factor(self.spot_class[spot], t) / 1000).min(950)
    }

    // -----------------------------------------------------------------------
    // Points on lanes.

    /// The point at `s` (mm) along an edge walked forwards or backwards,
    /// extended straight beyond its ends.
    fn edge_point(&self, edge: u32, fwd: bool, s: i64) -> EdgePoint {
        let e = &self.lanes.edges[edge as usize];
        let vs = &self.lanes.verts[e.first_vert as usize..(e.first_vert + e.verts) as usize];
        let se = if fwd { s } else { e.len - s };
        // The segment holding se (the first or last one beyond the ends).
        let j = vs.partition_point(|v| v.s <= se).clamp(1, vs.len() - 1);
        let (v0, v1) = (vs[j - 1], vs[j]);
        let ds = (v1.s - v0.s).max(1);
        let t = se - v0.s;
        let lerp = |a: i64, b: i64| a + (b - a) * t / ds;
        let pos = [lerp(v0.x, v1.x), lerp(v0.y, v1.y), lerp(v0.z, v1.z)];
        let mut dir = self.seg_dir[e.first_vert as usize + j];
        let tc = t.clamp(0, ds);
        let mut right = v0.right + (v1.right - v0.right) * tc / ds;
        let mut left = v0.left + (v1.left - v0.left) * tc / ds;
        if !fwd {
            dir = [-dir[0], -dir[1]];
            std::mem::swap(&mut right, &mut left);
        }
        EdgePoint {
            pos,
            dir,
            right,
            left,
        }
    }
}

/// Each spot's stay slots start at its own phase, so that spots are not
/// all free at once.
fn stay_phase(spot: u32) -> i64 {
    below(hash(&[TAG_VISIT, u64::from(spot)]), STAY_SLOT as u64) as i64
}

/// Mean party at a spot, in thousandths.
fn party_mean_milli(kind: u8) -> i64 {
    match kind {
        spot_kind::SQUARE => 2_150,
        spot_kind::STOP => 1_300,
        _ => 1_500,
    }
}

#[derive(Clone, Copy, Debug)]
struct EdgePoint {
    pos: [i64; 3],
    /// Walking direction, length [`UNIT`].
    dir: [i64; 2],
    /// Room to the walker's right and left (mm).
    right: i64,
    left: i64,
}

/// The walker's right of a direction (x east, z south).
fn right_of(dir: [i64; 2]) -> [i64; 2] {
    [-dir[1], dir[0]]
}

/// A point offset sideways by `lat` (to the walker's right), kept clear of
/// walls by a body's radius, and scaled by the facing's length so that it
/// shrinks in turns; returns the point and the offset used.
fn offset(p: &EdgePoint, lat: i64) -> ([i64; 3], i64) {
    let l = clamp_lateral(lat, p.right, p.left);
    let r = right_of(p.dir);
    (
        [
            p.pos[0] + r[0] * l / UNIT,
            p.pos[1],
            p.pos[2] + r[1] * l / UNIT,
        ],
        l,
    )
}

/// A lateral offset kept clear of walls by a body's radius.
fn clamp_lateral(lat: i64, right: i64, left: i64) -> i64 {
    let (lo, hi) = (-(left - BODY), right - BODY);
    if lo > hi {
        (hi + lo) / 2
    } else {
        lat.clamp(lo, hi)
    }
}

// ---------------------------------------------------------------------------
// Trips.

/// One stretch of a trip: walking an edge, or staying at a spot.
#[derive(Clone, Copy, Debug)]
struct Seg {
    edge: u32,
    fwd: bool,
    /// Staying at a spot (`edge` is then the spot's index).
    stay: bool,
    /// Route distance at the start (mm) and its length.
    s0: i64,
    len: i64,
    /// Time from the trip's start (thousandths of a tick) and duration.
    t0: i64,
    dur: i64,
}

#[derive(Clone, Copy, Debug, Default)]
struct Member {
    look: u32,
    height: i64,
    phase0: i64,
    stride: i64,
}

/// A group's trip, or a visit to a spot.
#[derive(Clone, Debug)]
pub struct Trip {
    pub id: u64,
    /// When the first member leaves (crowd ticks).
    pub start: i64,
    /// When the last member has arrived (crowd ticks).
    pub end: i64,
    segs: Vec<Seg>,
    total: i64,
    /// When the leader arrives (thousandths of a tick from the start).
    end_mt: i64,
    pub size: u8,
    members: [Member; 4],
    /// The group's speed (mm/s).
    pub speed: i64,
    /// The spot and kind of a visit.
    visit: Option<(u32, u8, u64)>,
    /// When the trip passes near the last query centre (crowd ticks).
    near: (i64, i64),
    /// Origin place (or spot) and end place, for checks.
    pub origin: i64,
    pub end_place: i32,
}

/// Where a person is and what they do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pose {
    /// Walking, at this phase of the step cycle (0 to 65535).
    Walk(u16),
    Stand,
    Sit,
}

/// A person worked out by the function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Person {
    /// The trip and the member: the same person on every tick of a trip.
    pub trip: u64,
    pub member: u8,
    /// Feet (region frame, mm).
    pub pos: [i64; 3],
    /// Facing, length [`UNIT`] (zero if unknown).
    pub dir: [i64; 2],
    pub pose: Pose,
    pub look: u32,
    pub height: i64,
    /// Where the feet may be inside a shell: an entrance or a passage.
    pub at_entrance: bool,
    /// The walker's right (length [`UNIT`]) and how far the person may
    /// move along it from here without nearing a wall (mm); zero for people
    /// who stand or sit.
    pub right: [i64; 2],
    pub lateral: (i64, i64),
    /// When the trip started and ends (crowd ticks).
    pub trip_start: i64,
    pub trip_end: i64,
    /// The kind of lane the person is on (0 at a spot), for reports.
    pub lane_kind: u8,
}

impl Person {
    pub fn key(&self) -> (u64, u8) {
        (self.trip, self.member)
    }
}

/// Twelve directions 30 degrees apart (length [`UNIT`]).
const RING: [[i64; 2]; 12] = [
    [1024, 0],
    [887, 512],
    [512, 887],
    [0, 1024],
    [-512, 887],
    [-887, 512],
    [-1024, 0],
    [-887, -512],
    [-512, -887],
    [0, -1024],
    [512, -887],
    [887, -512],
];

impl World {
    fn members(&self, inputs: &Inputs, key: u64, size: u8) -> [Member; 4] {
        let mut out = [Member::default(); 4];
        for (i, m) in out.iter_mut().enumerate().take(size as usize) {
            let h = hash(&[key, 100 + i as u64]);
            let height = normal(mix(h ^ 1), 1_715, 85).clamp(1_500, 1_980);
            *m = Member {
                look: below(mix(h ^ 2), u64::from(inputs.looks.max(1))) as u32,
                height,
                phase0: below(mix(h ^ 3), 65_536) as i64,
                // A step of 0.69 m for a 1.75 m person (the figures' legs).
                stride: 1_380 * height / 1_750,
            };
        }
        out
    }

    /// The trip of a group leaving place `place` at `t0` (crowd ticks).
    fn trip(&self, inputs: &Inputs, place: u32, key: u64, t0: i64, f: &[i64; CLASSES]) -> Trip {
        let p = self.lanes.places[place as usize];
        let size = match below(hash(&[key, 1]), 1000) {
            0..600 => 1,
            600..880 => 2,
            880..960 => 3,
            _ => 4,
        };
        let budget = between(hash(&[key, 2]), TRIP_BUDGET.0, TRIP_BUDGET.1);
        let mut legs = Vec::new();
        if let Some(a) = p.access {
            let e = &self.lanes.edges[a as usize];
            // From the door to the lane.
            legs.push((a, e.b == p.node));
        }
        let (walk, end) = self.walk(p.node, budget, hash(&[key, 3]), f);
        legs.extend(walk);
        if end >= 0
            && let Some(a) = self.lanes.places[end as usize].access
        {
            let e = &self.lanes.edges[a as usize];
            let lane = self.lanes.places[end as usize].node;
            legs.push((a, e.a == lane));
        }
        // Shopping and leisure are slower, a tram to catch faster.
        let purpose = |kind: u8| match kind {
            place_kind::SHOP | place_kind::SQUARE => 900,
            place_kind::PUBLIC => 950,
            place_kind::STOP => 1_050,
            place_kind::EDGE => 1_000,
            _ => 1_000,
        };
        let members = self.members(inputs, key, size);
        let speed = self.group_speed(key, size, purpose(p.kind));
        let mut trip = Trip {
            id: key,
            start: t0,
            end: t0,
            segs: Vec::new(),
            total: 0,
            end_mt: 0,
            size,
            members,
            speed,
            visit: None,
            near: (0, -1),
            origin: i64::from(place),
            end_place: end,
        };
        self.timeline(&mut trip, &legs, None);
        trip
    }

    /// A group walks at its members' mean speed (Weidmann's 1.34 m/s, sd
    /// 0.26), 0.06 m/s slower for each extra member, and slower or faster
    /// by the trip's purpose.
    fn group_speed(&self, key: u64, size: u8, purpose: i64) -> i64 {
        let mut v = 0;
        for i in 0..size {
            v += normal(hash(&[key, 200 + u64::from(i)]), 1_340, 260).clamp(800, 1_900);
        }
        let v = v / i64::from(size.max(1));
        ((v - 60 * (i64::from(size) - 1)) * purpose / 1000).max(700)
    }

    /// Lays the legs out in distance and time; `stay` inserts a stay of
    /// `ticks` at the spot after leg `at`.
    fn timeline(&self, trip: &mut Trip, legs: &[(u32, bool)], stay: Option<(usize, u32, i64)>) {
        let (mut s, mut t) = (0i64, 0i64);
        trip.segs.clear();
        for (i, &(e, fwd)) in legs.iter().enumerate() {
            let edge = &self.lanes.edges[e as usize];
            let v = trip.speed * self.profiles.kind_speed[edge.kind as usize] / 1000;
            let dur = (edge.len * 60_000 / v.max(1)).max(1);
            trip.segs.push(Seg {
                edge: e,
                fwd,
                stay: false,
                s0: s,
                len: edge.len,
                t0: t,
                dur,
            });
            s += edge.len;
            t += dur;
            if let Some((at, spot, ticks)) = stay
                && at == i
            {
                trip.segs.push(Seg {
                    edge: spot,
                    fwd: true,
                    stay: true,
                    s0: s,
                    len: 0,
                    t0: t,
                    dur: ticks * 1000,
                });
                t += ticks * 1000;
            }
        }
        trip.total = s;
        trip.end_mt = t;
        // The last member trails by up to three spacings.
        let tail = trip
            .segs
            .iter()
            .rev()
            .find(|g| !g.stay)
            .map_or(0, |g| 3 * SPACING * g.dur / g.len.max(1));
        trip.end = trip.start + (t + tail) / 1000 + 2;
    }

    /// A visit to spot `spot` staying from `t_stay` for `ticks`.
    fn visit(
        &self,
        inputs: &Inputs,
        spot: u32,
        key: u64,
        t_stay: i64,
        ticks: i64,
        f: &[i64; CLASSES],
    ) -> Trip {
        let sp = self.lanes.spots[spot as usize];
        let lane = self.spot_lane[spot as usize];
        let size = match sp.kind {
            spot_kind::SQUARE => match below(hash(&[key, 1]), 100) {
                0..25 => 1,
                25..70 => 2,
                70..90 => 3,
                _ => 4,
            },
            _ => 1 + u8::from(below(hash(&[key, 1]), 100) < 30),
        };
        let (walk_in, start) = self.walk(
            lane,
            between(hash(&[key, 2]), LEG_BUDGET.0, LEG_BUDGET.1),
            hash(&[key, 3]),
            f,
        );
        let (walk_out, end) = self.walk(
            lane,
            between(hash(&[key, 4]), LEG_BUDGET.0, LEG_BUDGET.1),
            hash(&[key, 5]),
            f,
        );
        let mut legs = Vec::new();
        if start >= 0
            && let Some(a) = self.lanes.places[start as usize].access
        {
            let e = &self.lanes.edges[a as usize];
            legs.push((a, e.b == self.lanes.places[start as usize].node));
        }
        legs.extend(walk_in.iter().rev().map(|&(e, fwd)| (e, !fwd)));
        let access = &self.lanes.edges[sp.access as usize];
        legs.push((sp.access, access.a == lane));
        let at = legs.len() - 1;
        legs.push((sp.access, access.a != lane));
        legs.extend(walk_out);
        if end >= 0
            && let Some(a) = self.lanes.places[end as usize].access
        {
            let e = &self.lanes.edges[a as usize];
            legs.push((a, e.a == self.lanes.places[end as usize].node));
        }
        let members = self.members(inputs, key, size);
        let speed = self.group_speed(key, size, 900);
        let mut trip = Trip {
            id: key,
            start: 0,
            end: 0,
            segs: Vec::new(),
            total: 0,
            end_mt: 0,
            size,
            members,
            speed,
            visit: Some((spot, sp.kind, hash(&[key, 6]))),
            near: (0, -1),
            origin: -1 - i64::from(spot),
            end_place: end,
        };
        self.timeline(&mut trip, &legs, Some((at, spot, ticks)));
        // Leave so as to arrive when the stay starts.
        let arrive = trip.segs.iter().find(|g| g.stay).map_or(0, |g| g.t0) / 1000;
        trip.start = t_stay - arrive;
        trip.end += trip.start;
        trip
    }

    /// The groups leaving place `p` in slot `slot`.
    fn slot_trips(&self, inputs: &Inputs, p: u32, slot: i64, out: &mut Vec<Trip>) {
        let t = slot * SLOT;
        let f = self.factors(t);
        let mix = self.place_mix[p as usize];
        let factor: i64 = (0..CLASSES).map(|c| mix[c] * f[c]).sum::<i64>() / 1000;
        let rate = self.rate[p as usize] * factor / 1000 * self.balance(&f) / 1000;
        if rate <= 0 {
            return;
        }
        let base = hash(&[
            inputs.seed,
            version_tag(),
            TAG_TRIP,
            u64::from(p),
            slot as u64,
        ]);
        let n = rate / 1_000_000 + i64::from((below(base, 1_000_000) as i64) < rate % 1_000_000);
        if n == 0 {
            return;
        }
        for j in 0..n {
            let key = hash(&[base, j as u64]);
            let t0 = t + below(hash(&[key, 9]), SLOT as u64) as i64;
            out.push(self.trip(inputs, p, key, t0, &f));
        }
    }

    /// The visit to spot `s` in stay slot `slot`, if any.
    fn slot_visit(&self, inputs: &Inputs, s: u32, slot: i64) -> Option<Trip> {
        let t = slot * STAY_SLOT + stay_phase(s);
        let spot = &self.lanes.spots[s as usize];
        let (lo, hi) = match spot.kind {
            spot_kind::SQUARE => (3, 10),
            spot_kind::STOP => (1, 6),
            _ => (5, 11),
        };
        let mean_ticks = (lo + hi) * 3_600 / 2;
        let p = self.occupancy(s as usize, t) * STAY_SLOT / mean_ticks;
        let base = hash(&[
            inputs.seed,
            version_tag(),
            TAG_VISIT,
            u64::from(s),
            slot as u64,
        ]);
        if below(base, 1000) as i64 >= p.min(1000) {
            return None;
        }
        let ticks = between(hash(&[base, 1]), lo * 3_600, hi * 3_600);
        let t_stay = t + below(hash(&[base, 2]), (STAY_SLOT - ticks) as u64) as i64;
        let f = self.factors(t_stay);
        Some(self.visit(inputs, s, hash(&[base, 3]), t_stay, ticks, &f))
    }
}

// ---------------------------------------------------------------------------
// Where a trip's people are.

/// A base position before avoidance between people.
#[derive(Clone, Copy, Debug)]
struct Base {
    person: Person,
    /// How still the person is (per mille): 1000 standing or sitting,
    /// in between while gathering at a spot or leaving it.
    still: i64,
    /// How far the person is into stepping aside (per mille): it fades in
    /// over the first metres of a route and out over the last.
    fade: i64,
    speed: i64,
}

impl World {
    /// The leader's route distance at `mt` thousandths of a tick after the
    /// start, extended past the end at the last pace; and the seg index.
    fn leader(&self, trip: &Trip, mt: i64) -> (i64, usize) {
        let k = trip.segs.partition_point(|g| g.t0 <= mt).max(1) - 1;
        let g = &trip.segs[k];
        if g.stay {
            return (g.s0, k);
        }
        let into = (mt - g.t0).max(0);
        if k + 1 == trip.segs.len() && into > g.dur {
            return (g.s0 + g.len + (into - g.dur) * g.len / g.dur.max(1), k);
        }
        (g.s0 + into.min(g.dur) * g.len / g.dur.max(1), k)
    }

    /// The walking seg holding route distance `s`.
    fn seg_at(&self, trip: &Trip, s: i64) -> usize {
        let k = trip.segs.partition_point(|g| g.s0 <= s).max(1) - 1;
        // A stay has no length: use the walk before it.
        let mut k = k.min(trip.segs.len() - 1);
        while trip.segs[k].stay && k > 0 {
            k -= 1;
        }
        k
    }

    /// The lane's point at route distance `s`, turns rounded off: the
    /// point, facing and room beside the lane all blend into the next or
    /// previous walk near a node, so none of them jumps there. The facing
    /// keeps its blended length, which shrinks in a turn (to nothing when
    /// turning round), and [`offset`] scales sideways offsets by it.
    /// Returns the seg holding `s` too.
    fn route_point(&self, trip: &Trip, s: i64) -> (EdgePoint, usize) {
        let k = self.seg_at(trip, s);
        let g = trip.segs[k];
        let local = s - g.s0;
        let mut p = self.edge_point(g.edge, g.fwd, local);
        let mut blend = |o: EdgePoint, w: i64| {
            for i in 0..3 {
                p.pos[i] += (o.pos[i] - p.pos[i]) * w / 1000;
            }
            for i in 0..2 {
                p.dir[i] += (o.dir[i] - p.dir[i]) * w / 1000;
            }
            // The room tends to the narrower of the two lanes at the node
            // (w is a half there), so turns into a footway narrow early.
            p.right += (p.right.min(o.right) - p.right) * w / 500;
            p.left += (p.left.min(o.left) - p.left) * w / 500;
        };
        // Into the next walk, unless a stay comes first.
        if let Some(n) = trip.segs.get(k + 1).filter(|n| !n.stay) {
            let b = BLEND.min(g.len / 2).min(n.len / 2);
            let rest = g.len - local;
            if b > 0 && rest < b {
                let w = (b - rest.max(0)) * 1000 / (2 * b);
                blend(self.edge_point(n.edge, n.fwd, local - g.len), w);
            }
        }
        if k > 0
            && let Some(q) = trip.segs.get(k - 1).filter(|q| !q.stay)
        {
            let b = BLEND.min(g.len / 2).min(q.len / 2);
            if b > 0 && local < b {
                let w = (b - local.max(0)) * 1000 / (2 * b);
                blend(self.edge_point(q.edge, q.fwd, q.len + local), w);
            }
        }
        // Near a node joining two walks, keep within the node's clearance
        // from walls (turns cut corners), relaxing over the turn's length.
        let cap = |node: u32, d: i64, b: i64, p: &mut EdgePoint| {
            let clear = self
                .lanes
                .node_clear
                .get(node as usize)
                .copied()
                .unwrap_or(NODE_CLEAR_MAX);
            if b > 0 && d < b && clear < NODE_CLEAR_MAX {
                let c = clear + (NODE_CLEAR_MAX - clear) * d.max(0) / b;
                p.right = p.right.min(c);
                p.left = p.left.min(c);
            }
        };
        let edge = &self.lanes.edges[g.edge as usize];
        let (from, to) = if g.fwd {
            (edge.a, edge.b)
        } else {
            (edge.b, edge.a)
        };
        if let Some(n) = trip.segs.get(k + 1).filter(|n| !n.stay) {
            cap(
                to,
                g.len - local,
                BLEND.min(g.len / 2).min(n.len / 2),
                &mut p,
            );
        }
        if k > 0
            && let Some(q) = trip.segs.get(k - 1).filter(|q| !q.stay)
        {
            cap(from, local, BLEND.min(g.len / 2).min(q.len / 2), &mut p);
        }
        (p, k)
    }

    /// Whether a point of a seg is where feet may enter a shell.
    fn at_entrance(&self, g: &Seg, local: i64) -> bool {
        let e = &self.lanes.edges[g.edge as usize];
        if e.flags & (flag::ACCESS | flag::PASSAGE) != 0 {
            return true;
        }
        let se = if g.fwd { local } else { e.len - local };
        (e.flags & flag::ENTRANCE_A != 0 && se < ENTRANCE_RANGE)
            || (e.flags & flag::ENTRANCE_B != 0 && se > e.len - ENTRANCE_RANGE)
    }

    /// The people of a trip at crowd time `t` (ticks) plus `frac`
    /// thousandths of a tick, before avoidance between people.
    fn trip_people(&self, inputs: &Inputs, trip: &Trip, t: i64, frac: i64, out: &mut Vec<Base>) {
        let mt = (t - trip.start) * 1000 + frac;
        if mt < 0 || t > trip.end {
            return;
        }
        let (lead, k) = self.leader(trip, mt);
        let n = i64::from(trip.size);
        // The formation, from the room where the leader is.
        let lead_s = lead.min(trip.total);
        let (here, k_lead) = self.route_point(trip, lead_s);
        // The room over the group's whole length: at the leader and where
        // the last member could be.
        let (mut room_r, mut room_l) = (here.right, here.left);
        if n > 1 {
            let (tail, _) = self.route_point(trip, (lead_s - (n - 1) * SPACING).max(0));
            room_r = room_r.min(tail.right);
            room_l = room_l.min(tail.left);
        }
        let (r, l) = ((room_r - BODY).max(0), (room_l - BODY).max(0));
        let a = if n > 1 {
            SPACING.min((r + l) / (n - 1))
        } else {
            0
        };
        // Staggered as the lane narrows, and as a turn draws people abreast
        // together (offsets shrink with the facing there): linear, so the
        // stagger changes no faster than the width (members stay at least
        // 0.49 m apart).
        let shrink = (here.dir[0] * here.dir[0] + here.dir[1] * here.dir[1])
            .isqrt()
            .min(UNIT);
        let b = SPACING - a * shrink / UNIT;
        let width = (n - 1) * a;
        let centre = match inputs.avoid {
            Avoid::None => 0,
            Avoid::Lanes => {
                let slow = if n == 1 {
                    ((1_500 - trip.speed) / 2).clamp(0, 500)
                } else {
                    0
                };
                width / 2 + 300 + slow
            }
        };
        let centre = if r + l < width {
            (r - l) / 2
        } else {
            centre.clamp(-l + width / 2, r - width / 2)
        };
        // A visit's stay and ring.
        let stay = trip
            .segs
            .iter()
            .position(|g| g.stay)
            .map(|i| (i, trip.segs[i]));
        let staying = stay.is_some_and(|(i, _)| i == k);
        for i in 0..n {
            let m = trip.members[i as usize];
            let lag = i * b;
            let s = lead - lag;
            let lat = centre + (i * 2 - (n - 1)) * a / 2;
            let mut walking = true;
            let mut still = 0i64;
            let (mut pos, mut dir, mut room, mut used, mut entrance);
            let mut lane_kind = 0;
            if s < 0 || s > trip.total {
                if !staying {
                    continue;
                }
                // Behind the leader at a stay: placed on the ring below.
                pos = [0; 3];
                dir = [0; 2];
                room = (0, 0);
                used = 0;
                entrance = false;
            } else {
                let (p, kk) = if s == lead_s {
                    (here, k_lead)
                } else {
                    self.route_point(trip, s)
                };
                let (at, l) = offset(&p, lat);
                pos = at;
                dir = p.dir;
                room = (p.right, p.left);
                used = l;
                let g = trip.segs[kk];
                entrance = self.at_entrance(&g, s - g.s0);
                lane_kind = self.lanes.edges[g.edge as usize].kind;
            }
            let mut pose =
                Pose::Walk((((s.max(0) * 65_536 / m.stride.max(1)) + m.phase0) & 0xFFFF) as u16);
            if let (Some((_, sg)), Some((spot, kind, rot))) = (stay, trip.visit) {
                // Gather into the ring as the leader nears the spot, and
                // leave it as the leader walks off.
                let d = if lead <= sg.s0 {
                    sg.s0 - lead
                } else {
                    lead - sg.s0
                };
                let w = if staying {
                    1000
                } else {
                    ((STAY_BLEND - d) * 1000 / STAY_BLEND).max(0)
                };
                if w > 0 {
                    still = w;
                    let centre_pos =
                        self.lanes.nodes[self.lanes.spots[spot as usize].node as usize];
                    let radius = [0, 0, 400, 420, 480][n as usize];
                    let step = 12 / n;
                    let ring = RING[((rot % 12) as i64 + i * step).rem_euclid(12) as usize];
                    let slot = [
                        centre_pos[0] + ring[0] * radius / UNIT,
                        centre_pos[1],
                        centre_pos[2] + ring[1] * radius / UNIT,
                    ];
                    let face = if n == 1 {
                        RING[(rot % 12) as usize]
                    } else {
                        [-ring[0], -ring[1]]
                    };
                    if staying || s < 0 || s > trip.total {
                        pos = slot;
                        dir = face;
                    } else {
                        for j in 0..3 {
                            pos[j] += (slot[j] - pos[j]) * w / 1000;
                        }
                        for j in 0..2 {
                            dir[j] += (face[j] - dir[j]) * w / 1000;
                        }
                    }
                    if staying {
                        walking = false;
                        pose = if kind == spot_kind::BENCH {
                            Pose::Sit
                        } else {
                            Pose::Stand
                        };
                        room = (0, 0);
                        used = 0;
                    }
                    entrance = false;
                }
            }
            let right = right_of(dir);
            let rl = (dir[0] * dir[0] + dir[1] * dir[1]).isqrt().max(1);
            let right = [right[0] * UNIT / rl, right[1] * UNIT / rl];
            // The offset as walked: it shrinks with the facing in turns.
            let used = used * (dir[0] * dir[0] + dir[1] * dir[1]).isqrt().min(UNIT) / UNIT;
            let lateral = if walking {
                (-(room.1 - BODY) - used, (room.0 - BODY) - used)
            } else {
                (0, 0)
            };
            out.push(Base {
                person: Person {
                    trip: trip.id,
                    member: i as u8,
                    pos,
                    dir,
                    pose,
                    look: m.look,
                    height: m.height,
                    at_entrance: entrance,
                    right,
                    lateral: (lateral.0.min(0), lateral.1.max(0)),
                    trip_start: trip.start,
                    trip_end: trip.end,
                    lane_kind: if walking { lane_kind } else { 0 },
                },
                still,
                fade: (s.min(trip.total - s) * 1000 / FADE_RAMP).clamp(0, 1000),
                speed: trip.speed,
            });
        }
    }
}

/// People fade into and out of stepping aside over the first and last
/// metres of their routes (mm).
const FADE_RAMP: i64 = 2_000;

/// Steps aside in closed form, from where everyone is at this tick (no
/// state): a walker closer than [`PUSH_RANGE`] to someone of another group
/// steps sideways by half the shortfall (all of it beside someone who
/// stands), away from the other; when the two are nearly in line, to the
/// side a rule gives (both to their right when they meet head on, the
/// faster one to the left when overtaking). Every term changes smoothly
/// with where people are and which way they face, and people fade into
/// and out of it over the first and last [`FADE_RAMP`] of their routes
/// and while turning round, so nobody jumps. Each step is then limited to
/// [`PUSH_MAX`] and to the room beside the lane.
fn avoid(bases: &mut [Base], within: impl Fn(&Base) -> bool) {
    let cell = PUSH_RANGE;
    let mut keys: Vec<(i64, i64, usize)> = bases
        .iter()
        .enumerate()
        .map(|(i, b)| {
            (
                b.person.pos[0].div_euclid(cell),
                b.person.pos[2].div_euclid(cell),
                i,
            )
        })
        .collect();
    keys.sort_unstable();
    // The cells (cx, cz - 1) to (cx, cz + 1) are next to each other in
    // the sorted keys.
    let row = |cx: i64, cz: i64| {
        let lo = keys.partition_point(|k| (k.0, k.1) < (cx, cz - 1));
        let hi = keys.partition_point(|k| (k.0, k.1) <= (cx, cz + 1));
        &keys[lo..hi]
    };
    let mut push = vec![0i64; bases.len()];
    for &(cx, cz, i) in &keys {
        let bi = &bases[i];
        if bi.still >= 1000 || !within(bi) {
            continue;
        }
        // Turning round (the facing shrinks through zero) also fades it.
        let facing = (bi.person.dir[0] * bi.person.dir[0] + bi.person.dir[1] * bi.person.dir[1])
            .isqrt()
            .min(UNIT);
        let me = (1000 - bi.still) * bi.fade / 1000 * facing / UNIT;
        for dx in -1..=1 {
            for &(_, _, j) in row(cx + dx, cz) {
                let bj = &bases[j];
                if j == i || bj.person.trip == bi.person.trip {
                    continue;
                }
                let d = [
                    bi.person.pos[0] - bj.person.pos[0],
                    bi.person.pos[2] - bj.person.pos[2],
                ];
                let d2 = d[0] * d[0] + d[1] * d[1];
                if d2 >= PUSH_RANGE * PUSH_RANGE {
                    continue;
                }
                let dist = d2.isqrt();
                let share = (1000 + bj.still) / 2;
                let g = (PUSH_RANGE - dist) * share / 1000 * bj.fade / 1000 * me / 1000;
                // The rule: head on, both to the right; overtaking, the
                // faster one to the left; blended by how they face.
                let dot = (bi.person.dir[0] * bj.person.dir[0]
                    + bi.person.dir[1] * bj.person.dir[1])
                    / UNIT;
                let same = if bi.speed != bj.speed {
                    if bi.speed > bj.speed { -1000 } else { 1000 }
                } else if bi.person.key() < bj.person.key() {
                    1000
                } else {
                    -1000
                };
                let rule = if dot <= -300 {
                    1000
                } else if dot >= 300 {
                    same
                } else {
                    1000 + (same - 1000) * (dot + 300) / 600
                };
                // Away from the other once well to one side.
                let ri = bi.person.right;
                let s_lat = (d[0] * ri[0] + d[1] * ri[1]) / UNIT;
                let away = if s_lat >= 0 { 1000 } else { -1000 };
                let a = s_lat.abs();
                let side = if a <= 400 {
                    rule
                } else if a >= 700 {
                    away
                } else {
                    rule + (away - rule) * (a - 400) / 300
                };
                push[i] += g * side / 1000;
            }
        }
    }
    for (b, p) in bases.iter_mut().zip(push) {
        if p == 0 {
            continue;
        }
        let side = p
            .clamp(-PUSH_MAX, PUSH_MAX)
            .clamp(b.person.lateral.0, b.person.lateral.1);
        let r = b.person.right;
        b.person.pos[0] += r[0] * side / UNIT;
        b.person.pos[2] += r[1] * side / UNIT;
        b.person.lateral = (b.person.lateral.0 - side, b.person.lateral.1 - side);
    }
}

// ---------------------------------------------------------------------------
// The crowd: trips worked out so far around a centre, and queries.

/// Trips worked out around a centre. It only remembers what the function
/// gives; [`Crowd::people`] returns the same people for the same time and
/// place whatever was asked before.
pub struct Crowd {
    pub inputs: Inputs,
    trips: Vec<Trip>,
    /// Next departure slot to work out, per place (None: out of range).
    next_slot: Vec<Option<i64>>,
    /// Next stay slot, per spot.
    next_stay: Vec<Option<i64>>,
    centre: [i64; 2],
    /// Where near intervals were last worked out, and for which radius.
    near_centre: Option<([i64; 2], i64)>,
    /// Places and spots within this distance of the centre are worked out.
    reach: i64,
    last_t: Option<i64>,
    bases: Vec<Base>,
}

/// What the crowd holds, for reports.
#[derive(Clone, Copy, Debug, Default)]
pub struct CrowdStats {
    pub trips: usize,
    pub places_in_reach: usize,
    pub spots_in_reach: usize,
    /// Trips that found no place to end at (none expected).
    pub ending_nowhere: usize,
}

impl Crowd {
    /// `radius` is the largest radius that will be asked for (mm): trips
    /// are worked out from places that close to the centre plus the
    /// longest walk.
    pub fn new(world: &World, inputs: Inputs, radius: i64) -> Crowd {
        let longest = world.max_route;
        Crowd {
            inputs,
            trips: Vec::new(),
            next_slot: vec![None; world.lanes.places.len()],
            next_stay: vec![None; world.lanes.spots.len()],
            centre: [i64::MIN, i64::MIN],
            near_centre: None,
            reach: radius + longest,
            last_t: None,
            bases: Vec::new(),
        }
    }

    pub fn stats(&self) -> CrowdStats {
        CrowdStats {
            trips: self.trips.len(),
            places_in_reach: self.next_slot.iter().filter(|s| s.is_some()).count(),
            spots_in_reach: self.next_stay.iter().filter(|s| s.is_some()).count(),
            ending_nowhere: self.trips.iter().filter(|t| t.end_place < 0).count(),
        }
    }

    #[cfg(test)]
    pub fn trips(&self) -> &[Trip] {
        &self.trips
    }

    /// Works trips out up to time `t` around `centre`.
    fn advance(&mut self, world: &World, t: i64, centre: [i64; 2]) {
        // Going back in time, or a jump: start again (the same people).
        if self.last_t.is_some_and(|l| t < l || t - l > world.window) {
            self.trips.clear();
            self.next_slot.iter_mut().for_each(|s| *s = None);
            self.next_stay.iter_mut().for_each(|s| *s = None);
            self.centre = [i64::MIN, i64::MIN];
            self.near_centre = None;
        }
        self.last_t = Some(t);
        let moved = self.centre[0] == i64::MIN
            || (centre[0] - self.centre[0]).abs() + (centre[1] - self.centre[1]).abs() > 50_000;
        if moved {
            self.centre = centre;
            let reach = self.reach;
            let inside = |p: [i64; 2], slack: i64| {
                let (dx, dz) = (p[0] - centre[0], p[1] - centre[1]);
                dx * dx + dz * dz <= (reach + slack) * (reach + slack)
            };
            let first_slot = (t - world.window).div_euclid(SLOT);
            for (i, p) in world.lanes.places.iter().enumerate() {
                let xz = world.node_xz(p.node);
                match self.next_slot[i] {
                    None if inside(xz, 0) => self.next_slot[i] = Some(first_slot),
                    Some(_) if !inside(xz, 100_000) => self.next_slot[i] = None,
                    _ => {}
                }
            }
            let first_stay = (t - 2 * world.leg_window - STAY_SLOT).div_euclid(STAY_SLOT);
            for (i, s) in world.lanes.spots.iter().enumerate() {
                let xz = world.node_xz(s.node);
                match self.next_stay[i] {
                    None if inside(xz, 0) => self.next_stay[i] = Some(first_stay),
                    Some(_) if !inside(xz, 100_000) => self.next_stay[i] = None,
                    _ => {}
                }
            }
            // Trips from places now out of reach go.
            let keep_place = |o: i64| {
                if o >= 0 {
                    self.next_slot[o as usize].is_some()
                } else {
                    self.next_stay[(-1 - o) as usize].is_some()
                }
            };
            self.trips.retain(|tr| keep_place(tr.origin));
            self.near_centre = None;
        }
        let before = self.trips.len();
        let inputs = self.inputs;
        for (i, slot) in self.next_slot.iter_mut().enumerate() {
            let Some(next) = slot else { continue };
            while *next * SLOT <= t {
                world.slot_trips(&inputs, i as u32, *next, &mut self.trips);
                *next += 1;
            }
        }
        for (i, slot) in self.next_stay.iter_mut().enumerate() {
            let Some(next) = slot else { continue };
            while *next * STAY_SLOT + stay_phase(i as u32) <= t + world.leg_window {
                if let Some(v) = world.slot_visit(&inputs, i as u32, *next) {
                    self.trips.push(v);
                }
                *next += 1;
            }
        }
        if self.trips.len() != before {
            // New trips have no near interval yet.
            if let Some((c, r)) = self.near_centre {
                for tr in &mut self.trips[before..] {
                    tr.near = near_interval(world, tr, c, r);
                }
            }
        }
        self.trips.retain(|tr| tr.end >= t);
    }

    /// Everyone within `radius` (mm) of `centre` at crowd time `t` ticks
    /// plus `frac` thousandths, sorted by person.
    pub fn people(
        &mut self,
        world: &World,
        t: i64,
        frac: i64,
        centre: [i64; 2],
        radius: i64,
        out: &mut Vec<Person>,
    ) {
        self.advance(world, t, centre);
        // Near intervals: when each trip passes within the radius (plus
        // the avoidance margin and the group's length).
        let margin = radius + 2 * PUSH_RANGE + 3 * SPACING + 2_000;
        let stale = self.near_centre.is_none_or(|(c, r)| {
            r < margin || (c[0] - centre[0]).abs() + (c[1] - centre[1]).abs() > (r - margin)
        });
        if stale {
            let r = margin + 10_000;
            for tr in &mut self.trips {
                tr.near = near_interval(world, tr, centre, r);
            }
            self.near_centre = Some((centre, r));
        }
        let mut bases = std::mem::take(&mut self.bases);
        bases.clear();
        let reach = radius + PUSH_RANGE + 100;
        for tr in &self.trips {
            if t < tr.near.0 || t > tr.near.1 {
                continue;
            }
            let before = bases.len();
            world.trip_people(&self.inputs, tr, t, frac, &mut bases);
            // Keep those within reach.
            let mut k = before;
            for i in before..bases.len() {
                let p = bases[i].person.pos;
                let (dx, dz) = (p[0] - centre[0], p[2] - centre[1]);
                if dx * dx + dz * dz <= reach * reach {
                    bases.swap(k, i);
                    k += 1;
                }
            }
            bases.truncate(k);
        }
        if self.inputs.avoid == Avoid::Lanes {
            let r2 = radius * radius;
            avoid(&mut bases, |b| {
                let (dx, dz) = (b.person.pos[0] - centre[0], b.person.pos[2] - centre[1]);
                dx * dx + dz * dz <= r2
            });
        }
        out.clear();
        for b in &bases {
            let p = b.person.pos;
            let (dx, dz) = (p[0] - centre[0], p[2] - centre[1]);
            if dx * dx + dz * dz <= radius * radius {
                out.push(b.person);
            }
        }
        out.sort_unstable_by_key(|p| p.key());
        self.bases = bases;
    }
}

/// When a trip passes within `r` of `c`: from the first such edge's start
/// to the last one's end (crowd ticks), or an empty interval.
fn near_interval(world: &World, trip: &Trip, c: [i64; 2], r: i64) -> (i64, i64) {
    let (mut lo, mut hi) = (i64::MAX, i64::MIN);
    for g in &trip.segs {
        let near = if g.stay {
            let p = world.node_xz(world.lanes.spots[g.edge as usize].node);
            let (dx, dz) = (p[0] - c[0], p[1] - c[1]);
            dx * dx + dz * dz <= r * r
        } else {
            world.edge_near(g.edge, c, r)
        };
        if near {
            lo = lo.min(trip.start + g.t0 / 1000);
            hi = hi.max(trip.start + (g.t0 + g.dur) / 1000 + 1);
        }
    }
    if lo == i64::MAX {
        return (0, -1);
    }
    // Members trail the leader by a few seconds at most.
    (lo, (hi + 600).min(trip.end))
}

/// A hash of everyone's position, for checking that two runs saw the same
/// people.
pub fn people_hash(people: &[Person]) -> String {
    let mut h = StateHasher::new("streetwide.crowd-proto.people");
    h.str(VERSION);
    for p in people {
        h.u64(p.trip).u8(p.member);
        for v in p.pos {
            h.i64(v);
        }
    }
    h.finish().hex()
}

// ---------------------------------------------------------------------------

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A grid of streets 40 m apart, `n` by `n` nodes, with sidewalks and a
    /// pedestrian street, doors on some blocks, the world's edge around it,
    /// a square spot and a bench.
    pub fn grid_world(n: i64) -> World {
        let mut lanes = Lanes::default();
        let step = 40_000;
        for z in 0..n {
            for x in 0..n {
                lanes.nodes.push([x * step, 0, z * step]);
                lanes.node_clear.push(NODE_CLEAR_MAX);
            }
        }
        let id = |x: i64, z: i64| (z * n + x) as u32;
        let add_edge = |lanes: &mut Lanes, a: u32, b: u32, k: u8, flags: u8| {
            let pa = lanes.nodes[a as usize];
            let pb = lanes.nodes[b as usize];
            let len = ((pb[0] - pa[0]).pow(2) + (pb[2] - pa[2]).pow(2)).isqrt();
            let first = lanes.verts.len() as u32;
            let parts = (len / 2_000).max(1);
            for i in 0..=parts {
                lanes.verts.push(Vert {
                    x: pa[0] + (pb[0] - pa[0]) * i / parts,
                    y: 0,
                    z: pa[2] + (pb[2] - pa[2]) * i / parts,
                    s: len * i / parts,
                    right: 2_000,
                    left: 2_000,
                });
            }
            lanes.edges.push(Edge {
                a,
                b,
                kind: k,
                class: if k == kind::PEDESTRIAN { CITY } else { RIVER },
                flags,
                square: -1,
                len,
                first_vert: first,
                verts: (parts + 1) as u32,
            });
            (lanes.edges.len() - 1) as u32
        };
        for z in 0..n {
            for x in 0..n {
                if x + 1 < n {
                    let k = if z == n / 2 {
                        kind::PEDESTRIAN
                    } else {
                        kind::SIDEWALK
                    };
                    add_edge(&mut lanes, id(x, z), id(x + 1, z), k, 0);
                }
                if z + 1 < n {
                    let k = if x == 1 { kind::STEPS } else { kind::FOOTWAY };
                    add_edge(&mut lanes, id(x, z), id(x, z + 1), k, 0);
                }
            }
        }
        // Doors: a node 5 m off the middle of some blocks.
        for z in 0..n {
            for x in 0..n {
                if (x + z) % 3 == 0 {
                    let p = lanes.nodes[id(x, z) as usize];
                    lanes.nodes.push([p[0] + 3_000, 0, p[2] + 4_000]);
                    lanes.node_clear.push(NODE_CLEAR_MAX);
                    let door = (lanes.nodes.len() - 1) as u32;
                    let a = add_edge(
                        &mut lanes,
                        id(x, z),
                        door,
                        kind::ACCESS,
                        flag::ACCESS | flag::ENTRANCE_B,
                    );
                    lanes.places.push(Place {
                        kind: place_kind::SHOP,
                        node: id(x, z),
                        weight: 1000,
                        access: Some(a),
                    });
                }
                if x == 0 || z == 0 || x == n - 1 || z == n - 1 {
                    lanes.places.push(Place {
                        kind: place_kind::EDGE,
                        node: id(x, z),
                        weight: 3000,
                        access: None,
                    });
                }
            }
        }
        for (k, x) in [(spot_kind::SQUARE, 2), (spot_kind::BENCH, 3)] {
            let p = lanes.nodes[id(x, 2) as usize];
            lanes.nodes.push([p[0] + 6_000, 0, p[2] - 6_000]);
            lanes.node_clear.push(NODE_CLEAR_MAX);
            let s = (lanes.nodes.len() - 1) as u32;
            let a = add_edge(&mut lanes, id(x, 2), s, kind::ACCESS, flag::ACCESS);
            lanes.spots.push(Spot {
                kind: k,
                node: s,
                access: a,
            });
        }
        World::new(lanes, Profiles::zurich())
    }

    fn inputs(seed: u64, avoid: Avoid) -> Inputs {
        Inputs {
            seed,
            avoid,
            looks: 64,
        }
    }

    fn clock(y: i32, m: u32, d: u32, minute: u32) -> Clock {
        Clock {
            year: y,
            month: m,
            day: d,
            minute,
        }
    }

    #[test]
    fn calendar() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        let d = days_from_civil(2026, 12, 21);
        assert_eq!(month_of(d), 12);
        assert_eq!(day_type(d), 0, "a Monday");
        assert_eq!(day_type(days_from_civil(2026, 6, 21)), 2, "a Sunday");
        assert_eq!(day_type(days_from_civil(2026, 6, 20)), 1, "a Saturday");
        assert_eq!(month_of(days_from_civil(2024, 2, 29)), 2);
    }

    #[test]
    fn the_same_inputs_give_the_same_people() {
        let world = grid_world(7);
        let t = crowd_time(&clock(2026, 12, 21, 17 * 60), 300);
        let c = [120_000, 120_000];
        for avoid in [Avoid::None, Avoid::Lanes] {
            let mut a = Crowd::new(&world, inputs(7, avoid), 200_000);
            let mut b = Crowd::new(&world, inputs(7, avoid), 200_000);
            let (mut pa, mut pb) = (Vec::new(), Vec::new());
            // b is asked other things first: elsewhere, earlier, wider.
            b.people(&world, t - 5_000, 0, [0, 0], 80_000, &mut pb);
            b.people(&world, t - 20, 0, [200_000, 0], 200_000, &mut pb);
            a.people(&world, t, 0, c, 150_000, &mut pa);
            b.people(&world, t, 0, c, 150_000, &mut pb);
            assert!(pa.len() > 20, "{}", pa.len());
            assert_eq!(pa, pb);
            assert_eq!(people_hash(&pa), people_hash(&pb));
            // Another seed gives other people.
            let mut other = Crowd::new(&world, inputs(8, avoid), 200_000);
            let mut po = Vec::new();
            other.people(&world, t, 0, c, 150_000, &mut po);
            assert_ne!(people_hash(&pa), people_hash(&po));
        }
    }

    #[test]
    fn trips_start_and_end_at_places() {
        let world = grid_world(7);
        let t = crowd_time(&clock(2026, 6, 20, 15 * 60), 0);
        let mut crowd = Crowd::new(&world, inputs(3, Avoid::Lanes), 300_000);
        let mut out = Vec::new();
        crowd.people(&world, t, 0, [120_000, 120_000], 300_000, &mut out);
        let place_nodes = |node: u32| {
            world.lanes.places.iter().any(|p| {
                p.node == node
                    || p.access.is_some_and(|a| {
                        let e = &world.lanes.edges[a as usize];
                        e.a == node || e.b == node
                    })
            })
        };
        let trips = crowd.trips();
        assert!(trips.len() > 50, "{}", trips.len());
        let (mut visits, mut checked) = (0, 0);
        for tr in trips {
            let first = tr.segs.first().unwrap();
            let last = tr.segs.iter().rev().find(|g| !g.stay).unwrap();
            let e0 = &world.lanes.edges[first.edge as usize];
            let e1 = &world.lanes.edges[last.edge as usize];
            let from = if first.fwd { e0.a } else { e0.b };
            let to = if last.fwd { e1.b } else { e1.a };
            assert!(place_nodes(from), "trip {} starts at node {from}", tr.id);
            assert!(place_nodes(to), "trip {} ends at node {to}", tr.id);
            assert!(tr.end_place >= 0);
            // Consecutive legs join.
            for w in tr.segs.windows(2) {
                let (a, b) = (w[0], w[1]);
                if a.stay || b.stay {
                    continue;
                }
                let ea = &world.lanes.edges[a.edge as usize];
                let eb = &world.lanes.edges[b.edge as usize];
                let end = if a.fwd { ea.b } else { ea.a };
                let start = if b.fwd { eb.a } else { eb.b };
                assert_eq!(end, start);
            }
            visits += usize::from(tr.visit.is_some());
            checked += 1;
        }
        assert!(
            checked > 0 && visits > 0,
            "{checked} trips, {visits} visits"
        );
    }

    #[test]
    fn counts_follow_the_profile() {
        let world = grid_world(9);
        let c = [160_000, 160_000];
        let r = 1_000_000;
        let mut ratios = Vec::new();
        for (d, minute) in [(21, 17 * 60), (21, 3 * 60), (20, 15 * 60), (22, 12 * 60)] {
            let t = crowd_time(&clock(2026, 6, d, minute), 0);
            // Averaged over three seeds and a few minutes: the grid holds
            // a few hundred people, and a few dozen at night.
            let mut sum = 0;
            for seed in [11, 12, 13] {
                let mut crowd = Crowd::new(&world, inputs(seed, Avoid::None), r);
                let mut out = Vec::new();
                for k in 0..5 {
                    crowd.people(&world, t + k * 3_600, 0, c, r, &mut out);
                    sum += out.len() as i64;
                }
            }
            let got = sum * 1000 / 15;
            let want = world.target(t, c, r);
            ratios.push((d, minute, got, want));
        }
        for &(d, minute, got, want) in &ratios {
            let ratio = got * 1000 / want.max(1);
            // At night (a few dozen people) the band is wider.
            let band = if minute == 3 * 60 {
                600..=1500
            } else {
                800..=1250
            };
            assert!(
                band.contains(&ratio),
                "June {d} at {minute}: {got} against {want} (thousandths)"
            );
        }
        // Night is far quieter than the afternoon.
        assert!(ratios[1].2 * 5 < ratios[0].2, "{ratios:?}");
    }

    #[test]
    fn groups_speeds_and_steps() {
        let world = grid_world(7);
        let t = crowd_time(&clock(2026, 12, 21, 17 * 60), 0);
        let mut crowd = Crowd::new(&world, inputs(5, Avoid::Lanes), 400_000);
        let mut out = Vec::new();
        crowd.people(&world, t, 0, [120_000, 120_000], 400_000, &mut out);
        let trips = crowd.trips();
        let people: i64 = trips.iter().map(|t| i64::from(t.size)).sum();
        let grouped: i64 = trips
            .iter()
            .filter(|t| t.size > 1)
            .map(|t| i64::from(t.size))
            .sum();
        assert!(grouped * 2 > people, "{grouped} of {people} in groups");
        assert!(trips.iter().all(|t| (1..=4).contains(&t.size)));
        let walkers: Vec<_> = trips.iter().filter(|t| t.visit.is_none()).collect();
        let mean = walkers.iter().map(|t| t.speed).sum::<i64>() / walkers.len() as i64;
        assert!((1_100..=1_400).contains(&mean), "mean speed {mean} mm/s");
        // Steps take twice as long per metre.
        let tr = walkers[0];
        let g = tr.segs.iter().find(|g| !g.stay).unwrap();
        let e = &world.lanes.edges[g.edge as usize];
        let per_m = g.dur * 1000 / g.len;
        let want = 60_000 * 1000 / (tr.speed * world.profiles.kind_speed[e.kind as usize] / 1000);
        assert!((per_m - want).abs() * 100 <= want, "{per_m} {want}");
    }

    #[test]
    fn nobody_jumps_between_ticks() {
        let world = grid_world(7);
        let t0 = crowd_time(&clock(2026, 6, 21, 18 * 60), 0);
        for avoid in [Avoid::None, Avoid::Lanes] {
            let mut crowd = Crowd::new(&world, inputs(9, avoid), 300_000);
            let (mut prev, mut now) = (Vec::new(), Vec::new());
            crowd.people(&world, t0, 0, [120_000, 120_000], 300_000, &mut prev);
            let mut worst = 0;
            for k in 1..600 {
                crowd.people(&world, t0 + k, 0, [120_000, 120_000], 300_000, &mut now);
                let mut j = 0;
                for p in &now {
                    while j < prev.len() && prev[j].key() < p.key() {
                        j += 1;
                    }
                    if j < prev.len() && prev[j].key() == p.key() {
                        let q = prev[j];
                        let d =
                            ((p.pos[0] - q.pos[0]).pow(2) + (p.pos[2] - q.pos[2]).pow(2)).isqrt();
                        worst = worst.max(d);
                    }
                }
                std::mem::swap(&mut prev, &mut now);
            }
            // 1.9 m/s is 32 mm a tick; turns, formations and stepping aside
            // may add a little, never a jump.
            assert!(worst < 120, "{avoid:?}: {worst} mm in one tick");
        }
    }

    #[test]
    fn people_stand_at_spots() {
        let world = grid_world(7);
        let mut seen_stand = 0;
        let mut seen_sit = 0;
        for h in 10..18 {
            let t = crowd_time(&clock(2026, 6, 20, h * 60), 0);
            let mut crowd = Crowd::new(&world, inputs(21, Avoid::Lanes), 300_000);
            let mut out = Vec::new();
            crowd.people(&world, t, 0, [120_000, 120_000], 300_000, &mut out);
            seen_stand += out.iter().filter(|p| p.pose == Pose::Stand).count();
            seen_sit += out.iter().filter(|p| p.pose == Pose::Sit).count();
        }
        assert!(
            seen_stand > 0 && seen_sit > 0,
            "{seen_stand} standing, {seen_sit} sitting"
        );
    }

    #[test]
    fn this_file_uses_no_floats_or_unordered_maps() {
        let source = include_str!("population.rs");
        for word in [
            concat!("f", "32"),
            concat!("f", "64"),
            concat!("Hash", "Map"),
            concat!("Hash", "Set"),
            concat!("Inst", "ant"),
            concat!("be", "vy"),
        ] {
            assert!(!source.contains(word), "population.rs mentions {word}");
        }
    }
}
