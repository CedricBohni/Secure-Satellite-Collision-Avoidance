//! Runtime configuration, read once from a `.env` file at start-up.
//!
//! Everything that describes the fixed-point representation lives here, because
//! the ring modulus `2^N_BIT` and the scaling factor `2^L_BIT` are needed by
//! virtually every other module.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Number representation and deployment settings.
#[derive(Clone, Debug)]
pub struct Config {
    /// Total bit width of a share; the ring is Z_{2^n_bit}.
    pub n_bit: usize,
    /// Integer bits, sign bit included.
    pub i_bit: usize,
    /// Fractional bits; a real x is represented as round(x * 2^l_bit).
    pub l_bit: usize,
    pub circuit: PathBuf,
    pub preprocessing_dir: PathBuf,
    pub input_dir: PathBuf,
    pub party0_addr: String,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

/// The configuration of this process. Panics if [`init`] has not run yet.
pub fn config() -> &'static Config {
    CONFIG
        .get()
        .expect("configuration used before config::init()")
}

/// Bit width of the ring, i.e. the ring is Z_{2^n}.
pub fn n_bit() -> usize {
    config().n_bit
}

/// Number of fractional bits of the fixed-point encoding.
pub fn l_bit() -> usize {
    config().l_bit
}

/// Parse `path` and install the result as the process-wide configuration.
pub fn init(path: &Path) -> Result<&'static Config, ConfigError> {
    let cfg = Config::load(path)?;
    Ok(CONFIG.get_or_init(|| cfg))
}

/// Install a fixed configuration for unit tests. All tests in a binary share
/// one process, so they share one representation: n = 64, i = 32, l = 32.
#[cfg(test)]
pub fn init_for_tests() {
    CONFIG.get_or_init(|| Config {
        n_bit: 64,
        i_bit: 32,
        l_bit: 32,
        circuit: PathBuf::from("circuit/circuit.netlist"),
        preprocessing_dir: PathBuf::from("preprocessing"),
        input_dir: PathBuf::from("inputs"),
        party0_addr: "127.0.0.1:7000".to_string(),
    });
}

impl Config {
    fn load(path: &Path) -> Result<Config, ConfigError> {
        let vars = parse_env_file(path)?;
        // A real environment variable wins over the file, so a single run can
        // be pointed at another circuit or port without editing `.env`.
        let get = |key: &str| -> Result<String, ConfigError> {
            std::env::var(key)
                .ok()
                .or_else(|| vars.get(key).cloned())
                .ok_or_else(|| ConfigError::Missing(key.to_string()))
        };
        let get_usize = |key: &str| -> Result<usize, ConfigError> {
            let raw = get(key)?;
            raw.parse::<usize>()
                .map_err(|_| ConfigError::NotANumber(key.to_string(), raw))
        };

        let cfg = Config {
            n_bit: get_usize("N_BIT")?,
            i_bit: get_usize("I_BIT")?,
            l_bit: get_usize("L_BIT")?,
            circuit: PathBuf::from(get("CIRCUIT")?),
            preprocessing_dir: PathBuf::from(get("PREPROCESSING_DIR")?),
            input_dir: PathBuf::from(get("INPUT_DIR")?),
            party0_addr: get("PARTY0_ADDR")?,
        };
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        // The ring is backed by a u64, and a signed value has to fit in an i128
        // during conversions.
        if self.n_bit < 2 || self.n_bit > 64 {
            return Err(ConfigError::Invalid(format!(
                "N_BIT must be in 2..=64, got {}",
                self.n_bit
            )));
        }
        if self.i_bit + self.l_bit != self.n_bit {
            return Err(ConfigError::Invalid(format!(
                "I_BIT + L_BIT must equal N_BIT, got {} + {} != {}",
                self.i_bit, self.l_bit, self.n_bit
            )));
        }
        if self.i_bit < 2 {
            return Err(ConfigError::Invalid(
                "I_BIT must be at least 2 (one sign bit and one integer bit)".to_string(),
            ));
        }
        Ok(())
    }

    /// Path of the preprocessing file belonging to `party`.
    pub fn preprocessing_file(&self, party: u8) -> PathBuf {
        self.preprocessing_dir.join(format!("party{party}.bin"))
    }

    /// Path of the cleartext input file belonging to `party`.
    pub fn input_file(&self, party: u8) -> PathBuf {
        self.input_dir.join(format!("party{party}.txt"))
    }
}

/// Minimal `.env` reader: `KEY=VALUE` per line, `#` starts a comment,
/// surrounding whitespace and optional quotes around the value are stripped.
fn parse_env_file(path: &Path) -> Result<HashMap<String, String>, ConfigError> {
    let text = fs::read_to_string(path)
        .map_err(|e| ConfigError::Io(path.display().to_string(), e.to_string()))?;

    let mut vars = HashMap::new();
    for (no, line) in text.lines().enumerate() {
        let line = match line.find('#') {
            Some(pos) => &line[..pos],
            None => line,
        }
        .trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| ConfigError::Syntax(no + 1, line.to_string()))?;
        let value = value.trim().trim_matches('"').trim_matches('\'');
        vars.insert(key.trim().to_string(), value.to_string());
    }
    Ok(vars)
}

#[derive(Debug)]
pub enum ConfigError {
    Io(String, String),
    Syntax(usize, String),
    Missing(String),
    NotANumber(String, String),
    Invalid(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io(path, e) => write!(f, "cannot read {path}: {e}"),
            ConfigError::Syntax(line, text) => {
                write!(f, "line {line}: expected KEY=VALUE, got `{text}`")
            }
            ConfigError::Missing(key) => write!(f, "missing setting {key}"),
            ConfigError::NotANumber(key, raw) => write!(f, "{key} is not a number: `{raw}`"),
            ConfigError::Invalid(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ConfigError {}
