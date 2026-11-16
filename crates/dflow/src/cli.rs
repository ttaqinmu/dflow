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
