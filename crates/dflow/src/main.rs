mod cli;

use clap::Parser;
use cli::{Cli, CompareMode, OutputFormat, OutputTarget, RenderTarget};
use dflow_core::{config::Config, diagram, git, report};
use std::io::IsTerminal;
use std::time::Instant;

fn profile_duration(name: &'static str, started: Instant) -> report::PhaseTiming {
    report::PhaseTiming {
        name,
        duration: started.elapsed(),
    }
}

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

    let capture_started = Instant::now();
    match git::capture_comparison(&comparison, discovery_dir.clone()) {
        Ok(capture) => {
            let mut timings = Vec::new();
            if options.profile_json.is_some() {
                timings.push(profile_duration("git_capture", capture_started));
            }
            let session = if options.profile_json.is_some() {
                let (session, build_timings) = report::AnalysisSession::new_profiled(capture);
                timings.extend(build_timings);
                session
            } else {
                report::AnalysisSession::new(capture)
            };
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
                    let value = if options.profile_json.is_some() {
                        let (value, traversal, assembly) =
                            session.report_profiled(&comparison, options.depth as usize);
                        timings.extend([traversal, assembly]);
                        value
                    } else {
                        session.report(&comparison, options.depth as usize)
                    };
                    let started = Instant::now();
                    let encoded = match serde_json::to_vec(&value) {
                        Ok(encoded) => encoded,
                        Err(error) => {
                            eprintln!("cannot encode JSON report: {error}");
                            std::process::exit(1);
                        }
                    };
                    if options.profile_json.is_some() {
                        timings.push(profile_duration("json_serialization", started));
                    }
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

            let started = Instant::now();
            if let Err(error) = output.write_all(&encoded) {
                eprintln!("cannot write {format_name}: {error}");
                std::process::exit(1);
            }
            if let Some(path) = &options.profile_json {
                timings.push(profile_duration("output_write", started));
                let payload = serde_json::json!({
                    "schema_version": 1,
                    "tool_version": env!("CARGO_PKG_VERSION"),
                    "complete": session.complete(),
                    "depth": options.depth,
                    "counts": {
                        "files": session.capture.pair.target.files.len(),
                        "symbols": session.base_graph.symbol_ids.len() + session.target_graph.symbol_ids.len(),
                        "edges": session.base_graph.graph.edge_count() + session.target_graph.graph.edge_count(),
                    },
                    "stages": timings.iter().map(|timing| serde_json::json!({
                        "name": timing.name,
                        "duration_ns": u64::try_from(timing.duration.as_nanos()).unwrap_or(u64::MAX),
                    })).collect::<Vec<_>>(),
                });
                let serialized = serde_json::to_vec_pretty(&payload)
                    .expect("profiling data contains only serializable values");
                if let Err(error) = std::fs::write(path, serialized) {
                    eprintln!("cannot write profile to {}: {error}", path.display());
                    std::process::exit(1);
                }
            }

            std::process::exit(if session.complete() { 0 } else { 1 });
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(error.exit_code());
        }
    }
}
