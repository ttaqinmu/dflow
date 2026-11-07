#[allow(dead_code)] // Some analysis fields are consumed by later milestones.
mod analysis;
mod cli;
mod git;
mod graph;
mod languages;
mod report;
mod tui;

use clap::Parser;
use cli::{Cli, CompareMode};

fn main() {
    let cli = Cli::parse();
    let options = match &cli.mode {
        CompareMode::Local(local) => &local.options,
        CompareMode::Branch(branch) => &branch.options,
    };
    if let Err(message) = options.selected_format(false, false) {
        eprintln!("{message}");
        std::process::exit(2);
    }
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!("cannot determine current directory: {error}");
            std::process::exit(2);
        }
    };
    match git::capture_comparison(&cli.mode, cwd) {
        Ok(capture) => {
            let session = report::AnalysisSession::new(capture);
            let value = session.report(&cli.mode, options.depth as usize);
            let mut encoded = match serde_json::to_vec(&value) {
                Ok(encoded) => encoded,
                Err(error) => {
                    eprintln!("cannot encode JSON report: {error}");
                    std::process::exit(1);
                }
            };
            encoded.push(b'\n');
            let stdout = std::io::stdout();
            let mut output = stdout.lock();
            use std::io::Write;
            if let Err(error) = output.write_all(&encoded) {
                eprintln!("cannot write JSON report: {error}");
                std::process::exit(1);
            }
            std::process::exit(if session.complete() { 0 } else { 1 });
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(error.exit_code());
        }
    }
}
