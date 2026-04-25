use anyhow::{bail, Context, Result};
use chromiumoxide::cdp::browser_protocol::network::{EventLoadingFinished, EventRequestWillBeSent};
use chromiumoxide::cdp::browser_protocol::page::PrintToPdfParams;
use futures::StreamExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::instrument;

use super::browser::browser_pool;
use super::{RenderOutput, Renderer};
use crate::config::{ConversionConfig, PdfOptions};

// Margin conversion constants (QUAL-004)
const INCHES_PER_MM: f64 = 1.0 / 25.4;
const INCHES_PER_CM: f64 = 1.0 / 2.54;
const INCHES_PER_PT: f64 = 1.0 / 72.0;
const INCHES_PER_PX: f64 = 1.0 / 96.0;
const DEFAULT_MARGIN_MM: f64 = 20.0;
const DEFAULT_MARGIN_INCHES: f64 = DEFAULT_MARGIN_MM * INCHES_PER_MM; // ~0.787 inches

struct PaperSize {
    width: f64,
    height: f64,
}

const PAPER_A4: PaperSize = PaperSize { width: 8.27, height: 11.69 };
const PAPER_A3: PaperSize = PaperSize { width: 11.69, height: 16.54 };
const PAPER_LETTER: PaperSize = PaperSize { width: 8.5, height: 11.0 };
const PAPER_LEGAL: PaperSize = PaperSize { width: 8.5, height: 14.0 };
const PAPER_TABLOID: PaperSize = PaperSize { width: 11.0, height: 17.0 };

/// Wait for network activity to settle
///
/// This is more robust than a fixed sleep as it waits until there are no
/// pending network requests for a short period (100ms) or until a timeout.
async fn wait_for_network_idle(page: &chromiumoxide::Page, timeout_ms: u64) -> Result<()> {
    use std::collections::HashSet;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    let (tx, mut rx) = mpsc::channel(1);
    let cancel_token = CancellationToken::new();
    let pending_requests: Arc<
        Mutex<HashSet<chromiumoxide::cdp::browser_protocol::network::RequestId>>,
    > = Arc::new(Mutex::new(HashSet::new()));

    let mut request_events = page.event_listener::<EventRequestWillBeSent>().await?;
    let mut finished_events = page.event_listener::<EventLoadingFinished>().await?;

    let pending_clone = Arc::clone(&pending_requests);
    let cancel_clone = cancel_token.clone();
    let handler = tokio::spawn(async move {
        let mut last_activity = std::time::Instant::now();
        const IDLE_THRESHOLD: Duration = Duration::from_millis(100);
        const CHECK_INTERVAL: Duration = Duration::from_millis(50);

        loop {
            tokio::select! {
                _ = cancel_clone.cancelled() => {
                    return;
                }
                Some(event) = request_events.next() => {
                    pending_clone.lock().await.insert(event.request_id.clone());
                    last_activity = std::time::Instant::now();
                }
                Some(event) = finished_events.next() => {
                    pending_clone.lock().await.remove(&event.request_id);
                    last_activity = std::time::Instant::now();
                }
                _ = tokio::time::sleep(CHECK_INTERVAL) => {
                    let pending = pending_clone.lock().await;
                    if pending.is_empty() && last_activity.elapsed() >= IDLE_THRESHOLD {
                        let _ = tx.send(()).await;
                        return;
                    }
                }
            }
        }
    });

    let timeout = Duration::from_millis(timeout_ms);
    let result = tokio::time::timeout(timeout, rx.recv()).await;

    cancel_token.cancel();
    let _ = handler.await; // Wait for handler to finish

    match result {
        Ok(Some(())) => Ok(()),
        _ => bail!("Timed out waiting for network idle"),
    }
}

/// A renderer that uses Headless Chrome to generate high-quality PDF documents.
///
/// This renderer supports complex CSS layouts, web fonts, and JavaScript-driven
/// content. It utilizes a `BrowserPool` to manage multiple Chrome instances
/// efficiently.
pub struct PdfRenderer;

impl Default for PdfRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl PdfRenderer {
    /// Creates a new PDF renderer instance.
    pub fn new() -> Self {
        Self
    }

    fn build_print_params(options: &PdfOptions) -> PrintToPdfParams {
        let mut params = PrintToPdfParams::default();

        // 1. apply paper format (QUAL-003)
        Self::apply_paper_format(&mut params, options.format.as_deref());

        // 2. Resolve and apply margins (QUAL-002, QUAL-003)
        let default_margin = resolve_margin(options.margin.as_deref(), DEFAULT_MARGIN_INCHES);

        params.margin_top = Some(resolve_margin(
            options.margin_top.as_deref(),
            default_margin,
        ));
        params.margin_bottom = Some(resolve_margin(
            options.margin_bottom.as_deref(),
            default_margin,
        ));
        params.margin_left = Some(resolve_margin(
            options.margin_left.as_deref(),
            default_margin,
        ));
        params.margin_right = Some(resolve_margin(
            options.margin_right.as_deref(),
            default_margin,
        ));

        params.print_background = Some(options.print_background);
        params.landscape = Some(options.landscape);

        // 3. Validate and clamp scale factor
        params.scale = Some(Self::clamp_scale(options.scale));
        params.prefer_css_page_size = Some(false);

        // Header/footer templates
        if let Some(header) = &options.header_template {
            params.display_header_footer = Some(true);
            params.header_template = Some(header.clone());
        }
        if let Some(footer) = &options.footer_template {
            params.display_header_footer = Some(true);
            params.footer_template = Some(footer.clone());
        }

        params
    }

    /// Helper to apply paper format dimensions
    fn apply_paper_format(params: &mut PrintToPdfParams, format: Option<&str>) {
        if let Some(f) = format {
            match f.to_lowercase().as_str() {
                "a4" => {
                    params.paper_width = Some(PAPER_A4.width);
                    params.paper_height = Some(PAPER_A4.height);
                }
                "a3" => {
                    params.paper_width = Some(PAPER_A3.width);
                    params.paper_height = Some(PAPER_A3.height);
                }
                "letter" => {
                    params.paper_width = Some(PAPER_LETTER.width);
                    params.paper_height = Some(PAPER_LETTER.height);
                }
                "legal" => {
                    params.paper_width = Some(PAPER_LEGAL.width);
                    params.paper_height = Some(PAPER_LEGAL.height);
                }
                "tabloid" => {
                    params.paper_width = Some(PAPER_TABLOID.width);
                    params.paper_height = Some(PAPER_TABLOID.height);
                }
                _ => tracing::warn!(format = %f, "Unknown paper format, using default"),
            }
        }
    }

    /// Helper to clamp scale factor to safe range [0.1, 2.0]
    fn clamp_scale(scale: f64) -> f64 {
        if scale.is_nan() || scale.is_infinite() {
            tracing::warn!(
                scale = scale,
                "Invalid scale factor (NaN/Infinity), using default 1.0"
            );
            1.0
        } else {
            let clamped = scale.clamp(0.1, 2.0);
            if (clamped - scale).abs() > f64::EPSILON {
                tracing::warn!(
                    requested = scale,
                    actual = clamped,
                    "Scale factor clamped to valid range [0.1, 2.0]"
                );
            }
            clamped
        }
    }

    async fn find_chrome() -> anyhow::Result<PathBuf> {
        // 1. Check candidates for current platform
        let candidates = Self::get_platform_chrome_paths();
        for path in &candidates {
            if tokio::fs::try_exists(path).await.unwrap_or(false) {
                tracing::debug!(path = %path.display(), "Found browser");
                return Ok(path.clone());
            }
        }

        // 2. Try PATH lookup
        if let Some(path) = Self::search_chrome_in_path().await {
            tracing::debug!(path = %path.display(), "Found browser via PATH");
            return Ok(path);
        }

        bail!(
            "Could not find Chrome/Chromium. Please:\n\
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

    /// Get list of common Chrome/Chromium locations for current platform
    fn get_platform_chrome_paths() -> Vec<PathBuf> {
        if cfg!(target_os = "macos") {
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
        }
    }

    /// Try to find Chrome executable in system PATH
    async fn search_chrome_in_path() -> Option<PathBuf> {
        let (which_cmd, browsers) = if cfg!(target_os = "windows") {
            ("where", vec!["chrome", "msedge"])
        } else {
            (
                "which",
                vec![
                    "chromium",
                    "chromium-browser",
                    "google-chrome",
                    "google-chrome-stable",
                    "chrome",
                ],
            )
        };

        for browser in browsers {
            if let Ok(output) = tokio::process::Command::new(which_cmd)
                .arg(browser)
                .output()
                .await
            {
                if output.status.success() {
                    let path = String::from_utf8_lossy(&output.stdout)
                        .lines()
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if !path.is_empty() {
                        return Some(PathBuf::from(path));
                    }
                }
            }
        }
        None
    }
}

/// Parse CSS margin units to inches
fn parse_margin_to_inches(margin: &str) -> f64 {
    let margin = margin.trim().to_lowercase();

    // Default fallback values in respective units (derived from DEFAULT_MARGIN_MM)
    let default_px = DEFAULT_MARGIN_MM / INCHES_PER_MM / INCHES_PER_PX; // ~75.6 px
    let default_pt = DEFAULT_MARGIN_MM / INCHES_PER_MM / INCHES_PER_PT; // ~56.7 pt
    let default_cm = DEFAULT_MARGIN_MM / 10.0; // 2.0 cm

    // Try to parse with unit suffix
    if let Some(mm) = margin.strip_suffix("mm") {
        return mm.parse::<f64>().unwrap_or(DEFAULT_MARGIN_MM) * INCHES_PER_MM;
    }
    if let Some(cm) = margin.strip_suffix("cm") {
        return cm.parse::<f64>().unwrap_or(default_cm) * INCHES_PER_CM;
    }
    if let Some(inches) = margin.strip_suffix("in") {
        return inches.parse::<f64>().unwrap_or(DEFAULT_MARGIN_INCHES);
    }
    if let Some(px) = margin.strip_suffix("px") {
        return px.parse::<f64>().unwrap_or(default_px) * INCHES_PER_PX;
    }
    if let Some(pt) = margin.strip_suffix("pt") {
        return pt.parse::<f64>().unwrap_or(default_pt) * INCHES_PER_PT;
    }

    // Default: assume mm if no unit
    margin.parse::<f64>().unwrap_or(DEFAULT_MARGIN_MM) * INCHES_PER_MM
}

/// Resolve a specific margin value, falling back to a default
fn resolve_margin(specific: Option<&str>, default: f64) -> f64 {
    specific.map(parse_margin_to_inches).unwrap_or(default)
}

#[async_trait::async_trait]
impl Renderer for PdfRenderer {
    /// Renders HTML content to a PDF buffer via Headless Chrome.
    ///
    /// This method:
    /// 1. Acquires a browser from the pool.
    /// 2. Creates a new page and sets its content to the provided HTML.
    /// 3. Waits for the network to be idle (up to 5 seconds) to ensure all assets are loaded.
    /// 4. Triggers the `PrintToPDF` command with user-specified parameters.
    ///
    /// # Errors
    /// Returns an error if browser acquisition fails, the page cannot be created,
    /// or the PDF generation command fails or times out.
    #[instrument(skip(self, html, config), fields(html_len = html.len()))]
    async fn render(&self, html: &str, config: &ConversionConfig) -> anyhow::Result<RenderOutput> {
        let chrome_path = match &config.chrome_path {
            Some(configured_path) => configured_path.clone(),
            None => Self::find_chrome().await?,
        };

        // Wrap entire operation in overall timeout for predictable behavior
        let overall_timeout = Duration::from_secs(config.timeout_secs);

        let result = tokio::time::timeout(overall_timeout, async {
            // Acquire browser from pool (this also limits concurrency via semaphore internally)
            let mut lease = browser_pool()
                .acquire(&chrome_path, config.timeout_secs, config.no_sandbox)
                .await?;

            // Create new page on the pooled browser
            let page = lease
                .browser()
                .new_page("about:blank")
                .await
                .context("Failed to create new page")?;

            // Set content
            page.set_content(html)
                .await
                .context("Failed to set page content")?;

            // Wait for network idle (fonts, images)
            // This is more efficient and robust than a fixed sleep
            if let Err(e) = wait_for_network_idle(&page, 5000).await {
                tracing::warn!(error = %e, "Failed to wait for network idle, proceeding anyway");
            }

            // Generate PDF with configured options
            let print_params = Self::build_print_params(&config.front_matter.pdf_options);
            let pdf_bytes = page
                .pdf(print_params)
                .await
                .context("Failed to generate PDF")?;

            tracing::info!(size_bytes = pdf_bytes.len(), "Generated PDF");

            // Mark browser as used and release back to pool
            lease.mark_used();
            lease.release().await;

            Ok::<_, anyhow::Error>(pdf_bytes)
        })
        .await;

        match result {
            Ok(res) => res.map(|bytes| RenderOutput {
                bytes,
                extension: "pdf",
            }),
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

    #[test]
    fn test_scale_factor_valid() {
        let options = PdfOptions {
            scale: 1.5,
            ..Default::default()
        };
        let params = PdfRenderer::build_print_params(&options);
        assert!((params.scale.unwrap() - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_scale_factor_clamped_high() {
        let options = PdfOptions {
            scale: 10.0,
            ..Default::default()
        };
        let params = PdfRenderer::build_print_params(&options);
        assert!((params.scale.unwrap() - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_scale_factor_clamped_low() {
        let options = PdfOptions {
            scale: 0.01,
            ..Default::default()
        };
        let params = PdfRenderer::build_print_params(&options);
        assert!((params.scale.unwrap() - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn test_scale_factor_zero() {
        let options = PdfOptions {
            scale: 0.0,
            ..Default::default()
        };
        let params = PdfRenderer::build_print_params(&options);
        assert!((params.scale.unwrap() - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn test_scale_factor_negative() {
        let options = PdfOptions {
            scale: -1.0,
            ..Default::default()
        };
        let params = PdfRenderer::build_print_params(&options);
        assert!((params.scale.unwrap() - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn test_scale_factor_nan() {
        let options = PdfOptions {
            scale: f64::NAN,
            ..Default::default()
        };
        let params = PdfRenderer::build_print_params(&options);
        assert!((params.scale.unwrap() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_scale_factor_infinity() {
        let options = PdfOptions {
            scale: f64::INFINITY,
            ..Default::default()
        };
        let params = PdfRenderer::build_print_params(&options);
        assert!((params.scale.unwrap() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_scale_factor_neg_infinity() {
        let options = PdfOptions {
            scale: f64::NEG_INFINITY,
            ..Default::default()
        };
        let params = PdfRenderer::build_print_params(&options);
        assert!((params.scale.unwrap() - 1.0).abs() < f64::EPSILON);
    }

    // ============ Edge Case Tests (TEST-004) ============

    #[test]
    fn test_margin_parsing_empty_string() {
        // Empty string should use default (20mm = ~0.787 inches)
        let result = parse_margin_to_inches("");
        assert!(
            (result - DEFAULT_MARGIN_INCHES).abs() < 0.01,
            "Empty should default to ~20mm"
        );
    }

    #[test]
    fn test_margin_parsing_whitespace_only() {
        let result = parse_margin_to_inches("   ");
        assert!(
            (result - DEFAULT_MARGIN_INCHES).abs() < 0.01,
            "Whitespace should default to ~20mm"
        );
    }

    #[test]
    fn test_margin_parsing_negative_value() {
        // Negative values: parser will parse, result is negative
        let result = parse_margin_to_inches("-10mm");
        assert!(result < 0.0, "Negative margin should be negative");
    }

    #[test]
    fn test_margin_parsing_zero() {
        let result = parse_margin_to_inches("0mm");
        assert!((result - 0.0).abs() < 0.01, "0mm should be 0 inches");
    }

    #[test]
    fn test_margin_parsing_zero_no_unit() {
        let result = parse_margin_to_inches("0");
        assert!((result - 0.0).abs() < 0.01, "0 should be 0 inches");
    }

    #[test]
    fn test_margin_parsing_invalid_number() {
        // "abc" cannot be parsed, should return default
        let result = parse_margin_to_inches("abc");
        assert!(
            (result - DEFAULT_MARGIN_INCHES).abs() < 0.01,
            "Invalid should default"
        );
    }

    #[test]
    fn test_margin_parsing_very_large_value() {
        let result = parse_margin_to_inches("1000mm");
        assert!(
            (result - (1000.0 * INCHES_PER_MM)).abs() < 0.01,
            "Large value should work"
        );
    }

    #[test]
    fn test_margin_parsing_decimal() {
        let result = parse_margin_to_inches("12.5mm");
        assert!(
            (result - (12.5 * INCHES_PER_MM)).abs() < 0.01,
            "Decimal should work"
        );
    }

    #[test]
    fn test_margin_parsing_uppercase_unit() {
        let result = parse_margin_to_inches("25.4MM");
        assert!((result - 1.0).abs() < 0.01, "Uppercase unit should work");
    }

    #[test]
    fn test_margin_parsing_mixed_case_unit() {
        let result = parse_margin_to_inches("25.4Mm");
        assert!((result - 1.0).abs() < 0.01, "Mixed case unit should work");
    }

    #[test]
    fn test_margin_parsing_extra_whitespace() {
        let result = parse_margin_to_inches("  25.4mm  ");
        assert!(
            (result - 1.0).abs() < 0.01,
            "Extra whitespace should be trimmed"
        );
    }

    #[test]
    fn test_print_params_unknown_format() {
        let options = PdfOptions {
            format: Some("unknown_format".to_string()),
            ..Default::default()
        };
        let params = PdfRenderer::build_print_params(&options);
        // Unknown format should leave paper_width/height as None (Chrome default)
        assert!(
            params.paper_width.is_none() && params.paper_height.is_none(),
            "Unknown format should not set paper dimensions"
        );
    }

    #[test]
    fn test_print_params_all_margins() {
        let options = PdfOptions {
            margin_top: Some("10mm".to_string()),
            margin_bottom: Some("20mm".to_string()),
            margin_left: Some("15mm".to_string()),
            margin_right: Some("25mm".to_string()),
            ..Default::default()
        };
        let params = PdfRenderer::build_print_params(&options);
        assert!((params.margin_top.unwrap() - (10.0 * INCHES_PER_MM)).abs() < 0.01);
        assert!((params.margin_bottom.unwrap() - (20.0 * INCHES_PER_MM)).abs() < 0.01);
        assert!((params.margin_left.unwrap() - (15.0 * INCHES_PER_MM)).abs() < 0.01);
        assert!((params.margin_right.unwrap() - (25.0 * INCHES_PER_MM)).abs() < 0.01);
    }

    #[test]
    fn test_print_params_with_header_footer() {
        let options = PdfOptions {
            header_template: Some("<span class='date'></span>".to_string()),
            footer_template: Some("<span class='pageNumber'></span>".to_string()),
            ..Default::default()
        };
        let params = PdfRenderer::build_print_params(&options);
        assert!(params.display_header_footer.unwrap());
        assert_eq!(params.header_template, options.header_template);
        assert_eq!(params.footer_template, options.footer_template);
    }

    #[test]
    fn test_print_params_letter_format() {
        let options = PdfOptions {
            format: Some("letter".to_string()),
            ..Default::default()
        };
        let params = PdfRenderer::build_print_params(&options);
        assert!((params.paper_width.unwrap() - 8.5).abs() < 0.01);
        assert!((params.paper_height.unwrap() - 11.0).abs() < 0.01);
    }

    #[test]
    fn test_print_params_legal_format() {
        let options = PdfOptions {
            format: Some("legal".to_string()),
            ..Default::default()
        };
        let params = PdfRenderer::build_print_params(&options);
        assert!((params.paper_width.unwrap() - 8.5).abs() < 0.01);
        assert!((params.paper_height.unwrap() - 14.0).abs() < 0.01);
    }

    #[test]
    fn test_print_params_tabloid_format() {
        let options = PdfOptions {
            format: Some("tabloid".to_string()),
            ..Default::default()
        };
        let params = PdfRenderer::build_print_params(&options);
        assert!((params.paper_width.unwrap() - 11.0).abs() < 0.01);
        assert!((params.paper_height.unwrap() - 17.0).abs() < 0.01);
    }
}
