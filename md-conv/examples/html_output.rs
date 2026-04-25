//! Convert markdown to HTML and print to stdout
//!
//! This example demonstrates how to use md-conv programmatically to convert a Markdown
//! file to HTML without requiring Chrome/Chromium. The HTML output is written to stdout
//! in quiet mode, making it suitable for piping to other tools.
//!
//! Usage:
//!   cargo run --example html_output
//!   cargo run --example html_output > output.html
//!
//! This example uses the Args struct with explicit field values to configure the conversion.

use md_conv::{run, Args};

#[tokio::main]
async fn main() -> Result<(), md_conv::ConversionError> {
    let args = Args {
        input: vec!["README.md".into()],
        format: Some(vec![md_conv::cli::OutputFormat::Html]),
        stdout: true,
        quiet: true,
        ..Args::default()
    };

    run(args).await
}
