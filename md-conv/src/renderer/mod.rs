//! # Output Renderers
//!
//! This module defines the common interface and infrastructure for generating various
//! output formats from the intermediate HTML representation.
//!
//! Current implementations include:
//! - **HTML**: Simple pass-through or template-wrapped output.
//! - **PDF**: Headless Chrome-based rendering with full CSS/JavaScript support.

mod browser;
pub mod html;
pub mod pdf;

pub(crate) use browser::browser_pool;

use std::path::Path;

/// The result of a successful rendering operation.
#[derive(Debug)]
pub struct RenderOutput {
    /// Raw binary content of the generated file.
    pub bytes: Vec<u8>,
    /// Standard file extension for this format (e.g., "pdf").
    pub extension: &'static str,
}

/// A common interface for all document output formats.
///
/// Implementations must be `Send` and `Sync` to support concurrent processing.
///
/// Note: We use the `async_trait` crate because native Rust async traits (1.75+)
/// do not yet support dynamic dispatch (`dyn Renderer`), which is required for
/// our multi-format processing loop.
#[async_trait::async_trait]
pub trait Renderer: Send + Sync {
    /// Converts intermediate HTML into the final output format.
    async fn render(
        &self,
        html: &str,
        config: &crate::config::ConversionConfig,
    ) -> anyhow::Result<RenderOutput>;

    /// Returns the file extension (without the dot) associated with this format.
    fn extension(&self) -> &'static str;

    /// Returns a human-readable name used for logging and diagnostics.
    fn name(&self) -> &'static str;
}

/// Factory function to instantiate the appropriate renderer for a given format.
pub fn create_renderer(format: &crate::cli::OutputFormat) -> Box<dyn Renderer> {
    match format {
        crate::cli::OutputFormat::Html => Box::new(html::HtmlRenderer),
        crate::cli::OutputFormat::Pdf => Box::new(pdf::PdfRenderer::new()),
    }
}

/// Persists the rendered content to the filesystem.
///
/// This function handles:
/// 1. Verifying and creating parent directories if they don't exist.
/// 2. Asynchronous file writing via `tokio::fs`.
///
/// # Errors
/// Returns an error if directory creation or file writing fails.
pub async fn write_output(output: &RenderOutput, path: &Path) -> anyhow::Result<()> {
    use anyhow::Context;
    use tokio::fs;

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)
                .await
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }
    }

    fs::write(path, &output.bytes)
        .await
        .with_context(|| format!("Failed to write file: {}", path.display()))?;

    Ok(())
}
