//! YARD - Yet Another Rust Desktop client.
//!
//! A native Wayland RDP client for Linux with multi-monitor fullscreen support.

use std::io::{self, Write};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use tokio::sync::mpsc;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use yard_protocol::{CertificateInfo, ConnectionConfig, FromNetwork, ToNetwork, spawn_network_thread};

/// Exit codes for YARD.
mod exit_codes {
    /// Successful execution.
    pub const SUCCESS: u8 = 0;
    /// Connection error.
    pub const CONNECTION_ERROR: u8 = 1;
    /// Authentication error.
    pub const AUTH_ERROR: u8 = 2;
    /// Protocol error.
    pub const PROTOCOL_ERROR: u8 = 3;
}

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

fn main() -> ExitCode {
    let cli = Cli::parse();

    // Initialize logging
    let filter = if cli.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };

    tracing_subscriber::fmt().with_env_filter(filter).init();

    match run(cli) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            error!("{:#}", e);
            ExitCode::from(exit_codes::CONNECTION_ERROR)
        }
    }
}

fn run(cli: Cli) -> Result<u8> {
    match cli.command {
        Some(Commands::Connect {
            host,
            port,
            username,
            domain,
        }) => {
            info!("YARD v{}", env!("CARGO_PKG_VERSION"));

            // Build config, parsing domain from username if present
            // Supports: DOMAIN\user, user@domain.com, or plain username
            let mut config = if let Some(ref user) = username {
                ConnectionConfig::from_username(&host, port, user)
            } else {
                ConnectionConfig::new(&host, port)
            };

            // Explicit -d/--domain flag overrides parsed domain
            if let Some(dom) = domain {
                config = config.with_domain(dom);
            }

            // Prompt for password if username is provided (secure, no echo)
            if config.username.is_some() {
                let password = prompt_password(&config)?;
                config = config.with_password(password);
            }

            run_connection(config)
        }

        Some(Commands::Completions { shell }) => {
            generate(shell, &mut Cli::command(), "yard", &mut io::stdout());
            Ok(exit_codes::SUCCESS)
        }

        None => {
            Cli::command().print_help()?;
            println!();
            Ok(exit_codes::SUCCESS)
        }
    }
}

/// Prompts for password securely (no echo).
///
/// SECURITY: The password is never logged (NFR-S2) and never appears in
/// command-line history (NFR-S4) since it's read from stdin, not CLI args.
fn prompt_password(config: &ConnectionConfig) -> Result<String> {
    // Show prompt with username and domain context
    match (&config.username, &config.domain) {
        (Some(user), Some(domain)) => eprint!("Password for {}\\{}: ", domain, user),
        (Some(user), None) => eprint!("Password for {}: ", user),
        _ => eprint!("Password: "),
    }
    io::stderr().flush().context("Failed to flush stderr")?;

    // Use rpassword for secure input (no echo)
    let password =
        rpassword::read_password().context("Failed to read password (is stdin a terminal?)")?;

    // Validate password is not empty
    if password.is_empty() {
        anyhow::bail!("Password cannot be empty");
    }

    Ok(password)
}

/// Prompts the user to accept or reject a server certificate.
///
/// Displays certificate details and asks for confirmation.
/// Returns true if accepted, false if rejected.
fn prompt_certificate_verification(server: &str, cert_info: &CertificateInfo) -> bool {
    eprintln!();
    eprintln!("┌─────────────────────────────────────────────────────────────────┐");
    eprintln!(
        "│ Unknown certificate from {:<38} │",
        truncate_string(server, 38)
    );
    eprintln!("├─────────────────────────────────────────────────────────────────┤");

    // Display certificate details
    for line in cert_info.display().lines() {
        eprintln!("│ {:<63} │", line);
    }

    eprintln!("├─────────────────────────────────────────────────────────────────┤");
    eprintln!("│ Accept this certificate? [y/N]                                  │");
    eprintln!("└─────────────────────────────────────────────────────────────────┘");
    eprint!("  > ");

    if io::stderr().flush().is_err() {
        return false;
    }

    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return false;
    }

    let input = input.trim().to_lowercase();
    matches!(input.as_str(), "y" | "yes")
}

/// Truncates a string to max_len characters, adding "..." if truncated.
/// Safe for multi-byte UTF-8 characters.
fn truncate_string(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        s.to_string()
    } else if max_len > 3 {
        let truncated: String = s.chars().take(max_len - 3).collect();
        format!("{}...", truncated)
    } else {
        s.chars().take(max_len).collect()
    }
}

/// Runs the RDP connection.
fn run_connection(config: ConnectionConfig) -> Result<u8> {
    info!("Connecting to {}...", config.address());

    if let Some(ref user) = config.username {
        if let Some(ref dom) = config.domain {
            info!("User: {}\\{}", dom, user);
        } else {
            info!("User: {}", user);
        }
    }

    // Create channel for receiving messages from network thread
    let (from_network_tx, mut from_network_rx) = mpsc::channel::<FromNetwork>(32);

    // Spawn network thread
    let to_network_tx = spawn_network_thread(from_network_tx);

    // Send connect command
    to_network_tx
        .blocking_send(ToNetwork::Connect(config))
        .map_err(|_| anyhow::anyhow!("Failed to send connect command"))?;

    // Simple blocking loop to receive messages
    // TODO: Replace with calloop event loop in Story 1.8
    let exit_code = loop {
        match from_network_rx.blocking_recv() {
            Some(FromNetwork::Connecting) => {
                info!("Establishing connection...");
            }
            Some(FromNetwork::Connected) => {
                info!("Connected successfully!");
            }
            Some(FromNetwork::Disconnected) => {
                info!("Disconnected.");
                break exit_codes::SUCCESS;
            }
            Some(FromNetwork::CertificateVerify { server, cert_info }) => {
                let accepted = prompt_certificate_verification(&server, &cert_info);
                if to_network_tx
                    .blocking_send(ToNetwork::CertificateDecision(accepted))
                    .is_err()
                {
                    error!("Failed to send certificate decision");
                    break exit_codes::CONNECTION_ERROR;
                }
            }
            Some(FromNetwork::Error(err)) => {
                error!("{}", err);
                break match err {
                    yard_protocol::ConnectionError::AuthenticationFailed(_) => {
                        exit_codes::AUTH_ERROR
                    }
                    yard_protocol::ConnectionError::Protocol(_) => exit_codes::PROTOCOL_ERROR,
                    _ => exit_codes::CONNECTION_ERROR,
                };
            }
            None => {
                error!("Network thread terminated unexpectedly");
                break exit_codes::CONNECTION_ERROR;
            }
        }
    };

    Ok(exit_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_string_short() {
        assert_eq!(truncate_string("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_string_exact() {
        assert_eq!(truncate_string("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_string_long() {
        assert_eq!(truncate_string("hello world", 8), "hello...");
    }

    #[test]
    fn test_truncate_string_very_short_max() {
        // When max_len <= 3, just truncate without ellipsis
        assert_eq!(truncate_string("hello", 3), "hel");
        assert_eq!(truncate_string("hello", 2), "he");
    }

    #[test]
    fn test_truncate_string_empty() {
        assert_eq!(truncate_string("", 10), "");
    }

    #[test]
    fn test_truncate_string_multibyte_safe() {
        // UTF-8 multi-byte characters should not panic
        assert_eq!(truncate_string("héllo", 4), "h...");
        assert_eq!(truncate_string("日本語テスト", 5), "日本...");
        assert_eq!(truncate_string("émoji🎉test", 6), "émo...");
    }
}
