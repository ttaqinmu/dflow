mod cli;

use clap::Parser;
use cli::{Cli, CompareMode, OutputFormat, OutputTarget, RenderTarget};
use dflow_core::{config::Config, diagram, git, report};
use std::io::IsTerminal;

fn main() {
    let cli = Cli::parse();
    Config::init(Config {
        include_test: cli.include_test,
    })
    .expect("configuration must be initialized only once per process");

    let options = match &cli.mode {
        CompareMode::Local(local) => &local.options,
        CompareMode::Branch(branch) => &branch.options,
    };

    let format = match options.selected_output(
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

    let comparison = cli.mode.comparison();

    match git::capture_comparison(&comparison, discovery_dir.clone()) {
        Ok(capture) => {
            let session = report::AnalysisSession::new(capture);
            match format {
                OutputTarget::Render(RenderTarget::Tui) => {
                    #[cfg(feature = "tui")]
                    {
                        let complete = match dflow_tui::run(
                            session,
                            &comparison,
                            options.depth as usize,
                            discovery_dir,
                        ) {
                            Ok(complete) => complete,
                            Err(error) => {
                                eprintln!("terminal UI error: {error}");
                                std::process::exit(1);
                            }
                        };
                        std::process::exit(if complete { 0 } else { 1 });
                    }
                    #[cfg(not(feature = "tui"))]
                    unreachable!("terminal selection requires the tui feature");
                }
                OutputTarget::Render(RenderTarget::Gui) => {
                    #[cfg(feature = "gui")]
                    {
                        let complete =
                            match dflow_gui::run(session, &comparison, options.depth as usize) {
                                Ok(complete) => complete,
                                Err(error) => {
                                    eprintln!("GUI startup error: {error}");
                                    std::process::exit(1);
                                }
                            };
                        std::process::exit(if complete { 0 } else { 1 });
                    }
                    #[cfg(not(feature = "gui"))]
                    unreachable!("GUI selection requires the gui feature");
                }
                OutputTarget::Format(_) => {}
            }

            let OutputTarget::Format(format) = format else {
                unreachable!()
            };

            let (mut encoded, format_name) = match format {
                OutputFormat::Json => {
                    let value = session.report(&comparison, options.depth as usize);
                    let encoded = match serde_json::to_vec(&value) {
                        Ok(encoded) => encoded,
                        Err(error) => {
                            eprintln!("cannot encode JSON report: {error}");
                            std::process::exit(1);
                        }
                    };
                    (encoded, "JSON report")
                }
                OutputFormat::Mermaid | OutputFormat::Dot => {
                    let diagram_format = if matches!(format, OutputFormat::Mermaid) {
                        diagram::DiagramFormat::Mermaid
                    } else {
                        diagram::DiagramFormat::Dot
                    };
                    (
                        diagram::render(&session, options.depth as usize, diagram_format)
                            .into_bytes(),
                        "diagram",
                    )
                }
                OutputFormat::Tui => unreachable!("legacy TUI format maps to render"),
            };

            use std::io::Write;
            encoded.push(b'\n');

            let stdout = std::io::stdout();
            let mut output = stdout.lock();

            if let Err(error) = output.write_all(&encoded) {
                eprintln!("cannot write {format_name}: {error}");
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
