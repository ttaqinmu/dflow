#[allow(dead_code)] // Some analysis fields are consumed by later milestones.
mod analysis;
mod cli;
mod git;
mod graph;
mod languages;
mod report;
mod tui;

use clap::Parser;
use cli::{Cli, CompareMode, OutputFormat};
use std::io::IsTerminal;

fn main() {
    let cli = Cli::parse();
    let options = match &cli.mode {
        CompareMode::Local(local) => &local.options,
        CompareMode::Branch(branch) => &branch.options,
    };
    let format = match options.selected_format(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    ) {
        Ok(format) => format,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!("cannot determine current directory: {error}");
            std::process::exit(2);
        }
    };
    let discovery_dir = match cli.dir.as_ref() {
        Some(dir) if dir.is_absolute() => dir.clone(),
        Some(dir) => cwd.join(dir),
        None => cwd,
    };
    match git::capture_comparison(&cli.mode, discovery_dir.clone()) {
        Ok(capture) => {
            let session = report::AnalysisSession::new(capture);
            if matches!(format, OutputFormat::Tui) {
                let complete =
                    match tui::run(session, &cli.mode, options.depth as usize, discovery_dir) {
                        Ok(complete) => complete,
                        Err(error) => {
                            eprintln!("terminal UI error: {error}");
                            std::process::exit(1);
                        }
                    };
                std::process::exit(if complete { 0 } else { 1 });
            }
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
