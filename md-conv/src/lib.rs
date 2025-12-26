pub mod cli;
pub mod config;
pub mod error;
pub mod parser;
pub mod renderer;
pub mod security;
pub mod template;

use crate::cli::Args;

/// Minimal run function for early development phases
pub async fn run(_args: Args) -> anyhow::Result<()> {
    println!("md-conv: Argument parsing successful.");
    // In later phases, this will implement the main loop
    Ok(())
}
