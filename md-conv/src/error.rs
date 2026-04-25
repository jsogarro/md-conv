use thiserror::Error;

/// Library-level errors with structured types
#[derive(Error, Debug)]
pub enum ConversionError {
    #[error("Template rendering failed")]
    TemplateError(#[from] handlebars::RenderError),

    #[error("Generic conversion error: {0}")]
    Generic(String),
}

/// Result type alias for conversion operations
pub type Result<T> = std::result::Result<T, ConversionError>;

impl ConversionError {
    /// Get exit code for CLI
    pub fn exit_code(&self) -> i32 {
        match self {
            ConversionError::TemplateError(_) => 6,
            ConversionError::Generic(_) => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = ConversionError::Generic("test".into());
        assert_eq!(err.to_string(), "Generic conversion error: test");
    }

    #[test]
    fn test_exit_codes() {
        assert_eq!(ConversionError::Generic("test".into()).exit_code(), 1);
    }
}
