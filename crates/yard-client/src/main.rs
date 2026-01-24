//! YARD - Yet Another Rust Desktop client.
//!
//! A native Wayland RDP client for Linux with multi-monitor fullscreen support.

use std::io;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use tracing::info;
use tracing_subscriber::EnvFilter;

/// YARD - Yet Another Rust Desktop client.
///
/// A native Wayland RDP client for Linux with multi-monitor fullscreen support.
#[derive(Parser)]
#[command(name = "yard")]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Enable verbose logging (debug level).
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Connect to an RDP server.
    Connect {
        /// Target hostname or IP address.
        host: String,

        /// Target port.
        #[arg(short, long, default_value = "3389")]
        port: u16,

        /// Username for authentication.
        #[arg(short, long)]
        username: Option<String>,

        /// Domain for authentication.
        #[arg(short, long)]
        domain: Option<String>,
    },

    /// Generate shell completion scripts.
    Completions {
        /// Shell to generate completions for.
        #[arg(value_enum)]
        shell: Shell,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    let filter = if cli.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };

    tracing_subscriber::fmt().with_env_filter(filter).init();

    match cli.command {
        Some(Commands::Connect {
            host,
            port,
            username,
            domain,
        }) => {
            info!("YARD v{}", env!("CARGO_PKG_VERSION"));
            info!("Connecting to {}:{}", host, port);

            if let Some(ref user) = username {
                if let Some(ref dom) = domain {
                    info!("User: {}\\{}", dom, user);
                } else {
                    info!("User: {}", user);
                }
            }

            // TODO: Implement actual RDP connection in Story 1.4
            info!("Connection not yet implemented. See Story 1.4.");
        }

        Some(Commands::Completions { shell }) => {
            generate(shell, &mut Cli::command(), "yard", &mut io::stdout());
        }

        None => {
            // No subcommand provided, show help
            Cli::command().print_help()?;
            println!();
        }
    }

    Ok(())
}
