use anyhow::{bail, Context};
use chromiumoxide::browser::{Browser, BrowserConfig, HeadlessMode};
use chromiumoxide::cdp::browser_protocol::page::PrintToPdfParams;
use futures::StreamExt;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tracing::instrument;

use super::{RenderOutput, Renderer};
use crate::config::{ConversionConfig, PdfOptions};

/// Maximum concurrent browser instances for batch processing.
/// This prevents resource exhaustion when processing many files.
const MAX_CONCURRENT_BROWSERS: usize = 3;

/// Global semaphore to limit concurrent browser instances.
/// Using OnceLock for lazy thread-safe initialization.
static BROWSER_SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();

fn browser_semaphore() -> &'static Semaphore {
    BROWSER_SEMAPHORE.get_or_init(|| Semaphore::new(MAX_CONCURRENT_BROWSERS))
}

/// RAII guard for browser cleanup.
///
/// Ensures the browser is properly closed and the handler task is
/// joined on ALL code paths - success, error, and panic.
///
/// This prevents:
/// - Zombie Chrome processes
/// - Handler task leaks
/// - Resource exhaustion from unclosed browsers
struct BrowserGuard {
    browser: Option<Browser>,
    handler: Option<JoinHandle<()>>,
}

impl BrowserGuard {
    fn new(browser: Browser, handler: JoinHandle<()>) -> Self {
        Self {
            browser: Some(browser),
            handler: Some(handler),
        }
    }

    fn browser(&self) -> &Browser {
        self.browser.as_ref().expect("browser already consumed")
    }

    /// Gracefully close the browser and wait for handler to finish.
    /// Should be called explicitly for graceful shutdown.
    async fn close(mut self) {
        // Drop browser first to trigger Chrome shutdown
        if let Some(mut browser) = self.browser.take() {
            // Attempt graceful close - ignore errors since we're cleaning up
            let _ = browser.close().await;
            drop(browser);
        }

        // Wait for handler with timeout to avoid hanging
        if let Some(handler) = self.handler.take() {
            match tokio::time::timeout(Duration::from_secs(5), handler).await {
                Ok(Ok(())) => tracing::debug!("Browser handler closed gracefully"),
                Ok(Err(e)) => tracing::warn!(error = %e, "Browser handler panicked"),
                Err(_) => {
                    tracing::warn!("Browser handler close timed out, aborting");
                    // Handler will be aborted when dropped
                }
            }
        }
    }
}

impl Drop for BrowserGuard {
    fn drop(&mut self) {
        // If close() wasn't called, force cleanup synchronously.
        // This handles error paths and panics.
        if let Some(handler) = self.handler.take() {
            handler.abort();
            tracing::debug!("Browser handler aborted in Drop");
        }
        // Browser Drop will handle its own process cleanup
    }
}

pub struct PdfRenderer {
    // Configuration can be extended here
}

impl Default for PdfRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl PdfRenderer {
    pub fn new() -> Self {
        Self {}
    }

    fn build_print_params(options: &PdfOptions) -> PrintToPdfParams {
        let mut params = PrintToPdfParams::default();

        // Page format
        if let Some(format) = &options.format {
            match format.to_lowercase().as_str() {
                "a4" => {
                    params.paper_width = Some(8.27); // inches
                    params.paper_height = Some(11.69);
                }
                "a3" => {
                    params.paper_width = Some(11.69);
                    params.paper_height = Some(16.54);
                }
                "letter" => {
                    params.paper_width = Some(8.5);
                    params.paper_height = Some(11.0);
                }
                "legal" => {
                    params.paper_width = Some(8.5);
                    params.paper_height = Some(14.0);
                }
                "tabloid" => {
                    params.paper_width = Some(11.0);
                    params.paper_height = Some(17.0);
                }
                _ => tracing::warn!(format = %format, "Unknown paper format, using default"),
            }
        }

        // Margins - convert CSS units to inches
        let default_margin = options
            .margin
            .as_deref()
            .map(parse_margin_to_inches)
            .unwrap_or(0.787); // ~20mm default

        params.margin_top = Some(
            options
                .margin_top
                .as_deref()
                .map(parse_margin_to_inches)
                .unwrap_or(default_margin),
        );
        params.margin_bottom = Some(
            options
                .margin_bottom
                .as_deref()
                .map(parse_margin_to_inches)
                .unwrap_or(default_margin),
        );
        params.margin_left = Some(
            options
                .margin_left
                .as_deref()
                .map(parse_margin_to_inches)
                .unwrap_or(default_margin),
        );
        params.margin_right = Some(
            options
                .margin_right
                .as_deref()
                .map(parse_margin_to_inches)
                .unwrap_or(default_margin),
        );

        params.print_background = Some(options.print_background);
        params.landscape = Some(options.landscape);
        params.scale = Some(options.scale.clamp(0.1, 2.0));
        params.prefer_css_page_size = Some(true);

        // Header/footer templates
        if options.header_template.is_some() || options.footer_template.is_some() {
            params.display_header_footer = Some(true);
            params.header_template = options.header_template.clone();
            params.footer_template = options.footer_template.clone();
        }

        params
    }

    fn find_chrome() -> anyhow::Result<PathBuf> {
        // Common Chrome/Chromium locations by platform
        let candidates: Vec<PathBuf> = if cfg!(target_os = "macos") {
            vec![
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into(),
                "/Applications/Chromium.app/Contents/MacOS/Chromium".into(),
                "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge".into(),
                "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser".into(),
            ]
        } else if cfg!(target_os = "windows") {
            vec![
                PathBuf::from(std::env::var("PROGRAMFILES").unwrap_or_default())
                    .join(r"Google\Chrome\Application\chrome.exe"),
                PathBuf::from(std::env::var("PROGRAMFILES(X86)").unwrap_or_default())
                    .join(r"Google\Chrome\Application\chrome.exe"),
                PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default())
                    .join(r"Google\Chrome\Application\chrome.exe"),
                PathBuf::from(std::env::var("PROGRAMFILES").unwrap_or_default())
                    .join(r"Microsoft\Edge\Application\msedge.exe"),
            ]
        } else {
            // Linux
            vec![
                "/usr/bin/google-chrome".into(),
                "/usr/bin/google-chrome-stable".into(),
                "/usr/bin/chromium".into(),
                "/usr/bin/chromium-browser".into(),
                "/snap/bin/chromium".into(),
                "/usr/bin/brave-browser".into(),
            ]
        };

        for path in &candidates {
            if path.exists() {
                tracing::debug!(path = %path.display(), "Found browser");
                return Ok(path.clone());
            }
        }

        // Try PATH lookup using `which` on Unix or `where` on Windows
        let which_cmd = if cfg!(target_os = "windows") { "where" } else { "which" };
        for browser in ["chromium", "chromium-browser", "google-chrome", "chrome"] {
            if let Ok(output) = std::process::Command::new(which_cmd)
                .arg(browser)
                .output()
            {
                if output.status.success() {
                    let path = String::from_utf8_lossy(&output.stdout)
                        .lines()
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if !path.is_empty() {
                        tracing::debug!(path = %path, "Found browser via PATH");
                        return Ok(PathBuf::from(path));
                    }
                }
            }
        }

        bail!(
            "Could find Chrome/Chromium. Please:\n\
            - Install Chrome, Chromium, or Edge, OR\n\
            - Set --chrome-path to the browser executable, OR\n\
            - Set the CHROME_PATH environment variable\n\n\
            Searched locations:\n  {}",
            candidates
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n  ")
        )
    }
}

/// Parse CSS margin units to inches
fn parse_margin_to_inches(margin: &str) -> f64 {
    let margin = margin.trim().to_lowercase();

    // Try to parse with unit suffix
    if let Some(mm) = margin.strip_suffix("mm") {
        return mm.parse::<f64>().unwrap_or(20.0) / 25.4;
    }
    if let Some(cm) = margin.strip_suffix("cm") {
        return cm.parse::<f64>().unwrap_or(2.0) / 2.54;
    }
    if let Some(inches) = margin.strip_suffix("in") {
        return inches.parse::<f64>().unwrap_or(0.79);
    }
    if let Some(px) = margin.strip_suffix("px") {
        return px.parse::<f64>().unwrap_or(75.6) / 96.0; // 96 DPI
    }
    if let Some(pt) = margin.strip_suffix("pt") {
        return pt.parse::<f64>().unwrap_or(56.7) / 72.0; // 72 pt per inch
    }

    // Default: assume mm if no unit
    margin.parse::<f64>().unwrap_or(20.0) / 25.4
}

#[async_trait::async_trait]
impl Renderer for PdfRenderer {
    #[instrument(skip(self, html, config), fields(html_len = html.len()))]
    async fn render(
        &self,
        html: &str,
        config: &ConversionConfig,
    ) -> anyhow::Result<RenderOutput> {
        // Acquire semaphore permit to limit concurrent browsers
        // This prevents resource exhaustion when processing many files
        let _permit = browser_semaphore()
            .acquire()
            .await
            .context("Failed to acquire browser semaphore")?;

        tracing::debug!("Acquired browser semaphore permit");

        let chrome_path = match &config.chrome_path {
            Some(p) => p.clone(),
            None => Self::find_chrome()?,
        };

        tracing::info!(browser = %chrome_path.display(), "Launching headless browser");

        // Wrap entire operation in overall timeout for predictable behavior
        let overall_timeout = Duration::from_secs(config.timeout_secs);

        let result = tokio::time::timeout(overall_timeout, async {
            // Build browser configuration
            let browser_config = BrowserConfig::builder()
                .chrome_executable(chrome_path)
                .headless_mode(HeadlessMode::True)
                .disable_default_args()
                .arg("--headless=new")
                .arg("--disable-gpu")
                .arg("--no-sandbox")
                .arg("--disable-dev-shm-usage")
                .arg("--disable-extensions")
                .arg("--disable-background-networking")
                .arg("--disable-sync")
                .arg("--disable-translate")
                .arg("--mute-audio")
                .arg("--no-first-run")
                .arg("--safebrowsing-disable-auto-update")
                .request_timeout(Duration::from_secs(config.timeout_secs))
                .build()
                .map_err(anyhow::Error::msg)
                .context("Failed to build browser configuration")?;

            let (browser, mut handler) = Browser::launch(browser_config)
                .await
                .context("Failed to launch browser")?;

            // Spawn handler task and wrap in guard for RAII cleanup
            let handle = tokio::spawn(async move {
                while (handler.next().await).is_some() {}
            });

            // BrowserGuard ensures cleanup on ALL paths (success, error, panic)
            let guard = BrowserGuard::new(browser, handle);

            // Create new page
            let page = guard
                .browser()
                .new_page("about:blank")
                .await
                .context("Failed to create new page")?;

            // Set content
            page.set_content(html)
                .await
                .context("Failed to set page content")?;

            // Wait for network idle (fonts, images)
            tokio::time::sleep(Duration::from_millis(200)).await;

            // Generate PDF with configured options
            let print_params = Self::build_print_params(&config.front_matter.pdf_options);
            let pdf_bytes = page.pdf(print_params).await.context("Failed to generate PDF")?;

            tracing::info!(size_bytes = pdf_bytes.len(), "Generated PDF");

            // Graceful cleanup - explicitly close browser and wait for handler
            guard.close().await;

            Ok::<_, anyhow::Error>(pdf_bytes)
        })
        .await;

        match result {
            Ok(Ok(bytes)) => Ok(RenderOutput {
                bytes,
                extension: "pdf",
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => bail!("PDF generation timed out after {}s", config.timeout_secs),
        }
    }

    fn extension(&self) -> &'static str {
        "pdf"
    }

    fn name(&self) -> &'static str {
        "PDF"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_margin_parsing_mm() {
        assert!((parse_margin_to_inches("25.4mm") - 1.0).abs() < 0.01);
        assert!((parse_margin_to_inches("20mm") - 0.787).abs() < 0.01);
    }

    #[test]
    fn test_margin_parsing_inches() {
        assert!((parse_margin_to_inches("1in") - 1.0).abs() < 0.01);
        assert!((parse_margin_to_inches("0.5in") - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_margin_parsing_cm() {
        assert!((parse_margin_to_inches("2.54cm") - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_margin_parsing_px() {
        assert!((parse_margin_to_inches("96px") - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_margin_parsing_pt() {
        assert!((parse_margin_to_inches("72pt") - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_margin_parsing_default() {
        // No unit = assume mm
        assert!((parse_margin_to_inches("25.4") - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_print_params_a4() {
        let options = PdfOptions {
            format: Some("A4".to_string()),
            ..Default::default()
        };
        let params = PdfRenderer::build_print_params(&options);
        assert!((params.paper_width.unwrap() - 8.27).abs() < 0.01);
        assert!((params.paper_height.unwrap() - 11.69).abs() < 0.01);
    }

    #[test]
    fn test_print_params_landscape() {
        let options = PdfOptions {
            landscape: true,
            ..Default::default()
        };
        let params = PdfRenderer::build_print_params(&options);
        assert_eq!(params.landscape, Some(true));
    }
}
