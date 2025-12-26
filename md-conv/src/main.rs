use clap::Parser;
use md_conv::cli;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = cli::Args::parse();

    // Initialize logging with spans for structured diagnostics
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(args.log_level()));

    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false)
                .with_thread_ids(args.verbose >= 3)
                .with_file(args.verbose >= 2)
                .with_line_number(args.verbose >= 2),
        )
        .init();

    // Validate arguments
    if let Err(e) = args.validate() {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }

    // Run conversion (calling into lib.rs)
    if let Err(e) = md_conv::run(args).await {
        tracing::error!(error = %e, "Conversion failed");
        eprintln!("\nError: {e:?}");
        std::process::exit(1);
    }

    Ok(())
}
