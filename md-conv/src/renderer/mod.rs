pub mod html;
pub mod pdf;

use std::path::Path;

/// Output from a render operation
#[derive(Debug)]
pub struct RenderOutput {
    pub bytes: Vec<u8>,
    pub extension: &'static str,
}

/// Common trait for all output renderers
///
/// Note: We use async-trait because native Rust async traits (1.75+)
/// don't support trait objects (dyn Trait). Since we need dynamic
/// dispatch via Box<dyn Renderer>, async-trait is required.
#[async_trait::async_trait]
pub trait Renderer: Send + Sync {
    /// Render the HTML content to the target format
    async fn render(
        &self,
        html: &str,
        config: &crate::config::ConversionConfig,
    ) -> anyhow::Result<RenderOutput>;

    /// File extension for this format (without dot)
    fn extension(&self) -> &'static str;

    /// Human-readable name for logging
    fn name(&self) -> &'static str;
}

/// Create a renderer based on output format
pub fn create_renderer(format: &crate::cli::OutputFormat) -> Box<dyn Renderer> {
    match format {
        crate::cli::OutputFormat::Html => Box::new(html::HtmlRenderer),
        crate::cli::OutputFormat::Pdf => Box::new(pdf::PdfRenderer::new()),
    }
}

/// Write render output to file
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
