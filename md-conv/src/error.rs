use std::path::PathBuf;
use thiserror::Error;

/// Library-level errors with structured types
#[derive(Error, Debug)]
pub enum ConversionError {
    #[error("File not found: {0}")]
    FileNotFound(PathBuf),

    #[error("Failed to read file '{path}'")]
    FileRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to write output '{path}'")]
    FileWrite {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Invalid front matter YAML: {message}")]
    FrontMatterParse { message: String },

    #[error("Template rendering failed")]
    TemplateError(#[from] handlebars::RenderError),

    #[error("Browser not found. Install Chrome/Chromium or set --chrome-path")]
    BrowserNotFound,

    #[error("PDF generation failed: {0}")]
    PdfGeneration(String),

    #[error("PDF generation timed out after {0} seconds")]
    PdfTimeout(u64),

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("Security violation: {0}")]
    SecurityViolation(String),

    #[error("File too large: {path} ({size_mb} MB exceeds {max_mb} MB limit)")]
    FileTooLarge {
        path: PathBuf,
        size_mb: u64,
        max_mb: u64,
    },
}

/// Result type alias for conversion operations
pub type Result<T> = std::result::Result<T, ConversionError>;

impl ConversionError {
    /// Get exit code for CLI
    pub fn exit_code(&self) -> i32 {
        match self {
            ConversionError::FileNotFound(_) => 2,
            ConversionError::FileRead { .. } => 3,
            ConversionError::FileWrite { .. } => 4,
            ConversionError::FrontMatterParse { .. } => 5,
            ConversionError::TemplateError(_) => 6,
            ConversionError::BrowserNotFound => 7,
            ConversionError::PdfGeneration(_) => 8,
            ConversionError::PdfTimeout(_) => 9,
            ConversionError::InvalidConfig(_) => 10,
            ConversionError::SecurityViolation(_) => 11,
            ConversionError::FileTooLarge { .. } => 12,
        }
    }

    /// Check if this error is recoverable
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            ConversionError::PdfTimeout(_) | ConversionError::PdfGeneration(_)
        )
    }
}
