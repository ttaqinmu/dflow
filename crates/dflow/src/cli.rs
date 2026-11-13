use clap::{Args, Parser, Subcommand, ValueEnum};
use dflow_core::comparison::{Comparison, LocalSelection};

#[derive(Debug, Parser)]
#[command(name = "dflow", about = "Analyze code changes in a Git repository")]
pub struct Cli {
    #[arg(long, global = true, value_name = "DIR")]
    pub dir: Option<std::path::PathBuf>,
    #[arg(long, global = true)]
    pub include_test: bool,
    #[command(subcommand)]
    pub mode: CompareMode,
}

#[derive(Debug, Subcommand)]
pub enum CompareMode {
    Local(LocalArgs),
    Branch(BranchArgs),
}

impl CompareMode {
    pub fn comparison(&self) -> Comparison {
        match self {
            Self::Local(args) => Comparison::Local(if args.staged {
                LocalSelection::Staged
            } else if args.unstaged {
                LocalSelection::Unstaged
            } else {
                LocalSelection::Combined
            }),
            Self::Branch(args) => Comparison::Branch {
                base: args.base.clone(),
                target: args.target.clone(),
            },
        }
    }
}

#[derive(Debug, Args)]
pub struct LocalArgs {
    #[arg(long, conflicts_with = "unstaged")]
    pub staged: bool,
    #[arg(long)]
    pub unstaged: bool,
    #[command(flatten)]
    pub options: AnalyzeOptions,
}

#[derive(Debug, Args)]
pub struct BranchArgs {
    #[arg(long)]
    pub base: String,
    #[arg(long)]
    pub target: String,
    #[command(flatten)]
    pub options: AnalyzeOptions,
}

#[derive(Debug, Args)]
pub struct AnalyzeOptions {
    #[arg(long, value_enum, conflicts_with = "render")]
    pub format: Option<OutputFormat>,
    #[arg(long, value_enum, conflicts_with = "format")]
    pub render: Option<RenderTarget>,
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32))]
    pub depth: u32,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    Tui,
    Json,
    Mermaid,
    Dot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RenderTarget {
    Gui,
    Tui,
}

#[derive(Debug, Clone, Copy)]
pub enum OutputTarget {
    Render(RenderTarget),
    Format(OutputFormat),
}

impl AnalyzeOptions {
    pub fn selected_output(
        &self,
        stdin_tty: bool,
        stdout_tty: bool,
    ) -> Result<OutputTarget, &'static str> {
        let target = match (self.render, self.format) {
            (Some(render), _) => OutputTarget::Render(render),
            (_, Some(OutputFormat::Tui)) => OutputTarget::Render(RenderTarget::Tui),
            (_, Some(format)) => OutputTarget::Format(format),
            (None, None) if cfg!(feature = "tui") && stdin_tty && stdout_tty => {
                OutputTarget::Render(RenderTarget::Tui)
            }
            (None, None) => OutputTarget::Format(OutputFormat::Json),
        };
        match target {
            OutputTarget::Render(RenderTarget::Tui) if !cfg!(feature = "tui") => {
                Err("TUI unavailable; rebuild with --features tui")
            }
            OutputTarget::Render(RenderTarget::Gui) if !cfg!(feature = "gui") => {
                Err("GUI unavailable; rebuild with --features gui")
            }
            OutputTarget::Render(RenderTarget::Tui) if !(stdin_tty && stdout_tty) => {
                Err("TUI requires terminal stdin and stdout")
            }
            _ => Ok(target),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_are_top_level_commands() {
        assert!(matches!(
            Cli::try_parse_from(["dflow", "local"]).unwrap().mode,
            CompareMode::Local(_)
        ));
        assert!(matches!(
            Cli::try_parse_from(["dflow", "branch", "--base", "main", "--target", "feature"])
                .unwrap()
                .mode,
            CompareMode::Branch(_)
        ));
        for args in [
            vec!["dflow", "--dir", "../other-repo", "local"],
            vec!["dflow", "local", "--dir", "../other-repo"],
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            assert!(matches!(cli.mode, CompareMode::Local(_)));
            assert_eq!(
                cli.dir.as_deref(),
                Some(std::path::Path::new("../other-repo"))
            );
        }
        for args in [
            vec![
                "dflow",
                "--dir",
                "../other-repo",
                "branch",
                "--base",
                "main",
                "--target",
                "feature",
            ],
            vec![
                "dflow",
                "branch",
                "--base",
                "main",
                "--target",
                "feature",
                "--dir",
                "../other-repo",
            ],
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            assert!(matches!(cli.mode, CompareMode::Branch(_)));
            assert_eq!(
                cli.dir.as_deref(),
                Some(std::path::Path::new("../other-repo"))
            );
        }
        assert_eq!(
            Cli::try_parse_from(["dflow", "local", "--dir"])
                .unwrap_err()
                .exit_code(),
            2
        );
        assert!(Cli::try_parse_from(["dflow", "analyze", "local"]).is_err());
    }
    #[test]
    fn format_defaults_follow_terminal_pair() {
        let options = AnalyzeOptions {
            format: None,
            render: None,
            depth: 1,
        };
        if cfg!(feature = "tui") {
            assert!(matches!(
                options.selected_output(true, true),
                Ok(OutputTarget::Render(RenderTarget::Tui))
            ));
        } else {
            assert!(matches!(
                options.selected_output(true, true),
                Ok(OutputTarget::Format(OutputFormat::Json))
            ));
        }
        assert!(matches!(
            options.selected_output(true, false),
            Ok(OutputTarget::Format(OutputFormat::Json))
        ));
        assert!(matches!(
            options.selected_output(false, true),
            Ok(OutputTarget::Format(OutputFormat::Json))
        ));
        let explicit_tui = AnalyzeOptions {
            format: Some(OutputFormat::Tui),
            render: None,
            depth: 1,
        };
        assert!(explicit_tui.selected_output(false, false).is_err());
        let explicit_json = AnalyzeOptions {
            format: Some(OutputFormat::Json),
            render: None,
            depth: 1,
        };
        assert!(matches!(
            explicit_json.selected_output(true, true),
            Ok(OutputTarget::Format(OutputFormat::Json))
        ));
    }
    #[test]
    fn render_flags_and_feature_errors() {
        for name in ["gui", "tui"] {
            let cli = Cli::try_parse_from(["dflow", "local", "--render", name]).unwrap();
            let CompareMode::Local(args) = cli.mode else {
                unreachable!()
            };
            assert_eq!(
                args.options.render,
                Some(if name == "gui" {
                    RenderTarget::Gui
                } else {
                    RenderTarget::Tui
                })
            );
        }
        assert!(
            Cli::try_parse_from(["dflow", "local", "--render", "gui", "--format", "dot"]).is_err()
        );
        let options = AnalyzeOptions {
            render: Some(RenderTarget::Gui),
            format: None,
            depth: 1,
        };
        assert_eq!(
            options.selected_output(false, false).is_ok(),
            cfg!(feature = "gui")
        );
        if !cfg!(feature = "gui") {
            assert!(
                options
                    .selected_output(false, false)
                    .unwrap_err()
                    .contains("--features gui")
            );
        }
        let options = AnalyzeOptions {
            render: Some(RenderTarget::Tui),
            format: None,
            depth: 1,
        };
        if !cfg!(feature = "tui") {
            assert!(
                options
                    .selected_output(true, true)
                    .unwrap_err()
                    .contains("--features tui")
            );
        } else {
            assert_eq!(
                options.selected_output(false, false).unwrap_err(),
                "TUI requires terminal stdin and stdout"
            );
        }
    }
}
