#[allow(dead_code)] // Shared report fields are consumed in later milestones.
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
            // M2 extracts symbols; M3 adds call resolution and a complete report.
            let base = analysis::extract_snapshot(&capture.pair.base);
            let target = analysis::extract_snapshot(&capture.pair.target);
            let changes = analysis::classify_symbols(&base, &target);
            for diagnostic in &capture.diagnostics {
                eprintln!(
                    "Git diagnostic at {:?}: {}",
                    diagnostic.path, diagnostic.message
                );
            }
            for diagnostic in &base.diagnostics {
                eprintln!(
                    "Python diagnostic at {:?}: {}",
                    diagnostic.path, diagnostic.message
                );
            }
            for diagnostic in &target.diagnostics {
                eprintln!(
                    "Python diagnostic at {:?}: {}",
                    diagnostic.path, diagnostic.message
                );
            }
            eprintln!(
                "Captured {} file changes and classified {} Python symbol changes (complete: {}); call graph and report are not implemented yet",
                capture.changes.len(),
                changes.changes.len(),
                changes.complete
            );
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(error.exit_code());
        }
    }
}
