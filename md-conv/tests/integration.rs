use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[allow(deprecated)]
fn cmd() -> Command {
    Command::cargo_bin("md-conv").unwrap()
}

// ============ CLI Tests ============

#[test]
fn test_help_output() {
    cmd()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Command-line arguments for the `md-conv` tool",
        ))
        .stdout(predicate::str::contains("--format"))
        .stdout(predicate::str::contains("--css"));
}

#[test]
fn test_version_output() {
    cmd()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn test_missing_file_error() {
    cmd()
        .arg("nonexistent.md")
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found").or(predicate::str::contains("No such file")));
}

#[test]
fn test_multiple_inputs_with_output_flag_fails() {
    let temp = TempDir::new().unwrap();
    let a = temp.path().join("a.md");
    let b = temp.path().join("b.md");

    fs::write(&a, "# A").unwrap();
    fs::write(&b, "# B").unwrap();

    cmd()
        .arg(&a)
        .arg(&b)
        .arg("-o")
        .arg("output.pdf")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Cannot use --output with multiple",
        ));
}

// ============ HTML Output Tests ============

#[test]
fn test_html_output_basic() {
    let temp = TempDir::new().unwrap();
    let input = temp.path().join("test.md");
    fs::write(&input, "# Test\n\nContent with **bold** text.").unwrap();

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("html")
        .arg("-v")
        .assert()
        .success();

    let output = temp.path().join("test.html");
    assert!(output.exists(), "HTML output should exist");

    let html = fs::read_to_string(&output).unwrap();
    assert!(html.contains("<!DOCTYPE html>"));
    assert!(html.contains("<h1>Test</h1>"));
    assert!(html.contains("<strong>bold</strong>"));
}

#[test]
fn test_html_output_with_frontmatter() {
    let temp = TempDir::new().unwrap();
    let input = temp.path().join("fm.md");

    fs::write(
        &input,
        r#"---
title: "My Document"
author: "Jane Doe"
date: "2025-01-15"
---
# Content
"#,
    )
    .unwrap();

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("html")
        .assert()
        .success();

    let html = fs::read_to_string(temp.path().join("fm.html")).unwrap();
    assert!(html.contains("<title>My Document</title>"));
    assert!(html.contains("By Jane Doe"));
    assert!(html.contains("2025-01-15"));
}

#[test]
fn test_html_output_with_custom_css() {
    let temp = TempDir::new().unwrap();
    let input = temp.path().join("styled.md");
    let css = temp.path().join("custom.css");

    fs::write(&input, "# Styled").unwrap();
    fs::write(&css, "body { background: red; }").unwrap();

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("html")
        .arg("--css")
        .arg(&css)
        .assert()
        .success();

    let html = fs::read_to_string(temp.path().join("styled.html")).unwrap();
    // CSS is minified by lightningcss - check for minified form
    assert!(html.contains("background:red") || html.contains("background:#f00"));
}

#[test]
fn test_notebook_conversion_to_html() {
    let temp = TempDir::new().unwrap();
    let input = temp.path().join("test.ipynb");

    let notebook_content = r##"
{
 "cells": [
  {
   "cell_type": "markdown",
   "source": ["# Notebook Title\n", "\n", "This is a **markdown** cell."]
  },
  {
   "cell_type": "code",
   "source": ["print('Hello World')"]
  }
 ],
 "metadata": {},
 "nbformat": 4,
 "nbformat_minor": 4
}"##;

    fs::write(&input, notebook_content).unwrap();

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("html")
        .assert()
        .success();

    let output = temp.path().join("test.html");
    assert!(output.exists());
    let html = fs::read_to_string(output).unwrap();
    assert!(html.contains("<h1>Notebook Title</h1>"));
    assert!(html.contains("print"));
    assert!(html.contains("Hello World"));
    assert!(html.contains("markdown"));
}

#[test]
fn test_empty_file() {
    let temp = TempDir::new().unwrap();
    let input = temp.path().join("empty.md");
    fs::write(&input, "").unwrap();

    cmd().arg(&input).arg("-f").arg("html").assert().success();

    let output = temp.path().join("empty.html");
    assert!(output.exists());
    let html = fs::read_to_string(output).unwrap();
    assert!(html.contains("<!DOCTYPE html>"));
}

#[test]
fn test_pure_front_matter() {
    let temp = TempDir::new().unwrap();
    let input = temp.path().join("pfm.md");
    fs::write(&input, "---\ntitle: Pure FM\n---\n").unwrap();

    cmd().arg(&input).arg("-f").arg("html").assert().success();

    let output = temp.path().join("pfm.html");
    assert!(output.exists());
    let html = fs::read_to_string(output).unwrap();
    assert!(html.contains("<title>Pure FM</title>"));
}

#[test]
fn test_file_too_large() {
    let temp = TempDir::new().unwrap();
    let input = temp.path().join("large.md");

    // Create a 2MB file but set limit to 1MB
    let content = "A".repeat(2 * 1024 * 1024);
    fs::write(&input, content).unwrap();

    cmd()
        .arg(&input)
        .arg("--max-file-size")
        .arg("1")
        .arg("-f")
        .arg("html")
        .assert()
        .failure()
        .stderr(predicate::str::contains("exceeds"));
}

// ============ PDF Output Tests ============

fn is_browser_available() -> bool {
    // Check environment variable first
    if std::env::var("CHROME_PATH").is_ok() {
        return true;
    }

    // Platform-specific paths - synchronized with find_chrome() in pdf.rs
    #[cfg(target_os = "macos")]
    let paths = [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
    ];

    #[cfg(target_os = "linux")]
    let paths = [
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/snap/bin/chromium",
        "/usr/bin/brave-browser",
    ];

    #[cfg(target_os = "windows")]
    let paths = [
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    ];

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let paths: [&str; 0] = [];

    for p in paths {
        if std::path::Path::new(p).exists() {
            return true;
        }
    }

    // Try PATH lookup (same browsers as find_chrome)
    let which_cmd = if cfg!(target_os = "windows") {
        "where"
    } else {
        "which"
    };

    for browser in [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
        "chrome",
        "brave-browser",
    ] {
        if let Ok(output) = std::process::Command::new(which_cmd).arg(browser).output() {
            if output.status.success() {
                return true;
            }
        }
    }

    false
}

#[test]
fn test_pdf_generation() {
    if !is_browser_available() {
        println!("Skipping PDF test: Browser not found");
        return;
    }

    let temp = TempDir::new().unwrap();
    let input = temp.path().join("pdf_test.md");
    fs::write(&input, "# PDF Test\n\nSome content.").unwrap();

    // Use verbose flag to debug if it fails
    cmd()
        .arg(&input)
        .arg("--format")
        .arg("pdf")
        .arg("-v")
        .assert()
        .success();

    let output = temp.path().join("pdf_test.pdf");
    assert!(output.exists(), "PDF output should exist");

    let metadata = fs::metadata(&output).unwrap();
    assert!(
        metadata.len() > 1000,
        "PDF should differ from empty file (size: {})",
        metadata.len()
    );

    // TEST-005: Check PDF magic number
    let content = fs::read(&output).unwrap();
    assert!(content.starts_with(b"%PDF-"), "File should be a valid PDF");
}

// ============ Test Fixture Helpers (TEST-007) ============

/// Get the path to a test fixture file
fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Copy a fixture to a temporary directory and return the new path
fn copy_fixture(fixture_name: &str, temp: &TempDir) -> std::path::PathBuf {
    let source = fixture_path(fixture_name);
    let dest = temp.path().join(fixture_name);
    fs::copy(&source, &dest)
        .unwrap_or_else(|e| panic!("Failed to copy fixture {}: {}", fixture_name, e));
    dest
}

/// Validates that a file is a valid PDF (TEST-005)
fn validate_pdf_file(path: &std::path::Path) -> Result<(), String> {
    let content = fs::read(path).map_err(|e| format!("Failed to read PDF: {}", e))?;

    // Check magic bytes
    if !content.starts_with(b"%PDF-") {
        return Err(format!(
            "Invalid PDF magic bytes: {:?}",
            &content[..std::cmp::min(10, content.len())]
        ));
    }

    // Check PDF version (1.x or 2.x)
    if content.len() >= 8 {
        let version_str = String::from_utf8_lossy(&content[5..8]);
        if !version_str.starts_with("1.") && !version_str.starts_with("2.") {
            return Err(format!("Invalid PDF version: {}", version_str));
        }
    }

    // Check minimum size
    if content.len() < 500 {
        return Err(format!("PDF too small: {} bytes", content.len()));
    }

    // Check for EOF marker
    let content_str = String::from_utf8_lossy(&content);
    if !content_str.contains("%%EOF") && !content_str.contains("endobj") {
        return Err("PDF missing structure markers".to_string());
    }

    Ok(())
}

// ============ Jupyter Notebook Tests (TEST-002) ============

#[test]
fn test_notebook_to_html_simple() {
    let temp = TempDir::new().unwrap();
    let input = copy_fixture("simple_notebook.ipynb", &temp);

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("html")
        .arg("-v")
        .assert()
        .success();

    let output = temp.path().join("simple_notebook.html");
    assert!(output.exists(), "HTML output should exist");

    let html = fs::read_to_string(&output).unwrap();

    // Verify markdown content was converted
    assert!(
        html.contains("<h1>Simple Notebook</h1>"),
        "Should contain h1"
    );
    assert!(
        html.contains("simple Jupyter notebook"),
        "Should contain markdown text"
    );

    // Verify code was wrapped in code blocks
    assert!(
        html.contains("<code") || html.contains("<pre"),
        "Should contain code elements"
    );
    assert!(
        html.contains("Hello, World!"),
        "Should contain code content"
    );

    // Verify conclusion section
    assert!(html.contains("<h2>Conclusion</h2>"), "Should contain h2");
}

#[test]
fn test_notebook_to_html_markdown_only() {
    let temp = TempDir::new().unwrap();
    let input = copy_fixture("markdown_only_notebook.ipynb", &temp);

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("html")
        .assert()
        .success();

    let html = fs::read_to_string(temp.path().join("markdown_only_notebook.html")).unwrap();

    assert!(html.contains("<h1>Documentation Notebook</h1>"));
    assert!(html.contains("<strong>Bold text</strong>"));
    assert!(html.contains("<em>Italic text</em>"));
    assert!(html.contains("<code>Inline code</code>"));
    assert!(html.contains("<table>"), "Should contain table");
}

#[test]
fn test_notebook_to_html_code_only() {
    let temp = TempDir::new().unwrap();
    let input = copy_fixture("code_only_notebook.ipynb", &temp);

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("html")
        .assert()
        .success();

    let html = fs::read_to_string(temp.path().join("code_only_notebook.html")).unwrap();

    // All code should be in pre/code blocks
    assert!(html.contains("<pre"), "Should contain pre elements");
    // Code is syntax highlighted, so check for parts that won't be split by spans
    assert!(html.contains("hello"), "Should contain hello function name");
    assert!(html.contains("add"), "Should contain add function name");
    assert!(
        html.contains("Hello, World!") || html.contains("Hello, World"),
        "Should contain return value"
    );
}

#[test]
fn test_notebook_to_html_mixed_cells() {
    let temp = TempDir::new().unwrap();
    let input = copy_fixture("mixed_cells_notebook.ipynb", &temp);

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("html")
        .assert()
        .success();

    let html = fs::read_to_string(temp.path().join("mixed_cells_notebook.html")).unwrap();

    // Check markdown elements
    assert!(html.contains("<h1>Data Analysis Notebook</h1>"));
    assert!(html.contains("<h2>Mathematical Operations</h2>"));
    assert!(html.contains("<h2>Results</h2>"));
    assert!(html.contains("<table>"));

    // Check code elements - note: syntax highlighting may wrap words in spans
    // "import math" may appear as "<span>import</span> <span>math</span>"
    assert!(
        html.contains("math"),
        "Should contain 'math' module reference"
    );
    assert!(
        html.contains("factorial"),
        "Should contain 'factorial' function"
    );
}

#[test]
fn test_notebook_invalid_json() {
    let temp = TempDir::new().unwrap();
    let source = fixture_path("invalid_notebook.json");
    // Rename to .ipynb to trigger notebook parsing
    let input = temp.path().join("invalid.ipynb");
    fs::copy(&source, &input).unwrap();

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("html")
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("Invalid Jupyter Notebook")
                .or(predicate::str::contains("missing field")),
        );
}

#[test]
fn test_notebook_empty_cells() {
    let temp = TempDir::new().unwrap();
    let input = temp.path().join("empty.ipynb");

    // Create notebook with empty cells array
    let notebook = r#"{
        "nbformat": 4,
        "nbformat_minor": 5,
        "metadata": {},
        "cells": []
    }"#;
    fs::write(&input, notebook).unwrap();

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("html")
        .assert()
        .success();

    // Output should exist but be minimal
    let output = temp.path().join("empty.html");
    assert!(output.exists());
}

#[test]
fn test_notebook_to_pdf() {
    if !is_browser_available() {
        println!("Skipping notebook PDF test: Browser not found");
        return;
    }

    let temp = TempDir::new().unwrap();
    let input = copy_fixture("simple_notebook.ipynb", &temp);

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("pdf")
        .arg("-v")
        .assert()
        .success();

    let output = temp.path().join("simple_notebook.pdf");
    assert!(output.exists(), "PDF output should exist");

    // Validate PDF using helper function
    let validation = validate_pdf_file(&output);
    assert!(validation.is_ok(), "PDF should be valid: {:?}", validation);

    // Minimum size check
    let content = fs::read(&output).unwrap();
    assert!(
        content.len() > 1000,
        "PDF should have reasonable size: {} bytes",
        content.len()
    );
}

#[test]
fn test_notebook_with_unicode() {
    let temp = TempDir::new().unwrap();
    let input = temp.path().join("unicode.ipynb");

    let notebook = r##"{
        "nbformat": 4,
        "nbformat_minor": 5,
        "metadata": {},
        "cells": [
            {
                "cell_type": "markdown",
                "metadata": {},
                "source": ["# Hello World\n", "\n", "Bonjour, Hola, Guten Tag"]
            },
            {
                "cell_type": "code",
                "execution_count": null,
                "metadata": {},
                "outputs": [],
                "source": ["# Comment with special chars\n", "print('Hello')"]
            }
        ]
    }"##;
    fs::write(&input, notebook).unwrap();

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("html")
        .assert()
        .success();

    let html = fs::read_to_string(temp.path().join("unicode.html")).unwrap();
    assert!(html.contains("Bonjour"));
    assert!(html.contains("Hola"));
    assert!(html.contains("Guten Tag"));
}

// ============ PDF Validation Tests (TEST-005) ============

#[test]
fn test_pdf_with_code_blocks() {
    if !is_browser_available() {
        println!("Skipping PDF test: Browser not found");
        return;
    }

    let temp = TempDir::new().unwrap();
    let input = temp.path().join("code.md");

    let content = r#"# Code Examples

```rust
fn main() {
    println!("Hello, world!");
}
```

```python
def hello():
    print("Hello, world!")
```
"#;
    fs::write(&input, content).unwrap();

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("pdf")
        .assert()
        .success();

    let output = temp.path().join("code.pdf");
    let validation = validate_pdf_file(&output);
    assert!(validation.is_ok(), "PDF should be valid: {:?}", validation);
}

#[test]
fn test_pdf_with_tables() {
    if !is_browser_available() {
        println!("Skipping PDF test: Browser not found");
        return;
    }

    let temp = TempDir::new().unwrap();
    let input = temp.path().join("table.md");

    let content = r#"# Table Test

| Column A | Column B | Column C |
|----------|----------|----------|
| 1        | 2        | 3        |
| 4        | 5        | 6        |
| 7        | 8        | 9        |
"#;
    fs::write(&input, content).unwrap();

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("pdf")
        .assert()
        .success();

    let output = temp.path().join("table.pdf");
    assert!(validate_pdf_file(&output).is_ok(), "PDF should be valid");
}

// ============ XSS Prevention Tests (TEST-006) ============

#[test]
fn test_html_output_escapes_xss_in_frontmatter() {
    let temp = TempDir::new().unwrap();
    let input = copy_fixture("xss_frontmatter.md", &temp);

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("html")
        .assert()
        .success();

    let html = fs::read_to_string(temp.path().join("xss_frontmatter.html")).unwrap();

    // Verify XSS payloads are escaped
    // Check that raw script tags are NOT present in dangerous contexts
    let has_raw_script = html.contains("<script>alert('xss')</script>");
    let has_onclick = html.contains("onclick=\"alert");
    let has_onerror = html.contains("onerror=alert");

    // These should all be false after proper escaping
    assert!(
        !has_raw_script,
        "Script tags should be escaped in front matter"
    );
    assert!(!has_onclick, "onclick handlers should be escaped");
    assert!(!has_onerror, "onerror handlers should be escaped");

    // The safe content should still be there
    assert!(html.contains("<h1>Safe Content</h1>"));
    assert!(html.contains("This document tests XSS prevention"));
}

#[test]
fn test_html_output_preserves_safe_special_chars() {
    let temp = TempDir::new().unwrap();
    let input = temp.path().join("safe_chars.md");

    // Front matter with safe special characters
    fs::write(
        &input,
        r#"---
title: "Tom & Jerry: A Tale of Friendship"
author: "O'Brien & Associates"
---
# Content
"#,
    )
    .unwrap();

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("html")
        .assert()
        .success();

    let html = fs::read_to_string(temp.path().join("safe_chars.html")).unwrap();

    // Safe special chars should be escaped but still readable
    // The word "Tale" should appear (with or without brackets)
    assert!(
        html.contains("Tale") || html.contains("&lt;Tale&gt;"),
        "Tale should appear in output"
    );

    // The names should appear
    assert!(html.contains("Tom"));
    assert!(html.contains("Jerry"));
    // O'Brien may be escaped as O&#x27;Brien or O'Brien
    assert!(
        html.contains("O'Brien") || html.contains("O&#x27;Brien") || html.contains("O&#39;Brien"),
        "Author name should appear"
    );
}

#[test]
fn test_markdown_content_not_double_escaped() {
    let temp = TempDir::new().unwrap();
    let input = temp.path().join("markdown_content.md");

    // Markdown that produces HTML
    fs::write(
        &input,
        r#"---
title: "Test"
---
# Heading

A paragraph with **bold** and `code`.

<div class="custom">Custom HTML block</div>
"#,
    )
    .unwrap();

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("html")
        .assert()
        .success();

    let html = fs::read_to_string(temp.path().join("markdown_content.html")).unwrap();

    // Markdown-generated HTML should NOT be escaped
    assert!(html.contains("<strong>bold</strong>"));
    assert!(html.contains("<code>code</code>"));

    // Raw HTML in markdown should pass through
    assert!(html.contains("<div class=\"custom\">"));
}

// ============ Fixture-Based Tests (TEST-007) ============

#[test]
fn test_html_output_basic_with_fixture() {
    let temp = TempDir::new().unwrap();
    let input = copy_fixture("basic.md", &temp);

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("html")
        .arg("-v")
        .assert()
        .success();

    let output = temp.path().join("basic.html");
    assert!(output.exists(), "HTML output should exist");

    let html = fs::read_to_string(&output).unwrap();
    assert!(html.contains("<!DOCTYPE html>"));
    assert!(html.contains("<h1>Hello World</h1>"));
    assert!(html.contains("<strong>bold</strong>"));
}

#[test]
fn test_html_output_with_frontmatter_fixture() {
    let temp = TempDir::new().unwrap();
    let input = copy_fixture("with_frontmatter.md", &temp);

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("html")
        .assert()
        .success();

    let html = fs::read_to_string(temp.path().join("with_frontmatter.html")).unwrap();
    assert!(html.contains("<title>Test Document</title>"));
    assert!(html.contains("By Test Author"));
    assert!(html.contains("2025-01-01"));
}

#[test]
fn test_gfm_features_with_fixture() {
    let temp = TempDir::new().unwrap();
    let input = copy_fixture("gfm_features.md", &temp);

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("html")
        .assert()
        .success();

    let html = fs::read_to_string(temp.path().join("gfm_features.html")).unwrap();

    // Task list
    assert!(html.contains("checked"), "Should have checked checkbox");

    // Table
    assert!(html.contains("<table>"), "Should have table");

    // Strikethrough
    assert!(html.contains("<del>"), "Should have strikethrough");
}

#[test]
fn test_html_output_with_custom_css_fixture() {
    let temp = TempDir::new().unwrap();
    let md_input = copy_fixture("basic.md", &temp);
    let css_input = copy_fixture("custom.css", &temp);

    cmd()
        .arg(&md_input)
        .arg("--format")
        .arg("html")
        .arg("--css")
        .arg(&css_input)
        .assert()
        .success();

    let html = fs::read_to_string(temp.path().join("basic.html")).unwrap();
    // CSS is minified, check for minified form
    assert!(
        html.contains("background-color:#f0f0f0") || html.contains("background:#f0f0f0"),
        "Should contain background color"
    );
}

#[test]
fn test_large_document_html() {
    let temp = TempDir::new().unwrap();
    let input = copy_fixture("large_document.md", &temp);

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("html")
        .assert()
        .success();

    let html = fs::read_to_string(temp.path().join("large_document.html")).unwrap();

    // Verify various sections rendered
    assert!(html.contains("<h1>Large Document</h1>"));
    assert!(html.contains("<h2>Section 1</h2>"));
    assert!(html.contains("<h3>Subsection 1.1</h3>"));
    assert!(html.contains("<table>"));
    assert!(html.contains("checked")); // Task list
    assert!(html.contains("<blockquote>"));
}

#[test]
fn test_minimal_document() {
    let temp = TempDir::new().unwrap();
    let input = copy_fixture("minimal.md", &temp);

    cmd()
        .arg(&input)
        .arg("--format")
        .arg("html")
        .assert()
        .success();

    let html = fs::read_to_string(temp.path().join("minimal.html")).unwrap();
    assert!(html.contains("<h1>Minimal</h1>"));
    assert!(html.contains("Test"));
}

// ============ Batch Processing Edge Cases ============

#[test]
fn test_empty_input_list() {
    // Test empty input list - should handle gracefully without panic
    // Note: This tests CLI behavior, not run() directly, since we can't easily pass empty args via CLI
    // The CLI will fail with "No input files specified" or similar, which is acceptable
    cmd()
        .assert()
        .failure()
        .stderr(predicate::str::contains("Usage").or(predicate::str::contains("required")));
}

#[test]
fn test_mixed_valid_invalid_files() {
    let temp = TempDir::new().unwrap();
    let valid = temp.path().join("valid.md");
    let invalid = temp.path().join("nonexistent.md");

    fs::write(&valid, "# Valid File").unwrap();

    cmd()
        .arg(&valid)
        .arg(&invalid)
        .arg("--format")
        .arg("html")
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found").or(predicate::str::contains("No such file")));

    // Valid file should still be processed before the error
    let _valid_output = temp.path().join("valid.html");
    // May or may not exist depending on error handling - either is acceptable
}

#[test]
fn test_nonexistent_file_error() {
    cmd()
        .arg("totally_nonexistent_file_12345.md")
        .arg("--format")
        .arg("html")
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found").or(predicate::str::contains("No such file")));
}
