//! Deterministic math for simulation code.
//!
//! The standard library's `f32::sin` and friends call the platform's math
//! library, whose last bits differ between operating systems and CPUs, so
//! they are banned in simulation crates (`clippy.toml`). These wrappers use
//! the pure-Rust `libm` crate instead, which gives the same bits
//! everywhere. Basic arithmetic and `sqrt` are exactly rounded by IEEE 754
//! and need no wrapper.

use core::f64::consts::TAU;

pub fn sin(x: f32) -> f32 {
    libm::sinf(x)
}

pub fn cos(x: f32) -> f32 {
    libm::cosf(x)
}

pub fn atan2(y: f32, x: f32) -> f32 {
    libm::atan2f(y, x)
}

/// Radians of an angle given in 1/65536 turns.
pub fn turn16_to_radians(t: u16) -> f32 {
    (f64::from(t) * (TAU / 65_536.0)) as f32
}

/// Radians of a signed angle given in 1/65536 turns.
pub fn turn16s_to_radians(t: i16) -> f32 {
    (f64::from(t) * (TAU / 65_536.0)) as f32
}

/// `(sin, cos)` of a heading in 1/65536 turns. Quarter turns are exact.
pub fn heading(t: u16) -> (f32, f32) {
    match t {
        0 => (0.0, 1.0),
        16_384 => (1.0, 0.0),
        32_768 => (0.0, -1.0),
        49_152 => (-1.0, 0.0),
        _ => {
            let a = turn16_to_radians(t);
            (sin(a), cos(a))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarter_turns_are_exact() {
        assert_eq!(heading(0), (0.0, 1.0));
        assert_eq!(heading(16_384), (1.0, 0.0));
        assert_eq!(heading(49_152), (-1.0, 0.0));
    }

    #[test]
    fn libm_bits_are_pinned() {
        // libm is pure Rust: these bit patterns must hold on every platform.
        let (s, c) = heading(1000);
        let a = atan2(s, c);
        let got = (s.to_bits(), c.to_bits(), a.to_bits());
        assert_eq!(got, PINNED, "{got:#x?}");
        assert!((a - turn16_to_radians(1000)).abs() < 1e-6);
    }

    const PINNED: (u32, u32, u32) = (0x3dc4_0c84, 0x3f7e_d309, 0x3dc4_597c);
}
