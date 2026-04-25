# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-04-25

### Added
- Markdown to PDF conversion via Headless Chrome with browser pooling
- Markdown to HTML conversion with template support
- Jupyter Notebook (.ipynb) conversion to PDF and HTML
- YAML front matter for per-document configuration
- Custom CSS support with CSS sanitization (lightningcss)
- HTML sanitization (ammonia) for XSS prevention
- TOCTOU-safe path validation (Linux, macOS, Windows)
- Content Security Policy meta tags in HTML output
- Syntect-based syntax highlighting for code blocks
- Automatic Table of Contents generation
- Watch mode for live preview on file changes
- Batch processing with concurrent file conversion
- stdin/stdout pipeline support
- JSON output mode for scripting
- Progress bar for batch operations
- Configurable PDF options (format, margins, scale, landscape, background)
- Typed error variants (Parse, Browser, Security, Io, Config, Notebook) with distinct exit codes
- `Default` implementation for `Args` enabling `..Args::default()` pattern
- Doc examples and doctests for all public API items
- Example programs: basic_conversion, html_output, library_usage, css_sanitization
- CI workflows for check, fmt, clippy, cross-platform tests, and crates.io publishing
- Exit codes documented in `--help` output
- CLI validation for zero timeout and file size values
- `blob:` URL scheme blocked in CSS sanitization
- Case-insensitive `.ipynb` extension matching
- Warning log when syntax highlighting fails
- Browser pool shutdown on watch mode exit
