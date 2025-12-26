use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Pdf,
    Html,
}

#[derive(Parser, Debug)]
#[command(name = "md-conv")]
#[command(author, version, about = "Convert Markdown to PDF and HTML")]
#[command(propagate_version = true)]
#[command(after_help = "EXAMPLES:
    md-conv document.md                    # Convert to PDF (default)
    md-conv document.md -f html            # Convert to HTML
    md-conv document.md -f pdf,html        # Convert to both formats
    md-conv *.md -O output/                # Batch convert to directory
    cat doc.md | md-conv --stdin -f html   # Read from stdin
    md-conv doc.md --stdout                # Write HTML to stdout")]
pub struct Args {
    /// Input Markdown file(s). Use --stdin to read from stdin instead.
    #[arg(required_unless_present = "stdin")]
    pub input: Vec<PathBuf>,

    /// Output format(s) - can be specified multiple times or comma-separated
    #[arg(
        short,
        long,
        value_enum,
        default_value = "pdf",
        env = "MDCONV_FORMAT",
        value_delimiter = ','
    )]
    pub format: Vec<OutputFormat>,

    /// Custom CSS file path
    #[arg(long, value_name = "FILE", env = "MDCONV_CSS")]
    pub css: Option<PathBuf>,

    /// Output file path (only valid with single input)
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Output directory for batch processing
    #[arg(short = 'O', long, value_name = "DIR")]
    pub output_dir: Option<PathBuf>,

    /// Configuration file path (YAML or TOML)
    #[arg(short = 'c', long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    // === Safety Flags ===
    /// Preview actions without executing (dry run)
    #[arg(long)]
    pub dry_run: bool,

    /// Overwrite existing files without prompting
    #[arg(short = 'y', long)]
    pub force: bool,

    /// Never overwrite existing files (error if file exists)
    #[arg(short = 'n', long, conflicts_with = "force")]
    pub no_clobber: bool,

    // === Output Control ===
    /// Suppress all output except errors
    #[arg(short = 'q', long, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Output results as JSON for scripting
    #[arg(long)]
    pub json: bool,

    /// Verbose logging (-v info, -vv debug, -vvv trace)
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    // === Pipeline Support ===
    /// Read Markdown from stdin
    #[arg(long)]
    pub stdin: bool,

    /// Write output to stdout (HTML only, not compatible with PDF)
    #[arg(long)]
    pub stdout: bool,

    // === Watch Mode ===
    /// Watch mode - rebuild on file changes
    #[arg(short, long)]
    pub watch: bool,

    // === Advanced Options ===
    /// Chrome/Chromium executable path (auto-detected if not specified)
    #[arg(long, env = "CHROME_PATH", value_name = "PATH")]
    pub chrome_path: Option<PathBuf>,

    /// Maximum file size in MB (default: 10)
    #[arg(
        long,
        default_value = "10",
        value_name = "MB",
        env = "MDCONV_MAX_FILE_SIZE"
    )]
    pub max_file_size: u64,

    /// PDF generation timeout in seconds (default: 30)
    #[arg(
        long,
        default_value = "30",
        value_name = "SECONDS",
        env = "MDCONV_TIMEOUT"
    )]
    pub timeout: u64,

    /// Allow CSS from paths outside the input file directory
    #[arg(long)]
    pub allow_external_css: bool,
}

impl Args {
    /// Validate CLI arguments before processing
    pub fn validate(&self) -> anyhow::Result<()> {
        use anyhow::bail;

        // Cannot use -o with multiple inputs
        if self.input.len() > 1 && self.output.is_some() {
            bail!("Cannot use --output with multiple input files");
        }

        // Verify input files exist
        for path in &self.input {
            if !path.exists() {
                bail!("Input file not found: {}", path.display());
            }
            if !path.is_file() {
                bail!("Input path is not a file: {}", path.display());
            }
        }

        // Verify CSS file if provided
        if let Some(css) = &self.css {
            if !css.exists() {
                bail!("CSS file not found: {}", css.display());
            }
            if !css.is_file() {
                bail!("CSS path is not a file: {}", css.display());
            }
        }

        // Verify Chrome path if provided
        if let Some(chrome) = &self.chrome_path {
            if !chrome.exists() {
                bail!("Chrome executable not found: {}", chrome.display());
            }
        }

        Ok(())
    }

    /// Get the log level based on verbosity
    pub fn log_level(&self) -> &'static str {
        match self.verbose {
            0 => "warn",
            1 => "info",
            2 => "debug",
            _ => "trace",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_format_is_pdf() {
        let args = Args::parse_from(["md-conv", "test.md"]);
        assert_eq!(args.format, vec![OutputFormat::Pdf]);
    }

    #[test]
    fn test_multiple_formats() {
        let args = Args::parse_from(["md-conv", "test.md", "-f", "pdf", "-f", "html"]);
        assert_eq!(args.format, vec![OutputFormat::Pdf, OutputFormat::Html]);
    }

    #[test]
    fn test_verbosity_levels() {
        let args = Args::parse_from(["md-conv", "test.md"]);
        assert_eq!(args.log_level(), "warn");

        let args = Args::parse_from(["md-conv", "test.md", "-v"]);
        assert_eq!(args.log_level(), "info");

        let args = Args::parse_from(["md-conv", "test.md", "-vvv"]);
        assert_eq!(args.log_level(), "trace");
    }
}
