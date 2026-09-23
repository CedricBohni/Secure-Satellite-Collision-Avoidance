//! Tests for the distributed comparison function (DCF) from libfss.
//!
//! `DCFKey::gen(alpha, beta)` splits the function f(x) = beta if x < alpha, else 0
//! into two keys. Each party evaluates its key at x; the two outputs are additive
//! shares of f(x). Bit strings are big-endian (most significant bit first).

use fss::dcf::DCFKey;
use fss::{u32_to_bits_BE, BinElm, Group, RingElm};

/// Evaluates both keys at `x` and reconstructs the result by adding the shares.
fn reconstruct<T: Group + Clone + fss::prg::FromRng + std::fmt::Debug>(
    keys: &(DCFKey<T>, DCFKey<T>),
    x: &Vec<bool>,
) -> T {
    let mut result = keys.0.eval(x);
    result.add(&keys.1.eval(x));
    result
}

#[test]
fn ring_dcf_exhaustive_small_domain() {
    const NBITS: usize = 5;
    let beta = RingElm::from(117u32);

    for alpha in 0..(1u32 << NBITS) {
        let keys = DCFKey::gen(&u32_to_bits_BE(NBITS, alpha), &beta);
        for x in 0..(1u32 << NBITS) {
            let expected = if x < alpha { beta.clone() } else { RingElm::zero() };
            assert_eq!(
                reconstruct(&keys, &u32_to_bits_BE(NBITS, x)),
                expected,
                "alpha = {alpha}, x = {x}"
            );
        }
    }
}

#[test]
fn ring_dcf_32_bit_domain() {
    const NBITS: usize = 32;
    let beta = RingElm::from(1u32);
    let alpha = 0x8000_0000u32;
    let keys = DCFKey::gen(&u32_to_bits_BE(NBITS, alpha), &beta);

    for x in [0, 1, alpha - 1, alpha, alpha + 1, u32::MAX] {
        let expected = if x < alpha { beta.clone() } else { RingElm::zero() };
        assert_eq!(reconstruct(&keys, &u32_to_bits_BE(NBITS, x)), expected, "x = {x}");
    }
}

#[test]
fn single_key_does_not_reveal_output() {
    // One party's share alone should not equal the real output.
    let beta = RingElm::from(117u32);
    let keys = DCFKey::gen(&u32_to_bits_BE(8, 200), &beta);
    let x = u32_to_bits_BE(8, 10);

    assert_ne!(keys.0.eval(&x), beta);
    assert_ne!(keys.1.eval(&x), beta);
}

#[test]
fn binary_dcf_exhaustive_small_domain() {
    const NBITS: usize = 4;
    let beta = BinElm::from(true);

    for alpha in 0..(1u32 << NBITS) {
        let keys = DCFKey::gen(&u32_to_bits_BE(NBITS, alpha), &beta);
        for x in 0..(1u32 << NBITS) {
            let expected = BinElm::from(x < alpha);
            assert_eq!(
                reconstruct(&keys, &u32_to_bits_BE(NBITS, x)),
                expected,
                "alpha = {alpha}, x = {x}"
            );
        }
    }
}
