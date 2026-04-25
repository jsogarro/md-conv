# md-conv

A fast, secure Markdown to PDF/HTML converter written in Rust.

## Features

- **Fast**: Native Rust performance with async PDF generation
- **Secure**: Input validation, CSS sanitization, path traversal protection
- **Flexible**: YAML front matter for per-document configuration
- **GFM Support**: Tables, task lists, strikethrough, footnotes, and more
- **Custom Styling**: Apply your own CSS to output documents

## Installation

### From Source

```bash
cargo install --path .
```

### From Releases

Download the appropriate binary from the [releases page](https://github.com/user/md-conv/releases).

## Usage

```bash
# Convert to PDF (default)
md-conv document.md

# Convert to HTML
md-conv document.md --format html

# Convert to both formats
md-conv document.md -f pdf -f html

# Use custom CSS
md-conv document.md --css custom.css

# Specify output path
md-conv document.md -o output.pdf

# Convert multiple files
md-conv *.md --format html

# Verbose output
md-conv document.md -vvv
```

## Front Matter

Add YAML front matter to your Markdown files:

```yaml
---
title: "Document Title"
author: "Your Name"
date: "2025-01-15"
css: "styles.css"  # or inline CSS
pdf_options:
  format: "A4"      # A4, Letter, Legal, Tabloid
  margin: "20mm"    # or margin_top, margin_bottom, etc.
  landscape: false
  print_background: true
---
```

## Requirements

- For PDF generation: Chrome, Chromium, or Edge browser installed
- Set `CHROME_PATH` environment variable if browser is in non-standard location

## Security

md-conv includes several security features:

- **CSS Sanitization**: Blocks dangerous CSS constructs
- **Path Validation**: Prevents path traversal attacks
- **File Size Limits**: Configurable maximum file size
- **Timeouts**: PDF generation timeout protection

## License

MIT
