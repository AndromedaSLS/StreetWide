//! The client's loading path (background threads, LRU cache, stalls)
//! must give exactly the same simulation as the synchronous store: the
//! sample replay's hash trail must reproduce through `StreamSource`.

use std::path::PathBuf;
use std::sync::Arc;

use op_kernel::TickOutcome;
use op_kernel::geometry::sim_origin_mm;
use op_replay::{Replay, new_sim};
use op_stream::{StreamSource, Streamer};

#[test]
fn streamed_geometry_reproduces_the_sample_trail() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let replay =
        Replay::from_bytes(&std::fs::read(repo.join("fixtures/replays/sample.oprep")).unwrap())
            .unwrap();
    // A tiny cache forces evictions and reloads while the player moves.
    let streamer = Arc::new(Streamer::open(&repo.join("fixtures/world"), 2, 64 * 1024).unwrap());
    assert_eq!(streamer.index().world_id(), replay.header.world_id);
    let mut source = StreamSource::new(streamer, sim_origin_mm(replay.header.spawn.position_mm));

    let mut sim = new_sim(&replay.header);
    let mut checkpoints = replay.checkpoints.iter();
    let mut stalls = 0u64;
    for input in &replay.inputs {
        loop {
            match sim.tick(input, &mut source).expect("tick") {
                TickOutcome::Advanced { tick, hash } => {
                    if tick % replay.header.checkpoint_interval == 0 || tick == replay.header.ticks
                    {
                        let want = checkpoints.next().expect("a checkpoint");
                        assert_eq!((want.tick, want.hash), (tick, hash), "diverged");
                    }
                    break;
                }
                TickOutcome::Stalled => {
                    stalls += 1;
                    std::thread::yield_now();
                }
            }
        }
    }
    assert!(checkpoints.next().is_none());
    assert!(stalls > 0, "the first tick waits for meshes");
}
