//! CSS sanitization example
//!
//! This example demonstrates how md-conv sanitizes CSS to prevent XSS attacks and
//! malicious CSS constructs. The sanitization process:
//!
//! 1. Parses CSS with lightningcss
//! 2. Validates syntax
//! 3. Removes dangerous constructs (javascript: URLs, @import, expression())
//! 4. Minifies the result
//!
//! Safe CSS passes through, while dangerous CSS is rejected.
//!
//! Usage:
//!   cargo run --example css_sanitization

use md_conv::security::sanitize_css;

fn main() -> Result<(), md_conv::ConversionError> {
    println!("=== CSS Sanitization Examples ===\n");

    // Example 1: Safe CSS passes through
    println!("1. Safe CSS:");
    let safe_css = r#"
        body {
            color: red;
            font-size: 16px;
            margin: 0 auto;
        }
        .container {
            max-width: 800px;
            padding: 20px;
        }
    "#;
    match sanitize_css(safe_css) {
        Ok(sanitized) => {
            println!("   Input:  {}", safe_css.trim());
            println!("   Output: {}", sanitized);
            println!("   ✓ Passed\n");
        }
        Err(e) => println!("   ✗ Unexpectedly blocked: {}\n", e),
    }

    // Example 2: Dangerous URL scheme is rejected
    println!("2. Dangerous javascript: URL:");
    let dangerous_url = "body { background: url('javascript:alert(1)'); }";
    match sanitize_css(dangerous_url) {
        Ok(_) => println!("   ✗ Unexpectedly passed\n"),
        Err(e) => {
            println!("   Input:  {}", dangerous_url);
            println!("   ✓ Blocked: {}\n", e);
        }
    }

    // Example 3: @import is rejected
    println!("3. @import directive:");
    let import_css = "@import url('https://evil.com/malicious.css');";
    match sanitize_css(import_css) {
        Ok(_) => println!("   ✗ Unexpectedly passed\n"),
        Err(e) => {
            println!("   Input:  {}", import_css);
            println!("   ✓ Blocked: {}\n", e);
        }
    }

    // Example 4: data: URLs in content are rejected
    println!("4. data: URL in content:");
    let data_url = "div::before { content: url('data:text/html,<script>alert(1)</script>'); }";
    match sanitize_css(data_url) {
        Ok(_) => println!("   ✗ Unexpectedly passed\n"),
        Err(e) => {
            println!("   Input:  {}", data_url);
            println!("   ✓ Blocked: {}\n", e);
        }
    }

    // Example 5: Invalid syntax is rejected
    println!("5. Invalid CSS syntax:");
    let invalid_css = "body { color: red; unclosed brace";
    match sanitize_css(invalid_css) {
        Ok(_) => println!("   ✗ Unexpectedly passed\n"),
        Err(e) => {
            println!("   Input:  {}", invalid_css);
            println!("   ✓ Blocked: {}\n", e);
        }
    }

    // Example 6: Complex valid CSS
    println!("6. Complex valid CSS:");
    let complex_css = r#"
        @media (min-width: 768px) {
            .grid {
                display: grid;
                grid-template-columns: repeat(3, 1fr);
                gap: 20px;
            }
        }
        .card:hover {
            transform: scale(1.05);
            box-shadow: 0 10px 20px rgba(0,0,0,0.2);
        }
    "#;
    match sanitize_css(complex_css) {
        Ok(sanitized) => {
            println!("   Input:  {}", complex_css.trim());
            println!("   Output: {}", sanitized);
            println!("   ✓ Passed\n");
        }
        Err(e) => println!("   ✗ Unexpectedly blocked: {}\n", e),
    }

    println!("=== Summary ===");
    println!("CSS sanitization provides defense-in-depth protection against:");
    println!("  • XSS via javascript: URLs");
    println!("  • CSS injection via @import");
    println!("  • Data exfiltration via malicious URLs");
    println!("  • Invalid/malformed CSS syntax");
    println!("\nAll CSS is validated, minified, and sanitized before rendering.");

    Ok(())
}
