use serde::Deserialize;
use std::path::{Path, PathBuf};

/// PDF-specific rendering options typically provided in Markdown front matter.
///
/// These options map to Headless Chrome's `PrintToPDF` parameters.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct PdfOptions {
    /// Page format: A4, Letter, Legal, etc.
    pub format: Option<String>,
    /// Top margin in CSS units (e.g., "20mm", "1in").
    pub margin_top: Option<String>,
    /// Bottom margin in CSS units.
    pub margin_bottom: Option<String>,
    /// Left margin in CSS units.
    pub margin_left: Option<String>,
    /// Right margin in CSS units.
    pub margin_right: Option<String>,
    /// Shorthand for all margins. Individual margins take precedence.
    pub margin: Option<String>,
    /// Whether to print background graphics. Defaults to `true`.
    #[serde(default = "default_true")]
    pub print_background: bool,
    /// Whether to use landscape orientation. Defaults to `false`.
    pub landscape: bool,
    /// Scale factor of the page rendering (0.1 to 2.0). Defaults to `1.0`.
    #[serde(default = "default_scale")]
    pub scale: f64,
    /// Optional HTML template for the page header.
    pub header_template: Option<String>,
    /// Optional HTML template for the page footer.
    pub footer_template: Option<String>,
}

fn default_true() -> bool {
    true
}
fn default_scale() -> f64 {
    1.0
}

/// Metadata and options extracted from the Markdown document's YAML front matter.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct FrontMatter {
    /// Title of the document.
    pub title: Option<String>,
    /// Author of the document.
    pub author: Option<String>,
    /// Publication or creation date.
    pub date: Option<String>,
    /// Short description/abstract for metadata.
    pub description: Option<String>,
    /// Keywords for SEO and metadata.
    pub keywords: Option<Vec<String>>,
    /// Path to a CSS file or a literal block of CSS rules.
    pub css: Option<String>,
    /// Rendering options specific to PDF output.
    pub pdf_options: PdfOptions,
    /// Document language code (e.g., "en", "ja").
    pub lang: Option<String>,
    /// Syntax highlighting theme (e.g., "base16-ocean.dark").
    pub highlight_theme: Option<String>,
}

/// Shared configuration file (YAML) mapping to CLI arguments.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ConfigFile {
    pub css: Option<PathBuf>,
    pub highlight_theme: Option<String>,
    pub format: Option<Vec<crate::cli::OutputFormat>>,
    pub chrome_path: Option<PathBuf>,
    pub timeout: Option<u64>,
    pub max_file_size: Option<u64>,
    pub allow_external_css: Option<bool>,
    pub no_sandbox: Option<bool>,
    pub output_dir: Option<PathBuf>,
}

/// The final, validated configuration used for the conversion process.
///
/// This struct is created by merging CLI arguments with document front matter.
#[derive(Debug, Clone)]
pub struct ConversionConfig {
    /// Merged metadata from the document.
    pub front_matter: FrontMatter,
    /// Sanitized CSS content to be injected into the template.
    pub css_content: Option<String>,
    /// Syntax highlighting theme to use.
    pub highlight_theme: String,
    /// Resolved path to the Chrome/Chromium executable.
    pub chrome_path: Option<PathBuf>,
    /// List of formats to generate.
    pub output_formats: Vec<crate::cli::OutputFormat>,
    /// Maximum allowed input file size in bytes.
    pub max_file_size_bytes: u64,
    /// Timeout for browser-based rendering operations.
    pub timeout_secs: u64,
    /// Whether to allow loading CSS from outside the input directory.
    pub allow_external_css: bool,
    /// Whether to bypass the Chrome sandbox (use with caution).
    pub no_sandbox: bool,
    /// The directory containing the input file (used for relative path resolution).
    pub input_dir: PathBuf,
    /// Target directory for output files.
    pub output_dir: Option<PathBuf>,
}

impl ConversionConfig {
    /// Merges CLI arguments with document front matter.
    ///
    /// CLI arguments generally take precedence over front matter settings.
    /// This function also handles:
    /// - Resolution and sanitization of CSS (either from file search or inline).
    /// - Enforcement of the `allow_external_css` security policy.
    /// - Conversion of MB limits to byte counts.
    ///
    /// # Errors
    /// Returns an error if CSS files cannot be read or fail sanitization.
    pub async fn merge(
        args: &crate::cli::Args,
        front_matter: FrontMatter,
        input_path: &Path,
    ) -> anyhow::Result<Self> {
        use anyhow::Context;

        let input_dir = input_path.parent().unwrap_or(Path::new(".")).to_path_buf();

        // Load config file if specified
        let config_file: ConfigFile = if let Some(config_path) = &args.config {
            let content = tokio::fs::read_to_string(config_path)
                .await
                .with_context(|| {
                    format!("Failed to read config file: {}", config_path.display())
                })?;
            serde_yaml::from_str(&content).with_context(|| {
                format!("Failed to parse config file: {}", config_path.display())
            })?
        } else {
            ConfigFile::default()
        };

        // Resolve options with precedence: CLI > Config File > Default/FrontMatter

        // CSS
        let css_content = if let Some(css_path) = &args.css {
            let content = tokio::fs::read_to_string(css_path)
                .await
                .with_context(|| format!("Failed to read CSS: {}", css_path.display()))?;
            Some(crate::security::sanitize_css(&content)?)
        } else if let Some(css_path) = &config_file.css {
            let content = tokio::fs::read_to_string(css_path).await.with_context(|| {
                format!("Failed to read CSS from config: {}", css_path.display())
            })?;
            Some(crate::security::sanitize_css(&content)?)
        } else if let Some(fm_css) = &front_matter.css {
            // Check if it's a path or inline CSS
            let path = Path::new(fm_css);
            // path.exists() is fast, but we could use tokio::fs::metadata(path).await.is_ok()
            if tokio::fs::try_exists(path).await.unwrap_or(false) {
                // If it's a path, validate and read it
                let content =
                    if args.allow_external_css || config_file.allow_external_css.unwrap_or(false) {
                        tokio::fs::read_to_string(path)
                            .await
                            .with_context(|| format!("Failed to read CSS: {}", path.display()))?
                    } else {
                        let validated =
                            crate::security::validate_and_open_file(path, &input_dir).await?;
                        validated.read_to_string().await?
                    };
                Some(crate::security::sanitize_css(&content)?)
            } else {
                Some(crate::security::sanitize_css(fm_css)?)
            }
        } else {
            None
        };

        // Syntax Theme
        let highlight_theme = front_matter
            .highlight_theme
            .clone()
            .or(config_file.highlight_theme)
            .unwrap_or_else(|| "base16-ocean.dark".to_string());

        // Output Formats
        let output_formats = args
            .format
            .clone()
            .or(config_file.format)
            .unwrap_or_else(|| vec![crate::cli::OutputFormat::Pdf]);

        Ok(Self {
            front_matter,
            css_content,
            highlight_theme,
            chrome_path: args.chrome_path.clone().or(config_file.chrome_path),
            output_formats,
            max_file_size_bytes: config_file.max_file_size.unwrap_or(args.max_file_size)
                * 1024
                * 1024,
            timeout_secs: config_file.timeout.unwrap_or(args.timeout),
            allow_external_css: args.allow_external_css
                || config_file.allow_external_css.unwrap_or(false),
            no_sandbox: args.no_sandbox || config_file.no_sandbox.unwrap_or(false),
            input_dir,
            output_dir: args.output_dir.clone().or(config_file.output_dir),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Args, OutputFormat};
    use clap::Parser;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_config_merge_basic() {
        let args = Args::parse_from(["md-conv", "test.md"]);
        let fm = FrontMatter::default();
        let config = ConversionConfig::merge(&args, fm, Path::new("test.md"))
            .await
            .unwrap();

        assert_eq!(config.output_formats, vec![OutputFormat::Pdf]);
        assert_eq!(config.timeout_secs, 30);
    }

    #[tokio::test]
    async fn test_config_merge_css_precedence() {
        let mut css_file = NamedTempFile::new().unwrap();
        write!(css_file, "body {{ color: blue; }}").unwrap();
        let css_path = css_file.path().to_path_buf();

        let args = Args::parse_from(["md-conv", "test.md", "--css", css_path.to_str().unwrap()]);

        let fm = FrontMatter {
            css: Some("body { color: red; }".into()),
            ..Default::default()
        };

        let config = ConversionConfig::merge(&args, fm, Path::new("test.md"))
            .await
            .unwrap();
        // lightningcss minifies CSS, so we check for the minified version
        assert_eq!(config.css_content, Some("body{color:#00f}".into()));
    }
}
