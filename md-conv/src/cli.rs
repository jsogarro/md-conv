use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Supported output formats for the conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    /// Portable Document Format (Headless Chrome required)
    Pdf,
    /// Standard HTML5 document
    Html,
}

/// Command-line arguments for the `md-conv` tool.
///
/// Most options can also be controlled via environmental variables prefixed with `MDCONV_`.
#[derive(Parser, Debug)]
#[command(name = "md-conv")]
#[command(author, version, about = "Convert Markdown to PDF and HTML")]
#[command(propagate_version = true)]
#[command(after_help = "EXAMPLES:
    md-conv document.md                    # Convert to PDF (default)
    md-conv document.md -f html            # Convert to HTML
    md-conv document.md -f pdf,html        # Convert to both formats
    md-conv doc1.md doc2.md                # Batch convert multiple files

EXIT CODES:
    0    Success
    1    General error
    2    I/O error (file not found, permission denied)
    3    Markdown parse error
    4    Configuration error
    5    Security violation (path escape, malicious CSS)
    6    Template rendering error
    7    Browser/Chrome error
    8    Notebook parse error")]
pub struct Args {
    /// Input Markdown file(s)
    #[arg(required_unless_present = "stdin")]
    pub input: Vec<PathBuf>,

    /// Read Markdown content from standard input
    #[arg(long, conflicts_with = "input")]
    pub stdin: bool,

    /// Output format(s) - can be specified multiple times or comma-separated
    #[arg(short, long, value_enum, env = "MDCONV_FORMAT", value_delimiter = ',')]
    pub format: Option<Vec<OutputFormat>>,

    /// Custom CSS file path
    #[arg(long, value_name = "FILE", env = "MDCONV_CSS")]
    pub css: Option<PathBuf>,

    /// Output file path (only valid with single input)
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Write HTML output to standard output (implies --quiet)
    #[arg(long, conflicts_with = "output", conflicts_with = "output_dir")]
    pub stdout: bool,

    /// Output directory for all converted files
    #[arg(short = 'O', long, value_name = "DIR", conflicts_with = "output")]
    pub output_dir: Option<PathBuf>,

    /// Output JSON summary of the conversion process
    #[arg(long)]
    pub json: bool,

    /// Path to a configuration file (YAML)
    #[arg(short = 'c', long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Watch for changes and re-compile
    #[arg(short = 'w', long)]
    pub watch: bool,

    // === Output Control ===
    /// Suppress all output except errors
    #[arg(short = 'q', long, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Verbose logging (-v info, -vv debug, -vvv trace)
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

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

    /// DANGEROUS: Disable Chrome sandbox (required in some containerized environments)
    ///
    /// WARNING: Only use this flag if you trust ALL input content completely.
    /// The sandbox protects against malicious content exploiting browser vulnerabilities.
    /// When disabled, XSS or malicious markdown could potentially compromise the host system.
    #[arg(long)]
    pub no_sandbox: bool,
}

impl Args {
    /// Validates CLI arguments before processing.
    ///
    /// Checks for:
    /// - Exclusive usage of `--output` with single input files.
    /// - Existence of input files, CSS files, and custom Chrome paths.
    ///
    /// # Errors
    /// Returns an error if validation fails.
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

        // Validate stdout usage
        if self.stdout {
            if let Some(formats) = &self.format {
                if formats.contains(&OutputFormat::Pdf) {
                    bail!("PDF output to stdout is currently not supported. Please use -f html.");
                }
            }
            if self.input.len() > 1 {
                bail!("Cannot write to stdout with multiple input files.");
            }
        }

        // Validate watch mode
        if self.watch && self.stdin {
            bail!("Cannot use --watch with --stdin.");
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

    /// Maps the CLI verbosity count (-v, -vv, -vvv) to a standard log level string.
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
    fn test_default_format_is_none() {
        let args = Args::parse_from(["md-conv", "test.md"]);
        assert!(args.format.is_none());
    }

    #[test]
    fn test_multiple_formats() {
        let args = Args::parse_from(["md-conv", "test.md", "-f", "pdf", "-f", "html"]);
        assert_eq!(
            args.format,
            Some(vec![OutputFormat::Pdf, OutputFormat::Html])
        );
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
