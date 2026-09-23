//! Two-party semi-honest evaluation of a fixed-point arithmetic circuit, with a
//! dealer supplying the correlated randomness of the offline phase.
//!
//! Following Hemenway, Lu, Ostrovsky and Welser, "High-precision Secure
//! Computation of Satellite Collision Probabilities" (SCN 2016), the circuit is
//! evaluated over additive shares in Z_{2^n} with a fixed-point encoding:
//! inputs are shared by their owner, constants are held by party 0, additions
//! are local and multiplications consume one Beaver triple each.
//!
//!   cargo run -- info                print the circuit the parties will run
//!   cargo run -- dealer              write preprocessing/party{0,1}.bin
//!   cargo run -- party 0             start party 0 (listens)
//!   cargo run -- party 1             start party 1 (connects)

mod circuit;
mod config;
mod inputs;
mod metrics;
mod net;
mod preprocessing;
mod protocol;
mod ring;

use std::env;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::exit;

use circuit::Circuit;

const USAGE: &str = "usage:
  info                 describe the configured circuit
  dealer               generate the preprocessing material for both parties
  party <0|1>          evaluate the circuit with the other party

options:
  --env <path>         configuration file (default: .env)";

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args: Vec<String> = env::args().skip(1).collect();

    // `--env <path>` may appear anywhere; strip it before reading the command.
    let mut env_path = PathBuf::from(".env");
    if let Some(pos) = args.iter().position(|a| a == "--env") {
        let value = args
            .get(pos + 1)
            .ok_or("--env needs a path")?
            .clone();
        env_path = PathBuf::from(value);
        args.drain(pos..pos + 2);
    }

    let cfg = config::init(&env_path)?;
    match args.first().map(String::as_str) {
        Some("info") => info(&cfg.circuit),
        Some("dealer") => dealer(&cfg.circuit),
        Some("party") => {
            let id: u8 = args
                .get(1)
                .ok_or("party needs an id, 0 or 1")?
                .parse()
                .map_err(|_| "party id must be 0 or 1")?;
            party(id)
        }
        _ => {
            eprintln!("{USAGE}");
            exit(1);
        }
    }
}

fn info(path: &Path) -> Result<(), Box<dyn Error>> {
    let cfg = config::config();
    let circuit = Circuit::load(path)?;
    println!("circuit           {}", path.display());
    println!("representation    n_bit={}, i_bit={}, l_bit={}", cfg.n_bit, cfg.i_bit, cfg.l_bit);
    println!(
        "fixed-point range -{0} <= x < {0}, resolution {1:e}",
        ring::max_magnitude(),
        1.0 / (1u128 << cfg.l_bit) as f64
    );
    println!("gates             {}", circuit.summary());
    println!("beaver triples    {}", circuit.triples());
    for (round, gates) in circuit.rounds().iter().enumerate() {
        let names: Vec<&str> = gates.iter().map(|&id| circuit.name(id)).collect();
        println!("round {round:<12}{}", names.join(" "));
    }
    for &id in circuit.inputs() {
        if let circuit::Gate::Input { owner } = circuit.gate(id) {
            println!("input {:<12}party {owner}", circuit.name(id));
        }
    }
    Ok(())
}

fn dealer(path: &Path) -> Result<(), Box<dyn Error>> {
    let circuit = Circuit::load(path)?;
    let report = preprocessing::run_dealer(&circuit)?;
    println!("{report}");
    Ok(())
}

fn party(id: u8) -> Result<(), Box<dyn Error>> {
    let mut party = protocol::Party::new(id)?;
    let outputs = party.run()?;
    for output in outputs {
        println!("{} = {} (raw 0x{:x})", output.name, output.value, output.raw.raw());
    }
    println!("{}", party.metrics());
    Ok(())
}
