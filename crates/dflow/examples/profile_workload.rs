//! Repeat the complete headless analysis pipeline for external stack samplers.

use dflow_core::comparison::{Comparison, LocalSelection};
use dflow_core::config::Config;
use dflow_core::{git, report};
use std::hint::black_box;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(
        args.next()
            .ok_or("usage: profile_workload REPO ITERATIONS [DEPTH]")?,
    );
    let iterations: usize = args
        .next()
        .ok_or("missing ITERATIONS")?
        .parse()
        .map_err(|_| "ITERATIONS must be a positive integer")?;
    let depth: usize = args
        .next()
        .unwrap_or_else(|| "2".into())
        .parse()
        .map_err(|_| "DEPTH must be a nonnegative integer")?;
    if iterations == 0 || args.next().is_some() {
        return Err("expected REPO ITERATIONS [DEPTH], with ITERATIONS > 0".into());
    }

    Config::init(Config {
        include_test: false,
    })
    .expect("configuration must be initialized only once per process");
    let mode = Comparison::Local(LocalSelection::Combined);
    for _ in 0..iterations {
        let capture = git::capture_comparison(&mode, root.clone())?;
        let session = report::AnalysisSession::new(capture);
        let encoded = serde_json::to_vec(&session.report(&mode, depth))?;
        black_box(encoded);
    }
    Ok(())
}
