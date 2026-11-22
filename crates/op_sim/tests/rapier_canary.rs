//! Stage 1 canary for the pinned physics engine (ADR-0003, ADR-0005).
//!
//! Drops eight balls on a ground plate for 200 steps and checks that:
//! 1. two runs in the same process are bit-identical, and
//! 2. the result matches a golden digest recorded when the pin was set, so a
//!    different platform (CI runs Linux, development happens on Windows)
//!    producing different bits fails loudly.
//!
//! If the golden digest ever mismatches, do not update it to make the test
//! pass. Find the cause (feature flags, toolchain, target float behaviour)
//! first; see CLAUDE.md "Never weaken or skip a determinism test".

use rapier3d::prelude::*;

/// Final ball translations as raw f32 bit patterns.
fn run_scene() -> Vec<[u32; 3]> {
    let mut world = PhysicsWorld::new();
    world
        .colliders
        .insert(ColliderBuilder::cuboid(100.0, 0.1, 100.0));

    let mut handles = Vec::new();
    for x in 0..2u8 {
        for y in 0..2u8 {
            for z in 0..2u8 {
                let body = RigidBodyBuilder::dynamic().translation(Vector::new(
                    f32::from(x) * 0.51 + 3.0,
                    f32::from(y) * 0.5 + 3.5,
                    f32::from(z) * 0.5 + 3.0,
                ));
                let handle = world.bodies.insert(body);
                world.colliders.insert_with_parent(
                    ColliderBuilder::ball(0.5).restitution(0.7),
                    handle,
                    &mut world.bodies,
                );
                handles.push(handle);
            }
        }
    }

    for _ in 0..200 {
        world.step();
    }

    handles
        .iter()
        .map(|h| {
            let t = world.bodies[*h].translation();
            [t.x.to_bits(), t.y.to_bits(), t.z.to_bits()]
        })
        .collect()
}

/// FNV-1a over the bit patterns: tiny, dependency-free and order-sensitive.
fn digest(bits: &[[u32; 3]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for word in bits.iter().flatten() {
        for byte in word.to_le_bytes() {
            h ^= u64::from(byte);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// Recorded on x86_64-pc-windows-msvc with Rust 1.100.0 and rapier3d 0.36.1
/// (`enhanced-determinism`).
const GOLDEN_DIGEST: u64 = 0xc9e3_ca4f_0c0c_2bb1;

#[test]
fn same_process_runs_are_bit_identical() {
    assert_eq!(run_scene(), run_scene(), "two identical runs diverged");
}

#[test]
fn matches_golden_digest() {
    let d = digest(&run_scene());
    assert_eq!(
        d, GOLDEN_DIGEST,
        "rapier canary digest changed: got {d:#018x}, expected {GOLDEN_DIGEST:#018x}"
    );
}
