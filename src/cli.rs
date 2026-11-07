use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "dflow", about = "Analyze code changes in a Git repository")]
pub struct Cli {
    #[command(subcommand)]
    pub mode: CompareMode,
}

#[derive(Debug, Subcommand)]
pub enum CompareMode {
    Local(LocalArgs),
    Branch(BranchArgs),
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
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32))]
    pub depth: u32,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    Tui,
    Json,
}

impl AnalyzeOptions {
    pub fn selected_format(
        &self,
        stdin_tty: bool,
        stdout_tty: bool,
    ) -> Result<OutputFormat, &'static str> {
        match self.format {
            Some(OutputFormat::Tui) if !(stdin_tty && stdout_tty) => {
                Err("TUI requires terminal stdin and stdout")
            }
            Some(format) => Ok(format),
            None if stdin_tty && stdout_tty => Ok(OutputFormat::Tui),
            None => Ok(OutputFormat::Json),
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
        assert!(Cli::try_parse_from(["dflow", "analyze", "local"]).is_err());
    }
}
