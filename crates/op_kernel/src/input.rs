//! Player input for one tick, quantised to integers.
//!
//! Inputs are what replays record, so they must be exact: no floats. The
//! client samples devices, quantises them into a [`PlayerInput`] and hands
//! one to the simulation per tick.

use std::fmt;

/// Size of one recorded input in bytes.
pub const RECORD_BYTES: usize = 8;

/// Largest movement axis value (full deflection).
pub const AXIS_MAX: i8 = 127;

/// Largest pitch magnitude: a quarter turn (straight up or down).
pub const PITCH_MAX: i16 = 16_384;

/// Button bits.
pub mod buttons {
    pub const JUMP: u8 = 1;
    pub const SPRINT: u8 = 1 << 1;
    /// Every defined bit. Other bits must be zero.
    pub const ALL: u8 = JUMP | SPRINT;
}

/// One tick of player input.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PlayerInput {
    /// Forward (+) / backward (-) movement, -127..=127.
    pub forward: i8,
    /// Right (+) / left (-) movement, -127..=127.
    pub strafe: i8,
    /// Heading in 1/65536 turns, 0 = north, clockwise seen from above.
    pub yaw: u16,
    /// Look pitch in 1/65536 turns, positive up, within +-[`PITCH_MAX`].
    pub pitch: i16,
    /// Pressed [`buttons`].
    pub buttons: u8,
}

/// Why bytes are not a valid input record.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InputError {
    AxisOutOfRange,
    PitchOutOfRange,
    UnknownButtons(u8),
    ReservedNotZero,
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InputError::AxisOutOfRange => write!(f, "movement axis is -128"),
            InputError::PitchOutOfRange => write!(f, "pitch beyond a quarter turn"),
            InputError::UnknownButtons(b) => write!(f, "unknown button bits {b:#04x}"),
            InputError::ReservedNotZero => write!(f, "reserved byte is not zero"),
        }
    }
}

impl std::error::Error for InputError {}

impl PlayerInput {
    pub fn pressed(&self, button: u8) -> bool {
        self.buttons & button != 0
    }

    pub fn check(&self) -> Result<(), InputError> {
        if self.forward == i8::MIN || self.strafe == i8::MIN {
            return Err(InputError::AxisOutOfRange);
        }
        if !(-PITCH_MAX..=PITCH_MAX).contains(&self.pitch) {
            return Err(InputError::PitchOutOfRange);
        }
        if self.buttons & !buttons::ALL != 0 {
            return Err(InputError::UnknownButtons(self.buttons));
        }
        Ok(())
    }

    /// The 8-byte record: forward, strafe, yaw (LE), pitch (LE), buttons,
    /// and a reserved zero byte.
    pub fn to_bytes(&self) -> [u8; RECORD_BYTES] {
        let [y0, y1] = self.yaw.to_le_bytes();
        let [p0, p1] = self.pitch.to_le_bytes();
        [
            self.forward.to_le_bytes()[0],
            self.strafe.to_le_bytes()[0],
            y0,
            y1,
            p0,
            p1,
            self.buttons,
            0,
        ]
    }

    pub fn from_bytes(b: [u8; RECORD_BYTES]) -> Result<Self, InputError> {
        if b[7] != 0 {
            return Err(InputError::ReservedNotZero);
        }
        let input = PlayerInput {
            forward: i8::from_le_bytes([b[0]]),
            strafe: i8::from_le_bytes([b[1]]),
            yaw: u16::from_le_bytes([b[2], b[3]]),
            pitch: i16::from_le_bytes([b[4], b[5]]),
            buttons: b[6],
        };
        input.check()?;
        Ok(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_is_little_endian() {
        let i = PlayerInput {
            forward: -127,
            strafe: 5,
            yaw: 0x1234,
            pitch: -2,
            buttons: buttons::JUMP | buttons::SPRINT,
        };
        let b = i.to_bytes();
        assert_eq!(b, [0x81, 5, 0x34, 0x12, 0xfe, 0xff, 3, 0]);
        assert_eq!(PlayerInput::from_bytes(b), Ok(i));
    }

    #[test]
    fn rejects_invalid_records() {
        let ok = PlayerInput::default().to_bytes();
        let mut b = ok;
        b[0] = 0x80;
        assert_eq!(PlayerInput::from_bytes(b), Err(InputError::AxisOutOfRange));
        let mut b = ok;
        b[6] = 4;
        assert_eq!(
            PlayerInput::from_bytes(b),
            Err(InputError::UnknownButtons(4))
        );
        let mut b = ok;
        b[7] = 1;
        assert_eq!(PlayerInput::from_bytes(b), Err(InputError::ReservedNotZero));
        let mut b = ok;
        b[4..6].copy_from_slice(&16_385i16.to_le_bytes());
        assert_eq!(PlayerInput::from_bytes(b), Err(InputError::PitchOutOfRange));
    }
}
