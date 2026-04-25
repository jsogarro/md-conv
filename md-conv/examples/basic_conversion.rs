//! Basic markdown to PDF conversion
//!
//! This example demonstrates the simplest use case: converting a Markdown file to PDF
//! using the default settings.
//!
//! Usage:
//!   cargo run --example basic_conversion -- document.md
//!
//! The program parses CLI arguments and delegates to md-conv's main run() function,
//! which handles the entire conversion pipeline.

use clap::Parser;
use md_conv::{run, Args};

#[tokio::main]
async fn main() -> Result<(), md_conv::ConversionError> {
    let args = Args::parse();
    run(args).await
}
