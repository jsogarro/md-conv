//! Browser pool management module
//!
//! This module provides a thread-safe, singleton browser connection pool
//! for reusing Chrome instances across multiple PDF conversions.

use anyhow::Context;
use chromiumoxide::browser::{Browser, BrowserConfig, HeadlessMode};
use chromiumoxide::handler::viewport::Viewport;
use futures::StreamExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;

/// Maximum concurrent browser instances for batch processing.
///
/// Chosen to balance throughput with system resource constraints. Each Chrome instance
/// consumes ~200-400MB of RAM. 3 instances provides good concurrency without overwhelming
/// most systems.
const MAX_CONCURRENT_BROWSERS: usize = 3;

/// Maximum age of a pooled browser before it should be recycled (5 minutes).
///
/// Long-running Chrome processes can accumulate memory leaks or become unstable.
/// Recycling prevents degraded performance over time.
const BROWSER_MAX_AGE_SECS: u64 = 300;

/// Maximum number of pages rendered before recycling a browser.
///
/// After 50 PDF renders, Chrome's internal state may become fragmented. Recycling
/// ensures consistent performance and prevents memory growth.
const BROWSER_MAX_RENDERS: usize = 50;

/// A wrapper around a Chrome-compatible browser instance with lifecycle metadata.
///
/// Tracks usage statistics to enable recycling decisions based on age and render count.
/// The temporary profile directory is automatically cleaned up when this struct is dropped.
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
/// Implements the RAII pattern: when the lease is dropped or explicitly released,
/// the browser is automatically returned to the pool for reuse, or recycled if it
/// has exceeded its lifespan.
///
/// # Drop Behavior
///
/// On drop, the browser is asynchronously returned to the pool via `tokio::spawn`.
/// This ensures drop never blocks even though returning to the pool is async.
pub(crate) struct BrowserLease {
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

#[allow(dead_code)]
struct BrowserPoolConfig {
    chrome_path: Option<PathBuf>,
    timeout_secs: u64,
}

#[allow(dead_code)]
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

/// A thread-safe, singleton browser connection pool for reusing Chrome instances.
///
/// Reusing browser instances significantly improves performance by reducing the
/// high overhead of launching the Chrome executable for every PDF conversion.
/// Benchmarks show ~5x speedup for batch conversions (3s vs 15s for 10 files).
///
/// # Thread Safety
///
/// All methods are async and use `tokio::sync::Mutex` for interior mutability.
/// The pool is safe to share across threads via `Arc`.
///
/// # Configuration
///
/// Browser instances are configured with:
/// - User data directory: Unique temporary directory per instance
/// - Headless mode: Always enabled
/// - Viewport: 800x1100 pixels (suitable for A4/Letter)
/// - Sandbox: Enabled by default (disable with `--no-sandbox` flag)
///
/// The pool manages:
/// - **Concurrency**: Bounded by a semaphore (`MAX_CONCURRENT_BROWSERS`).
/// - **Lifecycle**: Recycles browsers based on age (`BROWSER_MAX_AGE_SECS`) or
///   usage count (`BROWSER_MAX_RENDERS`) to avoid memory leaks.
/// - **Cleanup**: Gracefully shuts down all instances on demand.
pub(crate) struct BrowserPool {
    inner: Arc<BrowserPoolInner>,
}

#[allow(dead_code)]
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
pub(crate) fn browser_pool() -> &'static BrowserPool {
    BROWSER_POOL.get_or_init(BrowserPool::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_browser_pool_configure() {
        let pool = BrowserPool::new();
        let chrome_path = PathBuf::from("/usr/bin/google-chrome");
        pool.configure(chrome_path.clone(), 45).await;

        let config = pool.inner.config.lock().await;
        assert_eq!(config.chrome_path, Some(chrome_path));
        assert_eq!(config.timeout_secs, 45);
    }
}
