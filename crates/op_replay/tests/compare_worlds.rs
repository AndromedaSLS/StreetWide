//! `op_replay compare-worlds`: the gate's G9 and G10 measurements between
//! two versions of a world, for review.

mod common;

use common::{
    Temp, WALL_HOUSE, apply_unchecked, block_on_a_street, fixture_world, op_replay, read, removal,
    repo, sample_path_file, sample_replay, world_copy, world_id,
};
use op_replay::compare::{Comparison, compare_worlds};
use op_replay::gate::{blocked_detail, check_change};
use op_world::cjson;
use op_world::contrib::CheckReport;
use op_world::sample_change::SAMPLE_AUTHOR;
use serde_json::Value;

fn details(r: &CheckReport, id: &str) -> Vec<String> {
    r.checks
        .iter()
        .find(|c| c.id == id)
        .unwrap()
        .details
        .clone()
}

/// The world after the change set in `bundle`, applied to a copy of the
/// fixture world, and the gate's report on that change set.
fn after(t: &Temp, bundle: &std::path::Path) -> (std::path::PathBuf, CheckReport) {
    let report = check_change(
        &fixture_world(),
        bundle,
        &t.join("candidate"),
        &[sample_replay()],
    )
    .unwrap();
    let world = world_copy(t, "after");
    apply_unchecked(&world, bundle, &t.join("scratch"));
    (world, report)
}

/// For a single change set, the comparison measures what the gate does:
/// the same G9 details and the same G10 details.
fn assert_agrees_with_the_gate(c: &Comparison, report: &CheckReport) {
    let w = c.walkability.as_ref().unwrap();
    let g9 = if w.probes.is_empty() {
        vec!["no public way within 2 m of the change".to_string()]
    } else if w.newly_blocked.is_empty() {
        vec![format!(
            "{} probe points on public ways stay free",
            w.probes.len()
        )]
    } else {
        let mut d: Vec<String> = w.newly_blocked.iter().take(5).map(blocked_detail).collect();
        d.push(format!(
            "{} of {} probe points newly blocked",
            w.newly_blocked.len(),
            w.probes.len()
        ));
        d
    };
    assert_eq!(details(report, "G9"), g9);
    let mut g10: Vec<String> = c
        .replays
        .iter()
        .filter_map(|r| {
            let waypoints = r.waypoints.as_ref()?;
            Some(match r.missed().first() {
                Some(m) => format!(
                    "{}: at tick {} the player is {:.1} m off the recorded path",
                    r.name,
                    m.tick,
                    m.off_m()
                ),
                None => format!("{}: all {} waypoints reached", r.name, waypoints.len()),
            })
        })
        .collect();
    if g10.is_empty() {
        g10.push("no registered replay passes near the change".into());
    }
    assert_eq!(details(report, "G10"), g10);
}

#[test]
fn identical_worlds_have_nothing_to_compare() {
    let t = Temp::new("compare-same");
    let copy = world_copy(&t, "copy");
    let w = world_id(&fixture_world());
    for other in [fixture_world(), copy] {
        let c = compare_worlds(&fixture_world(), &other, &[sample_replay()]).unwrap();
        assert_eq!(
            (
                c.changed_assets,
                c.changed_colliders,
                c.region,
                c.findings()
            ),
            (0, 0, None, 0)
        );
        assert_eq!(c.walkability, None);
        assert_eq!(c.replays.len(), 1);
        assert_eq!(c.replays[0].waypoints, None);
        assert_eq!(
            c.lines(),
            [
                format!("worlds {w} -> {w}: no asset differs"),
                format!("{}: not played, as no collider differs", c.replays[0].name),
                "0 findings, for review".to_string(),
            ]
        );
    }
}

#[test]
fn a_block_on_a_street_newly_blocks_probe_points() {
    let t = Temp::new("compare-block");
    let bundle = t.join("block");
    block_on_a_street(&bundle);
    let (world, report) = after(&t, &bundle);
    let c = compare_worlds(&fixture_world(), &world, &[sample_replay()]).unwrap();
    assert_eq!((c.changed_assets, c.changed_colliders), (1, 1));
    let w = c.walkability.as_ref().unwrap();
    assert!(!w.newly_blocked.is_empty(), "{c:#?}");
    assert_agrees_with_the_gate(&c, &report);
    let lines = c.lines();
    assert!(
        lines.contains(&format!(
            "walkability: {} probe points on public ways, {} newly blocked",
            w.probes.len(),
            w.newly_blocked.len()
        )),
        "{lines:#?}"
    );
    for p in &w.newly_blocked {
        assert!(lines.contains(&format!("  {}", blocked_detail(p))));
    }
}

#[test]
fn removing_the_house_the_walk_runs_into_shows_drift() {
    let t = Temp::new("compare-house");
    let bundle = t.join("house");
    removal(&fixture_world(), &[WALL_HOUSE], SAMPLE_AUTHOR, &bundle);
    let (world, report) = after(&t, &bundle);
    let replays = [sample_replay()];
    let c = compare_worlds(&fixture_world(), &world, &replays).unwrap();
    assert_eq!((c.changed_assets, c.changed_colliders), (1, 1));
    assert!(
        c.walkability.as_ref().unwrap().newly_blocked.is_empty(),
        "a removal blocks no way"
    );
    let missed = c.replays[0].missed();
    assert!(!missed.is_empty());
    assert_eq!(c.findings(), missed.len());
    assert_agrees_with_the_gate(&c, &report);
    assert!(
        c.lines().contains(&format!(
            "{}: {} of {} waypoints more than 2 m off its path in the world before",
            c.replays[0].name,
            missed.len(),
            c.replays[0].waypoints.as_ref().unwrap().len()
        )),
        "{:#?}",
        c.lines()
    );

    // The report: canonical JSON, the same bytes every time, no local
    // paths, every off waypoint with its distance in whole millimetres.
    let bytes = c.to_bytes().unwrap();
    assert_eq!(
        compare_worlds(&fixture_world(), &world, &replays)
            .unwrap()
            .to_bytes()
            .unwrap(),
        bytes
    );
    let v: Value = cjson::from_canonical(&bytes).unwrap();
    assert_eq!(v["report"], "streetwide world comparison");
    assert_eq!(v["report_version"], "0.1.0");
    assert_eq!(v["before_world_id"], world_id(&fixture_world()).to_string());
    assert_eq!(v["after_world_id"], world_id(&world).to_string());
    assert_eq!(v["findings"], missed.len());
    assert_eq!(v["rules"]["waypoint_tolerance_mm"], 2000);
    let replay = &v["replays"][0];
    assert_eq!(replay["played"], true);
    assert_eq!(
        replay["replay_id"],
        op_world::ContentId::of(&read(sample_path_file())).to_string()
    );
    let off = replay["off"].as_array().unwrap();
    assert_eq!(off.len(), missed.len());
    for (o, m) in off.iter().zip(&missed) {
        assert_eq!(o["tick"], m.tick);
        let squares: i64 = (0..3)
            .map(|k| (m.base_mm[k] - m.candidate_mm[k]).pow(2))
            .sum();
        let mm = o["off_mm"].as_i64().unwrap();
        assert!(mm > 2_000 && mm * mm <= squares && squares < (mm + 1) * (mm + 1));
    }
    let text = String::from_utf8(bytes).unwrap();
    assert!(!text.contains('\\') && !text.contains("fixtures"), "{text}");
}

#[test]
fn the_command_line_exits_0_with_findings() {
    let t = Temp::new("compare-cli");
    let bundle = t.join("block");
    block_on_a_street(&bundle);
    let (world, _) = after(&t, &bundle);
    let report = t.join("comparison.json");
    let (code, out, err) = op_replay(&[
        &"compare-worlds",
        &fixture_world(),
        &world,
        &"--replay",
        &sample_path_file(),
        &"--report",
        &report,
    ]);
    assert_eq!(code, 0, "{err}");
    let c = compare_worlds(&fixture_world(), &world, &[sample_replay()]).unwrap();
    let bytes = read(&report);
    assert_eq!(bytes, c.to_bytes().unwrap());
    let mut lines = c.lines();
    let last = lines.pop().unwrap();
    lines.push(format!(
        "{last} (report {})",
        op_world::ContentId::of(&bytes)
    ));
    assert_eq!(out.lines().collect::<Vec<_>>(), lines);
    assert!(out.contains("newly blocked at"), "{out}");

    // The replays must have been recorded in the world before.
    let (code, _, err) = op_replay(&[
        &"compare-worlds",
        &world,
        &fixture_world(),
        &"--replay",
        &sample_path_file(),
    ]);
    assert_eq!(code, 2);
    assert!(err.contains("was recorded in world"), "{err}");
    let (code, _, err) = op_replay(&[&"compare-worlds", &world]);
    assert_eq!(code, 2);
    assert!(
        err.contains(
            "op_replay compare-worlds <before-dir> <after-dir> [--replay <file>]... [--report <file>]"
        ),
        "{err}"
    );
}

/// `--replay` takes a regular file of at most `MAX_REPLAY_BYTES`, as every
/// command that loads a replay does, and the error names it.
#[test]
fn a_replay_that_is_not_a_file_is_refused() {
    let t = Temp::new("compare-replay-dir");
    let dir = t.join("not-a-replay");
    std::fs::create_dir(&dir).unwrap();
    let (code, _, err) = op_replay(&[
        &"compare-worlds",
        &fixture_world(),
        &fixture_world(),
        &"--replay",
        &dir,
    ]);
    assert_eq!(
        (code, err.trim_end()),
        (
            2,
            format!("error: {}: not a regular file", dir.display()).as_str()
        )
    );
}

/// The district's water, added by the ingest (local stores, gitignored).
#[test]
fn the_district_water_needs_no_simulation() {
    let before = repo().join("data/world/zurich-altstadt-before-water");
    let after = repo().join("data/world/zurich-altstadt");
    if !before.join("world.json").is_file() || !after.join("world.json").is_file() {
        eprintln!("skipped: the local district stores are absent");
        return;
    }
    let c = compare_worlds(&before, &after, &[]).unwrap();
    assert_eq!(
        (c.changed_assets, c.changed_colliders, c.findings()),
        (8, 0, 0)
    );
    assert_eq!(c.walkability, None);
    assert_eq!(
        c.lines()[1],
        "no changed asset has a collider, so no probe point or replay can change"
    );
}
