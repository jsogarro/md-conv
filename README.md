# md-conv

[![CI](https://github.com/jsogarro/markdown_converter/actions/workflows/ci.yml/badge.svg)](https://github.com/jsogarro/markdown_converter/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

A fast, secure Markdown to PDF and HTML converter built in Rust.

md-conv uses Headless Chrome for pixel-perfect PDF rendering and includes
first-class support for Jupyter Notebooks, YAML front matter, custom CSS,
and batch processing.

## Features

- Markdown to PDF via pooled Chrome instances (5x faster for batches)
- Markdown to HTML with Handlebars templating
- Jupyter Notebook (.ipynb) conversion to both formats
- YAML front matter for per-document configuration
- Custom CSS with automatic sanitization
- Syntax highlighting via syntect
- Automatic Table of Contents generation
- Watch mode for live preview
- Batch processing with concurrent conversion
- stdin/stdout pipeline support
- JSON output for scripting

## Requirements

- Rust 1.75 or later
- Chrome, Chromium, or Edge (for PDF generation)

## Installation

### From source

```bash
git clone https://github.com/jsogarro/markdown_converter.git
cd markdown_converter/md-conv
cargo install --path .
```

### Verify installation

```bash
md-conv --help
```

## Quick Start

Convert a Markdown file to PDF:

```bash
md-conv document.md
```

Convert to HTML:

```bash
md-conv document.md -f html
```

Convert a Jupyter notebook:

```bash
md-conv analysis.ipynb
```

## Usage

```bash
# Single file
md-conv report.md                       # PDF (default)
md-conv report.md -f html               # HTML
md-conv report.md -f pdf,html           # Both formats

# Batch conversion
md-conv docs/*.md -O output/            # All files to output/

# Custom CSS
md-conv report.md --css styles.css

# Explicit output path
md-conv report.md -o final-report.pdf

# Pipeline
cat input.md | md-conv --stdin -f html --stdout > output.html

# Watch mode (re-converts on file changes)
md-conv report.md --watch

# JSON output for scripting
md-conv report.md -f html --json

# Verbose logging
md-conv report.md -vvv
```

## Front Matter

Control rendering per-document with YAML front matter:

```yaml
---
title: "Quarterly Report"
author: "Jane Smith"
date: "2026-01-15"
css: "./styles/report.css"
highlight_theme: "base16-ocean.dark"
pdf_options:
  format: "A4"
  margin: "20mm"
  landscape: false
  print_background: true
  scale: 1.0
---
```

## Library Usage

md-conv can be used as a Rust library. Add to your `Cargo.toml`:

```toml
[dependencies]
md-conv = "0.1"
```

### Parse markdown

```rust
use md_conv::parser::parse_front_matter;

let markdown = "---\ntitle: My Doc\n---\n# Hello\n\nWorld";
let (front_matter, body) = parse_front_matter(markdown).unwrap();
assert_eq!(front_matter.title.as_deref(), Some("My Doc"));
```

### Sanitize CSS

```rust
use md_conv::security::sanitize_css;

let safe = sanitize_css("body { color: red; }").unwrap();

// Dangerous CSS is rejected
let result = sanitize_css("body { background: url('javascript:alert(1)') }");
assert!(result.is_err());
```

### Full conversion

```rust,no_run
use md_conv::{Args, run};

#[tokio::main]
async fn main() -> Result<(), md_conv::ConversionError> {
    let args = Args {
        input: vec!["report.md".into()],
        ..Args::default()
    };
    run(args).await
}
```

API documentation: [docs.rs/md-conv](https://docs.rs/md-conv)

## Security

md-conv is designed to handle untrusted input safely:

- **CSS sanitization** -- lightningcss-based parsing blocks javascript:, data:, blob:, file: URLs and @import rules. Defense-in-depth with post-serialization scanning.
- **HTML sanitization** -- ammonia strips XSS vectors (script tags, event handlers, iframes) from rendered output.
- **TOCTOU-safe paths** -- Platform-specific file descriptor resolution (fcntl on macOS, /proc/self/fd on Linux, Win32 API on Windows) prevents symlink race conditions.
- **Content Security Policy** -- CSP meta tags in HTML output.
- **File size limits** -- Configurable via --max-file-size (default 10MB).
- **Timeouts** -- PDF generation timeout via --timeout (default 30s).

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | General error |
| 2 | I/O error |
| 3 | Markdown parse error |
| 4 | Configuration error |
| 5 | Security violation |
| 6 | Template rendering error |
| 7 | Browser/Chrome error |
| 8 | Notebook parse error |

## Project Structure

```
markdown_converter/
  md-conv/           # Rust crate (CLI + library)
    src/
      cli.rs         # Command-line argument parsing
      config.rs      # Configuration merging (CLI + front matter + config file)
      error.rs       # Typed error variants with exit codes
      lib.rs         # Core conversion pipeline
      parser.rs      # Markdown and notebook parsing
      template.rs    # Handlebars HTML templating
      renderer/
        browser.rs   # Chrome browser pool management
        html.rs      # HTML renderer
        pdf.rs       # PDF renderer via Headless Chrome
      security/
        css.rs       # CSS sanitization
        path.rs      # Path validation
        toctou.rs    # TOCTOU-safe file operations
    examples/        # Runnable example programs
    tests/           # Integration test suite
```

## License

MIT
