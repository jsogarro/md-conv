//! Input validation and sanitization utilities

use anyhow::{bail, Context, Result};
use std::path::Path;

/// Dangerous CSS constructs that could enable attacks
const DANGEROUS_CSS_PATTERNS: &[&str] = &[
    "javascript:",
    "expression(",
    "behavior:",
    "binding:",
    "-moz-binding:",
    "@import",
    "url(\"data:",
    "url('data:",
    "url(data:",
];

/// Validate that a path doesn't escape the allowed directory
pub fn validate_path_safety(path: &Path, allowed_base: &Path) -> Result<()> {
    let canonical_path = path
        .canonicalize()
        .with_context(|| format!("Cannot resolve path: {}", path.display()))?;

    let canonical_base = allowed_base
        .canonicalize()
        .with_context(|| format!("Cannot resolve base: {}", allowed_base.display()))?;

    if !canonical_path.starts_with(&canonical_base) {
        bail!(
            "Security: Path '{}' is outside allowed directory '{}'",
            path.display(),
            allowed_base.display()
        );
    }

    Ok(())
}

/// Sanitize CSS to remove potentially dangerous constructs
pub fn sanitize_css(css: &str) -> Result<String> {
    let css_lower = css.to_lowercase();

    for pattern in DANGEROUS_CSS_PATTERNS {
        if css_lower.contains(pattern) {
            tracing::warn!(
                pattern = pattern,
                "Removing potentially dangerous CSS construct"
            );
            // For now, reject entirely - could be made more granular
            bail!(
                "CSS contains potentially dangerous construct: '{}'. \
                 Use --allow-external-css to override (not recommended).",
                pattern
            );
        }
    }

    // Basic sanitization passed
    Ok(css.to_string())
}

/// Validate file size is within limits
pub fn validate_file_size(path: &Path, max_bytes: u64) -> Result<()> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("Cannot read file metadata: {}", path.display()))?;

    if metadata.len() > max_bytes {
        bail!(
            "File '{}' exceeds maximum size ({} MB). Use --max-file-size to increase.",
            path.display(),
            max_bytes / (1024 * 1024)
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_css_passes() {
        let css = "body { color: red; font-size: 16px; }";
        assert!(sanitize_css(css).is_ok());
    }

    #[test]
    fn test_javascript_url_blocked() {
        let css = "a { background: url('javascript:alert(1)'); }";
        assert!(sanitize_css(css).is_err());
    }

    #[test]
    fn test_expression_blocked() {
        let css = "div { width: expression(alert(1)); }";
        assert!(sanitize_css(css).is_err());
    }

    #[test]
    fn test_import_blocked() {
        let css = "@import url('evil.css');";
        assert!(sanitize_css(css).is_err());
    }
}
