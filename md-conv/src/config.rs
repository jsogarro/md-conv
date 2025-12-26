use serde::Deserialize;
use std::path::{Path, PathBuf};

/// PDF-specific options from front matter
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct PdfOptions {
    /// Page format: A4, Letter, Legal, etc.
    pub format: Option<String>,
    /// Margins in CSS units (e.g., "20mm", "1in")
    pub margin_top: Option<String>,
    pub margin_bottom: Option<String>,
    pub margin_left: Option<String>,
    pub margin_right: Option<String>,
    /// Shorthand for all margins
    pub margin: Option<String>,
    /// Print background graphics
    #[serde(default = "default_true")]
    pub print_background: bool,
    /// Landscape orientation
    pub landscape: bool,
    /// Scale factor (0.1 to 2.0)
    #[serde(default = "default_scale")]
    pub scale: f64,
    /// Header template (HTML)
    pub header_template: Option<String>,
    /// Footer template (HTML)
    pub footer_template: Option<String>,
}

fn default_true() -> bool {
    true
}
fn default_scale() -> f64 {
    1.0
}

/// Front matter metadata
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct FrontMatter {
    pub title: Option<String>,
    pub author: Option<String>,
    pub date: Option<String>,
    pub description: Option<String>,
    pub keywords: Option<Vec<String>>,
    /// CSS file path or inline CSS
    pub css: Option<String>,
    /// PDF-specific options
    pub pdf_options: PdfOptions,
    /// Language code (e.g., "en", "fr")
    pub lang: Option<String>,
}

/// Merged configuration from CLI args + front matter
#[derive(Debug, Clone)]
pub struct ConversionConfig {
    pub front_matter: FrontMatter,
    pub css_content: Option<String>,
    pub chrome_path: Option<PathBuf>,
    pub output_formats: Vec<crate::cli::OutputFormat>,
    pub max_file_size_bytes: u64,
    pub timeout_secs: u64,
    pub allow_external_css: bool,
    pub input_dir: PathBuf,
}

impl ConversionConfig {
    /// Merge CLI arguments with front matter, CLI takes precedence
    pub fn merge(
        args: &crate::cli::Args,
        front_matter: FrontMatter,
        input_path: &Path,
    ) -> anyhow::Result<Self> {
        use anyhow::Context;

        let input_dir = input_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();

        // CLI CSS takes precedence over front matter
        let css_content = if let Some(css_path) = &args.css {
            let content = std::fs::read_to_string(css_path)
                .with_context(|| format!("Failed to read CSS: {}", css_path.display()))?;
            Some(crate::security::sanitize_css(&content)?)
        } else if let Some(fm_css) = &front_matter.css {
            // Check if it's a path or inline CSS
            let path = Path::new(fm_css);
            if path.exists() {
                // Validate path is safe
                if !args.allow_external_css {
                    crate::security::validate_path_safety(path, &input_dir)?;
                }
                let content = std::fs::read_to_string(path)
                    .with_context(|| format!("Failed to read CSS: {}", path.display()))?;
                Some(crate::security::sanitize_css(&content)?)
            } else {
                Some(crate::security::sanitize_css(fm_css)?)
            }
        } else {
            None
        };

        Ok(Self {
            front_matter,
            css_content,
            chrome_path: args.chrome_path.clone(),
            output_formats: args.format.clone(),
            max_file_size_bytes: args.max_file_size * 1024 * 1024,
            timeout_secs: args.timeout,
            allow_external_css: args.allow_external_css,
            input_dir,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Args, OutputFormat};
    use clap::Parser;
    use tempfile::NamedTempFile;
    use std::io::Write;

    #[test]
    fn test_config_merge_basic() {
        let args = Args::parse_from(["md-conv", "test.md"]);
        let fm = FrontMatter::default();
        let config = ConversionConfig::merge(&args, fm, Path::new("test.md")).unwrap();

        assert_eq!(config.output_formats, vec![OutputFormat::Pdf]);
        assert_eq!(config.timeout_secs, 30);
    }

    #[test]
    fn test_config_merge_css_precedence() {
        let mut css_file = NamedTempFile::new().unwrap();
        write!(css_file, "body {{ color: blue; }}").unwrap();
        let css_path = css_file.path().to_path_buf();

        let args = Args::parse_from([
            "md-conv",
            "test.md",
            "--css",
            css_path.to_str().unwrap(),
        ]);

        let fm = FrontMatter {
            css: Some("body { color: red; }".into()),
            ..Default::default()
        };

        let config = ConversionConfig::merge(&args, fm, Path::new("test.md")).unwrap();
        assert_eq!(config.css_content, Some("body { color: blue; }".into()));
    }
}
