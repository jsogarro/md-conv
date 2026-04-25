use thiserror::Error;

/// Library-level errors with structured types
#[derive(Error, Debug)]
pub enum ConversionError {
    #[error("Template rendering failed: {0}")]
    Template(#[from] handlebars::RenderError),

    #[error("Markdown parse error: {0}")]
    Parse(String),

    #[error("Browser error: {0}")]
    Browser(String),

    #[error("Security violation: {0}")]
    Security(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Notebook parse error: {0}")]
    Notebook(String),

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
            ConversionError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "file"
            ))
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
        let io_err =
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
        let conv_err: ConversionError = io_err.into();
        assert!(matches!(conv_err, ConversionError::Io(_)));
        assert_eq!(conv_err.exit_code(), 2);
    }
}
