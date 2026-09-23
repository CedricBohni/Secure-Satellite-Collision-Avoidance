//! In-memory representation of the arithmetic circuit the parties evaluate.

mod parser;

pub use parser::ParseError;

use std::fmt;
use std::path::Path;

/// Index of a gate, which doubles as the identifier of the wire it drives.
pub type WireId = usize;

/// How the shift amount of a truncation gate is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TruncMode {
    /// `>> c`: the shift amount is a public constant.
    Constant,
}

#[derive(Clone, Debug)]
pub enum Gate {
    /// A value contributed in the clear by `owner` and shared with the peer.
    Input { owner: u8 },
    /// A public constant. The literal is kept unscaled: whether it means a real
    /// number or a plain bit count depends on the gate that consumes it.
    Const { literal: f64 },
    Add { left: WireId, right: WireId },
    /// Multiplication, consuming the Beaver triple with index `triple`.
    Mul {
        left: WireId,
        right: WireId,
        triple: usize,
    },
    /// Arithmetic right shift of `value` by the constant on wire `shift`.
    Trunc {
        value: WireId,
        shift: WireId,
        mode: TruncMode,
    },
    /// The bit 1{value < 0}.
    Ltz { value: WireId },
    /// A value revealed to both parties at the end of the protocol.
    Output { value: WireId },
}

impl Gate {
    /// Wires this gate reads.
    pub fn operands(&self) -> Vec<WireId> {
        match *self {
            Gate::Input { .. } | Gate::Const { .. } => Vec::new(),
            Gate::Add { left, right } | Gate::Mul { left, right, .. } => vec![left, right],
            Gate::Trunc { value, shift, .. } => vec![value, shift],
            Gate::Ltz { value } | Gate::Output { value } => vec![value],
        }
    }

    /// Whether evaluating this gate costs a round of communication.
    pub fn is_interactive(&self) -> bool {
        matches!(self, Gate::Mul { .. } | Gate::Trunc { .. } | Gate::Ltz { .. })
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Gate::Input { .. } => "input",
            Gate::Const { .. } => "constant",
            Gate::Add { .. } => "addition",
            Gate::Mul { .. } => "multiplication",
            Gate::Trunc { .. } => "truncation",
            Gate::Ltz { .. } => "less-than-zero",
            Gate::Output { .. } => "output",
        }
    }
}

/// A parsed netlist, in topological order.
#[derive(Clone, Debug)]
pub struct Circuit {
    gates: Vec<Gate>,
    names: Vec<String>,
    inputs: Vec<WireId>,
    outputs: Vec<WireId>,
    triples: usize,
    rounds: Vec<Vec<WireId>>,
    digest: u64,
}

impl Circuit {
    pub fn load(path: &Path) -> Result<Circuit, ParseError> {
        parser::parse_file(path)
    }

    pub fn gates(&self) -> &[Gate] {
        &self.gates
    }

    pub fn gate(&self, id: WireId) -> &Gate {
        &self.gates[id]
    }

    /// The netlist name of a wire, for diagnostics.
    pub fn name(&self, id: WireId) -> &str {
        &self.names[id]
    }

    /// Input gates, in declaration order.
    pub fn inputs(&self) -> &[WireId] {
        &self.inputs
    }

    /// Output gates, in declaration order.
    pub fn outputs(&self) -> &[WireId] {
        &self.outputs
    }

    /// Number of Beaver triples the preprocessing has to provide.
    pub fn triples(&self) -> usize {
        self.triples
    }

    /// Gate ids grouped by communication round. All interactive gates within a
    /// round are independent and are therefore opened in a single message.
    pub fn rounds(&self) -> &[Vec<WireId>] {
        &self.rounds
    }

    /// Fingerprint of the netlist, so that a party can tell whether its
    /// preprocessing was produced for the circuit it is about to evaluate.
    pub fn digest(&self) -> u64 {
        self.digest
    }

    /// Input gates owned by `party`, in declaration order.
    pub fn inputs_of(&self, party: u8) -> Vec<WireId> {
        self.inputs
            .iter()
            .copied()
            .filter(|&id| matches!(self.gates[id], Gate::Input { owner } if owner == party))
            .collect()
    }

    /// Number of gates of each kind, for the `info` command.
    pub fn summary(&self) -> Summary {
        let mut s = Summary::default();
        for gate in &self.gates {
            match gate {
                Gate::Input { .. } => s.inputs += 1,
                Gate::Const { .. } => s.constants += 1,
                Gate::Add { .. } => s.additions += 1,
                Gate::Mul { .. } => s.multiplications += 1,
                Gate::Trunc { .. } => s.truncations += 1,
                Gate::Ltz { .. } => s.ltz += 1,
                Gate::Output { .. } => s.outputs += 1,
            }
        }
        s.rounds = self.rounds.iter().filter(|r| !r.is_empty()).count();
        s
    }

    /// Build the round schedule. A gate sits one round behind any interactive
    /// gate it depends on, so local gates never wait for a message.
    fn schedule(gates: &[Gate]) -> Vec<Vec<WireId>> {
        let mut round_of = vec![0usize; gates.len()];
        let mut rounds: Vec<Vec<WireId>> = Vec::new();
        for (id, gate) in gates.iter().enumerate() {
            let round = gate
                .operands()
                .iter()
                .map(|&p| round_of[p] + usize::from(gates[p].is_interactive()))
                .max()
                .unwrap_or(0);
            round_of[id] = round;
            while rounds.len() <= round {
                rounds.push(Vec::new());
            }
            rounds[round].push(id);
        }
        rounds
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Summary {
    pub inputs: usize,
    pub constants: usize,
    pub additions: usize,
    pub multiplications: usize,
    pub truncations: usize,
    pub ltz: usize,
    pub outputs: usize,
    pub rounds: usize,
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} input(s), {} constant(s), {} addition(s), {} multiplication(s), \
             {} truncation(s), {} less-than-zero, {} output(s), {} round(s)",
            self.inputs,
            self.constants,
            self.additions,
            self.multiplications,
            self.truncations,
            self.ltz,
            self.outputs,
            self.rounds
        )
    }
}
