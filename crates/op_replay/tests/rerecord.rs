//! `op_replay rerecord`: moving a replay to a new version of its world, as
//! RFC 0002 moves the replay suite after every merge.

mod common;

use common::{
    Temp, WALL_HOUSE, apply_unchecked, better, facade_sample, files_under, fixture_world,
    op_replay, read, removal, sample_path_file, sample_replay, world_copy, world_id,
};
use op_replay::{Recorder, Replay, Verdict, check, open_world, rerecord};
use op_world::contrib::Decision;
use op_world::merge::{baseline, merge};
use op_world::sample_change::SAMPLE_AUTHOR;

/// The fixture world with the sample facade merged in.
fn with_facade(t: &Temp) -> std::path::PathBuf {
    let world = world_copy(t, "world");
    let base = baseline(&world, "The ingest fixture.")
        .unwrap()
        .to_bytes()
        .unwrap();
    merge(
        &world,
        &facade_sample(),
        &base,
        Decision::Merged,
        vec![better()],
        "Test facade.",
        &t.join("scratch"),
    )
    .unwrap();
    world
}

fn checks_in(replay: &Replay, world: &std::path::Path) -> Verdict {
    let mut source = open_world(&replay.header, world).unwrap();
    check(replay, &mut source).unwrap()
}

#[test]
fn a_visual_change_keeps_every_checkpoint() {
    let t = Temp::new("rerecord-facade");
    let world = with_facade(&t);
    let sample = sample_replay();
    assert_ne!(sample.header.world_id, world_id(&world));

    let moved = rerecord(&sample, &world).unwrap();
    assert_eq!(
        moved.checkpoints, sample.checkpoints,
        "a facade has no collider"
    );
    assert_eq!(moved.inputs, sample.inputs);
    let mut header = sample.header.clone();
    header.world_id = world_id(&world);
    assert_eq!(moved.header, header, "only the world id changes");
    assert_eq!(
        Replay::from_bytes(&moved.to_bytes().unwrap()).unwrap(),
        moved
    );
    assert_eq!(
        checks_in(&moved, &world),
        Verdict::Match(sample.checkpoints.last().unwrap().hash)
    );
}

#[test]
fn a_changed_collider_gives_a_new_trail_for_the_new_world() {
    let t = Temp::new("rerecord-house");
    let world = world_copy(&t, "world");
    removal(&world, &[WALL_HOUSE], SAMPLE_AUTHOR, &t.join("bundle"));
    apply_unchecked(&world, &t.join("bundle"), &t.join("scratch"));
    let sample = sample_replay();

    let moved = rerecord(&sample, &world).unwrap();
    assert_eq!(moved.inputs, sample.inputs);
    assert_eq!(moved.checkpoints.len(), sample.checkpoints.len());
    assert_ne!(
        moved.checkpoints.last(),
        sample.checkpoints.last(),
        "the walk no longer stops at the wall"
    );
    assert_eq!(moved.header.world_id, world_id(&world));
    assert_eq!(
        checks_in(&moved, &world),
        Verdict::Match(moved.checkpoints.last().unwrap().hash)
    );
    assert!(
        open_world(&moved.header, &fixture_world()).is_err(),
        "it names the new world"
    );
}

#[test]
fn rerecord_keeps_the_header_spawn_seed_recorder_and_interval() {
    let sample = sample_replay();
    let mut custom = sample.clone();
    // One metre east of the world's spawn, facing north.
    custom.header.spawn.position_mm[0] += 1_000;
    custom.header.spawn.yaw = 0;
    custom.header.seed = 7;
    custom.header.recorder = Recorder {
        name: "test recorder".into(),
        version: "9.9.9".into(),
    };
    custom.header.checkpoint_interval = 30;
    // The old trail does not fit this header; rerecord makes a new one.
    let moved = rerecord(&custom, &fixture_world()).unwrap();
    assert_eq!(moved.header, custom.header);
    assert_ne!(
        moved.header.spawn,
        op_world::Store::new(fixture_world())
            .read_world()
            .unwrap()
            .1
            .spawn,
        "the header's spawn, not the world's"
    );
    assert_eq!(moved.checkpoints.len(), 1200 / 30);
    assert_eq!(
        checks_in(&moved, &fixture_world()),
        Verdict::Match(moved.checkpoints.last().unwrap().hash)
    );
}

#[test]
fn the_command_line_rerecords_in_place() {
    let t = Temp::new("rerecord-cli");
    let world = with_facade(&t);
    let suite = t.join("suite");
    std::fs::create_dir_all(&suite).unwrap();
    let file = suite.join("sample.oprep");
    std::fs::copy(sample_path_file(), &file).unwrap();

    let (code, out, err) = op_replay(&[&"rerecord", &file, &"--world", &world, &"--out", &file]);
    assert_eq!(code, 0, "{err}");
    let w1 = world_id(&world);
    assert_eq!(
        out.trim_end(),
        format!(
            "rerecorded in world {w1} (recorded in {}): 20 of 20 checkpoints unchanged, final state {}",
            sample_replay().header.world_id,
            sample_replay().checkpoints.last().unwrap().hash.hex()
        )
    );
    assert_eq!(
        files_under(&suite),
        ["sample.oprep"],
        "no temporary file left"
    );
    assert_eq!(
        Replay::from_bytes(&read(&file)).unwrap().header.world_id,
        w1
    );
    let (code, out, err) = op_replay(&[&"check", &file, &"--world", &world]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.starts_with("ok: 1200 ticks, 20 checkpoints match"),
        "{out}"
    );

    let (code, _, err) = op_replay(&[&"rerecord", &file, &"--world", &world]);
    assert_eq!(
        (code, err.trim_end()),
        (2, "error: --out <file> is required")
    );
    let (code, _, err) = op_replay(&[&"rerecord", &file, &"--out", &file]);
    assert_eq!(
        (code, err.trim_end()),
        (2, "error: --world <dir> is required")
    );
}

/// The replay is loaded like every replay (a regular file of at most
/// `MAX_REPLAY_BYTES`), and the output is written through a temporary file
/// made anew: a file a crash left there is replaced, a directory there
/// stops the command, and a link there is removed, never written through.
/// Found by review: the temporary file was opened with `fs::write`, which
/// follows links.
#[test]
fn rerecord_reads_and_writes_only_regular_files() {
    let t = Temp::new("rerecord-files");
    let world = fixture_world();
    let out = t.join("out.oprep");
    let rerecord = |replay: &std::path::Path| {
        op_replay(&[&"rerecord", &replay, &"--world", &world, &"--out", &out])
    };
    let not_a_file = t.join("not-a-replay");
    std::fs::create_dir(&not_a_file).unwrap();
    let (code, _, err) = rerecord(&not_a_file);
    assert_eq!(
        (code, err.trim_end()),
        (
            2,
            format!("error: {}: not a regular file", not_a_file.display()).as_str()
        )
    );
    assert!(!out.exists());

    let tmp = t.join("out.oprep.tmp");
    std::fs::write(&tmp, b"left by a crash").unwrap();
    let (code, _, err) = rerecord(&sample_path_file());
    assert_eq!(code, 0, "{err}");
    assert!(!tmp.exists());
    assert_eq!(
        read(&out),
        read(sample_path_file()),
        "rerecorded in its own world, the sample is unchanged"
    );

    std::fs::remove_file(&out).unwrap();
    std::fs::create_dir(&tmp).unwrap();
    let (code, _, err) = rerecord(&sample_path_file());
    assert_eq!(code, 2);
    assert!(
        err.starts_with(&format!("error: {}: ", out.display())),
        "{err}"
    );
    assert!(!out.exists());
    std::fs::remove_dir(&tmp).unwrap();

    // A rename that fails (a directory where the replay goes) leaves no
    // temporary file behind. Found by review: it stayed there.
    std::fs::create_dir_all(out.join("inside")).unwrap();
    let (code, _, err) = rerecord(&sample_path_file());
    assert_eq!(code, 2);
    assert!(
        err.starts_with(&format!("error: {}: ", out.display())),
        "{err}"
    );
    assert!(std::fs::symlink_metadata(&tmp).is_err(), "{err}");
    std::fs::remove_dir_all(&out).unwrap();

    #[cfg(unix)]
    {
        let outside = t.join("outside.txt");
        std::fs::write(&outside, b"not a replay").unwrap();
        std::os::unix::fs::symlink(&outside, &tmp).unwrap();
        let (code, _, err) = rerecord(&sample_path_file());
        assert_eq!(code, 0, "{err}");
        assert_eq!(read(&outside), b"not a replay");
        assert!(std::fs::symlink_metadata(&tmp).is_err());
        assert_eq!(read(&out), read(sample_path_file()));
    }
}
