mod analysis;
mod cli;
mod git;
mod graph;
mod languages;
mod report;
mod tui;

use clap::Parser;
use cli::{Cli, CompareMode};
use std::io::IsTerminal;

fn main() {
    let cli = Cli::parse();
    let options = match &cli.mode {
        CompareMode::Local(local) => &local.options,
        CompareMode::Branch(branch) => &branch.options,
    };
    if let Err(message) = options.selected_format(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    ) {
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
            // Later milestones consume this captured source; M1 does not emit an analysis report.
            let _ = (
                capture.pair.base.files.len(),
                capture.pair.target.files.len(),
                capture.changes.len(),
                capture.diagnostics.len(),
            );
            eprintln!("Git snapshots captured; code analysis is not implemented yet");
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(error.exit_code());
        }
    }
}
