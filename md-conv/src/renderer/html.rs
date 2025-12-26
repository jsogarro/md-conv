use tracing::instrument;

use super::{RenderOutput, Renderer};

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
            chrome_path: None,
            output_formats: vec![],
            max_file_size_bytes: 10 * 1024 * 1024,
            timeout_secs: 30,
            allow_external_css: false,
            input_dir: PathBuf::from("."),
        };

        let output = renderer.render("<p>Test</p>", &config).await.unwrap();
        assert_eq!(output.extension, "html");
        assert_eq!(output.bytes, b"<p>Test</p>");
    }
}
