//! TOCTOU (Time-of-Check to Time-of-Use) protection module
//!
//! This module provides platform-specific file descriptor to path resolution
//! to eliminate TOCTOU vulnerabilities in path validation.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::AsyncReadExt;

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
pub(crate) fn get_canonical_path_from_file(file: &std::fs::File) -> Result<PathBuf> {
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

#[cfg(test)]
mod tests {
    use super::*;

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

            // Open file for testing
            let std_file = fs::File::open(&file_path).unwrap();

            // Should either succeed or fail gracefully with a clear error
            let result = get_canonical_path_from_file(&std_file);

            // Either it works or it gives a meaningful error - no panics or undefined behavior
            match result {
                Ok(_canonical_path) => {
                    // Success - path was resolved
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
