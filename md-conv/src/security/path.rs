//! Path validation and sanitization module
//!
//! This module provides path safety functions for secure file access.

use anyhow::{bail, Context, Result};
use std::path::Path;
use tokio::fs::File;

use super::toctou::{get_canonical_path_from_file, ValidatedFile};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
