//! Pushing into a concave corner must not make the player oscillate.
//!
//! The corner is the one found in stage 4: at the top of the lane east of
//! the Grossmünster, a building's wall meets terrain steeper than the
//! walkable 45 degrees. With Rapier's character controller alone, the
//! capsule moved about 2 cm back and forth every tick there (ADR-0004,
//! ADR-0011).

use std::path::PathBuf;

use op_replay::synth::{SPRINT, Segment, inputs};
use op_replay::{Recorder, record};

#[test]
fn pushing_into_a_corner_does_not_oscillate() {
    let world = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/world");
    let script = [
        Segment {
            ticks: 60,
            heading_deg: 270,
            forward: 0,
            buttons: 0,
        },
        // Across the square, then up the lane into the corner at its top.
        Segment {
            ticks: 330,
            heading_deg: 108,
            forward: 127,
            buttons: SPRINT,
        },
        Segment {
            ticks: 600,
            heading_deg: 0,
            forward: 127,
            buttons: SPRINT,
        },
        // Turn round and walk back down.
        Segment {
            ticks: 120,
            heading_deg: 180,
            forward: 127,
            buttons: 0,
        },
    ];
    let mut previous: Option<[f32; 2]> = None;
    let (mut reversals, mut pushing_ticks) = (0, 0);
    let (mut in_corner, mut end) = ([0.0f32; 3], [0.0f32; 3]);
    record(
        &world,
        1,
        None,
        inputs(&script),
        Recorder {
            name: "test".into(),
            version: "0".into(),
        },
        |sim| {
            let m = sim.last_move().actual;
            let h = [m[0], m[2]];
            let tick = sim.ticks();
            // The last 200 ticks of pushing into the corner.
            if (791..=990).contains(&tick) {
                pushing_ticks += 1;
                if let Some(p) = previous {
                    let moving = (h[0] * h[0] + h[1] * h[1]).sqrt() > 0.001;
                    if moving && p[0] * h[0] + p[1] * h[1] < 0.0 {
                        reversals += 1;
                    }
                }
            }
            if tick == 990 {
                in_corner = sim.player().feet();
            }
            end = sim.player().feet();
            previous = Some(h);
        },
    )
    .unwrap();
    assert_eq!(pushing_ticks, 200);
    assert_eq!(
        reversals, 0,
        "the player reversed direction {reversals} times in the last 200 ticks"
    );
    let (dx, dz) = (end[0] - in_corner[0], end[2] - in_corner[2]);
    let left = (dx * dx + dz * dz).sqrt();
    assert!(left > 2.0, "walked only {left} m out of the corner in 2 s");
}
