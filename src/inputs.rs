//! Cleartext inputs a party contributes to the computation.
//!
//! One `<wire> <value>` pair per line, values written as decimal reals in the
//! units of the circuit:
//!
//! ```text
//! # inputs/party0.txt
//! I1 1234.5
//! ```

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::Path;

/// Values for the input wires owned by one party, keyed by wire name.
#[derive(Clone, Debug, Default)]
pub struct Inputs(HashMap<String, f64>);

impl Inputs {
    pub fn load(path: &Path) -> Result<Inputs, InputError> {
        let text = fs::read_to_string(path)
            .map_err(|e| InputError::Io(path.display().to_string(), e.to_string()))?;
        Inputs::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Inputs, InputError> {
        let mut values = HashMap::new();
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
            let mut tokens = line.split_whitespace();
            let (Some(wire), Some(value)) = (tokens.next(), tokens.next()) else {
                return Err(InputError::Syntax(no, line.to_string()));
            };
            if tokens.next().is_some() {
                return Err(InputError::Syntax(no, line.to_string()));
            }
            let value = value
                .parse::<f64>()
                .map_err(|_| InputError::NotANumber(no, value.to_string()))?;
            values.insert(wire.to_string(), value);
        }
        Ok(Inputs(values))
    }

    pub fn get(&self, wire: &str) -> Option<f64> {
        self.0.get(wire).copied()
    }
}

#[derive(Debug)]
pub enum InputError {
    Io(String, String),
    Syntax(usize, String),
    NotANumber(usize, String),
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InputError::Io(path, e) => write!(f, "cannot read {path}: {e}"),
            InputError::Syntax(line, text) => {
                write!(f, "line {line}: expected `<wire> <value>`, got `{text}`")
            }
            InputError::NotANumber(line, text) => write!(f, "line {line}: `{text}` is not a number"),
        }
    }
}

impl std::error::Error for InputError {}
