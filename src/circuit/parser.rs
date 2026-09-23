//! Netlist reader.
//!
//! One gate per line, whitespace separated, operands referring to gates defined
//! on an earlier line:
//!
//! ```text
//! I1              input wire, owned by party 0 unless `P0`/`P1` follows
//! I2 P1           input wire, owned by party 1
//! G1 * I1 I2      multiplication
//! G2 C 40         public constant
//! G3 >> c G1 G2   truncate G1 by the constant on G2
//! G4 + I2 I1      addition
//! G7 < 0 G6       the bit 1{G6 < 0}
//! O1 G7           reveal G7
//! ```
//!
//! Input wires with no explicit owner are handed out round-robin (`I1` to
//! party 0, `I2` to party 1, ...), which matches the two-satellite setting
//! where each party contributes one ephemeris.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::Path;

use super::{Circuit, Gate, TruncMode, WireId};

pub fn parse_file(path: &Path) -> Result<Circuit, ParseError> {
    let text = fs::read_to_string(path)
        .map_err(|e| ParseError::Io(path.display().to_string(), e.to_string()))?;
    parse(&text)
}

pub fn parse(text: &str) -> Result<Circuit, ParseError> {
    let mut gates: Vec<Gate> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut by_name: HashMap<String, WireId> = HashMap::new();
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    let mut triples = 0usize;
    let mut digest = Fnv::new();

    for (index, raw_line) in text.lines().enumerate() {
        let line = match raw_line.find('#') {
            Some(pos) => &raw_line[..pos],
            None => raw_line,
        }
        .trim();
        if line.is_empty() {
            continue;
        }
        let no = index + 1;
        let tokens: Vec<&str> = line.split_whitespace().collect();
        digest.update(line);

        let name = tokens[0];
        // `lookup` resolves an operand, which must already be defined.
        let lookup = |token: &str| -> Result<WireId, ParseError> {
            by_name
                .get(token)
                .copied()
                .ok_or_else(|| ParseError::UnknownWire(no, token.to_string()))
        };

        let gate = if tokens.len() == 1 || (tokens.len() == 2 && is_owner(tokens[1])) {
            // Input declaration, with or without an explicit owner.
            if !name.starts_with('I') {
                return Err(ParseError::Syntax(
                    no,
                    format!("`{name}` looks like an input but does not start with I"),
                ));
            }
            let owner = match tokens.get(1) {
                Some(token) => parse_owner(no, token)?,
                None => (inputs.len() % 2) as u8,
            };
            Gate::Input { owner }
        } else if tokens.len() == 2 {
            // Output declaration: `O1 <wire>`.
            if !name.starts_with('O') {
                return Err(ParseError::Syntax(
                    no,
                    format!("`{name}` looks like an output but does not start with O"),
                ));
            }
            Gate::Output {
                value: lookup(tokens[1])?,
            }
        } else {
            match tokens[1] {
                "C" => {
                    let literal = tokens[2].parse::<f64>().map_err(|_| {
                        ParseError::Syntax(no, format!("`{}` is not a number", tokens[2]))
                    })?;
                    expect_arity(no, &tokens, 3)?;
                    Gate::Const { literal }
                }
                "+" => {
                    expect_arity(no, &tokens, 4)?;
                    Gate::Add {
                        left: lookup(tokens[2])?,
                        right: lookup(tokens[3])?,
                    }
                }
                "*" => {
                    expect_arity(no, &tokens, 4)?;
                    let gate = Gate::Mul {
                        left: lookup(tokens[2])?,
                        right: lookup(tokens[3])?,
                        triple: triples,
                    };
                    triples += 1;
                    gate
                }
                ">>" => {
                    expect_arity(no, &tokens, 5)?;
                    let mode = match tokens[2] {
                        "c" => TruncMode::Constant,
                        other => {
                            return Err(ParseError::Syntax(
                                no,
                                format!("unknown truncation mode `{other}`, expected `c`"),
                            ))
                        }
                    };
                    Gate::Trunc {
                        value: lookup(tokens[3])?,
                        shift: lookup(tokens[4])?,
                        mode,
                    }
                }
                "<" => {
                    expect_arity(no, &tokens, 4)?;
                    if tokens[2] != "0" {
                        return Err(ParseError::Syntax(
                            no,
                            format!(
                                "only comparison against 0 is supported, got `{}`",
                                tokens[2]
                            ),
                        ));
                    }
                    Gate::Ltz {
                        value: lookup(tokens[3])?,
                    }
                }
                other => {
                    return Err(ParseError::Syntax(no, format!("unknown gate `{other}`")));
                }
            }
        };

        let id = gates.len();
        if by_name.insert(name.to_string(), id).is_some() {
            return Err(ParseError::Duplicate(no, name.to_string()));
        }
        match gate {
            Gate::Input { .. } => inputs.push(id),
            Gate::Output { .. } => outputs.push(id),
            _ => {}
        }
        gates.push(gate);
        names.push(name.to_string());
    }

    if gates.is_empty() {
        return Err(ParseError::Empty);
    }
    if outputs.is_empty() {
        return Err(ParseError::NoOutput);
    }

    let rounds = Circuit::schedule(&gates);
    Ok(Circuit {
        gates,
        names,
        inputs,
        outputs,
        triples,
        rounds,
        digest: digest.finish(),
    })
}

fn is_owner(token: &str) -> bool {
    matches!(token, "P0" | "P1" | "p0" | "p1")
}

fn parse_owner(line: usize, token: &str) -> Result<u8, ParseError> {
    match token {
        "P0" | "p0" => Ok(0),
        "P1" | "p1" => Ok(1),
        other => Err(ParseError::Syntax(
            line,
            format!("`{other}` is not an input owner, expected P0 or P1"),
        )),
    }
}

fn expect_arity(line: usize, tokens: &[&str], expected: usize) -> Result<(), ParseError> {
    if tokens.len() != expected {
        return Err(ParseError::Syntax(
            line,
            format!(
                "gate `{}` takes {} token(s), got {}",
                tokens[1],
                expected,
                tokens.len()
            ),
        ));
    }
    Ok(())
}

/// FNV-1a over the whitespace-normalised netlist. Used only to detect a
/// mismatch between the preprocessing and the circuit, never for security.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn update(&mut self, line: &str) {
        for token in line.split_whitespace() {
            for byte in token.as_bytes() {
                self.0 ^= u64::from(*byte);
                self.0 = self.0.wrapping_mul(0x1000_0000_01b3);
            }
            self.0 ^= 0x20;
            self.0 = self.0.wrapping_mul(0x1000_0000_01b3);
        }
    }

    fn finish(self) -> u64 {
        self.0
    }
}

#[derive(Debug)]
pub enum ParseError {
    Io(String, String),
    Syntax(usize, String),
    UnknownWire(usize, String),
    Duplicate(usize, String),
    Empty,
    NoOutput,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Io(path, e) => write!(f, "cannot read {path}: {e}"),
            ParseError::Syntax(line, msg) => write!(f, "line {line}: {msg}"),
            ParseError::UnknownWire(line, name) => write!(
                f,
                "line {line}: `{name}` is not defined; operands must refer to an earlier gate"
            ),
            ParseError::Duplicate(line, name) => write!(f, "line {line}: `{name}` defined twice"),
            ParseError::Empty => write!(f, "the netlist is empty"),
            ParseError::NoOutput => write!(f, "the netlist has no output gate"),
        }
    }
}

impl std::error::Error for ParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    const NETLIST: &str = "
I1
I2
G1 * I1 I2
G2 C 40
G3 >> c G1 G2
G4 + I2 I1
G5 >> c G4 G2
G6 + G3 G5
G7 < 0 G6
O1 G7
";

    #[test]
    fn parses_the_reference_netlist() {
        let circuit = parse(NETLIST).expect("netlist should parse");
        let summary = circuit.summary();
        assert_eq!(summary.inputs, 2);
        assert_eq!(summary.constants, 1);
        assert_eq!(summary.additions, 2);
        assert_eq!(summary.multiplications, 1);
        assert_eq!(summary.truncations, 2);
        assert_eq!(summary.ltz, 1);
        assert_eq!(summary.outputs, 1);
        assert_eq!(circuit.triples(), 1);
    }

    #[test]
    fn inputs_default_to_alternating_owners() {
        let circuit = parse(NETLIST).unwrap();
        assert_eq!(circuit.inputs_of(0).len(), 1);
        assert_eq!(circuit.inputs_of(1).len(), 1);
    }

    #[test]
    fn explicit_owner_overrides_the_default() {
        let circuit = parse("I1 P1\nI2 P1\nG1 + I1 I2\nO1 G1\n").unwrap();
        assert!(circuit.inputs_of(0).is_empty());
        assert_eq!(circuit.inputs_of(1).len(), 2);
    }

    #[test]
    fn rounds_batch_independent_multiplications() {
        // Two independent products, then one that depends on both.
        let circuit = parse("I1\nI2\nA * I1 I2\nB * I2 I1\nC * A B\nO1 C\n").unwrap();
        let rounds = circuit.rounds();
        assert_eq!(rounds.len(), 3, "two parallel muls, the final mul, the output");
        // The two independent multiplications share the first round.
        let muls: Vec<usize> = rounds[0]
            .iter()
            .filter(|&&id| matches!(circuit.gate(id), Gate::Mul { .. }))
            .copied()
            .collect();
        assert_eq!(muls.len(), 2);
    }

    #[test]
    fn unknown_operand_is_reported_with_its_line() {
        let err = parse("I1\nI2\nG1 + I1 I2\nO1 G8\n").unwrap_err();
        assert!(matches!(err, ParseError::UnknownWire(4, ref n) if n == "G8"));
    }

    #[test]
    fn triples_are_numbered_in_netlist_order() {
        let circuit = parse("I1\nI2\nA * I1 I2\nB * A I1\nO1 B\n").unwrap();
        let indices: Vec<usize> = circuit
            .gates()
            .iter()
            .filter_map(|g| match g {
                Gate::Mul { triple, .. } => Some(*triple),
                _ => None,
            })
            .collect();
        assert_eq!(indices, vec![0, 1]);
    }
}
