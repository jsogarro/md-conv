use thiserror::Error;

/// Library-level errors with structured types for md-conv operations.
///
/// Each variant maps to a specific exit code for CLI error reporting.
#[derive(Error, Debug)]
pub enum ConversionError {
    /// Returned when handlebars template rendering fails (exit code 6).
    #[error("Template rendering failed: {0}")]
    Template(#[from] handlebars::RenderError),

    /// Returned when markdown content cannot be parsed (exit code 3).
    ///
    /// This includes malformed front matter YAML and invalid markdown structures.
    #[error("Markdown parse error: {0}")]
    Parse(String),

    /// Returned when Chrome/Chromium fails to render PDF (exit code 7).
    ///
    /// Common causes: browser not found, timeout, page load errors.
    #[error("Browser error: {0}")]
    Browser(String),

    /// Returned when security validation fails (exit code 5).
    ///
    /// Includes path escape attempts, malicious CSS, dangerous URL schemes.
    #[error("Security violation: {0}")]
    Security(String),

    /// Returned for file system errors (exit code 2).
    ///
    /// Includes file not found, permission denied, disk full.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Returned when configuration is invalid or cannot be loaded (exit code 4).
    #[error("Configuration error: {0}")]
    Config(String),

    /// Returned when Jupyter Notebook JSON is malformed (exit code 8).
    #[error("Notebook parse error: {0}")]
    Notebook(String),

    /// Generic error for all other cases (exit code 1).
    #[error("{0}")]
    Generic(String),
}

/// Result type alias for conversion operations
pub type Result<T> = std::result::Result<T, ConversionError>;

impl ConversionError {
    /// Get exit code for CLI
    pub fn exit_code(&self) -> i32 {
        match self {
            ConversionError::Io(_) => 2,
            ConversionError::Parse(_) => 3,
            ConversionError::Config(_) => 4,
            ConversionError::Security(_) => 5,
            ConversionError::Template(_) => 6,
            ConversionError::Browser(_) => 7,
            ConversionError::Notebook(_) => 8,
            ConversionError::Generic(_) => 1,
        }
    }
}

/// Catch-all conversion from anyhow::Error to ConversionError
impl From<anyhow::Error> for ConversionError {
    fn from(err: anyhow::Error) -> Self {
        ConversionError::Generic(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = ConversionError::Generic("test".into());
        assert_eq!(err.to_string(), "test");
    }

    #[test]
    fn test_exit_codes() {
        assert_eq!(ConversionError::Generic("test".into()).exit_code(), 1);
        assert_eq!(
            ConversionError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "file"))
                .exit_code(),
            2
        );
        assert_eq!(ConversionError::Parse("bad markdown".into()).exit_code(), 3);
        assert_eq!(
            ConversionError::Config("invalid config".into()).exit_code(),
            4
        );
        assert_eq!(
            ConversionError::Security("XSS attempt".into()).exit_code(),
            5
        );
        assert_eq!(
            ConversionError::Browser("Chrome crashed".into()).exit_code(),
            7
        );
        assert_eq!(
            ConversionError::Notebook("invalid ipynb".into()).exit_code(),
            8
        );
    }

    #[test]
    fn test_from_anyhow() {
        let anyhow_err = anyhow::anyhow!("anyhow error");
        let conv_err: ConversionError = anyhow_err.into();
        assert!(matches!(conv_err, ConversionError::Generic(_)));
        assert_eq!(conv_err.exit_code(), 1);
    }

    #[test]
    fn test_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
        let conv_err: ConversionError = io_err.into();
        assert!(matches!(conv_err, ConversionError::Io(_)));
        assert_eq!(conv_err.exit_code(), 2);
    }

    #[test]
    fn test_from_handlebars_render_error() {
        // Create a template with invalid syntax to trigger RenderError
        let mut hb = handlebars::Handlebars::new();
        // Use strict mode to make missing variables an error
        hb.set_strict_mode(true);
        hb.register_template_string("test", "{{missing_var}}")
            .unwrap();
        let data = serde_json::json!({});
        let render_result = hb.render("test", &data);

        match render_result {
            Err(e) => {
                let conv_err: ConversionError = e.into();
                assert!(matches!(conv_err, ConversionError::Template(_)));
                assert_eq!(conv_err.exit_code(), 6);
                assert!(conv_err.to_string().contains("Template rendering failed"));
            }
            Ok(_) => panic!("Expected render error when strict mode is enabled"),
        }
    }

    #[test]
    fn test_all_error_variant_display() {
        let parse_err = ConversionError::Parse("bad markdown".into());
        assert_eq!(parse_err.to_string(), "Markdown parse error: bad markdown");

        let browser_err = ConversionError::Browser("chrome crash".into());
        assert_eq!(browser_err.to_string(), "Browser error: chrome crash");

        let security_err = ConversionError::Security("xss attempt".into());
        assert_eq!(security_err.to_string(), "Security violation: xss attempt");

        let io_err = ConversionError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file not found",
        ));
        assert!(io_err.to_string().contains("I/O error"));

        let config_err = ConversionError::Config("invalid yaml".into());
        assert_eq!(config_err.to_string(), "Configuration error: invalid yaml");

        let notebook_err = ConversionError::Notebook("invalid json".into());
        assert_eq!(
            notebook_err.to_string(),
            "Notebook parse error: invalid json"
        );

        let generic_err = ConversionError::Generic("something failed".into());
        assert_eq!(generic_err.to_string(), "something failed");
    }

    #[test]
    fn test_template_error_exit_code_missing() {
        // Verify Template variant exit code (was not tested in original test_exit_codes)
        let mut hb = handlebars::Handlebars::new();
        hb.register_template_string("test", "{{missing}}").unwrap();
        let render_result = hb.render("test", &serde_json::json!({}));

        if let Err(e) = render_result {
            let conv_err: ConversionError = e.into();
            assert_eq!(conv_err.exit_code(), 6);
        }
    }
}
