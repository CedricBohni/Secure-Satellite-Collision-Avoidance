//! Two-party comparison of a secret-shared input against a public threshold.
//!
//! The parties hold additive shares x0 + x1 = x (mod 2^32) and compute shares of
//! 1{x < c} for a public threshold c, without learning x.
//!
//! Offline, a dealer picks a random mask r, secret-shares it, and generates two
//! DCF keys with thresholds r and r' = r + c (mod 2^32). Online, each party sends
//! x_b + r_b, so both learn only the masked value m = x + r, which is uniformly
//! random. With u = 1 if r + c wraps around 2^32 (also secret-shared):
//!
//!     1{x < c} = 1{m < r'} - 1{m < r} + u
//!
//! The first two terms come from evaluating the DCF keys at m, so every party
//! ends up with an additive share of the comparison bit.

use fss::dcf::DCFKey;
use fss::{u32_to_bits_BE, RingElm, Share};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

const NBITS: usize = 32;
const KEY_DIR: &str = "keys";
const ADDR: &str = "127.0.0.1:7000";

/// Everything one party receives from the dealer.
#[derive(Serialize, Deserialize)]
pub struct PartyKey {
    threshold: u32,
    /// Share of the mask r.
    r_share: RingElm,
    /// Share of the wraparound bit u.
    u_share: RingElm,
    /// Key for 1{m < r}.
    dcf_r: DCFKey<RingElm>,
    /// Key for 1{m < r + c}.
    dcf_rc: DCFKey<RingElm>,
}

/// Splits `value` into two random additive shares.
pub fn split(value: RingElm) -> (RingElm, RingElm) {
    let share0 = RingElm::random();
    (share0, value - share0)
}

pub fn gen_keys(threshold: u32) -> (PartyKey, PartyKey) {
    // Masks
    let r = RingElm::random().to_u32().unwrap();
    let (r_rc, wrapped) = r.overflowing_add(threshold);
    let (r0, r1) = split(RingElm::from(r));
    let (u0, u1) = split(RingElm::from(wrapped as u32));
    
    // this is the alpha
    let one = RingElm::from(1u32);

    // 2 different DCFkeys for different cases
    let (dcf_r0, dcf_r1) = DCFKey::gen(&u32_to_bits_BE(NBITS, r), &one);
    let (dcf_rc0, dcf_rc1) = DCFKey::gen(&u32_to_bits_BE(NBITS, r_rc), &one);

    (
        PartyKey { threshold, r_share: r0, u_share: u0, dcf_r: dcf_r0, dcf_rc: dcf_rc0 },
        PartyKey { threshold, r_share: r1, u_share: u1, dcf_r: dcf_r1, dcf_rc: dcf_rc1 },
    )
}

/// This party's share of the masked input m = x + r.
pub fn masked_share(key: &PartyKey, x_share: RingElm) -> RingElm {
    x_share + key.r_share
}

/// This party's share of 1{x < c}, given the public masked input m.
pub fn compare_share(key: &PartyKey, m: RingElm) -> RingElm {
    let m_bits = u32_to_bits_BE(NBITS, m.to_u32().unwrap());
    key.dcf_rc.eval(&m_bits) - key.dcf_r.eval(&m_bits) + key.u_share
}

fn key_path(party: u8) -> PathBuf {
    PathBuf::from(KEY_DIR).join(format!("dcf_key{party}.bin"))
}

// create shares of the original input
pub fn share_input(x: u32) {
    let (x0, x1) = split(RingElm::from(x));
    println!("x = {x} split into shares (x0 + x1 = x mod 2^32):");
    println!("  party 0: x0 = {}", x0.to_u32().unwrap());
    println!("  party 1: x1 = {}", x1.to_u32().unwrap());
}

pub fn dealer(threshold: u32) {
    let (key0, key1) = gen_keys(threshold);

    // save keys in file "share data via document not socket"
    fs::create_dir_all(KEY_DIR).expect("create key directory");
    for (party, key) in [(0, &key0), (1, &key1)] {
        let bytes = bincode::serialize(key).expect("serialize key");
        fs::write(key_path(party), bytes).expect("write key file");
        println!("wrote {}", key_path(party).display());
    }
    println!("dealer: keys compute shares of 1{{x < {threshold}}}");
}

pub fn party(id: u8, x_share: u32) {
    assert!(id <= 1, "party id must be 0 or 1");

    let bytes = fs::read(key_path(id)).expect("read key file (run the dealer first)");
    let key: PartyKey = bincode::deserialize(&bytes).expect("deserialize key");
    let mut stream = connect(id);
    println!("party {id}: my share of x = {x_share}, threshold c = {}", key.threshold);

    // Round 1: open the masked input m = x + r.
    let m_share: RingElm = masked_share(&key, RingElm::from(x_share));
    let m: RingElm = m_share + exchange(&mut stream, m_share);
    println!("party {id}: masked input m = {}", m.to_u32().unwrap());

    let my_share: RingElm = compare_share(&key, m);
    println!("party {id}: my share of 1{{x < c}}    = {}", my_share.to_u32().unwrap());

    // Round 2: open the result.
    let their_share = exchange(&mut stream, my_share);
    println!("party {id}: other share of 1{{x < c}} = {}", their_share.to_u32().unwrap());
    let result = my_share + their_share;
    println!("party {id}: result 1{{x < c}}          = {}", result.to_u32().unwrap());
}

/// Sends our share and returns the other party's share.
fn exchange(stream: &mut TcpStream, share: RingElm) -> RingElm {
    stream.write_all(&share.to_u8_vec()).expect("send share");
    let mut buf = [0u8; 4];
    stream.read_exact(&mut buf).expect("receive share");
    RingElm::from(buf.to_vec())
}

/// Party 0 listens and party 1 connects, retrying until party 0 is up.
fn connect(id: u8) -> TcpStream {
    if id == 0 {
        println!("party 0: waiting for party 1 on {ADDR}");
        let listener = TcpListener::bind(ADDR).expect("bind");
        listener.accept().expect("accept").0
    } else {
        loop {
            match TcpStream::connect(ADDR) {
                Ok(stream) => return stream,
                Err(_) => thread::sleep(Duration::from_millis(200)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs both parties locally and reconstructs 1{x < c}.
    fn compare(x: u32, c: u32) -> u32 {
        let (key0, key1) = gen_keys(c);
        let (x0, x1) = split(RingElm::from(x));
        let m = masked_share(&key0, x0) + masked_share(&key1, x1);
        (compare_share(&key0, m) + compare_share(&key1, m)).to_u32().unwrap()
    }

    #[test]
    fn matches_plain_comparison() {
        let edges = [0, 1, 2, 1000, 0x7fff_ffff, 0x8000_0000, u32::MAX - 1, u32::MAX];
        for &c in &edges {
            for &x in &edges {
                assert_eq!(compare(x, c), (x < c) as u32, "x = {x}, c = {c}");
            }
        }
        for _ in 0..200 {
            let x = RingElm::random().to_u32().unwrap();
            let c = RingElm::random().to_u32().unwrap();
            assert_eq!(compare(x, c), (x < c) as u32, "x = {x}, c = {c}");
        }
    }
}
