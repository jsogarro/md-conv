use anyhow::{bail, Context, Result};
use chromiumoxide::browser::{Browser, BrowserConfig, HeadlessMode};
use chromiumoxide::cdp::browser_protocol::network::{EventLoadingFinished, EventRequestWillBeSent};
use chromiumoxide::cdp::browser_protocol::page::PrintToPdfParams;
use chromiumoxide::handler::viewport::Viewport;
use futures::StreamExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;
use tracing::instrument;

use super::{RenderOutput, Renderer};
use crate::config::{ConversionConfig, PdfOptions};

/// Maximum concurrent browser instances for batch processing.
/// This prevents resource exhaustion when processing many files.
const MAX_CONCURRENT_BROWSERS: usize = 3;

/// Maximum age of a pooled browser before it should be recycled (5 minutes)
const BROWSER_MAX_AGE_SECS: u64 = 300;

/// Maximum number of pages rendered before recycling a browser
const BROWSER_MAX_RENDERS: usize = 50;

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

// =============================================================================
// BrowserPool Implementation
// =============================================================================

/// A thread-safe, singleton browser connection pool for reusing Chrome instances.
///
/// Reusing browser instances significantly improves performance by reducing the
/// high overhead of launching the Chrome executable for every PDF conversion.
///
/// The pool manages:
/// - **Concurrencry**: Bounded by a semaphore (`MAX_CONCURRENT_BROWSERS`).
/// - **Lifecycle**: Recycles browsers based on age (`BROWSER_MAX_AGE_SECS`) or
///   usage count (`BROWSER_MAX_RENDERS`) to avoid memory leaks.
/// - **Cleanup**: Gracefully shuts down all instances on demand.
pub struct BrowserPool {
    inner: Arc<BrowserPoolInner>,
}

/// A wrapper around a Chrome-compatible browser instance with lifecycle metadata.
struct PooledBrowser {
    /// The actual `chromiumoxide` browser handle.
    browser: Browser,
    /// The background message handler task.
    handler: JoinHandle<()>,
    /// Instant when the browser process was launched.
    created_at: std::time::Instant,
    /// Number of documents rendered by this instance.
    render_count: usize,
    /// Temporary user profile directory (automatically cleaned up on drop).
    _profile_dir: Option<tempfile::TempDir>,
}

impl PooledBrowser {
    fn new(
        browser: Browser,
        handler: JoinHandle<()>,
        _profile_dir: Option<tempfile::TempDir>,
    ) -> Self {
        Self {
            browser,
            handler,
            created_at: std::time::Instant::now(),
            render_count: 0,
            _profile_dir,
        }
    }

    /// Determines if the browser instance should be closed and replaced.
    ///
    /// Recycling helps mitigate potential memory leaks or stability issues in long-running
    /// headless Chrome processes.
    fn should_recycle(&self) -> bool {
        self.created_at.elapsed().as_secs() > BROWSER_MAX_AGE_SECS
            || self.render_count >= BROWSER_MAX_RENDERS
    }

    fn increment_render_count(&mut self) {
        self.render_count += 1;
    }

    fn browser(&self) -> &Browser {
        &self.browser
    }

    /// Attempts a graceful shutdown of the Chrome process.
    async fn close(mut self) {
        let _ = self.browser.close().await;
        drop(self.browser);

        match tokio::time::timeout(Duration::from_secs(5), self.handler).await {
            Ok(Ok(())) => tracing::debug!("Pooled browser closed gracefully"),
            Ok(Err(e)) => tracing::warn!(error = %e, "Pooled browser handler panicked"),
            Err(_) => {
                tracing::warn!("Pooled browser handler close timed out");
            }
        }
    }
}

/// A smart pointer that provides exclusive access to a pooled browser instance.
///
/// When the lease is dropped or explicitly released, the browser is returned
/// to the pool for reuse, or recycled if it has exceeded its lifespan.
pub struct BrowserLease {
    browser: Option<PooledBrowser>,
    pool: Arc<BrowserPoolInner>,
    /// Holds the semaphore permit for the duration of the lease.
    _permit: OwnedSemaphorePermit,
}

impl BrowserLease {
    /// Provides access to the underlying `chromiumoxide::Browser`.
    pub fn browser(&self) -> &Browser {
        self.browser
            .as_ref()
            .expect("browser already consumed")
            .browser()
    }

    /// Informs the pool that a rendering operation was performed.
    pub fn mark_used(&mut self) {
        if let Some(ref mut b) = self.browser {
            b.increment_render_count();
        }
    }

    /// Returns the browser to the pool immediately.
    pub async fn release(mut self) {
        if let Some(browser) = self.browser.take() {
            self.pool.return_browser(browser).await;
        }
    }
}

impl Drop for BrowserLease {
    fn drop(&mut self) {
        if let Some(browser) = self.browser.take() {
            let pool = Arc::clone(&self.pool);
            tokio::spawn(async move {
                if let Err(e) = pool.return_browser_with_result(browser).await {
                    tracing::error!("Failed to return browser to pool: {}", e);
                }
            });
        }
    }
}

struct BrowserPoolConfig {
    chrome_path: Option<PathBuf>,
    timeout_secs: u64,
}

struct BrowserPoolInner {
    available: Mutex<Vec<PooledBrowser>>,
    config: Mutex<BrowserPoolConfig>,
    semaphore: Arc<Semaphore>,
}

impl BrowserPoolInner {
    fn new() -> Self {
        Self {
            available: Mutex::new(Vec::with_capacity(MAX_CONCURRENT_BROWSERS)),
            config: Mutex::new(BrowserPoolConfig {
                chrome_path: None,
                timeout_secs: 30,
            }),
            semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_BROWSERS)),
        }
    }

    async fn return_browser(&self, browser: PooledBrowser) {
        let _ = self.return_browser_with_result(browser).await;
    }

    async fn return_browser_with_result(&self, browser: PooledBrowser) -> anyhow::Result<()> {
        if browser.should_recycle() {
            tracing::debug!(
                age_secs = browser.created_at.elapsed().as_secs(),
                render_count = browser.render_count,
                "Recycling old browser"
            );
            tokio::spawn(async move {
                browser.close().await;
            });
        } else {
            let mut available = self.available.lock().await;
            available.push(browser);
            tracing::debug!(pool_size = available.len(), "Browser returned to pool");
        }
        Ok(())
    }
}

impl BrowserPool {
    /// Creates a new, empty browser pool.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(BrowserPoolInner::new()),
        }
    }

    /// Sets global configuration used when launching new browser instances.
    pub async fn configure(&self, chrome_path: PathBuf, timeout_secs: u64) {
        let mut config = self.inner.config.lock().await;
        config.chrome_path = Some(chrome_path);
        config.timeout_secs = timeout_secs;
    }

    /// Borrows a browser from the pool or launches a new one if necessary.
    ///
    /// This method will wait if the maximum number of concurrent browsers has been reached.
    pub async fn acquire(
        &self,
        chrome_path: &PathBuf,
        timeout_secs: u64,
        no_sandbox: bool,
    ) -> anyhow::Result<BrowserLease> {
        // Acquire semaphore permit first to limit concurrent browsers
        let permit = self
            .inner
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .context("Failed to acquire browser semaphore")?;

        tracing::debug!("Acquired browser semaphore permit");

        // Try to get an existing browser from the pool
        let existing = {
            let mut available = self.inner.available.lock().await;
            available.pop()
        };

        let browser = if let Some(pooled) = existing {
            // Verify the pooled browser is still healthy
            if pooled.should_recycle() {
                tracing::debug!("Pooled browser needs recycling, creating new one");
                // Close old browser asynchronously
                tokio::spawn(async move {
                    pooled.close().await;
                });
                // Create new browser
                self.create_browser(chrome_path, timeout_secs, no_sandbox)
                    .await?
            } else {
                tracing::debug!(
                    age_secs = pooled.created_at.elapsed().as_secs(),
                    render_count = pooled.render_count,
                    "Reusing pooled browser"
                );
                pooled
            }
        } else {
            tracing::debug!("No pooled browsers available, creating new one");
            self.create_browser(chrome_path, timeout_secs, no_sandbox)
                .await?
        };

        Ok(BrowserLease {
            browser: Some(browser),
            pool: Arc::clone(&self.inner),
            _permit: permit,
        })
    }

    /// Create a new browser instance
    async fn create_browser(
        &self,
        chrome_path: &PathBuf,
        timeout_secs: u64,
        no_sandbox: bool,
    ) -> anyhow::Result<PooledBrowser> {
        tracing::info!(
            browser = %chrome_path.display(),
            no_sandbox = no_sandbox,
            "Launching headless browser"
        );

        // Create a unique temporary directory for this browser's profile
        // This prevents "SingletonLock" errors when multiple browsers are launched
        let profile_dir = tempfile::Builder::new()
            .prefix("md-conv-profile-")
            .tempdir()
            .context("Failed to create temporary profile directory")?;

        let mut builder = BrowserConfig::builder()
            .chrome_executable(chrome_path)
            .user_data_dir(profile_dir.path())
            .headless_mode(HeadlessMode::True)
            .disable_default_args()
            .arg("--disable-gpu")
            .arg("--disable-dev-shm-usage")
            .arg("--disable-extensions")
            .arg("--disable-background-networking")
            .arg("--disable-sync")
            .arg("--disable-translate")
            .arg("--mute-audio")
            .arg("--no-first-run")
            .arg("--safebrowsing-disable-auto-update")
            .request_timeout(Duration::from_secs(timeout_secs))
            .viewport(Viewport {
                width: 800,
                height: 1100,
                device_scale_factor: Some(1.0),
                ..Default::default()
            });

        if no_sandbox {
            tracing::warn!("DANGEROUS: Running browser with --no-sandbox");
            builder = builder.arg("--no-sandbox");
        }

        let browser_config = builder
            .build()
            .map_err(anyhow::Error::msg)
            .context("Failed to build browser configuration")?;

        let (browser, mut handler) = Browser::launch(browser_config)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to launch browser: {:?}", e))?;

        // Spawn handler task
        let handle = tokio::spawn(async move { while (handler.next().await).is_some() {} });

        Ok(PooledBrowser::new(browser, handle, Some(profile_dir)))
    }

    /// Gracefully shutdown the pool, closing all browsers
    pub async fn shutdown(&self) {
        let browsers = {
            let mut available = self.inner.available.lock().await;
            std::mem::take(&mut *available)
        };

        tracing::info!(count = browsers.len(), "Shutting down browser pool");

        for browser in browsers {
            browser.close().await;
        }
    }

    /// Get current pool statistics
    pub async fn stats(&self) -> (usize, usize) {
        let available = self.inner.available.lock().await;
        let available_count = available.len();
        let in_use = MAX_CONCURRENT_BROWSERS - self.inner.semaphore.available_permits();
        (available_count, in_use)
    }
}

impl Default for BrowserPool {
    fn default() -> Self {
        Self::new()
    }
}

/// Global browser pool instance
static BROWSER_POOL: std::sync::OnceLock<BrowserPool> = std::sync::OnceLock::new();

/// Get the global browser pool
pub fn browser_pool() -> &'static BrowserPool {
    BROWSER_POOL.get_or_init(BrowserPool::new)
}

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

    #[tokio::test]
    async fn test_browser_pool_configure() {
        let pool = BrowserPool::new();
        let chrome_path = PathBuf::from("/usr/bin/google-chrome");
        pool.configure(chrome_path.clone(), 45).await;

        let config = pool.inner.config.lock().await;
        assert_eq!(config.chrome_path, Some(chrome_path));
        assert_eq!(config.timeout_secs, 45);
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
