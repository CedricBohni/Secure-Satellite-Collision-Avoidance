//! Offline phase: the dealer produces the correlated randomness both parties
//! need, and each party stores its own half on disk.
//!
//! For the gates implemented so far this is exactly one Beaver triple per
//! multiplication gate. The dealer is trusted for correctness and privacy of
//! the preprocessing but learns nothing about the inputs, which are only shared
//! in the online phase.

use std::fs;
use std::path::Path;
use std::time::Instant;

use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};

use crate::circuit::Circuit;
use crate::config;
use crate::metrics::{self, OfflineMetrics};
use crate::ring::Ring;

/// One party's half of a Beaver triple: shares of `a`, `b` and `a * b`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct BeaverTriple {
    pub a: Ring,
    pub b: Ring,
    pub c: Ring,
}

/// Everything one party receives from the dealer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preprocessing {
    pub party: u8,
    /// Ring width the material was generated for.
    pub n_bit: usize,
    /// Fractional bits the material was generated for.
    pub l_bit: usize,
    /// Fingerprint of the netlist the dealer worked from.
    pub circuit_digest: u64,
    pub triples: Vec<BeaverTriple>,
}

/// Run the whole offline phase: generate the correlated randomness, write one
/// bundle per party, and report what it cost.
pub fn run_dealer(circuit: &Circuit) -> Result<OfflineMetrics, PreprocessingError> {
    let cfg = config::config();
    let start = Instant::now();

    let (p0, p1) = deal(circuit);
    let generation = start.elapsed();

    let storage_start = Instant::now();
    let mut file_sizes = Vec::new();
    for bundle in [&p0, &p1] {
        let path = cfg.preprocessing_file(bundle.party);
        bundle.save(&path)?;
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        file_sizes.push((path.display().to_string(), size));
    }
    let storage = storage_start.elapsed();

    Ok(OfflineMetrics {
        triples: circuit.triples(),
        generation,
        storage,
        total: start.elapsed(),
        file_sizes,
        peak_memory: metrics::peak_memory(),
    })
}

/// Generate the correlated randomness for one circuit, one bundle per party.
pub fn deal(circuit: &Circuit) -> (Preprocessing, Preprocessing) {
    let mut rng = OsRng;
    let cfg = config::config();

    let mut triples0 = Vec::with_capacity(circuit.triples());
    let mut triples1 = Vec::with_capacity(circuit.triples());
    for _ in 0..circuit.triples() {
        let a = Ring::random(&mut rng);
        let b = Ring::random(&mut rng);
        let (a0, a1) = a.share(&mut rng);
        let (b0, b1) = b.share(&mut rng);
        let (c0, c1) = (a * b).share(&mut rng);
        triples0.push(BeaverTriple {
            a: a0,
            b: b0,
            c: c0,
        });
        triples1.push(BeaverTriple {
            a: a1,
            b: b1,
            c: c1,
        });
    }

    let bundle = |party: u8, triples: Vec<BeaverTriple>| Preprocessing {
        party,
        n_bit: cfg.n_bit,
        l_bit: cfg.l_bit,
        circuit_digest: circuit.digest(),
        triples,
    };
    (bundle(0, triples0), bundle(1, triples1))
}

impl Preprocessing {
    pub fn save(&self, path: &Path) -> Result<(), PreprocessingError> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| PreprocessingError::io(path, e))?;
        }
        let bytes = bincode::serialize(self).map_err(|e| PreprocessingError::Codec(e.to_string()))?;
        fs::write(path, bytes).map_err(|e| PreprocessingError::io(path, e))
    }

    pub fn load(path: &Path) -> Result<Preprocessing, PreprocessingError> {
        let bytes = fs::read(path).map_err(|e| PreprocessingError::io(path, e))?;
        bincode::deserialize(&bytes).map_err(|e| PreprocessingError::Codec(e.to_string()))
    }

    /// Refuse material that was produced for a different circuit or a different
    /// number representation; either would silently corrupt the result.
    pub fn check(&self, party: u8, circuit: &Circuit) -> Result<(), PreprocessingError> {
        let cfg = config::config();
        if self.party != party {
            return Err(PreprocessingError::Mismatch(format!(
                "this bundle belongs to party {}, not party {party}",
                self.party
            )));
        }
        if self.n_bit != cfg.n_bit || self.l_bit != cfg.l_bit {
            return Err(PreprocessingError::Mismatch(format!(
                "generated for n_bit={}, l_bit={}, but this party runs n_bit={}, l_bit={}",
                self.n_bit, self.l_bit, cfg.n_bit, cfg.l_bit
            )));
        }
        if self.circuit_digest != circuit.digest() {
            return Err(PreprocessingError::Mismatch(
                "generated for a different circuit; re-run the dealer".to_string(),
            ));
        }
        if self.triples.len() < circuit.triples() {
            return Err(PreprocessingError::Mismatch(format!(
                "{} Beaver triple(s) available, {} needed",
                self.triples.len(),
                circuit.triples()
            )));
        }
        Ok(())
    }

    pub fn triple(&self, index: usize) -> BeaverTriple {
        self.triples[index]
    }
}

#[derive(Debug)]
pub enum PreprocessingError {
    Io(String, String),
    Codec(String),
    Mismatch(String),
}

impl PreprocessingError {
    fn io(path: &Path, e: std::io::Error) -> Self {
        PreprocessingError::Io(path.display().to_string(), e.to_string())
    }
}

impl std::fmt::Display for PreprocessingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PreprocessingError::Io(path, e) => write!(f, "{path}: {e}"),
            PreprocessingError::Codec(e) => write!(f, "malformed preprocessing file: {e}"),
            PreprocessingError::Mismatch(msg) => write!(f, "unusable preprocessing: {msg}"),
        }
    }
}

impl std::error::Error for PreprocessingError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit;
    use crate::config;

    /// Replay the online multiplication on both halves of a dealt triple and
    /// check that the shares recombine to the product.
    #[test]
    fn beaver_triples_reconstruct_a_product() {
        config::init_for_tests();
        let circuit = circuit::Circuit::load(std::path::Path::new("circuit/smoke.netlist"))
            .expect("smoke circuit should parse");
        let (p0, p1) = deal(&circuit);
        assert_eq!(p0.triples.len(), circuit.triples());
        assert_eq!(p1.triples.len(), circuit.triples());

        let mut rng = rand::rngs::OsRng;
        for i in 0..circuit.triples() {
            let (t0, t1) = (p0.triple(i), p1.triple(i));
            // a, b and ab are consistent across the two bundles.
            assert_eq!((t0.a + t1.a) * (t0.b + t1.b), t0.c + t1.c);

            let x = Ring::random(&mut rng);
            let y = Ring::random(&mut rng);
            let (x0, x1) = x.share(&mut rng);
            let (y0, y1) = y.share(&mut rng);

            let d = (x0 - t0.a) + (x1 - t1.a);
            let e = (y0 - t0.b) + (y1 - t1.b);
            let z0 = d * e + d * t0.b + e * t0.a + t0.c;
            let z1 = d * t1.b + e * t1.a + t1.c;
            assert_eq!(z0 + z1, x * y);
        }
    }
}
