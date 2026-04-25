//! Using md-conv as a library for markdown parsing
//!
//! This example demonstrates using the parsing pipeline directly without requiring
//! Chrome for PDF generation. It shows how to:
//!
//! 1. Parse front matter from a Markdown document
//! 2. Generate HTML with syntax highlighting
//! 3. Access the structured output (metadata, HTML content, table of contents)
//!
//! This is useful when you want to integrate md-conv's parsing capabilities into your
//! own application without the full conversion pipeline.
//!
//! Usage:
//!   cargo run --example library_usage

use md_conv::parser::{generate_html, parse_front_matter};

fn main() -> anyhow::Result<()> {
    let markdown = r#"---
title: Example Document
author: Test Author
date: 2025-04-25
description: A demonstration of md-conv's parsing capabilities
keywords:
  - rust
  - markdown
  - parsing
---

# Hello World

This is a **bold** statement with `inline code`.

## Features

- GitHub Flavored Markdown support
- Syntax highlighting
- Table of contents generation
- Front matter extraction

## Code Example

```rust
fn main() {
    println!("Hello from md-conv!");
}
```

### Nested Section

More content here with ~~strikethrough~~ and **emphasis**.
"#;

    // Parse front matter and body
    let (front_matter, body) = parse_front_matter(markdown)?;

    println!("=== Front Matter ===");
    println!("Title: {:?}", front_matter.title);
    println!("Author: {:?}", front_matter.author);
    println!("Date: {:?}", front_matter.date);
    println!("Description: {:?}", front_matter.description);
    println!("Keywords: {:?}", front_matter.keywords);
    println!();

    // Generate HTML with syntax highlighting
    let theme = front_matter
        .highlight_theme
        .as_deref()
        .unwrap_or("base16-ocean.dark");
    let (html, toc) = generate_html(&body, theme)?;

    println!("=== Generated HTML ===");
    println!("Size: {} bytes", html.len());
    println!("Preview (first 500 chars):");
    println!("{}", &html[..html.len().min(500)]);
    println!();

    if !toc.is_empty() {
        println!("=== Table of Contents ===");
        println!("{}", toc);
    }

    Ok(())
}
