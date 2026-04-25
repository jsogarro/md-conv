//! CSS sanitization module
//!
//! This module provides defense-in-depth CSS sanitization using `lightningcss`
//! to prevent XSS attacks via malicious CSS constructs.

use lightningcss::rules::CssRule;
use lightningcss::stylesheet::{ParserOptions, PrinterOptions, StyleSheet};
use lightningcss::values::url::Url;
use lightningcss::visitor::{Visit, VisitTypes, Visitor};

/// Dangerous URL schemes that could lead to script execution
const DANGEROUS_URL_SCHEMES: &[&str] = &["javascript:", "vbscript:", "data:", "file:"];

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
/// # Examples
///
/// ```rust
/// use md_conv::security::sanitize_css;
///
/// // Safe CSS passes through (minified)
/// let safe_css = "body { color: red; margin: 10px; }";
/// let sanitized = sanitize_css(safe_css).unwrap();
/// assert!(sanitized.contains("color:red") || sanitized.contains("color: red"));
///
/// // Dangerous CSS is rejected
/// let dangerous = "body { background: url('javascript:alert(1)') }";
/// assert!(sanitize_css(dangerous).is_err());
///
/// // @import rules are blocked
/// let import_css = "@import 'malicious.css';";
/// assert!(sanitize_css(import_css).is_err());
/// ```
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
}
