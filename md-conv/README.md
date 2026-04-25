[![Crates.io](https://img.shields.io/crates/v/md-conv.svg)](https://crates.io/crates/md-conv)
[![Documentation](https://docs.rs/md-conv/badge.svg)](https://docs.rs/md-conv)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

# md-conv

A fast, secure Markdown to PDF/HTML converter written in Rust.

## Features

- **Fast PDF Generation**: Pooled Headless Chrome instances minimize startup overhead
- **Jupyter Support**: Native `.ipynb` notebook conversion to PDF and HTML
- **Security Focused**: CSS sanitization, TOCTOU-safe path validation, HTML sanitization
- **YAML Front Matter**: Per-document configuration for title, author, CSS, PDF options
- **Custom Styling**: Apply your own CSS with automatic sanitization
- **Syntax Highlighting**: Built-in code highlighting via syntect
- **Table of Contents**: Automatic TOC generation from headings
- **Watch Mode**: Live preview with automatic re-conversion on file changes
- **Batch Processing**: Concurrent file conversion with progress bar
- **Pipeline Support**: stdin/stdout and JSON output for scripting

## Installation

### From crates.io

```bash
cargo install md-conv
```

### From source

```bash
git clone https://github.com/jsogarro/md-conv
cd md-conv/md-conv
cargo install --path .
```

### Requirements

PDF generation requires Chrome, Chromium, or Edge installed locally.
Set `CHROME_PATH` if the browser is in a non-standard location.

## Usage

```bash
# Convert to PDF (default)
md-conv document.md

# Convert to HTML
md-conv document.md -f html

# Convert to both formats
md-conv document.md -f pdf,html

# Batch convert multiple files
md-conv doc1.md doc2.md -O output/

# Use custom CSS
md-conv document.md --css styles.css

# Specify output path
md-conv document.md -o report.pdf

# Jupyter notebook conversion
md-conv analysis.ipynb

# Pipeline usage
cat input.md | md-conv --stdin -f html --stdout > output.html

# Watch mode (live preview)
md-conv document.md --watch

# Verbose logging
md-conv document.md -vvv
```

## Library Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
md-conv = "0.1"
tokio = { version = "1", features = ["full"] }
```

### Parse and inspect markdown

```rust
use md_conv::parser::parse_front_matter;

let markdown = "---\ntitle: My Doc\n---\n# Hello\n\nWorld";
let (front_matter, body) = parse_front_matter(markdown).unwrap();
assert_eq!(front_matter.title.as_deref(), Some("My Doc"));
assert!(body.contains("Hello"));
```

### Full CLI-style conversion

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

### CSS sanitization

```rust
use md_conv::security::sanitize_css;

let safe = sanitize_css("body { color: red; }").unwrap();
assert!(safe.contains("color"));

// Dangerous CSS is rejected
let result = sanitize_css("body { background: url('javascript:alert(1)') }");
assert!(result.is_err());
```

Full API documentation at [docs.rs](https://docs.rs/md-conv).

## Front Matter

Add YAML front matter to your Markdown files:

```yaml
---
title: "Document Title"
author: "Your Name"
date: "2026-01-15"
css: "styles.css"
highlight_theme: "base16-ocean.dark"
pdf_options:
  format: "A4"
  margin: "20mm"
  landscape: false
  print_background: true
  scale: 1.0
---
```

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | General error |
| 2 | I/O error (file not found, permission denied) |
| 3 | Markdown parse error |
| 4 | Configuration error |
| 5 | Security violation (path escape, malicious CSS) |
| 6 | Template rendering error |
| 7 | Browser/Chrome error |
| 8 | Notebook parse error |

## Security

md-conv includes defense-in-depth security:

- **CSS Sanitization**: `lightningcss`-based parsing blocks `javascript:`, `data:`, `file:` URLs and `@import` rules
- **HTML Sanitization**: `ammonia` strips XSS vectors from rendered output
- **TOCTOU-Safe Paths**: Platform-specific fd resolution (`fcntl`/`/proc`/Win32) prevents symlink races
- **Content Security Policy**: CSP meta tags in HTML output
- **File Size Limits**: Configurable via `--max-file-size`
- **Timeouts**: PDF generation timeout via `--timeout`

## License

MIT
