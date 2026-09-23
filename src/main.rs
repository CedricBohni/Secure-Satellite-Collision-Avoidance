mod dcf_demo;
#[cfg(test)]
mod dcf_tests;

use std::env;
use std::process::exit;

const USAGE: &str = "usage:
  share <x>               split a secret input x into two additive shares
  dealer <c>              generate keys for the comparison 1{x < c}
  party <0|1> <x_share>   run one party on its share of x";

fn parse(arg: Option<&String>) -> u32 {
    match arg.and_then(|s| s.parse().ok()) {
        Some(n) => n,
        None => {
            eprintln!("{USAGE}");
            exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("share") => dcf_demo::share_input(parse(args.get(1))),
        Some("dealer") => dcf_demo::dealer(parse(args.get(1))),
        Some("party") => dcf_demo::party(parse(args.get(1)) as u8, parse(args.get(2))),
        _ => {
            eprintln!("{USAGE}");
            exit(1);
        }
    }
}
