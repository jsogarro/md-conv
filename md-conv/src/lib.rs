pub mod cli;
pub mod config;
pub mod error;
pub mod parser;
pub mod renderer;
pub mod security;
pub mod template;

use anyhow::Context;
use futures::stream::{self, StreamExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, instrument};

pub use cli::Args;
pub use config::ConversionConfig;
pub use error::ConversionError;
pub use parser::ParsedDocument;

/// Maximum concurrent file conversions
/// Browser semaphore provides additional backpressure for PDF rendering
const MAX_CONCURRENT_CONVERSIONS: usize = 4;

/// Process a single Markdown file
#[instrument(skip(args), fields(input = %input_path.display()))]
pub async fn convert_file(input_path: &Path, args: &Args) -> anyhow::Result<Vec<PathBuf>> {
    info!("Starting conversion");

    // 1. Validate file size
    security::validate_file_size(input_path, args.max_file_size * 1024 * 1024)?;

    // 2. Read the file
    let content = std::fs::read_to_string(input_path)
        .with_context(|| format!("Failed to read: {}", input_path.display()))?;

    // 3. Parse Markdown + Front Matter
    let doc = parser::parse_markdown(&content)?;

    // 4. Merge configuration
    let config = ConversionConfig::merge(args, doc.front_matter.clone(), input_path)?;

    // 5. Generate full HTML
    let template_ctx = template::create_context(&doc, &config);
    let full_html = template::render_html(template_ctx)?;

    // 6. Render to each requested format
    let mut output_paths = Vec::new();

    for format in &config.output_formats {
        let renderer = renderer::create_renderer(format);
        let span = tracing::info_span!("render", format = renderer.name());
        let _guard = span.enter();

        info!("Rendering to {}", renderer.name());

        let output = renderer.render(&full_html, &config).await?;

        // Determine output path
        let output_path = if let Some(explicit) = &args.output {
            explicit.clone()
        } else {
            input_path.with_extension(output.extension)
        };

        // Write output
        renderer::write_output(&output, &output_path).await?;

        info!(path = %output_path.display(), "Created output");
        output_paths.push(output_path);
    }

    Ok(output_paths)
}

/// Main entry point with parallel file processing
///
/// Uses bounded concurrency to prevent resource exhaustion when
/// processing many files. The browser semaphore in pdf.rs provides
/// additional backpressure specifically for Chrome instances.
#[instrument(skip(args), name = "md_conv")]
pub async fn run(args: Args) -> anyhow::Result<()> {
    let args = Arc::new(args);

    // Shared collectors for results and errors (protected by mutex)
    let all_outputs: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
    let errors: Arc<Mutex<Vec<(PathBuf, anyhow::Error)>>> = Arc::new(Mutex::new(Vec::new()));

    // Process files concurrently with bounded parallelism
    let input_paths: Vec<PathBuf> = args.input.clone();

    stream::iter(input_paths)
        .map(|input_path| {
            let args = Arc::clone(&args);
            let all_outputs = Arc::clone(&all_outputs);
            let errors = Arc::clone(&errors);

            async move {
                match convert_file(&input_path, &args).await {
                    Ok(outputs) => {
                        let mut guard = all_outputs.lock().await;
                        guard.extend(outputs);
                    }
                    Err(e) => {
                        tracing::error!(path = %input_path.display(), error = %e, "Conversion failed");
                        let mut guard = errors.lock().await;
                        guard.push((input_path, e));
                    }
                }
            }
        })
        .buffer_unordered(MAX_CONCURRENT_CONVERSIONS)
        .collect::<Vec<()>>()
        .await;

    // Extract results from mutex (no longer need locks after stream completes)
    let all_outputs = Arc::try_unwrap(all_outputs)
        .expect("all references dropped")
        .into_inner();
    let errors = Arc::try_unwrap(errors)
        .expect("all references dropped")
        .into_inner();

    // Report results
    if !all_outputs.is_empty() {
        println!("\nConverted {} file(s):", all_outputs.len());
        for path in &all_outputs {
            println!("  -> {}", path.display());
        }
    }

    if !errors.is_empty() {
        eprintln!("\nFailed {} file(s):", errors.len());
        for (path, error) in &errors {
            eprintln!("  X  {}: {}", path.display(), error);
        }
        anyhow::bail!("{} file(s) failed to convert", errors.len());
    }

    Ok(())
}
