//! Arithmetic in Z_{2^n} and the fixed-point encoding layered on top of it.
//!
//! Every wire of the circuit carries a `Ring` element.  A real number x is
//! represented by the ring element `round(x * 2^l)` taken modulo `2^n`, and a
//! ring element is read back as a two's-complement signed integer divided by
//! `2^l`.  Addition and multiplication of the encodings correspond to addition
//! and multiplication of the reals, except that a product carries `2 * l`
//! fractional bits and has to be brought back down by a truncation gate.

use std::fmt;
use std::ops::{Add, Mul, Neg, Sub};

use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::config::{l_bit, n_bit};

/// An element of Z_{2^n}, always stored reduced.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ring(u64);

/// Bit mask of the low `n` bits.
#[inline]
fn mask() -> u64 {
    let n = n_bit();
    if n >= 64 {
        u64::MAX
    } else {
        (1u64 << n) - 1
    }
}

impl Ring {
    pub const BYTES: usize = 8;

    /// Reduce a raw integer into the ring.
    #[inline]
    pub fn from_raw(value: u64) -> Self {
        Ring(value & mask())
    }

    #[inline]
    pub fn zero() -> Self {
        Ring(0)
    }

    /// The reduced representative in `0..2^n`.
    #[inline]
    pub fn raw(self) -> u64 {
        self.0
    }

    /// Uniformly random ring element, used for masks and Beaver triples.
    pub fn random<R: Rng>(rng: &mut R) -> Self {
        Ring::from_raw(rng.gen::<u64>())
    }

    /// The two's-complement reading of this element, in `-2^(n-1)..2^(n-1)`.
    pub fn to_signed(self) -> i128 {
        let n = n_bit();
        let v = self.0 as i128;
        if (self.0 >> (n - 1)) & 1 == 1 {
            v - (1i128 << n)
        } else {
            v
        }
    }

    /// Split into two uniformly random additive shares.
    pub fn share<R: Rng>(self, rng: &mut R) -> (Ring, Ring) {
        let s0 = Ring::random(rng);
        (s0, self - s0)
    }

    pub fn to_bytes(self) -> [u8; Ring::BYTES] {
        self.0.to_be_bytes()
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut buf = [0u8; Ring::BYTES];
        buf.copy_from_slice(bytes);
        Ring::from_raw(u64::from_be_bytes(buf))
    }
}

impl Add for Ring {
    type Output = Ring;
    #[inline]
    fn add(self, rhs: Ring) -> Ring {
        Ring::from_raw(self.0.wrapping_add(rhs.0))
    }
}

impl Sub for Ring {
    type Output = Ring;
    #[inline]
    fn sub(self, rhs: Ring) -> Ring {
        Ring::from_raw(self.0.wrapping_sub(rhs.0))
    }
}

impl Mul for Ring {
    type Output = Ring;
    #[inline]
    fn mul(self, rhs: Ring) -> Ring {
        Ring::from_raw(self.0.wrapping_mul(rhs.0))
    }
}

impl Neg for Ring {
    type Output = Ring;
    #[inline]
    fn neg(self) -> Ring {
        Ring::from_raw(self.0.wrapping_neg())
    }
}

impl fmt::Debug for Ring {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Ring(0x{:x})", self.0)
    }
}

/// Encode a real number as a fixed-point ring element.
///
/// Returns an error when the value does not fit into the configured integer
/// range, because silently wrapping would turn a large distance into a small
/// one without anybody noticing.
pub fn encode(x: f64) -> Result<Ring, FixedPointError> {
    if !x.is_finite() {
        return Err(FixedPointError::NotFinite(x));
    }
    let scaled = (x * (1u128 << l_bit()) as f64).round();
    // Both bounds are powers of two and therefore exact as f64, so this rejects
    // exactly the values that would wrap around the ring.
    let bound = (1u128 << (n_bit() - 1)) as f64;
    if scaled < -bound || scaled >= bound {
        return Err(FixedPointError::OutOfRange(x));
    }
    Ok(Ring::from_raw(scaled as i128 as u64))
}

/// Decode a fixed-point ring element back into a real number.
pub fn decode(value: Ring) -> f64 {
    value.to_signed() as f64 / (1u128 << l_bit()) as f64
}

/// Exclusive bound on the magnitude representable with the configured integer
/// bits: encoding succeeds exactly for `-max_magnitude() <= x < max_magnitude()`.
pub fn max_magnitude() -> f64 {
    (1u128 << (n_bit() - 1)) as f64 / (1u128 << l_bit()) as f64
}

#[derive(Debug)]
pub enum FixedPointError {
    NotFinite(f64),
    OutOfRange(f64),
}

impl fmt::Display for FixedPointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FixedPointError::NotFinite(x) => write!(f, "{x} is not a finite number"),
            FixedPointError::OutOfRange(x) => write!(
                f,
                "{x} does not fit the fixed-point range (-{0} <= x < {0})",
                max_magnitude()
            ),
        }
    }
}

impl std::error::Error for FixedPointError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    fn setup() {
        config::init_for_tests();
    }

    #[test]
    fn encoding_round_trips_within_the_resolution() {
        setup();
        for x in [0.0, 1.0, -1.0, 3.5, -1.25, 1234.5678, -98765.4321] {
            let back = decode(encode(x).unwrap());
            assert!((back - x).abs() < 1e-9, "{x} decoded as {back}");
        }
    }

    #[test]
    fn values_outside_the_integer_range_are_rejected() {
        setup();
        assert!(encode(max_magnitude()).is_err());
        assert!(encode(-max_magnitude() - 1.0).is_err());
        assert!(encode(f64::NAN).is_err());
    }

    #[test]
    fn shares_add_back_to_the_secret() {
        setup();
        let mut rng = rand::rngs::OsRng;
        for _ in 0..100 {
            let secret = Ring::random(&mut rng);
            let (s0, s1) = secret.share(&mut rng);
            assert_eq!(s0 + s1, secret);
        }
    }

    #[test]
    fn negative_values_decode_as_two_s_complement() {
        setup();
        let x = encode(-2.5).unwrap();
        assert!(x.to_signed() < 0);
        assert_eq!(decode(x), -2.5);
    }

    #[test]
    fn bytes_round_trip() {
        setup();
        let mut rng = rand::rngs::OsRng;
        let x = Ring::random(&mut rng);
        assert_eq!(Ring::from_bytes(&x.to_bytes()), x);
    }
}
