//! The benchmark's report (ADR-0018): statistics per stop and over the
//! whole route, the machine, the build and the settings, as JSON with
//! sorted keys, and a short summary for the terminal.
//!
//! Percentiles are nearest-rank: the p-th percentile of n values is the
//! value at rank ceil(p/100 × n) in ascending order, so it is always one
//! of the measured values. Times are in milliseconds, rounded to 1 µs.
//! Every stop has the same keys; what was not measured is `null`.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::route::Stop;

pub const FORMAT: &str = "streetwide.benchmark";
pub const VERSION: u64 = 1;
/// ADR-0018's budget: the 99th percentile frame time at every stop, on
/// the high preset (60 frames per second).
pub const BUDGET_P99_MS: f64 = 16.7;

/// The value at a quantile `q` (0 to 1) of values sorted in ascending
/// order, by nearest rank; `None` if there are none.
pub fn percentile(sorted: &[f64], q: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (q * sorted.len() as f64).ceil() as usize;
    Some(sorted[rank.clamp(1, sorted.len()) - 1])
}

/// Statistics of a series of times.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    pub n: usize,
    pub min: f64,
    pub median: f64,
    pub mean: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

impl Stats {
    pub fn of(values: &[f64]) -> Option<Stats> {
        let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
        if v.is_empty() {
            return None;
        }
        v.sort_by(f64::total_cmp);
        let at = |q| percentile(&v, q).unwrap_or(f64::NAN);
        Some(Stats {
            n: v.len(),
            min: v[0],
            median: at(0.5),
            mean: v.iter().sum::<f64>() / v.len() as f64,
            p95: at(0.95),
            p99: at(0.99),
            max: v[v.len() - 1],
        })
    }

    fn json(&self) -> Value {
        json!({
            "n": self.n,
            "min": ms(self.min),
            "median": ms(self.median),
            "mean": ms(self.mean),
            "p95": ms(self.p95),
            "p99": ms(self.p99),
            "max": ms(self.max),
        })
    }
}

/// A time in ms, rounded to 1 µs.
fn ms(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn stats_json(values: &[f64]) -> Value {
    Stats::of(values).map_or(Value::Null, |s| s.json())
}

/// GPU time per frame: the sum of the top-level render passes, and each
/// pass (Bevy's render diagnostics, Vulkan and DX12 only).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuTimes {
    pub total: Vec<f64>,
    /// By pass, as Bevy names it (`main_opaque_pass_3d`, nested passes as
    /// `parent/child`).
    pub passes: BTreeMap<String, Vec<f64>>,
}

impl GpuTimes {
    fn json(&self) -> Value {
        if self.total.is_empty() && self.passes.is_empty() {
            return Value::Null;
        }
        let passes: serde_json::Map<String, Value> = self
            .passes
            .iter()
            .map(|(name, v)| (name.clone(), stats_json(v)))
            .collect();
        json!({"total": stats_json(&self.total), "passes": passes})
    }

    fn extend(&mut self, other: &GpuTimes) {
        self.total.extend(&other.total);
        for (name, v) in &other.passes {
            self.passes.entry(name.clone()).or_default().extend(v);
        }
    }
}

/// What one stop gave.
#[derive(Clone, Debug, PartialEq)]
pub struct StopResult {
    pub stop: Stop,
    /// Why it was not measured.
    pub error: Option<String>,
    /// From leaving the previous stop until every mesh in range was shown.
    pub loaded_after_s: Option<f64>,
    pub sun_elevation_deg: Option<f64>,
    pub meshes_shown: Option<usize>,
    /// The window's size in physical pixels during the turn.
    pub resolution: Option<[u32; 2]>,
    pub screenshot: Option<String>,
    /// Time from one frame to the next, for every frame of the turn.
    pub frame_ms: Vec<f64>,
    /// The main update's CPU time (Bevy's `First` to `Last`) per frame.
    pub main_update_ms: Vec<f64>,
    pub gpu: GpuTimes,
}

impl StopResult {
    #[cfg(test)]
    fn failed(stop: Stop, error: String) -> StopResult {
        StopResult {
            error: Some(error),
            ..StopResult::new(stop)
        }
    }

    /// A stop before anything is measured.
    pub fn new(stop: Stop) -> StopResult {
        StopResult {
            stop,
            error: None,
            loaded_after_s: None,
            sun_elevation_deg: None,
            meshes_shown: None,
            resolution: None,
            screenshot: None,
            frame_ms: Vec::new(),
            main_update_ms: Vec::new(),
            gpu: GpuTimes::default(),
        }
    }

    pub fn measured(&self) -> bool {
        self.error.is_none() && !self.frame_ms.is_empty()
    }

    /// The 99th percentile frame time of a measured stop. A stop that
    /// failed part way has some frames, which do not count.
    fn p99(&self) -> Option<f64> {
        if !self.measured() {
            return None;
        }
        Stats::of(&self.frame_ms).map(|s| s.p99)
    }
}

/// The GPU as wgpu reports it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gpu {
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub device_type: String,
    pub backend: String,
    pub driver: String,
    pub driver_info: String,
}

/// The machine, as far as Bevy and wgpu report it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Machine {
    pub gpu: Option<Gpu>,
    pub cpu: Option<String>,
    pub cpu_cores: Option<String>,
    pub memory: Option<String>,
    pub os: Option<String>,
    pub kernel: Option<String>,
}

/// The build the client runs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Build {
    /// The Git commit at build time, if known.
    pub commit: Option<String>,
    /// Whether the code differed from that commit, if known.
    pub dirty: Option<bool>,
    /// Cargo's profile (`release` or `debug`) and optimisation level.
    pub profile: String,
    pub opt_level: String,
    pub version: String,
}

/// The run's settings and context.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub route_file: String,
    pub route_world: String,
    pub world_name: String,
    pub world_id: String,
    pub preset: String,
    /// The window size asked for, in physical pixels.
    pub resolution: [u32; 2],
    pub present_mode: String,
    pub turn_s: f64,
    pub settle_s: f64,
    pub started_utc: String,
    pub machine: Machine,
    pub build: Build,
}

fn opt<T: Into<Value>>(v: Option<T>) -> Value {
    v.map_or(Value::Null, Into::into)
}

fn stop_json(r: &StopResult) -> Value {
    let s = &r.stop;
    let p99 = r.p99();
    json!({
        "name": s.name,
        "place": s.place,
        "lon": s.lon,
        "lat": s.lat,
        "yaw_deg": s.yaw_deg,
        "date": s.date(),
        "date_given": s.date_given,
        "day_type": s.day_type.name(),
        "time": s.time(),
        "measured": r.measured(),
        "error": r.error,
        "loaded_after_s": opt(r.loaded_after_s.map(|x| (x * 100.0).round() / 100.0)),
        "sun_elevation_deg": opt(r.sun_elevation_deg.map(|x| (x * 10.0).round() / 10.0)),
        "meshes_shown": opt(r.meshes_shown),
        "resolution": opt(r.resolution.map(|[w, h]| json!([w, h]))),
        "screenshot": r.screenshot,
        "frames": r.frame_ms.len(),
        "frame_ms": stats_json(&r.frame_ms),
        "main_update_ms": stats_json(&r.main_update_ms),
        "gpu_ms": r.gpu.json(),
        "frame_p99_within_budget": opt(p99.map(|p| p <= BUDGET_P99_MS)),
        "frame_log_ms": r.frame_ms.iter().map(|&x| ms(x)).collect::<Vec<_>>(),
    })
}

/// The whole report.
pub fn report(run: &Run, stops: &[StopResult]) -> Value {
    let measured: Vec<&StopResult> = stops.iter().filter(|r| r.measured()).collect();
    let mut frames = Vec::new();
    let mut main = Vec::new();
    let mut gpu = GpuTimes::default();
    for r in &measured {
        frames.extend(&r.frame_ms);
        main.extend(&r.main_update_ms);
        gpu.extend(&r.gpu);
    }
    let over: Vec<&str> = stops
        .iter()
        .filter(|r| r.p99().is_some_and(|p| p > BUDGET_P99_MS))
        .map(|r| r.stop.name.as_str())
        .collect();
    let not_measured: Vec<&str> = stops
        .iter()
        .filter(|r| !r.measured())
        .map(|r| r.stop.name.as_str())
        .collect();
    let m = &run.machine;
    let b = &run.build;
    json!({
        "format": FORMAT,
        "version": VERSION,
        "started_utc": run.started_utc,
        "route": {
            "file": run.route_file,
            "world": run.route_world,
            "stops": stops.len(),
        },
        "world": {"name": run.world_name, "id": run.world_id},
        "preset": run.preset,
        "resolution": run.resolution,
        "present_mode": run.present_mode,
        "turn_s": run.turn_s,
        "settle_s": run.settle_s,
        "percentiles": "nearest rank",
        "machine": {
            "gpu": m.gpu.as_ref().map(|g| json!({
                "name": g.name,
                "vendor_id": format!("0x{:04x}", g.vendor_id),
                "device_id": format!("0x{:04x}", g.device_id),
                "device_type": g.device_type,
                "backend": g.backend,
                "driver": g.driver,
                "driver_info": g.driver_info,
            })),
            "cpu": m.cpu,
            "cpu_cores": m.cpu_cores,
            "memory": m.memory,
            "os": m.os,
            "kernel": m.kernel,
        },
        "build": {
            "commit": b.commit,
            "dirty": b.dirty,
            "profile": b.profile,
            "opt_level": b.opt_level,
            "version": b.version,
        },
        "budget": {
            "frame_p99_ms": BUDGET_P99_MS,
            "preset": "high",
            "applies": run.preset == "high",
            "every_stop_within": over.is_empty() && not_measured.is_empty(),
            "stops_over": over,
            "stops_not_measured": not_measured,
        },
        "overall": {
            "stops_measured": measured.len(),
            "frames": frames.len(),
            "frame_ms": stats_json(&frames),
            "main_update_ms": stats_json(&main),
            "gpu_ms": gpu.json(),
        },
        "stops": stops.iter().map(stop_json).collect::<Vec<_>>(),
    })
}

/// A number from the report, as text with two decimals, or `-`.
fn num(v: &Value) -> String {
    v.as_f64().map_or("-".into(), |x| format!("{x:.2}"))
}

fn row(label: &str, frames: &Value, gpu: &Value, n: &Value, verdict: &str) -> String {
    format!(
        "  {label:<38} {:>7} {:>7} {:>7} {:>7} {:>8} {:>6}  {verdict}",
        num(&frames["median"]),
        num(&frames["p95"]),
        num(&frames["p99"]),
        num(&frames["max"]),
        num(&gpu["total"]["median"]),
        n.as_u64().map_or("-".into(), |n| n.to_string()),
    )
}

/// A short summary of a report for the terminal.
pub fn summary(report: &Value) -> String {
    let m = &report["machine"];
    let gpu = &m["gpu"];
    let b = &report["build"];
    let commit = match (b["commit"].as_str(), b["dirty"].as_bool()) {
        (Some(c), Some(true)) => format!("{} with changes", &c[..c.len().min(12)]),
        (Some(c), _) => c[..c.len().min(12)].to_string(),
        (None, _) => "no commit".into(),
    };
    let world_id = report["world"]["id"].as_str().unwrap_or("");
    let mut lines = vec![
        format!(
            "StreetWide benchmark: {} on {} (world {}), preset {}, {} x {}",
            report["route"]["file"].as_str().unwrap_or(""),
            report["world"]["name"].as_str().unwrap_or(""),
            &world_id[..world_id.len().min(8)],
            report["preset"].as_str().unwrap_or(""),
            report["resolution"][0],
            report["resolution"][1],
        ),
        format!(
            "  GPU {} ({}, driver {} {}); CPU {}; {}; build {commit}, {}",
            gpu["name"].as_str().unwrap_or("unknown"),
            gpu["backend"].as_str().unwrap_or("?"),
            gpu["driver"].as_str().unwrap_or("?"),
            gpu["driver_info"].as_str().unwrap_or(""),
            m["cpu"].as_str().unwrap_or("unknown"),
            m["memory"].as_str().unwrap_or("memory unknown"),
            b["profile"].as_str().unwrap_or("?"),
        ),
        format!(
            "  {:<38} {:>7} {:>7} {:>7} {:>7} {:>8} {:>6}",
            "Frame time (ms)", "median", "p95", "p99", "worst", "GPU med", "frames"
        ),
    ];
    for (i, s) in report["stops"].as_array().into_iter().flatten().enumerate() {
        let label = format!("{} {}", i + 1, s["name"].as_str().unwrap_or(""));
        let verdict = match (s["error"].as_str(), s["frame_p99_within_budget"].as_bool()) {
            (Some(e), _) => format!("not measured: {e}"),
            (None, Some(true)) => "within 16.7".into(),
            (None, Some(false)) => "OVER 16.7".into(),
            (None, None) => "not measured".into(),
        };
        lines.push(row(
            &label,
            &s["frame_ms"],
            &s["gpu_ms"],
            &s["frames"],
            &verdict,
        ));
    }
    let o = &report["overall"];
    lines.push(row(
        "All stops",
        &o["frame_ms"],
        &o["gpu_ms"],
        &o["frames"],
        "",
    ));
    let budget = &report["budget"];
    let count = |key: &str| budget[key].as_array().map_or(0, Vec::len);
    let (over, missing) = (count("stops_over"), count("stops_not_measured"));
    let stops = |n: usize| format!("{n} stop{}", if n == 1 { "" } else { "s" });
    let verdict = match (over, missing) {
        (0, 0) => "every stop within 16.7 ms".to_string(),
        (0, m) => format!("not shown, {} not measured", stops(m)),
        (o, 0) => format!("{} OVER 16.7 ms", stops(o)),
        (o, m) => format!("{} OVER 16.7 ms, {m} not measured", stops(o)),
    };
    lines.push(if budget["applies"].as_bool() == Some(true) {
        format!(
            "Budget (99th percentile at or under 16.7 ms at every stop, high preset): {}",
            if (over, missing) == (0, 0) {
                "met".to_string()
            } else {
                format!("not met: {verdict}")
            }
        )
    } else {
        format!("Budget: applies to the high preset only; on this preset: {verdict}")
    });
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::benchmark::route::DayType;
    use crate::geo::DateTime;

    #[test]
    fn percentiles_are_nearest_rank() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&v, 0.5), Some(50.0));
        assert_eq!(percentile(&v, 0.95), Some(95.0));
        assert_eq!(percentile(&v, 0.99), Some(99.0));
        assert_eq!(percentile(&v, 1.0), Some(100.0));
        assert_eq!(percentile(&v, 0.0), Some(1.0));
        // Ten values: the 99th percentile is the worst, the median the 5th.
        let ten: Vec<f64> = (1..=10).map(f64::from).collect();
        assert_eq!(percentile(&ten, 0.99), Some(10.0));
        assert_eq!(percentile(&ten, 0.95), Some(10.0));
        assert_eq!(percentile(&ten, 0.5), Some(5.0));
        // 1,000 values: the 99th percentile is the 990th, ten frames below
        // the worst.
        let k: Vec<f64> = (1..=1000).map(f64::from).collect();
        assert_eq!(percentile(&k, 0.99), Some(990.0));
        assert_eq!(percentile(&[7.0], 0.99), Some(7.0));
        assert_eq!(percentile(&[], 0.5), None);
    }

    #[test]
    fn stats_sort_their_values() {
        let s = Stats::of(&[5.0, 1.0, 3.0, f64::NAN, 2.0, 4.0]).unwrap();
        assert_eq!(
            s,
            Stats {
                n: 5,
                min: 1.0,
                median: 3.0,
                mean: 3.0,
                p95: 5.0,
                p99: 5.0,
                max: 5.0
            }
        );
        assert_eq!(Stats::of(&[]), None);
        assert_eq!(stats_json(&[]), Value::Null);
        // Rounded to 1 µs.
        assert_eq!(stats_json(&[1.234_567_8])["median"], json!(1.235));
    }

    fn stop(name: &str) -> Stop {
        Stop {
            name: name.into(),
            place: Some("Lindenhof".into()),
            lon: 8.5407,
            lat: 47.37296,
            yaw_deg: 90.0,
            clock: DateTime {
                year: 2026,
                month: 6,
                day: 21,
                minute: 18 * 60,
            },
            day_type: DayType::Sunday,
            date_given: "2026-06-21".into(),
        }
    }

    fn run(preset: &str) -> Run {
        Run {
            route_file: "worlds/zurich-altstadt.route.json".into(),
            route_world: "zurich-altstadt".into(),
            world_name: "zurich-altstadt".into(),
            world_id: "4689c972".into(),
            preset: preset.into(),
            resolution: [1920, 1080],
            present_mode: "AutoNoVsync".into(),
            turn_s: 20.0,
            settle_s: 3.0,
            started_utc: "2026-11-25T12:00:00Z".into(),
            machine: Machine {
                gpu: Some(Gpu {
                    name: "A GPU".into(),
                    vendor_id: 0x10de,
                    device_id: 0x2484,
                    device_type: "DiscreteGpu".into(),
                    backend: "Vulkan".into(),
                    driver: "NVIDIA".into(),
                    driver_info: "1.0".into(),
                }),
                cpu: Some("A CPU".into()),
                cpu_cores: Some("12".into()),
                memory: Some("31.9 GiB".into()),
                os: Some("Windows 11".into()),
                kernel: Some("22000".into()),
            },
            build: Build {
                commit: Some("225d35c".into()),
                dirty: Some(false),
                profile: "release".into(),
                opt_level: "3".into(),
                version: "0.1.0".into(),
            },
        }
    }

    fn measured(name: &str, frame: f64) -> StopResult {
        let gpu = GpuTimes {
            total: vec![4.0, 4.5],
            passes: BTreeMap::from([("main_opaque_pass_3d".to_string(), vec![2.0, 2.5])]),
        };
        StopResult {
            stop: stop(name),
            error: None,
            loaded_after_s: Some(2.345),
            sun_elevation_deg: Some(31.64),
            meshes_shown: Some(1500),
            resolution: Some([1920, 1080]),
            screenshot: None,
            frame_ms: (0..200).map(|i| frame + f64::from(i) * 0.01).collect(),
            main_update_ms: vec![1.0; 200],
            gpu,
        }
    }

    /// Every key of every object, as paths, so that two reports can be
    /// compared by their structure alone.
    fn keys(v: &Value, path: &str, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                for (k, x) in o {
                    let p = format!("{path}/{k}");
                    out.push(p.clone());
                    keys(x, &p, out);
                }
            }
            Value::Array(a) => {
                for (i, x) in a.iter().enumerate() {
                    keys(x, &format!("{path}[{i}]"), out);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn reports_have_one_shape_with_sorted_keys() {
        let stops = vec![
            measured("Lindenhof, 21 June, 18:00", 6.0),
            StopResult::failed(stop("Nowhere"), "could not land".into()),
            measured("Hirschenplatz, 21 June, 18:00", 15.0),
        ];
        let r = report(&run("high"), &stops);
        // Sorted keys, at every level, also once written out.
        let text = serde_json::to_string_pretty(&r).unwrap();
        let top: Vec<&str> = r.as_object().unwrap().keys().map(String::as_str).collect();
        let mut sorted = top.clone();
        sorted.sort_unstable();
        assert_eq!(top, sorted);
        assert!(text.find("\"budget\"").unwrap() < text.find("\"build\"").unwrap());
        assert!(text.find("\"build\"").unwrap() < text.find("\"format\"").unwrap());
        for k in [
            "budget",
            "build",
            "format",
            "machine",
            "overall",
            "preset",
            "resolution",
            "route",
            "stops",
            "world",
        ] {
            assert!(top.contains(&k), "{k}");
        }
        assert_eq!(r["format"], "streetwide.benchmark");
        assert_eq!(r["world"]["id"], "4689c972");
        assert_eq!(r["build"]["commit"], "225d35c");
        assert_eq!(r["machine"]["gpu"]["vendor_id"], "0x10de");
        assert_eq!(r["resolution"], json!([1920, 1080]));
        // Every stop has the same keys, measured or not.
        let shape = |s: &Value| {
            let mut out = Vec::new();
            keys(&json!({"s": s}), "", &mut out);
            out.retain(|p| p.matches('/').count() <= 2);
            out
        };
        let stops_json = r["stops"].as_array().unwrap();
        assert_eq!(stops_json.len(), 3);
        assert_eq!(shape(&stops_json[0]), shape(&stops_json[1]));
        for k in ["median", "p95", "p99", "max"] {
            assert!(stops_json[0]["frame_ms"][k].is_number(), "{k}");
            assert!(r["overall"]["frame_ms"][k].is_number(), "{k}");
        }
        assert_eq!(stops_json[0]["frame_ms"]["median"], json!(6.99));
        assert_eq!(stops_json[0]["frame_ms"]["p99"], json!(7.97));
        assert_eq!(stops_json[0]["frame_ms"]["max"], json!(7.99));
        assert_eq!(stops_json[0]["frame_p99_within_budget"], true);
        assert_eq!(
            stops_json[0]["gpu_ms"]["passes"]["main_opaque_pass_3d"]["max"],
            json!(2.5)
        );
        assert_eq!(stops_json[0]["frame_log_ms"].as_array().unwrap().len(), 200);
        assert_eq!(stops_json[1]["measured"], false);
        assert!(stops_json[1]["frame_ms"].is_null() && stops_json[1]["gpu_ms"].is_null());
        assert_eq!(stops_json[2]["frame_p99_within_budget"], false);
        // Overall pools the measured stops' frames.
        assert_eq!(r["overall"]["frames"], 400);
        assert_eq!(r["overall"]["stops_measured"], 2);
        assert_eq!(r["overall"]["frame_ms"]["max"], json!(16.99));
        assert_eq!(r["overall"]["gpu_ms"]["total"]["n"], 4);
        assert_eq!(
            r["budget"]["stops_over"],
            json!(["Hirschenplatz, 21 June, 18:00"])
        );
        assert_eq!(r["budget"]["stops_not_measured"], json!(["Nowhere"]));
        assert_eq!(r["budget"]["every_stop_within"], false);
        assert_eq!(r["budget"]["applies"], true);
        // Other values, the same structure.
        let other = report(
            &run("fast"),
            &[
                measured("Lindenhof, 21 June, 18:00", 3.0),
                StopResult::failed(stop("Nowhere"), "x".into()),
                measured("Hirschenplatz, 21 June, 18:00", 4.0),
            ],
        );
        let (mut a, mut b) = (Vec::new(), Vec::new());
        keys(&r, "", &mut a);
        keys(&other, "", &mut b);
        assert_eq!(a, b);
        assert_eq!(other["budget"]["applies"], false);
        assert_eq!(other["budget"]["every_stop_within"], false);
    }

    #[test]
    fn the_summary_names_each_stop_and_the_budget() {
        let stops = vec![
            measured("Lindenhof, 21 June, 18:00", 6.0),
            measured("Hirschenplatz, 21 June, 18:00", 15.0),
        ];
        let text = summary(&report(&run("high"), &stops));
        assert!(text.contains("1 Lindenhof, 21 June, 18:00"), "{text}");
        assert!(
            text.contains("within 16.7") && text.contains("OVER 16.7"),
            "{text}"
        );
        assert!(text.contains("All stops"), "{text}");
        assert!(
            text.contains("high preset): not met: 1 stop OVER 16.7 ms"),
            "{text}"
        );
        assert!(text.contains("GPU A GPU (Vulkan"), "{text}");
        let met = summary(&report(&run("high"), &stops[..1]));
        assert!(met.contains("high preset): met"), "{met}");
        let fast = summary(&report(&run("fast"), &stops[..1]));
        assert!(
            fast.contains("applies to the high preset only; on this preset: every stop within"),
            "{fast}"
        );
    }

    /// A stop that fails during its turn (its window minimised) keeps the
    /// frames it had, for a look, but is not measured: it is not within
    /// the budget, and the whole route's figures leave it out.
    #[test]
    fn a_stop_that_fails_part_way_does_not_count() {
        let mut partial = measured("Hirschenplatz, 21 June, 18:00", 1.0);
        partial.error = Some("the window was 0 x 0 pixels".into());
        let r = report(
            &run("high"),
            &[measured("Lindenhof, 21 June, 18:00", 6.0), partial],
        );
        let s = &r["stops"][1];
        assert_eq!(s["measured"], false);
        assert_eq!(s["frames"], 200);
        assert!(s["frame_p99_within_budget"].is_null());
        assert_eq!(r["overall"]["frames"], 200);
        assert_eq!(r["overall"]["stops_measured"], 1);
        assert_eq!(r["budget"]["stops_over"], json!([]));
        assert_eq!(
            r["budget"]["stops_not_measured"],
            json!(["Hirschenplatz, 21 June, 18:00"])
        );
        assert_eq!(r["budget"]["every_stop_within"], false);
        let text = summary(&r);
        assert!(
            text.contains("not measured: the window was 0 x 0 pixels"),
            "{text}"
        );
        assert!(
            text.contains("high preset): not met: not shown, 1 stop not measured"),
            "{text}"
        );
    }
}
