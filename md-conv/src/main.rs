use clap::Parser;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() {
    let args = md_conv::cli::Args::parse();

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

    // Run conversion
    if let Err(_e) = md_conv::run(args).await {
        // Error already logged via tracing
        std::process::exit(1);
    }
}
