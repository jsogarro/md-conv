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

mod css;
mod path;
mod toctou;

pub use css::sanitize_css;
pub(crate) use path::validate_and_open_file;
#[allow(unused_imports, deprecated)]
pub(crate) use path::validate_path_safety;
pub(crate) use toctou::validate_file_size;
#[allow(unused_imports)]
pub(crate) use toctou::ValidatedFile;
