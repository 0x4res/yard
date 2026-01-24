//! YARD - Yet Another Rust Desktop client.
//!
//! A native Wayland RDP client for Linux with multi-monitor fullscreen support.

use anyhow::Result;
use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;

/// YARD - Yet Another Rust Desktop client.
#[derive(Parser, Debug)]
#[command(name = "yard")]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Target hostname or IP address.
    #[arg(short = 'H', long)]
    host: Option<String>,

    /// Target port (default: 3389).
    #[arg(short, long, default_value = "3389")]
    port: u16,

    /// Enable verbose logging.
    #[arg(short, long)]
    verbose: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging
    let filter = if args.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };

    tracing_subscriber::fmt().with_env_filter(filter).init();

    info!("YARD v{}", env!("CARGO_PKG_VERSION"));

    if let Some(host) = &args.host {
        info!("Target: {}:{}", host, args.port);
    } else {
        info!("No host specified. Use --host to connect.");
    }

    Ok(())
}
