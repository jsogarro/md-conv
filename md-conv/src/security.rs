//! # Security and Validation
//!
//! This module provides the primary defense-in-depth mechanisms for `md-conv`.
//! It focuses on two main areas:
//! 1. **Path Safety**: Preventing directory traversal attacks (TOCTOU-safe).
//! 2. **Content Sanitization**: Protecting against XSS and malicious CSS constructs.
//!
//! ## TOCTOU Protection
//!
//! Path validation uses platform-specific file descriptor to path resolution to
//! eliminate Time-of-Check to Time-of-Use vulnerabilities:
//!
//! - **Linux**: `/proc/self/fd/<fd>` symlink resolution
//! - **macOS**: `fcntl(F_GETPATH)` system call
//! - **Windows**: `GetFinalPathNameByHandleW` Win32 API
//!
//! This ensures that path validation occurs on the already-opened file descriptor,
//! eliminating the race condition window where an attacker could replace a file
//! with a symlink.
//!
//! All file I/O operations should go through the validation functions in this module.

use anyhow::{bail, Context, Result};
use lightningcss::rules::CssRule;
use lightningcss::stylesheet::{ParserOptions, PrinterOptions, StyleSheet};
use lightningcss::values::url::Url;
use lightningcss::visitor::{Visit, VisitTypes, Visitor};
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::AsyncReadExt;

/// Dangerous URL schemes that could lead to script execution
const DANGEROUS_URL_SCHEMES: &[&str] = &["javascript:", "vbscript:", "data:", "file:"];

// ============================================================================
// Platform-Specific File Descriptor to Path Resolution
// ============================================================================
//
// These functions implement TOCTOU-safe path resolution by asking the OS
// "what is the absolute path of this already-opened file descriptor?"
// rather than re-resolving the original path string.

/// Get the canonical path of an already-opened file descriptor (Linux).
///
/// Uses `/proc/self/fd/<fd>` symlink resolution.
#[cfg(target_os = "linux")]
fn get_path_from_fd_linux(fd: std::os::unix::io::RawFd) -> Result<PathBuf> {
    use std::os::unix::io::AsRawFd;

    let proc_path = format!("/proc/self/fd/{}", fd);
    std::fs::read_link(&proc_path).with_context(|| {
        format!(
            "Failed to resolve file descriptor {} via /proc/self/fd. \
             This may indicate /proc is not mounted (rare on Linux) or a permissions issue. \
             Troubleshooting: Check 'mount | grep proc' and ensure process has read access to /proc.",
            fd
        )
    })
}

/// Get the canonical path of an already-opened file descriptor (macOS).
///
/// Uses `fcntl(F_GETPATH)` system call.
#[cfg(target_os = "macos")]
fn get_path_from_fd_macos(fd: std::os::unix::io::RawFd) -> Result<PathBuf> {
    use std::ffi::CStr;
    use std::os::raw::{c_char, c_int};

    const F_GETPATH: c_int = 50; // macOS-specific fcntl command
    // Use system-defined PATH_MAX for robustness (currently 1024 on macOS)
    const PATH_MAX: usize = libc::PATH_MAX as usize;

    let mut buf = vec![0u8; PATH_MAX];

    unsafe {
        let ret = libc::fcntl(fd, F_GETPATH, buf.as_mut_ptr() as *mut c_char);
        if ret == -1 {
            let err = std::io::Error::last_os_error();
            bail!("fcntl(F_GETPATH) failed for fd {}: {}", fd, err);
        }
    }

    // Find the null terminator and convert to PathBuf
    let c_str = CStr::from_bytes_until_nul(&buf)
        .with_context(|| format!("Invalid path string from fcntl(F_GETPATH) for fd {}", fd))?;

    let path_str = c_str
        .to_str()
        .with_context(|| format!("Non-UTF8 path from fcntl(F_GETPATH) for fd {}", fd))?;

    Ok(PathBuf::from(path_str))
}

/// Get the canonical path of an already-opened file descriptor (Windows).
///
/// Uses `GetFinalPathNameByHandleW` Win32 API.
#[cfg(target_os = "windows")]
fn get_path_from_fd_windows(handle: std::os::windows::io::RawHandle) -> Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use std::ffi::OsString;
    use winapi::um::fileapi::GetFinalPathNameByHandleW;
    use winapi::um::winnt::FILE_NAME_NORMALIZED;

    const BUFFER_SIZE: usize = 32768; // Windows max path length
    let mut buffer = vec![0u16; BUFFER_SIZE];

    unsafe {
        let len = GetFinalPathNameByHandleW(
            handle as *mut _,
            buffer.as_mut_ptr(),
            BUFFER_SIZE as u32,
            FILE_NAME_NORMALIZED,
        );

        if len == 0 {
            let err = std::io::Error::last_os_error();
            bail!("GetFinalPathNameByHandleW failed: {}", err);
        }

        if len as usize >= BUFFER_SIZE {
            bail!("Path too long for Windows buffer (max {} bytes)", BUFFER_SIZE);
        }

        let os_string = OsString::from_wide(&buffer[..len as usize]);
        let mut path = PathBuf::from(os_string);

        // Windows returns paths with \\?\ prefix - strip it for consistency
        // Fail explicitly if path contains non-UTF8 characters (security: fail closed)
        let path_str = path
            .to_str()
            .context("Windows path contains non-UTF8 characters - cannot safely validate")?;

        if path_str.starts_with(r"\\?\") {
            path = PathBuf::from(&path_str[4..]);
        }

        Ok(path)
    }
}

/// Get the canonical path of an already-opened file descriptor.
///
/// This is TOCTOU-safe because we ask the OS for the path of the
/// already-opened file, rather than re-resolving the original path string.
///
/// # Platform Support
///
/// - **Linux**: Uses `/proc/self/fd/<fd>`
/// - **macOS**: Uses `fcntl(F_GETPATH)`
/// - **Windows**: Uses `GetFinalPathNameByHandleW`
///
/// # Errors
///
/// Returns an error if:
/// - The platform is not supported
/// - The OS syscall fails (rare, but possible with invalid fd)
/// - The path cannot be converted to UTF-8 (rare on modern systems)
fn get_canonical_path_from_file(file: &std::fs::File) -> Result<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd;
        get_path_from_fd_linux(file.as_raw_fd())
    }

    #[cfg(target_os = "macos")]
    {
        use std::os::unix::io::AsRawFd;
        get_path_from_fd_macos(file.as_raw_fd())
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::io::AsRawHandle;
        get_path_from_fd_windows(file.as_raw_handle())
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        bail!(
            "Platform not supported for TOCTOU-safe path resolution. \
             Supported platforms: Linux, macOS, Windows."
        )
    }
}

// ============================================================================
// End Platform-Specific Implementations
// ============================================================================

/// Sanitize a path for user-facing error messages.
///
/// This removes potentially sensitive information like usernames from paths
/// while keeping enough context to be useful for debugging.
fn sanitize_path_for_display(path: &Path) -> String {
    let path_str = path.display().to_string();

    // Get the last 2-3 path components for context
    let components: Vec<_> = path.components().collect();

    if components.len() <= 3 {
        // Short path, show as-is
        return path_str;
    }

    // Take last 3 components
    let last_components: Vec<_> = components
        .iter()
        .rev()
        .take(3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    let truncated: std::path::PathBuf = last_components.iter().map(|c| c.as_os_str()).collect();

    format!(".../{}", truncated.display())
}

/// A handle to a file that has been verified to reside within an allowed directory.
#[derive(Debug)]
pub(crate) struct ValidatedFile {
    /// The open, readable file handle.
    pub(crate) file: File,
    /// The absolute, resolved path to the file.
    pub(crate) canonical_path: std::path::PathBuf,
}

impl ValidatedFile {
    /// Read the entire file contents as a string
    pub async fn read_to_string(mut self) -> Result<String> {
        let mut contents = String::new();
        self.file
            .read_to_string(&mut contents)
            .await
            .with_context(|| {
                format!(
                    "Failed to read file: {}",
                    sanitize_path_for_display(&self.canonical_path)
                )
            })?;
        Ok(contents)
    }
}

/// Validates that a path is safe (within an allowed base directory) and opens it.
///
/// This implementation is TOCTOU-safe: it opens the file first, then asks the OS
/// "what is the absolute path of this already-opened file descriptor?" rather than
/// re-resolving the original path string. This eliminates the race condition window
/// where an attacker could replace the file with a symlink.
///
/// # Security
///
/// The TOCTOU vulnerability is eliminated by:
/// 1. Opening the file (gets a file descriptor)
/// 2. Asking the OS for the path of that already-opened fd (platform-specific)
/// 3. Validating that the fd's path is within the allowed base
///
/// An attacker cannot change what the fd points to after step 1.
///
/// # Errors
///
/// Returns an error if:
/// - The file cannot be opened
/// - The file descriptor's path cannot be resolved (platform limitation)
/// - The resolved path escapes the allowed base directory
pub(crate) async fn validate_and_open_file(path: &Path, allowed_base: &Path) -> Result<ValidatedFile> {
    // 1. Open the file first - this gives us a file descriptor
    let file = File::open(path)
        .await
        .with_context(|| format!("Cannot open file: {}", sanitize_path_for_display(path)))?;

    // 2. TOCTOU-safe: Get the canonical path of the already-opened file descriptor
    //    Clone the file handle so we can convert to std::fs::File without consuming
    //    the original tokio::fs::File. This is necessary because:
    //    - get_canonical_path_from_file() needs std::fs::File (blocking syscalls)
    //    - We need to return the original tokio::fs::File in ValidatedFile
    //    The dup() syscall adds minimal overhead (~1μs) for significant security benefit.
    let std_file = file
        .try_clone()
        .await
        .context("Failed to clone file handle for path resolution")?
        .into_std()
        .await;

    // Run the blocking syscall in a blocking thread pool to avoid blocking the async runtime
    let canonical_path = tokio::task::spawn_blocking(move || get_canonical_path_from_file(&std_file))
        .await
        .context("Task to get canonical path panicked")??;

    // 3. Validate the allowed base directory (this is safe to do with the original path)
    let canonical_base = tokio::fs::canonicalize(allowed_base)
        .await
        .with_context(|| {
            format!(
                "Cannot resolve base: {}",
                sanitize_path_for_display(allowed_base)
            )
        })?;

    // 4. Check if the opened file's path is within the allowed base
    if !canonical_path.starts_with(&canonical_base) {
        tracing::trace!(
            path = %path.display(),
            canonical_path = %canonical_path.display(),
            allowed_base = %allowed_base.display(),
            canonical_base = %canonical_base.display(),
            "Path escape attempt detected"
        );
        bail!("Security: Path escapes allowed directory");
    }

    Ok(ValidatedFile {
        file,
        canonical_path,
    })
}

/// Legacy wrapper for compatibility - validates path without opening
#[deprecated(note = "Use validate_and_open_file() to prevent TOCTOU vulnerabilities")]
#[allow(dead_code)]
pub(crate) async fn validate_path_safety(path: &Path, allowed_base: &Path) -> Result<()> {
    let canonical_path = tokio::fs::canonicalize(path)
        .await
        .with_context(|| format!("Cannot resolve path: {}", sanitize_path_for_display(path)))?;

    let canonical_base = tokio::fs::canonicalize(allowed_base)
        .await
        .with_context(|| {
            format!(
                "Cannot resolve base: {}",
                sanitize_path_for_display(allowed_base)
            )
        })?;

    if !canonical_path.starts_with(&canonical_base) {
        bail!("Security: Path escapes allowed directory");
    }

    Ok(())
}

/// A visitor that checks for dangerous CSS constructs.
struct SecurityVisitor;

impl<'i> Visitor<'i> for SecurityVisitor {
    type Error = String;

    fn visit_types(&self) -> VisitTypes {
        VisitTypes::URLS | VisitTypes::RULES
    }

    fn visit_url(&mut self, url: &mut Url<'i>) -> Result<(), Self::Error> {
        let u = url.url.to_lowercase();
        // Block dangerous schemes
        if u.starts_with("javascript:")
            || u.starts_with("vbscript:")
            || u.starts_with("data:")
            || u.starts_with("file:")
        {
            return Err(format!("Dangerous URL scheme detected: {}", url.url));
        }
        Ok(())
    }

    fn visit_rule(&mut self, rule: &mut CssRule<'i>) -> Result<(), Self::Error> {
        match rule {
            CssRule::Import(import) => {
                Err(format!("@import rules are not allowed: {}", import.url))
            }
            _ => Ok(()),
        }
    }
}

/// Check the serialized CSS output for any dangerous URL schemes.
/// This is a defense-in-depth measure in case the visitor doesn't catch all URLs.
fn check_serialized_css_for_dangerous_urls(css: &str) -> Result<(), crate::error::ConversionError> {
    let lower = css.to_lowercase();
    for scheme in DANGEROUS_URL_SCHEMES {
        if lower.contains(scheme) {
            return Err(crate::error::ConversionError::Security(format!(
                "CSS contains dangerous URL scheme: '{}'. This is blocked for security reasons.",
                scheme.trim_end_matches(':')
            )));
        }
    }
    Ok(())
}

/// Sanitizes CSS content to remove potentially dangerous constructs using `lightningcss`.
///
/// This uses a full CSS parser to safely handle obfuscated attacks that regex might miss.
/// After parsing and visiting, it also checks the serialized output for any remaining
/// dangerous patterns as a defense-in-depth measure.
///
/// # Errors
/// Returns an error if dangerous constructs (javascript: URLs, @import, etc.) are found.
pub fn sanitize_css(css: &str) -> Result<String, crate::error::ConversionError> {
    let mut stylesheet = StyleSheet::parse(css, ParserOptions::default())
        .map_err(|e| crate::error::ConversionError::Security(format!("Failed to parse CSS: {}", e)))?;

    // Visit and check for dangerous content (catches @import rules)
    stylesheet
        .visit(&mut SecurityVisitor)
        .map_err(|e| crate::error::ConversionError::Security(format!("Security check failed: {}", e)))?;

    // Minify and serialize
    let output = stylesheet
        .to_css(PrinterOptions {
            minify: true,
            ..PrinterOptions::default()
        })
        .map_err(|e| crate::error::ConversionError::Security(format!("Failed to serialize CSS: {}", e)))?;

    // Defense-in-depth: check serialized output for dangerous URL schemes
    // lightningcss normalizes escapes, so "javascript:" will appear in plain text
    check_serialized_css_for_dangerous_urls(&output.code)?;

    Ok(output.code)
}

/// Validates that a file's size is within a specified limit before opening it fully.
pub(crate) async fn validate_file_size(path: &Path, max_bytes: u64) -> Result<File> {
    let file = File::open(path)
        .await
        .with_context(|| format!("Cannot open file: {}", sanitize_path_for_display(path)))?;

    let metadata = file.metadata().await.with_context(|| {
        format!(
            "Cannot read file metadata: {}",
            sanitize_path_for_display(path)
        )
    })?;

    if metadata.len() > max_bytes {
        tracing::debug!(
            path = %path.display(),
            size_mb = metadata.len() / (1024 * 1024),
            max_mb = max_bytes / (1024 * 1024),
            "File size limit exceeded"
        );

        bail!(
            "File '{}' exceeds maximum size ({} MB). Use --max-file-size to increase.",
            sanitize_path_for_display(path),
            max_bytes / (1024 * 1024)
        );
    }

    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_css_passes() {
        let css = "body { color: red; }";
        let sanitized = sanitize_css(css).unwrap();
        // lightningcss minifies, so spaces might be gone
        assert!(sanitized.contains("color:red") || sanitized.contains("color: red"));
    }

    #[test]
    fn test_dangerous_url_blocked() {
        let css = "body { background: url('javascript:alert(1)') }";
        let result = sanitize_css(css);
        assert!(result.is_err(), "CSS with javascript: should be REJECTED by defense-in-depth");
    }

    #[test]
    fn test_import_blocked() {
        let css = "@import 'evil.css';";
        assert!(sanitize_css(css).is_err());
    }

    #[test]
    fn test_obfuscated_url_blocked() {
        // lightningcss handles escapes automatically during parsing
        let css = r"body { background: url('\6a\61\76\61\73\63\72\69\70\74:alert(1)') }";
        let result = sanitize_css(css);
        assert!(result.is_err(), "Obfuscated javascript: should be REJECTED");
    }

    // ============ Extended CSS Sanitization Tests (TEST-001) ============

    #[test]
    fn test_complex_valid_css() {
        let css = r#"
            .container {
                display: flex;
                justify-content: center;
                background: linear-gradient(to right, #fff, #000);
            }
            @media (max-width: 768px) {
                .container { flex-direction: column; }
            }
        "#;
        let result = sanitize_css(css);
        assert!(result.is_ok(), "Complex valid CSS should pass");
    }

    #[test]
    fn test_valid_url_with_https() {
        // Regular https URLs should be allowed
        let css = "body { background: url('https://example.com/image.png'); }";
        let result = sanitize_css(css);
        assert!(result.is_ok(), "HTTPS URLs should be allowed");
    }

    #[test]
    fn test_javascript_url_case_insensitive() {
        let cases = vec![
            "a { background: url('JAVASCRIPT:alert(1)'); }",
            "a { background: url('JavaScript:alert(1)'); }",
            "a { background: url('jAvAsCrIpT:alert(1)'); }",
        ];

        for css in cases {
            let result = sanitize_css(css);
            if result.is_ok() {
                let sanitized = result.unwrap();
                assert!(
                    !sanitized.to_lowercase().contains("javascript:"),
                    "Case variant should be blocked: {}",
                    css
                );
            }
        }
    }

    #[test]
    fn test_vbscript_url_blocked() {
        let css = "a { background: url('vbscript:alert(1)'); }";
        let result = sanitize_css(css);
        if result.is_ok() {
            let sanitized = result.unwrap();
            assert!(!sanitized.contains("vbscript:"));
        }
    }

    #[test]
    fn test_data_url_blocked() {
        let css = r#"body { background: url("data:text/html,<script>alert(1)</script>"); }"#;
        let result = sanitize_css(css);
        if result.is_ok() {
            let sanitized = result.unwrap();
            assert!(!sanitized.contains("data:text/html"));
        }
    }

    #[test]
    fn test_file_url_blocked() {
        let css = "body { background: url('file:///etc/passwd'); }";
        let result = sanitize_css(css);
        if result.is_ok() {
            let sanitized = result.unwrap();
            assert!(!sanitized.contains("file:"));
        }
    }

    #[test]
    fn test_import_with_url_blocked() {
        let css = "@import url('https://evil.com/steal.css');";
        let result = sanitize_css(css);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_css() {
        let result = sanitize_css("");
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_whitespace_only_css() {
        let result = sanitize_css("   \n\t  ");
        assert!(result.is_ok());
    }

    #[test]
    fn test_legitimate_import_like_classname() {
        // A class named '.import-button' should be allowed
        let css = ".import-button { color: blue; }";
        let result = sanitize_css(css);
        // 'import' without '@' should pass
        assert!(result.is_ok(), "import without @ should pass");
    }

    #[test]
    fn test_very_long_css() {
        let css = ".class { color: red; }\n".repeat(1000);
        let result = sanitize_css(&css);
        assert!(result.is_ok(), "Long CSS should be handled");
    }

    #[test]
    fn test_css_with_comments() {
        let css = "/* This is a comment */ body { color: red; }";
        let result = sanitize_css(css);
        assert!(result.is_ok(), "CSS with comments should be parsed");
    }

    #[test]
    fn test_css_with_pseudo_elements() {
        let css = "p::first-line { font-weight: bold; }";
        let result = sanitize_css(css);
        assert!(result.is_ok(), "Pseudo-elements should work");
    }

    #[test]
    fn test_css_with_pseudo_classes() {
        let css = "a:hover { color: blue; }";
        let result = sanitize_css(css);
        assert!(result.is_ok(), "Pseudo-classes should work");
    }

    #[test]
    fn test_css_keyframes() {
        let css = r#"
            @keyframes slide {
                from { transform: translateX(0); }
                to { transform: translateX(100px); }
            }
        "#;
        let result = sanitize_css(css);
        assert!(result.is_ok(), "Keyframes should be allowed");
    }

    #[test]
    fn test_css_font_face() {
        let css = r#"
            @font-face {
                font-family: 'CustomFont';
                src: url('https://example.com/font.woff2') format('woff2');
            }
        "#;
        let result = sanitize_css(css);
        assert!(result.is_ok(), "Font-face with HTTPS should be allowed");
    }

    #[test]
    fn test_css_variables() {
        let css = r#"
            :root {
                --primary-color: #007bff;
            }
            body {
                color: var(--primary-color);
            }
        "#;
        let result = sanitize_css(css);
        assert!(result.is_ok(), "CSS variables should work");
    }

    #[test]
    fn test_css_calc() {
        let css = "div { width: calc(100% - 20px); }";
        let result = sanitize_css(css);
        assert!(result.is_ok(), "calc() should work");
    }

    #[test]
    fn test_css_grid() {
        let css = r#"
            .grid {
                display: grid;
                grid-template-columns: repeat(3, 1fr);
                gap: 10px;
            }
        "#;
        let result = sanitize_css(css);
        assert!(result.is_ok(), "Grid layout should work");
    }

    #[test]
    fn test_css_flexbox() {
        let css = r#"
            .flex {
                display: flex;
                flex-direction: row;
                justify-content: space-between;
                align-items: center;
            }
        "#;
        let result = sanitize_css(css);
        assert!(result.is_ok(), "Flexbox should work");
    }

    // ============ TOCTOU Protection Tests ============

    #[tokio::test]
    async fn test_validate_and_open_file_normal() {
        use std::fs;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("test.txt");
        fs::write(&file_path, "test content").unwrap();

        let validated = validate_and_open_file(&file_path, temp.path())
            .await
            .expect("Should successfully validate and open normal file");

        // Verify we can read from it
        let contents = validated.read_to_string().await.unwrap();
        assert_eq!(contents, "test content");
    }

    #[tokio::test]
    async fn test_validate_and_open_file_rejects_escape() {
        use std::fs;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();

        let outside_file = outside.path().join("secret.txt");
        fs::write(&outside_file, "secret").unwrap();

        // Try to open a file outside the allowed base
        let result = validate_and_open_file(&outside_file, temp.path()).await;
        assert!(
            result.is_err(),
            "Should reject file outside allowed directory"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_validate_and_open_file_rejects_symlink_escape() {
        use std::fs;
        use std::os::unix::fs as unix_fs;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();

        // Create a file outside the allowed base
        let target = outside.path().join("secret.txt");
        fs::write(&target, "secret data").unwrap();

        // Create a symlink inside the allowed base that points outside
        let link = temp.path().join("link.txt");
        unix_fs::symlink(&target, &link).unwrap();

        // This should be rejected - the symlink resolves to a path outside the base
        let result = validate_and_open_file(&link, temp.path()).await;
        assert!(
            result.is_err(),
            "Should reject symlink pointing outside allowed directory"
        );

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("escapes allowed directory"),
            "Error should mention path escape: {}",
            err_msg
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_validate_and_open_file_allows_symlink_within_base() {
        use std::fs;
        use std::os::unix::fs as unix_fs;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();

        // Create a file inside the allowed base
        let target = temp.path().join("target.txt");
        fs::write(&target, "safe data").unwrap();

        // Create a symlink also inside the allowed base
        let link = temp.path().join("link.txt");
        unix_fs::symlink(&target, &link).unwrap();

        // This should be allowed - both link and target are within the base
        let validated = validate_and_open_file(&link, temp.path())
            .await
            .expect("Should allow symlink within allowed directory");

        let contents = validated.read_to_string().await.unwrap();
        assert_eq!(contents, "safe data");
    }

    #[tokio::test]
    async fn test_get_canonical_path_from_file_matches_expected() {
        use std::fs;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("test.txt");
        fs::write(&file_path, "test").unwrap();

        // Open the file using std::fs
        let std_file = fs::File::open(&file_path).unwrap();

        // Get the canonical path from the fd
        let fd_path = get_canonical_path_from_file(&std_file).unwrap();

        // Get the expected canonical path
        let expected = fs::canonicalize(&file_path).unwrap();

        assert_eq!(
            fd_path, expected,
            "Path from fd should match canonical path"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_get_canonical_path_resolves_symlink() {
        use std::fs;
        use std::os::unix::fs as unix_fs;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();

        // Create target file
        let target = temp.path().join("target.txt");
        fs::write(&target, "data").unwrap();

        // Create symlink
        let link = temp.path().join("link.txt");
        unix_fs::symlink(&target, &link).unwrap();

        // Open the symlink
        let std_file = fs::File::open(&link).unwrap();

        // Get the canonical path from the fd - should resolve to target
        let fd_path = get_canonical_path_from_file(&std_file).unwrap();
        let expected_target = fs::canonicalize(&target).unwrap();

        assert_eq!(
            fd_path, expected_target,
            "Path from fd should resolve symlink to target"
        );
    }

    #[tokio::test]
    async fn test_validate_and_open_file_detailed_error() {
        use std::fs;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();

        let outside_file = outside.path().join("secret.txt");
        fs::write(&outside_file, "secret").unwrap();

        let result = validate_and_open_file(&outside_file, temp.path()).await;
        assert!(result.is_err());

        // Check that error message is helpful
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("escapes") || msg.contains("Security"),
            "Error should explain security issue: {}",
            msg
        );
    }

    #[tokio::test]
    async fn test_very_long_path_handling() {
        use std::fs;
        use tempfile::TempDir;

        let temp = TempDir::new().unwrap();

        // Create a deeply nested directory structure approaching platform limits
        // Linux: 4096 bytes, Windows: 32768 bytes, macOS: 1024 bytes
        let mut deep_path = temp.path().to_path_buf();
        for i in 0..40 {
            // 40 * 20 chars = 800 bytes, under macOS 1024 limit
            deep_path = deep_path.join(format!("dir_{:016}", i));
        }

        // Attempt to create the deep path - may fail on some platforms
        if fs::create_dir_all(&deep_path).is_ok() {
            let file_path = deep_path.join("test.txt");
            fs::write(&file_path, "test content").unwrap();

            // Should either succeed or fail gracefully with a clear error
            let result = validate_and_open_file(&file_path, temp.path()).await;

            // Either it works or it gives a meaningful error - no panics or undefined behavior
            match result {
                Ok(validated) => {
                    // Verify we can still read from deeply nested files
                    let contents = validated.read_to_string().await.unwrap();
                    assert_eq!(contents, "test content");
                }
                Err(e) => {
                    // Error message should be non-empty and informative
                    let msg = e.to_string();
                    assert!(
                        !msg.is_empty(),
                        "Error for long path should have a message"
                    );
                }
            }
        }
        // If create_dir_all fails, that's fine - we're testing graceful handling
    }
}
