//! Keyboard and mouse: first-person look, movement and session keys.
//!
//! Mouse look turns the camera every frame with full precision; the
//! simulation receives the heading quantised to 1/65536 turn in the input
//! latched for the next tick. While flying or on the map (`travel`), the
//! player gets no movement input and stands still.
//!
//! Movement uses key positions (W A S D on QWERTY, Z Q S D on AZERTY);
//! letter shortcuts in `travel` use the letters printed on the keys.

use std::f32::consts::{FRAC_PI_2, TAU};

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use op_kernel::PlayerInput;
use op_kernel::input::{PITCH_MAX, buttons};

use crate::Options;
use crate::session::{LatchedInput, Session};
use crate::travel::{Travel, View};

/// Radians per pixel of mouse movement.
const MOUSE_SENSITIVITY: f32 = 0.0025;

/// Where the player looks (radians). Yaw is clockwise from north.
#[derive(Resource, Default)]
pub struct Look {
    pub yaw: f32,
    pub pitch: f32,
}

pub fn yaw_to_turn16(yaw: f32) -> u16 {
    let t = (yaw / TAU).rem_euclid(1.0);
    ((t * 65_536.0).round() as u32 % 65_536) as u16
}

pub fn turn16_to_yaw(t: u16) -> f32 {
    f32::from(t) / 65_536.0 * TAU
}

/// Looking up or down stops just short of straight up or down.
pub fn clamp_pitch(pitch: f32) -> f32 {
    pitch.clamp(-FRAC_PI_2 * 0.98, FRAC_PI_2 * 0.98)
}

fn pitch_to_turn16(pitch: f32) -> i16 {
    ((pitch / TAU * 65_536.0).round() as i32).clamp(-i32::from(PITCH_MAX), i32::from(PITCH_MAX))
        as i16
}

pub struct ControlsPlugin;

impl Plugin for ControlsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Look>()
            .add_systems(Startup, init_look)
            .add_systems(Update, (grab_cursor, look_and_move, session_keys));
    }
}

fn init_look(session: Res<Session>, options: Res<Options>, mut look: ResMut<Look>) {
    look.yaw = turn16_to_yaw(session.spawn_yaw());
    look.pitch = crate::travel::start_pitch(&options);
}

fn grab_cursor(
    mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    travel: Res<Travel>,
) {
    // On the map, clicks choose where to go.
    if mouse.just_pressed(MouseButton::Left) && !matches!(travel.view, View::Map { .. }) {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
    if keys.just_pressed(KeyCode::Escape) {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

fn look_and_move(
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    motion: Res<AccumulatedMouseMotion>,
    keys: Res<ButtonInput<KeyCode>>,
    travel: Res<Travel>,
    mut look: ResMut<Look>,
    mut latched: ResMut<LatchedInput>,
) {
    let captured = cursor.grab_mode != CursorGrabMode::None;
    if travel.view != View::Walk {
        if captured {
            look.yaw = (look.yaw + motion.delta.x * MOUSE_SENSITIVITY).rem_euclid(TAU);
            look.pitch = clamp_pitch(look.pitch - motion.delta.y * MOUSE_SENSITIVITY);
        }
        // The player waits where it is.
        latched.input = PlayerInput {
            yaw: latched.input.yaw,
            pitch: latched.input.pitch,
            ..PlayerInput::default()
        };
        latched.jump_pending = false;
        return;
    }
    if captured {
        look.yaw = (look.yaw + motion.delta.x * MOUSE_SENSITIVITY).rem_euclid(TAU);
        look.pitch = clamp_pitch(look.pitch - motion.delta.y * MOUSE_SENSITIVITY);
    }
    let axis = |plus: KeyCode, minus: KeyCode| -> i8 {
        match (keys.pressed(plus), keys.pressed(minus)) {
            (true, false) => 127,
            (false, true) => -127,
            _ => 0,
        }
    };
    let mut b = 0;
    if keys.pressed(KeyCode::Space) {
        b |= buttons::JUMP;
    }
    if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
        b |= buttons::SPRINT;
    }
    if keys.just_pressed(KeyCode::Space) {
        latched.jump_pending = true;
    }
    latched.input = PlayerInput {
        forward: axis(KeyCode::KeyW, KeyCode::KeyS),
        strafe: axis(KeyCode::KeyD, KeyCode::KeyA),
        yaw: yaw_to_turn16(look.yaw),
        pitch: pitch_to_turn16(look.pitch),
        buttons: b,
    };
}

fn session_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut session: ResMut<Session>,
    mut look: ResMut<Look>,
    mut travel: ResMut<Travel>,
) {
    if session.is_playback() {
        return;
    }
    if keys.just_pressed(KeyCode::F9) {
        let msg = match session.save_replay(std::path::Path::new("replays")) {
            Ok(path) => format!("Saved {}", path.display()),
            Err(e) => format!("Could not save the replay: {e}"),
        };
        info!("{msg}");
        session.message = Some((msg, 5.0));
    }
    if keys.just_pressed(KeyCode::KeyR) {
        session.restart();
        look.yaw = turn16_to_yaw(session.spawn_yaw());
        look.pitch = 0.0;
        travel.view = View::Walk;
        travel.pending = None;
        travel.cut = true;
        session.message = Some(("Back at the spawn point".into(), 3.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_quantisation_round_trips() {
        for t in [0u16, 1, 16_384, 49_152, 65_535] {
            assert_eq!(yaw_to_turn16(turn16_to_yaw(t)), t);
        }
        assert_eq!(yaw_to_turn16(-0.0001), 65_535);
        assert_eq!(pitch_to_turn16(10.0), PITCH_MAX);
    }
}
