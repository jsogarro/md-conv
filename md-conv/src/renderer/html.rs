use tracing::instrument;

use super::{RenderOutput, Renderer};

/// A renderer that produces standard HTML output.
///
/// This is primarily a pass-through renderer that takes the intermediate HTML
/// generated from the template engine and provides it as the final output.
pub struct HtmlRenderer;

#[async_trait::async_trait]
impl Renderer for HtmlRenderer {
    #[instrument(skip(self, html, _config), fields(html_len = html.len()))]
    async fn render(
        &self,
        html: &str,
        _config: &crate::config::ConversionConfig,
    ) -> anyhow::Result<RenderOutput> {
        tracing::debug!("Rendering HTML output");

        // Note: The .to_vec() copy is necessary because `html` is a borrowed &str
        // and RenderOutput requires owned data (Vec<u8>). This is an unavoidable
        // allocation given the current API design.
        //
        // For very large documents, consider implementing a StreamingRenderer trait
        // that writes directly to the output file, bypassing the intermediate buffer.
        Ok(RenderOutput {
            bytes: html.as_bytes().to_vec(),
            extension: "html",
        })
    }

    fn extension(&self) -> &'static str {
        "html"
    }

    fn name(&self) -> &'static str {
        "HTML"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FrontMatter;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_html_render() {
        let renderer = HtmlRenderer;
        let config = crate::config::ConversionConfig {
            front_matter: FrontMatter::default(),
            css_content: None,
            highlight_theme: "base16-ocean.dark".to_string(),
            chrome_path: None,
            output_formats: vec![],
            max_file_size_bytes: 10 * 1024 * 1024,
            timeout_secs: 30,
            allow_external_css: false,
            no_sandbox: false,
            input_dir: PathBuf::from("."),
            output_dir: None,
        };

        let output = renderer.render("<p>Test</p>", &config).await.unwrap();
        assert_eq!(output.extension, "html");
        assert_eq!(output.bytes, b"<p>Test</p>");
    }
}
