//! # md-conv
//!
//! A high-performance Markdown to PDF and HTML converter built with Rust.
//!
//! This library provides the core logic for parsing Markdown (including Jupyter Notebooks),
//! managing configurations, and rendering to various output formats using a pooled
//! Headless Chrome architecture.
//!
//! ## Key Features
//! - **Fast PDF Generation**: Uses a connection pool of Chrome instances to minimize startup overhead.
//! - **Jupyter Support**: Seamlessly converts `.ipynb` files via internal markdown extraction.
//! - **Security Focused**: Implements CSS sanitization, path escape protection, and file size limits.
//! - **Concurrent Processing**: Automatically processes multiple files in parallel with bounded resource usage.
//!
//! ## Quick Start (CLI-style)
//!
//! ```rust,no_run
//! use md_conv::{Args, run, ConversionError};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), ConversionError> {
//!     let args = Args {
//!         input: vec!["document.md".into()],
//!         ..Args::default()
//!     };
//!     run(args).await
//! }
//! ```
//!
//! ## Quick Start (Library)
//!
//! ```rust
//! use md_conv::parser::parse_front_matter;
//!
//! let markdown = "# Hello\n\nWorld";
//! let (front_matter, body) = parse_front_matter(markdown).unwrap();
//! assert!(body.contains("Hello"));
//! ```

pub mod cli;
pub mod config;
pub mod error;
pub mod parser;
pub mod renderer;
pub mod security;
pub mod template;

use anyhow::Context;
pub use cli::Args;
pub use config::ConversionConfig;
pub use error::ConversionError;
use futures::stream::{self, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use notify::{RecursiveMode, Result as NotifyResult, Watcher};
pub use parser::ParsedDocument;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tracing::{info, instrument};

/// Maximum concurrent file conversions.
///
/// The browser pool in `renderer/pdf.rs` provides additional backpressure
/// specifically for Chrome instances.
const MAX_CONCURRENT_CONVERSIONS: usize = 4;

/// Source of the input content.
#[derive(Debug, Clone)]
pub(crate) enum InputSource {
    File(PathBuf),
    Stdin,
}

impl std::fmt::Display for InputSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InputSource::File(p) => write!(f, "{}", p.display()),
            InputSource::Stdin => write!(f, "<stdin>"),
        }
    }
}

/// Structured summary of a single conversion result.
#[derive(Debug, Clone, Serialize)]
pub struct ConversionResult {
    pub input: String,
    pub output: Vec<PathBuf>,
    pub status: String,
    pub error: Option<String>,
}

/// Process a single input source.
///
/// This function handles the entire conversion pipeline:
/// 1. Reading content (File or Stdin)
/// 2. Security validation
/// 3. Parsing (Split phase)
/// 4. Configuration merging
/// 5. Rendering
/// 6. Output writing (File or Stdout)
#[instrument(skip(args, content), fields(source = %source))]
#[instrument(skip(args, content), fields(source = %source))]
async fn process_input(
    source: InputSource,
    content: Option<String>,
    args: &Args,
) -> anyhow::Result<ConversionResult> {
    info!("Starting conversion");
    let start_time = std::time::Instant::now();

    // 1. Determine input path and read content
    let (input_path, content) = read_input_content(&source, content, args).await?;

    // 2. Parse and merge configuration
    let (doc, config) = parse_and_config(&input_path, &content, args).await?;

    // 3. Generate full HTML
    let template_ctx = template::create_context(&doc, &config);
    let full_html = template::render_html(template_ctx)?;

    // 4. Render to each requested format
    let mut output_paths = Vec::new();

    for format in &config.output_formats {
        let renderer = renderer::create_renderer(format);
        let output_path = render_and_save(
            renderer.as_ref(),
            &full_html,
            &config,
            args,
            &input_path,
            &source,
        )
        .await?;

        if let Some(path) = output_path {
            output_paths.push(path);
        }
    }

    let duration = start_time.elapsed();
    info!("Conversion completed in {:?}", duration);

    Ok(ConversionResult {
        input: source.to_string(),
        output: output_paths,
        status: "success".to_string(),
        error: None,
    })
}

async fn read_input_content(
    source: &InputSource,
    content: Option<String>,
    args: &Args,
) -> anyhow::Result<(PathBuf, String)> {
    match source {
        InputSource::File(path) => {
            if let Some(c) = content {
                Ok((path.clone(), c))
            } else {
                let mut file =
                    security::validate_file_size(path, args.max_file_size * 1024 * 1024).await?;
                let mut c = String::new();
                file.read_to_string(&mut c)
                    .await
                    .with_context(|| format!("Failed to read: {}", path.display()))?;
                Ok((path.clone(), c))
            }
        }
        InputSource::Stdin => {
            if let Some(c) = content {
                Ok((PathBuf::from("stdin.md"), c))
            } else {
                let mut c = String::new();
                let max_bytes = args.max_file_size * 1024 * 1024;
                let mut stdin = tokio::io::stdin().take(max_bytes);
                stdin
                    .read_to_string(&mut c)
                    .await
                    .context("Failed to read stdin")?;
                Ok((PathBuf::from("stdin.md"), c))
            }
        }
    }
}

async fn parse_and_config(
    input_path: &Path,
    content: &str,
    args: &Args,
) -> anyhow::Result<(parser::ParsedDocument, config::ConversionConfig)> {
    // 2. Parse content (Front Matter Phase)
    let (front_matter, raw_markdown) = if input_path.extension().is_some_and(|ext| ext == "ipynb") {
        let md = parser::parse_notebook_raw(content)?;
        parser::parse_front_matter(&md)?
    } else {
        parser::parse_front_matter(content)?
    };

    // 3. Merge configuration
    let config = ConversionConfig::merge(args, front_matter.clone(), input_path).await?;

    // 4. Generate HTML (Body Phase) with highlighting
    let (html_content, toc_html) = parser::generate_html(&raw_markdown, &config.highlight_theme)?;

    let doc = parser::ParsedDocument {
        front_matter,
        html_content,
        toc_html: Some(toc_html),
    };

    Ok((doc, config))
}

async fn render_and_save(
    renderer: &dyn renderer::Renderer,
    full_html: &str,
    config: &config::ConversionConfig,
    args: &Args,
    input_path: &Path,
    source: &InputSource,
) -> anyhow::Result<Option<PathBuf>> {
    let span = tracing::info_span!("render", format = renderer.name());
    let _guard = span.enter();

    info!("Rendering to {}", renderer.name());

    let output = renderer.render(full_html, config).await?;

    // Determine output destination
    if args.stdout && matches!(output.extension, "html") {
        let mut stdout = tokio::io::stdout();
        stdout.write_all(&output.bytes).await?;
        stdout.flush().await?;
        Ok(None)
    } else {
        // Determine output path
        let output_path = if let Some(explicit) = &args.output {
            explicit.clone()
        } else if let Some(dir) = &config.output_dir {
            let filename = input_path
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("output"));
            let mut p = dir.join(filename);
            p.set_extension(output.extension);
            p
        } else {
            let mut p = input_path.to_path_buf();
            if matches!(source, InputSource::Stdin) {
                p = PathBuf::from("output"); // Default name for stdin
            }
            p.set_extension(output.extension);
            p
        };

        // Write output
        renderer::write_output(&output, &output_path).await?;
        info!(path = %output_path.display(), "Created output");
        Ok(Some(output_path))
    }
}

/// Process a single Markdown or Jupyter Notebook file.
/// (Maintained for backward compatibility/tests, delegates to process_input)
///
/// # Examples
///
/// ```rust,no_run
/// use md_conv::{Args, convert_file};
/// use std::path::Path;
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let args = Args::default();
///     let output_paths = convert_file(Path::new("document.md"), &args).await?;
///     println!("Generated: {:?}", output_paths);
///     Ok(())
/// }
/// ```
pub async fn convert_file(input_path: &Path, args: &Args) -> anyhow::Result<Vec<PathBuf>> {
    let result = process_input(InputSource::File(input_path.to_path_buf()), None, args).await?;
    Ok(result.output)
}

/// Main entry point for the conversion tool.
#[instrument(skip(args), name = "md_conv")]
pub async fn run(args: Args) -> Result<(), ConversionError> {
    let args = Arc::new(args);

    // Watch mode hook
    if args.watch {
        return run_watch_mode(args).await;
    }

    // Prepare inputs
    let inputs: Vec<InputSource> = if args.stdin {
        vec![InputSource::Stdin]
    } else {
        args.input
            .iter()
            .map(|p| InputSource::File(p.clone()))
            .collect()
    };

    let num_inputs = inputs.len() as u64;
    let pb = if !args.quiet && !args.json && num_inputs > 0 {
        let pb = ProgressBar::new(num_inputs);
        pb.set_style(ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}",
        )
        .map_err(|e| ConversionError::Generic(e.to_string()))?
        .progress_chars("#>-"));
        Some(pb)
    } else {
        None
    };

    // Shared collectors
    let results: Arc<Mutex<Vec<ConversionResult>>> = Arc::new(Mutex::new(Vec::new()));

    // Stream processing
    stream::iter(inputs)
        .map(|source| {
            let args = Arc::clone(&args);
            let results = Arc::clone(&results);
            let pb = pb.clone();
            async move {
                if let Some(bar) = &pb {
                    bar.set_message(format!("Processing {}", source));
                }

                let res = match process_input(source.clone(), None, &args).await {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::error!(source = %source, error = %e, "Conversion failed");
                        ConversionResult {
                            input: source.to_string(),
                            output: vec![],
                            status: "error".to_string(),
                            error: Some(e.to_string()),
                        }
                    }
                };
                results.lock().await.push(res);

                if let Some(bar) = &pb {
                    bar.inc(1);
                }
            }
        })
        .buffer_unordered(MAX_CONCURRENT_CONVERSIONS)
        .collect::<Vec<()>>()
        .await;

    if let Some(bar) = &pb {
        bar.finish_with_message("Done!");
    }

    // Report results
    let results = results.lock().await;

    if args.json {
        let json_output = serde_json::to_string_pretty(&*results)
            .map_err(|e| ConversionError::Generic(e.to_string()))?;
        println!("{}", json_output);
    } else if !args.quiet {
        // Standard textual summary
        let success_count = results.iter().filter(|r| r.status == "success").count();
        let error_count = results.iter().filter(|r| r.status == "error").count();

        if success_count > 0 {
            println!("\nConverted {} file(s):", success_count);
            for res in results.iter().filter(|r| r.status == "success") {
                for path in &res.output {
                    println!("  -> {}", path.display());
                }
            }
        }
        if error_count > 0 {
            eprintln!("\nFailed {} file(s):", error_count);
            for res in results.iter().filter(|r| r.status == "error") {
                eprintln!(
                    "  X  {}: {}",
                    res.input,
                    res.error.as_deref().unwrap_or("Unknown error")
                );
            }
            // If explicit files requested failed, we error out
            if success_count == 0 {
                return Err(ConversionError::Generic(format!(
                    "{} file(s) failed to convert",
                    error_count
                )));
            }
        }
    } else {
        // Quiet mode - only check for errors to return exit code
        let error_count = results.iter().filter(|r| r.status == "error").count();
        if error_count > 0 {
            return Err(ConversionError::Generic(format!(
                "{} file(s) failed to convert",
                error_count
            )));
        }
    }

    // Shutdown browser pool
    renderer::browser_pool().shutdown().await;

    Ok(())
}

/// Watch mode loop
async fn run_watch_mode(args: Arc<Args>) -> Result<(), ConversionError> {
    info!("Starting watch mode...");
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    // Setup watcher
    let mut watcher = notify::recommended_watcher(move |res: NotifyResult<notify::Event>| {
        match res {
            Ok(event) => {
                // Filter for Modify/Create events
                if matches!(
                    event.kind,
                    notify::EventKind::Modify(_) | notify::EventKind::Create(_)
                ) {
                    let _ = tx.blocking_send(event);
                }
            }
            Err(e) => tracing::error!("Watch error: {:?}", e),
        }
    })
    .map_err(|e| ConversionError::Generic(e.to_string()))?;

    // Watch inputs
    for input in &args.input {
        if input.exists() {
            watcher
                .watch(input, RecursiveMode::NonRecursive)
                .map_err(|e| ConversionError::Generic(e.to_string()))?;
        }
    }
    // Watch CSS if provided
    if let Some(css) = &args.css {
        if css.exists() {
            watcher
                .watch(css, RecursiveMode::NonRecursive)
                .map_err(|e| ConversionError::Generic(e.to_string()))?;
        }
    }

    println!("Watching for changes... (Press Ctrl+C to stop)");

    // Debounce/Event loop
    // Simple implementation: convert whatever file changed.
    while let Some(event) = rx.recv().await {
        for path in event.paths {
            // Check if it's one of our inputs
            if args.input.contains(&path) {
                info!("File changed: {}", path.display());
                match process_input(InputSource::File(path.clone()), None, &args).await {
                    Ok(res) => {
                        println!("Re-converted: {}", path.display());
                        if let Some(err) = res.error {
                            eprintln!("  Error: {}", err);
                        }
                    }
                    Err(e) => eprintln!("Failed to re-convert {}: {}", path.display(), e),
                }
            } else if Some(&path) == args.css.as_ref() {
                // If CSS changed, re-convert ALL inputs
                info!("CSS changed, re-converting all files");
                for input in &args.input {
                    let _ = process_input(InputSource::File(input.clone()), None, &args).await;
                }
            }
        }
    }

    Ok(())
}
