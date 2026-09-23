//! Online phase: two semi-honest parties evaluate the circuit over additive
//! shares in Z_{2^n}, using the correlated randomness the dealer left on disk.
//!
//! Sharing convention: a value `x` is held as `x0 + x1 = x (mod 2^n)`, with
//! party `i` holding `xi`.  Constants are represented by giving party 0 the
//! whole value and party 1 zero, so that addition with a constant, negation and
//! scaling stay local.
//!
//! The circuit is evaluated round by round.  Local gates are folded into the
//! round of their operands; every interactive gate of a round contributes its
//! openings to a single message, so a circuit costs one round per layer of
//! multiplications rather than one round per multiplication.

use std::fmt;
use std::io;
use std::time::{Duration, Instant};

use crate::circuit::{Circuit, Gate, WireId};
use crate::config;
use crate::inputs::Inputs;
use crate::metrics::{self, OnlineMetrics};
use crate::net::Channel;
use crate::preprocessing::Preprocessing;
use crate::ring::{self, Ring};

/// What a wire carries during evaluation.
#[derive(Clone, Copy, Debug)]
enum Value {
    /// An additive share of a secret value.
    Share(Ring),
    /// A public constant, kept in its unscaled form until it is used.
    Const(f64),
}

/// A revealed output wire.
pub struct OutputValue {
    pub name: String,
    pub raw: Ring,
    pub value: f64,
}

/// One party's state for a single evaluation of the circuit.
pub struct Party {
    id: u8,
    circuit: Circuit,
    material: Preprocessing,
    inputs: Inputs,
    channel: Channel,
    wires: Vec<Option<Value>>,
    loading: Duration,
    connecting: Duration,
    protocol: Duration,
}

impl Party {
    /// Load everything from disk and connect to the peer.
    pub fn new(id: u8) -> Result<Party, ProtocolError> {
        if id > 1 {
            return Err(ProtocolError::UnknownParty(id));
        }
        let cfg = config::config();
        let load_start = Instant::now();

        let circuit = Circuit::load(&cfg.circuit).map_err(|e| ProtocolError::Setup(e.to_string()))?;
        eprintln!("[party {id}] circuit {}: {}", cfg.circuit.display(), circuit.summary());

        let path = cfg.preprocessing_file(id);
        let material = Preprocessing::load(&path).map_err(|e| {
            ProtocolError::Setup(format!("{e} (run the dealer first: `cargo run -- dealer`)"))
        })?;
        material
            .check(id, &circuit)
            .map_err(|e| ProtocolError::Setup(e.to_string()))?;
        eprintln!(
            "[party {id}] preprocessing {}: {} Beaver triple(s)",
            path.display(),
            material.triples.len()
        );

        let owned = circuit.inputs_of(id);
        let inputs = if owned.is_empty() {
            Inputs::default()
        } else {
            Inputs::load(&cfg.input_file(id)).map_err(|e| ProtocolError::Setup(e.to_string()))?
        };

        let loading = load_start.elapsed();

        let connect_start = Instant::now();
        let channel = Channel::connect(id, &cfg.party0_addr)?;
        let connecting = connect_start.elapsed();

        let wires = vec![None; circuit.gates().len()];
        Ok(Party {
            id,
            circuit,
            material,
            inputs,
            channel,
            wires,
            loading,
            connecting,
            protocol: Duration::ZERO,
        })
    }

    /// Run the whole online protocol and return the revealed outputs.
    pub fn run(&mut self) -> Result<Vec<OutputValue>, ProtocolError> {
        let start = Instant::now();
        self.share_inputs()?;
        for round in 0..self.circuit.rounds().len() {
            self.evaluate_round(round)?;
        }
        let outputs = self.reveal_outputs()?;
        self.protocol = start.elapsed();
        Ok(outputs)
    }

    /// What this party's run cost. Meaningful once [`Party::run`] has returned.
    pub fn metrics(&self) -> OnlineMetrics {
        OnlineMetrics {
            party: self.id,
            loading: self.loading,
            connecting: self.connecting,
            protocol: self.protocol,
            total: self.loading + self.connecting + self.protocol,
            transcript: self.channel.transcript(),
            peak_memory: metrics::peak_memory(),
        }
    }

    /// Round 0 of the protocol: every party splits the inputs it owns and sends
    /// one share of each to the peer.  Both directions travel in one exchange.
    fn share_inputs(&mut self) -> Result<(), ProtocolError> {
        let mine = self.circuit.inputs_of(self.id);
        let theirs = self.circuit.inputs_of(1 - self.id);

        let mut rng = rand::rngs::OsRng;
        let mut payload = Vec::with_capacity(mine.len() * Ring::BYTES);
        let mut kept = Vec::with_capacity(mine.len());
        for &id in &mine {
            let name = self.circuit.name(id);
            let value = self
                .inputs
                .get(name)
                .ok_or_else(|| ProtocolError::MissingInput(name.to_string(), self.id))?;
            let encoded = ring::encode(value).map_err(|e| ProtocolError::Encoding(name.to_string(), e.to_string()))?;
            let (to_peer, keep) = encoded.share(&mut rng);
            payload.extend_from_slice(&to_peer.to_bytes());
            kept.push(keep);
        }

        let received = self.channel.exchange(&payload)?;
        let expected = theirs.len() * Ring::BYTES;
        if received.len() != expected {
            return Err(ProtocolError::Protocol(format!(
                "input round: expected {expected} B from the peer, got {}",
                received.len()
            )));
        }

        for (&id, share) in mine.iter().zip(kept) {
            self.wires[id] = Some(Value::Share(share));
        }
        for (slot, &id) in theirs.iter().enumerate() {
            let bytes = &received[slot * Ring::BYTES..(slot + 1) * Ring::BYTES];
            self.wires[id] = Some(Value::Share(Ring::from_bytes(bytes)));
        }
        eprintln!(
            "[party {}] shared {} input(s), received {} from the peer",
            self.id,
            mine.len(),
            theirs.len()
        );
        Ok(())
    }

    /// Evaluate one layer: all local gates first, then the interactive gates of
    /// the layer batched into a single message.
    fn evaluate_round(&mut self, round: usize) -> Result<(), ProtocolError> {
        let gates = self.circuit.rounds()[round].clone();

        let mut interactive = Vec::new();
        for id in gates {
            match *self.circuit.gate(id) {
                Gate::Input { .. } => {} // already set by share_inputs
                Gate::Const { literal } => self.wires[id] = Some(Value::Const(literal)),
                Gate::Add { left, right } => {
                    let sum = self.share_of(left)? + self.share_of(right)?;
                    self.wires[id] = Some(Value::Share(sum));
                }
                Gate::Output { .. } => {} // revealed after the last round
                Gate::Mul { .. } | Gate::Trunc { .. } | Gate::Ltz { .. } => interactive.push(id),
            }
        }

        if interactive.is_empty() {
            return Ok(());
        }
        self.evaluate_interactive(round, &interactive)
    }

    /// The interactive gates of one layer.  Each contributes the shares it wants
    /// opened; the concatenation is exchanged in one message and every gate then
    /// finishes locally.
    fn evaluate_interactive(&mut self, round: usize, gates: &[WireId]) -> Result<(), ProtocolError> {
        let mut payload = Vec::new();
        // Per gate: the masked operands it will need after the opening.
        let mut pending: Vec<(WireId, usize)> = Vec::new();

        for &id in gates {
            match *self.circuit.gate(id) {
                Gate::Mul { left, right, triple } => {
                    let t = self.material.triple(triple);
                    // d = x - a and e = y - b, both opened below.
                    let d = self.share_of(left)? - t.a;
                    let e = self.share_of(right)? - t.b;
                    payload.extend_from_slice(&d.to_bytes());
                    payload.extend_from_slice(&e.to_bytes());
                    pending.push((id, 2));
                }
                Gate::Trunc { .. } | Gate::Ltz { .. } => {
                    return Err(ProtocolError::Unsupported(
                        self.circuit.gate(id).kind(),
                        self.circuit.name(id).to_string(),
                    ));
                }
                _ => unreachable!("only interactive gates reach this point"),
            }
        }

        let received = self.channel.exchange(&payload)?;
        if received.len() != payload.len() {
            return Err(ProtocolError::Protocol(format!(
                "round {round}: expected {} B from the peer, got {}",
                payload.len(),
                received.len()
            )));
        }

        // Reconstruct every opened value by adding the two shares.
        let opened: Vec<Ring> = payload
            .chunks_exact(Ring::BYTES)
            .zip(received.chunks_exact(Ring::BYTES))
            .map(|(mine, theirs)| Ring::from_bytes(mine) + Ring::from_bytes(theirs))
            .collect();

        let mut cursor = 0usize;
        for (id, width) in pending {
            let values = &opened[cursor..cursor + width];
            cursor += width;
            match *self.circuit.gate(id) {
                Gate::Mul { triple, .. } => {
                    let t = self.material.triple(triple);
                    let (d, e) = (values[0], values[1]);
                    // x*y = d*e + d*[b] + e*[a] + [ab]; only one party adds d*e.
                    let mut z = d * t.b + e * t.a + t.c;
                    if self.id == 0 {
                        z = z + d * e;
                    }
                    self.wires[id] = Some(Value::Share(z));
                }
                _ => unreachable!(),
            }
        }
        eprintln!(
            "[party {}] round {round}: {} interactive gate(s), {} B exchanged",
            self.id,
            gates.len(),
            payload.len()
        );
        Ok(())
    }

    /// Final round: exchange the shares of every output wire and decode.
    fn reveal_outputs(&mut self) -> Result<Vec<OutputValue>, ProtocolError> {
        let outputs = self.circuit.outputs().to_vec();
        let mut payload = Vec::with_capacity(outputs.len() * Ring::BYTES);
        for &id in &outputs {
            let source = match *self.circuit.gate(id) {
                Gate::Output { value } => value,
                _ => unreachable!("output list holds output gates"),
            };
            payload.extend_from_slice(&self.share_of(source)?.to_bytes());
        }

        let received = self.channel.exchange(&payload)?;
        if received.len() != payload.len() {
            return Err(ProtocolError::Protocol(format!(
                "output round: expected {} B from the peer, got {}",
                payload.len(),
                received.len()
            )));
        }

        let mut revealed = Vec::with_capacity(outputs.len());
        for (slot, &id) in outputs.iter().enumerate() {
            let range = slot * Ring::BYTES..(slot + 1) * Ring::BYTES;
            let raw = Ring::from_bytes(&payload[range.clone()]) + Ring::from_bytes(&received[range]);
            revealed.push(OutputValue {
                name: self.circuit.name(id).to_string(),
                raw,
                value: ring::decode(raw),
            });
        }
        Ok(revealed)
    }

    /// The share this party holds for a wire.  A constant is turned into a
    /// share on demand: party 0 takes the whole value, party 1 takes zero.
    fn share_of(&self, id: WireId) -> Result<Ring, ProtocolError> {
        match self.wires[id] {
            Some(Value::Share(share)) => Ok(share),
            Some(Value::Const(literal)) => {
                if self.id == 0 {
                    ring::encode(literal).map_err(|e| {
                        ProtocolError::Encoding(self.circuit.name(id).to_string(), e.to_string())
                    })
                } else {
                    Ok(Ring::zero())
                }
            }
            None => Err(ProtocolError::Protocol(format!(
                "wire {} was read before it was assigned",
                self.circuit.name(id)
            ))),
        }
    }
}

#[derive(Debug)]
pub enum ProtocolError {
    UnknownParty(u8),
    Setup(String),
    MissingInput(String, u8),
    Encoding(String, String),
    Unsupported(&'static str, String),
    Protocol(String),
    Io(String),
}

impl From<io::Error> for ProtocolError {
    fn from(e: io::Error) -> Self {
        ProtocolError::Io(e.to_string())
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolError::UnknownParty(id) => write!(f, "party id {id} does not exist, use 0 or 1"),
            ProtocolError::Setup(msg) => write!(f, "{msg}"),
            ProtocolError::MissingInput(wire, party) => write!(
                f,
                "no value for input wire {wire}; add a `{wire} <value>` line to the input file of party {party}"
            ),
            ProtocolError::Encoding(wire, e) => write!(f, "wire {wire}: {e}"),
            ProtocolError::Unsupported(kind, name) => {
                write!(f, "gate {name}: the {kind} gate is not implemented yet")
            }
            ProtocolError::Protocol(msg) => write!(f, "protocol error: {msg}"),
            ProtocolError::Io(e) => write!(f, "network error: {e}"),
        }
    }
}

impl std::error::Error for ProtocolError {}
