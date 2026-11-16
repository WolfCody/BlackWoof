//! CI helpers for BLACK WOOF. Reads `cargo metadata` JSON from stdin.
//!
//! Subcommands:
//!   check-log-facade  — the `log` crate must appear exactly once and be built
//!                       with `max_level_off` (every log call compiled out).

use std::io::{self, Read};
use std::process::ExitCode;

use serde_json::Value;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let cmd = match args.next().as_deref() {
        Some("check-log-facade") => check_log_facade,
        _ => {
            eprintln!("usage: xtask <command>");
            eprintln!("commands:");
            eprintln!("  check-log-facade   (reads cargo metadata JSON from stdin)");
            return ExitCode::from(2);
        }
    };

    match cmd() {
        Ok(msg) => {
            println!("{msg}");
            ExitCode::SUCCESS
        }
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::FAILURE
        }
    }
}

fn check_log_facade() -> Result<String, String> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| format!("FAIL: cannot read stdin: {e}"))?;

    let meta: Value = serde_json::from_str(&input)
        .map_err(|e| format!("FAIL: cannot parse cargo metadata JSON: {e}"))?;

    let packages = meta["packages"]
        .as_array()
        .ok_or_else(|| "FAIL: cargo metadata has no `packages` array".to_string())?;

    let log_packages: Vec<&Value> = packages
        .iter()
        .filter(|p| p["name"].as_str() == Some("log"))
        .collect();

    if log_packages.len() != 1 {
        return Err(format!(
            "FAIL: expected exactly one `log` crate in the dependency graph, found {}",
            log_packages.len()
        ));
    }

    let log_id = log_packages[0]["id"]
        .as_str()
        .ok_or_else(|| "FAIL: `log` package has no `id`".to_string())?;

    let nodes = meta["resolve"]["nodes"]
        .as_array()
        .ok_or_else(|| "FAIL: cargo metadata has no `resolve.nodes` array".to_string())?;

    let node = nodes
        .iter()
        .find(|n| n["id"].as_str() == Some(log_id))
        .ok_or_else(|| format!("FAIL: no resolve node for `log` ({log_id})"))?;

    let features = node["features"]
        .as_array()
        .ok_or_else(|| "FAIL: `log` resolve node has no `features` array".to_string())?;

    let has_max_level_off = features.iter().any(|f| f.as_str() == Some("max_level_off"));

    if !has_max_level_off {
        let found: Vec<&str> = features.iter().filter_map(|f| f.as_str()).collect();
        return Err(format!(
            "FAIL: `log` crate is not built with the `max_level_off` feature\n  found: {}",
            found.join(" ")
        ));
    }

    Ok("OK: `log` crate appears exactly once and is built with `max_level_off`".to_string())
}
