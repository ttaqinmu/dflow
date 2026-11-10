use clap::{Args, Parser, Subcommand, ValueEnum};

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
    Mermaid,
    Dot,
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
            depth: 1,
        };
        assert!(matches!(
            options.selected_format(true, true),
            Ok(OutputFormat::Tui)
        ));
        assert!(matches!(
            options.selected_format(true, false),
            Ok(OutputFormat::Json)
        ));
        assert!(matches!(
            options.selected_format(false, true),
            Ok(OutputFormat::Json)
        ));
        let explicit_tui = AnalyzeOptions {
            format: Some(OutputFormat::Tui),
            depth: 1,
        };
        assert!(explicit_tui.selected_format(false, false).is_err());
        let explicit_json = AnalyzeOptions {
            format: Some(OutputFormat::Json),
            depth: 1,
        };
        assert!(matches!(
            explicit_json.selected_format(true, true),
            Ok(OutputFormat::Json)
        ));
    }
}
