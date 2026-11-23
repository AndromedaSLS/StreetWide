//! The scripted test walk behind `fixtures/replays/sample.oprep`.
//!
//! In the ingest fixture world (Grossmünster, `fixtures/world`) the player
//! starts on the Grossmünsterplatz and, in 20 seconds: walks, turns, runs
//! into a wall, jumps and walks up a slope. [`Meter`] measures that each of
//! these really happened, so the fixture keeps exercising collision,
//! jumping and slopes if the world or the physics change.

use op_kernel::PlayerInput;
use op_kernel::input::buttons;
use op_sim::Sim;

/// Seed of the sample replay.
pub const SEED: u64 = 20_261_123;

/// One scripted segment: hold an input for some ticks.
#[derive(Clone, Copy, Debug)]
pub struct Segment {
    pub ticks: u32,
    /// Heading in degrees, clockwise from north.
    pub heading_deg: u32,
    pub forward: i8,
    pub buttons: u8,
}

const fn seg(ticks: u32, heading_deg: u32, forward: i8, buttons: u8) -> Segment {
    Segment {
        ticks,
        heading_deg,
        forward,
        buttons,
    }
}

/// The sample script: 20 s at 60 Hz, from the Grossmünsterplatz spawn
/// (facing west).
pub const SCRIPT: &[Segment] = &[
    // Fall the last metre onto the square.
    seg(60, 270, 0, 0),
    // Sprint west into the wall of the house about 11 m away; keep pushing.
    seg(240, 270, 127, SPRINT),
    // Jump in place and land.
    seg(1, 270, 0, JUMP),
    seg(59, 270, 0, 0),
    // Turn and sprint east-south-east across the square.
    seg(480, 103, 127, SPRINT),
    // Turn north and run up the lane east of the Grossmünster, about 3 m
    // uphill.
    seg(360, 0, 127, SPRINT),
];

pub fn heading(deg: u32) -> u16 {
    ((deg % 360) * 65_536 / 360) as u16
}

/// Expands segments into one input per tick. Turns between segments are
/// spread over the first 30 ticks of the next segment.
pub fn inputs(script: &[Segment]) -> Vec<PlayerInput> {
    let mut out = Vec::new();
    let mut yaw = heading(script.first().map_or(0, |s| s.heading_deg));
    for s in script {
        let target = heading(s.heading_deg);
        let turn = target.wrapping_sub(yaw) as i16;
        for t in 0..s.ticks {
            let k = (t + 1).min(30) as i32;
            let now = yaw.wrapping_add((i32::from(turn) * k / 30) as u16);
            out.push(PlayerInput {
                forward: s.forward,
                strafe: 0,
                yaw: now,
                pitch: 0,
                buttons: s.buttons,
            });
        }
        yaw = target;
    }
    out
}

/// What happened during a run.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Report {
    pub ticks: u64,
    /// Horizontal distance covered after first touching the ground (m).
    pub walked_m: f32,
    /// Ticks on the ground pushing forward but moving less than 20 % of it.
    pub blocked_ticks: u32,
    /// Ticks in the air after first touching the ground.
    pub airborne_ticks: u32,
    /// Height gained while on the ground (m).
    pub climbed_m: f32,
    /// Distinct headings used (in 1/65536 turns).
    pub headings: u32,
    pub final_feet: [f32; 3],
}

#[derive(Default)]
pub struct Meter {
    report: Report,
    landed: bool,
    last_yaw: Option<u16>,
}

impl Meter {
    pub fn observe(&mut self, sim: &Sim) {
        let p = sim.player();
        let m = sim.last_move();
        let r = &mut self.report;
        r.ticks = sim.ticks();
        r.final_feet = p.feet();
        if self.last_yaw != Some(p.yaw) {
            r.headings += 1;
            self.last_yaw = Some(p.yaw);
        }
        if p.grounded {
            self.landed = true;
        }
        if !self.landed {
            return;
        }
        let desired = (m.desired[0] * m.desired[0] + m.desired[2] * m.desired[2]).sqrt();
        let actual = (m.actual[0] * m.actual[0] + m.actual[2] * m.actual[2]).sqrt();
        r.walked_m += actual;
        if p.grounded && desired > 0.01 && actual < 0.2 * desired {
            r.blocked_ticks += 1;
        }
        if !p.grounded {
            r.airborne_ticks += 1;
        }
        if p.grounded && m.actual[1] > 0.0 {
            r.climbed_m += m.actual[1];
        }
    }

    pub fn report(&self) -> Report {
        self.report
    }
}

/// Inputs of the sample replay.
pub fn sample_inputs() -> Vec<PlayerInput> {
    inputs(SCRIPT)
}

pub const JUMP: u8 = buttons::JUMP;
pub const SPRINT: u8 = buttons::SPRINT;
