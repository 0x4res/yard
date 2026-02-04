//! YARD - Yet Another Rust Desktop client.
//!
//! A native Wayland RDP client for Linux with multi-monitor fullscreen support.

use std::io::{self, Write};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use tokio::sync::mpsc;
use tracing::{debug, error, info, info_span, warn};
use tracing_subscriber::EnvFilter;
use yard_core::Config;
use yard_protocol::{
    CertificateInfo, ConnectionConfig, DesktopSize, FromNetwork, ToNetwork, spawn_network_thread,
};

#[cfg(target_os = "linux")]
use yard_protocol::MouseButton;

#[cfg(target_os = "linux")]
use yard_protocol::cliprdr::file_transfer::FileTransferManager;

use yard_audio::AudioThread;
use yard_wayland::WindowConfig;

// Story 6.2: Overlay content types (Linux only - requires Wayland)
#[cfg(target_os = "linux")]
use yard_wayland::{ConnectionStatus, OverlayContent};

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
#[command(author, version, about)]
#[command(
    long_about = "A native Wayland RDP client for Linux with multi-monitor fullscreen support.\n\n\
Exit codes:\n  \
  0  Success\n  \
  1  Connection error (network, timeout, server unreachable)\n  \
  2  Authentication error (invalid credentials)\n  \
  3  Protocol error (RDP negotiation failed)\n\n\
Logging:\n  \
  Set RUST_LOG for fine-grained control (e.g., RUST_LOG=yard_protocol=trace)"
)]
struct Cli {
    /// Enable verbose logging (debug level for yard_* crates).
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Connect to an RDP server.
    ///
    /// The HOST argument can be either:
    /// - A profile name defined in config.toml (e.g., `yard connect work`)
    /// - A hostname or IP address (e.g., `yard connect server.example.com`)
    ///
    /// If a profile name matches, it takes precedence over treating the argument as a hostname.
    /// When using a profile, its settings are applied but CLI arguments take precedence.
    Connect {
        /// Target hostname, IP address, or profile name from config.toml.
        /// Profile names take precedence: if "work" is both a profile and a valid hostname,
        /// the profile settings are used.
        host: String,

        /// Target port (default: 3389, or from config file).
        #[arg(short, long)]
        port: Option<u16>,

        /// Username for authentication.
        #[arg(short, long)]
        username: Option<String>,

        /// Domain for authentication.
        #[arg(short, long)]
        domain: Option<String>,

        /// Start in fullscreen mode.
        /// Overrides config defaults and profile settings.
        /// Can also be set in config.toml [defaults] or per-profile.
        #[arg(short = 'f', long, conflicts_with = "no_fullscreen")]
        fullscreen: bool,

        /// Explicitly disable fullscreen mode (Story 6.7).
        /// Overrides config defaults and profile settings.
        #[arg(long, conflicts_with = "fullscreen")]
        no_fullscreen: bool,

        /// Enable multi-monitor fullscreen mode (Story 3.4).
        /// Creates a separate window on each connected monitor.
        /// Only effective when fullscreen is enabled (-f or via config).
        /// Can also be set in config.toml [defaults] or per-profile.
        #[arg(long)]
        all_monitors: bool,

        /// Disable audio output and input.
        #[arg(long)]
        no_audio: bool,

        /// Disable microphone input only.
        #[arg(long)]
        no_microphone: bool,

        /// Disable clipboard synchronization (Story 5.1).
        #[arg(long)]
        no_clipboard: bool,
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

    // Initialize logging with priority: RUST_LOG > --verbose > default (warn)
    let filter = if std::env::var("RUST_LOG").is_ok() {
        // RUST_LOG takes full priority for fine-grained control
        EnvFilter::from_default_env()
    } else if cli.verbose {
        // --verbose enables debug level for all yard crates
        EnvFilter::new(
            "warn,yard_client=debug,yard_protocol=debug,yard_core=debug,\
             yard_wayland=debug,yard_video=debug,yard_audio=debug",
        )
    } else {
        // Default: only errors and warnings (AC 1)
        EnvFilter::new("warn")
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true) // Show crate/module in logs
        .init();

    match run(cli) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            error!("{:#}", e);
            ExitCode::from(exit_codes::CONNECTION_ERROR)
        }
    }
}

fn run(cli: Cli) -> Result<u8> {
    // Load configuration file (defaults if missing)
    let app_config = load_config();

    match cli.command {
        Some(Commands::Connect {
            host,
            port,
            username,
            domain,
            fullscreen,
            no_fullscreen,
            all_monitors,
            no_audio,
            no_microphone,
            no_clipboard,
        }) => {
            // User feedback (always visible, not affected by log level)
            eprintln!("YARD v{}", env!("CARGO_PKG_VERSION"));

            // Story 6.6: Check if 'host' is a profile name
            // If it matches a profile, use profile settings (with CLI overrides)
            // If not, treat it as a hostname (existing behavior)
            let profile = if app_config.has_profile(&host) {
                let p = app_config
                    .get_profile(&host)
                    .expect("profile exists after has_profile check");
                info!("Using profile '{}' to connect to '{}'", host, p.host);
                Some(p)
            } else {
                None
            };

            // Determine effective host: from profile or CLI argument
            let effective_host = profile
                .as_ref()
                .map(|p| p.host.clone())
                .unwrap_or_else(|| host.clone());

            // Apply settings with precedence: CLI > profile > global defaults
            // Port: CLI --port > profile.port > defaults.port
            let effective_port = port
                .or_else(|| profile.as_ref().and_then(|p| p.port))
                .unwrap_or_else(|| app_config.defaults.effective_port());

            // Username: CLI -u > profile.username > defaults.username
            let effective_username = username
                .or_else(|| profile.as_ref().and_then(|p| p.username.clone()))
                .or_else(|| app_config.defaults.username.clone());

            // Domain: CLI -d > profile.domain > defaults.domain
            let effective_domain = domain
                .or_else(|| profile.as_ref().and_then(|p| p.domain.clone()))
                .or_else(|| app_config.defaults.domain.clone());

            // Fullscreen: CLI --no-fullscreen negates; CLI -f enables; otherwise profile > defaults
            // Story 6.7: Added defaults.fullscreen support
            let effective_fullscreen = if no_fullscreen {
                false // CLI --no-fullscreen wins
            } else if fullscreen {
                true // CLI -f wins
            } else {
                profile
                    .as_ref()
                    .and_then(|p| p.fullscreen)
                    .or(app_config.defaults.fullscreen)
                    .unwrap_or(false)
            };

            // All monitors: CLI --all-monitors OR profile.all_monitors OR defaults.all_monitors
            // Story 6.7: Added defaults.all_monitors support
            let effective_all_monitors = all_monitors
                || profile
                    .as_ref()
                    .and_then(|p| p.all_monitors)
                    .or(app_config.defaults.all_monitors)
                    .unwrap_or(false);

            // Audio: CLI --no-audio negates; otherwise profile.audio > config.audio.enabled
            let audio_enabled = if no_audio {
                false
            } else {
                profile
                    .as_ref()
                    .and_then(|p| p.audio)
                    .unwrap_or(app_config.audio.enabled)
            };

            // Microphone: CLI --no-microphone negates; otherwise profile.microphone > config.audio.microphone
            let microphone_enabled = if no_microphone {
                false
            } else {
                profile
                    .as_ref()
                    .and_then(|p| p.microphone)
                    .unwrap_or(app_config.audio.microphone)
            };

            // Clipboard: CLI --no-clipboard negates; otherwise profile.clipboard > config.clipboard.enabled
            let clipboard_enabled = if no_clipboard {
                false
            } else {
                profile
                    .as_ref()
                    .and_then(|p| p.clipboard)
                    .unwrap_or(app_config.clipboard.enabled)
            };

            // Build connection config, parsing domain from username if present
            // Supports: DOMAIN\user, user@domain.com, or plain username
            let mut config = if let Some(ref user) = effective_username {
                ConnectionConfig::from_username(&effective_host, effective_port, user)
            } else {
                ConnectionConfig::new(&effective_host, effective_port)
            };

            // Explicit domain (CLI, profile, or global config) overrides parsed domain from username
            if let Some(dom) = effective_domain {
                config = config.with_domain(dom);
            }

            // Prompt for password if username is provided (secure, no echo)
            // Note: Profiles don't store passwords for security
            if config.username.is_some() {
                let password = prompt_password(&config)?;
                config = config.with_password(password);
            }

            // Apply clipboard setting
            if !clipboard_enabled {
                config = config.without_clipboard();
            }

            run_connection(
                config,
                effective_fullscreen,
                effective_all_monitors,
                audio_enabled,
                microphone_enabled,
            )
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

/// Loads the application configuration from the config file.
///
/// Returns default configuration if the file doesn't exist.
/// Logs a warning and returns defaults if the file exists but is invalid.
fn load_config() -> Config {
    match Config::load() {
        Ok(config) => {
            debug!("Config loaded from {}", Config::config_path().display());
            config
        }
        Err(e) => {
            warn!("Failed to load config: {}. Using defaults.", e);
            Config::default()
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
fn run_connection(
    config: ConnectionConfig,
    start_fullscreen: bool,
    all_monitors: bool,
    audio_enabled: bool,
    microphone_enabled: bool,
) -> Result<u8> {
    // Create structured span with connection context (AC 4)
    let connection_span = info_span!(
        "rdp_connection",
        server = %config.host,
        port = config.port,
        user = config.username.as_deref().unwrap_or("<none>"),
    );
    let _guard = connection_span.enter();

    // User feedback (always visible, not affected by log level)
    eprintln!("Connecting to {}...", config.address());

    if let Some(ref user) = config.username {
        if let Some(ref dom) = config.domain {
            debug!(domain = %dom, username = %user, "Authenticating with domain credentials");
        } else {
            debug!(username = %user, "Authenticating with local credentials");
        }
    }

    // Prepare window config for later use
    let window_config =
        WindowConfig::with_connection_info(&config.host, config.port, config.username.as_deref())
            .with_fullscreen(start_fullscreen);
    debug!(
        ?window_config,
        all_monitors, audio_enabled, "Window config prepared"
    );

    // Initialize audio thread (Story 4.1)
    // Audio is spawned early but runs independently - doesn't block connection
    let mut audio_thread: Option<AudioThread> = if audio_enabled {
        match AudioThread::spawn() {
            Ok(audio) => {
                info!("Audio thread initialized");
                Some(audio)
            }
            Err(e) => {
                // Graceful degradation: continue without audio
                warn!("Audio unavailable: {}. Continuing without audio.", e);
                eprintln!("⚠ Audio unavailable: {}", e);
                None
            }
        }
    } else {
        debug!("Audio disabled by configuration");
        None
    };

    // Get audio sender for network thread (Story 4.2)
    // The sender is cloned so the audio thread retains ownership for shutdown
    let audio_tx = audio_thread.as_ref().map(|a| a.sender());

    // Get capture receiver for AUDIN microphone input (Story 4.3)
    // Only take the receiver if microphone is enabled
    let capture_rx = if microphone_enabled {
        audio_thread
            .as_mut()
            .and_then(|a| a.take_capture_receiver())
    } else {
        debug!("Microphone disabled by configuration");
        None
    };

    // Create channel for receiving messages from network thread
    let (from_network_tx, mut from_network_rx) = mpsc::channel::<FromNetwork>(32);

    // Spawn network thread with audio sender and capture receiver
    let to_network_tx = spawn_network_thread(from_network_tx, audio_tx, capture_rx);

    // Keep audio thread alive for the duration of the connection
    // It will be dropped when this function returns
    let _audio_thread = audio_thread;

    // Send connect command
    to_network_tx
        .blocking_send(ToNetwork::Connect(config))
        .map_err(|_| anyhow::anyhow!("Failed to send connect command"))?;

    // Event loop for handling messages
    // On Linux, this will be replaced with calloop + Wayland window
    // For now, use blocking receive
    let exit_code = run_event_loop(
        &mut from_network_rx,
        &to_network_tx,
        window_config,
        all_monitors,
    )?;

    Ok(exit_code)
}

/// Event loop dispatch timeout (~60fps).
/// This determines how often we poll for events when idle.
#[cfg(target_os = "linux")]
const EVENT_LOOP_TIMEOUT_MS: u64 = 16;

/// Placeholder color (dark gray) shown before real frames arrive.
#[cfg(target_os = "linux")]
const PLACEHOLDER_COLOR: (u8, u8, u8) = (40, 40, 40);

/// Number of event loop dispatch rounds for initial monitor detection (Story 3.4).
/// This allows Wayland to enumerate outputs before we check monitor count.
#[cfg(target_os = "linux")]
const MONITOR_DETECTION_ROUNDS: usize = 5;

/// Timeout per dispatch round during monitor detection (milliseconds).
#[cfg(target_os = "linux")]
const MONITOR_DETECTION_TIMEOUT_MS: u64 = 50;

/// Runs the main event loop.
///
/// On Linux with Wayland, this creates a window and uses calloop.
/// On other platforms, this uses a simple blocking receive loop.
#[cfg(target_os = "linux")]
fn run_event_loop(
    from_network_rx: &mut mpsc::Receiver<FromNetwork>,
    to_network_tx: &mpsc::Sender<ToNetwork>,
    window_config: WindowConfig,
    all_monitors: bool,
) -> Result<u8> {
    use yard_wayland::{WaylandWindow, WindowEvent};

    // Wait for initial connection before creating window
    let mut desktop_size: Option<DesktopSize> = None;
    while desktop_size.is_none() {
        match from_network_rx.blocking_recv() {
            Some(FromNetwork::Connecting) => {
                info!("Establishing connection...");
            }
            Some(FromNetwork::Connected(size)) => {
                info!(
                    "Connected successfully! Desktop: {}x{}",
                    size.width, size.height
                );
                desktop_size = Some(size);
            }
            Some(FromNetwork::CertificateVerify { server, cert_info }) => {
                let accepted = prompt_certificate_verification(&server, &cert_info);
                if to_network_tx
                    .blocking_send(ToNetwork::CertificateDecision(accepted))
                    .is_err()
                {
                    error!("Failed to send certificate decision");
                    return Ok(exit_codes::CONNECTION_ERROR);
                }
            }
            Some(FromNetwork::Disconnected) => {
                info!("Disconnected before window created.");
                return Ok(exit_codes::SUCCESS);
            }
            Some(FromNetwork::Error(err)) => {
                error!("{}", err);
                return Ok(map_error_to_exit_code(&err));
            }
            Some(FromNetwork::Frame(_)) => {
                // Frames received before window created are discarded
            }
            Some(FromNetwork::MonitorLayoutAccepted { desktop_size: size }) => {
                // Multi-monitor layout accepted - update desktop size
                info!(
                    "Multi-monitor layout accepted: {}x{}",
                    size.width, size.height
                );
                desktop_size = Some(size);
            }
            Some(FromNetwork::MultiMonitorNotSupported) => {
                // Server doesn't support multi-monitor, continue with single monitor
                warn!("Server does not support multi-monitor mode");
            }
            Some(FromNetwork::ClipboardTextAvailable { .. })
            | Some(FromNetwork::ClipboardText { .. })
            | Some(FromNetwork::ClipboardRequestFailed)
            | Some(FromNetwork::ClipboardFilesAvailable { .. })
            | Some(FromNetwork::ClipboardFilesReceived { .. })
            | Some(FromNetwork::ClipboardFileSizeReceived { .. })
            | Some(FromNetwork::ClipboardFileContentReceived { .. })
            | Some(FromNetwork::ClipboardFileTransferFailed { .. })
            | Some(FromNetwork::LatencyUpdate { .. }) => {
                // Story 5.2/5.4/6.4: Clipboard and latency events during setup phase - ignore
            }
            None => {
                error!("Network thread terminated unexpectedly");
                return Ok(exit_codes::CONNECTION_ERROR);
            }
        }
    }

    // Use server's desktop size for window dimensions
    let desktop_size = desktop_size.expect("desktop_size must be set after loop");
    let window_config = window_config.with_size(
        u32::from(desktop_size.width),
        u32::from(desktop_size.height),
    );

    // Extract server name for overlay BEFORE moving window_config
    // Story 6.2: Parse server name from title (format: "YARD - user@server:port")
    let server_name = window_config
        .title
        .split('@')
        .nth(1)
        .unwrap_or("Unknown")
        .to_string();

    // Create Wayland window
    info!("Creating Wayland window...");
    let (mut event_loop, mut window, event_rx) = match WaylandWindow::new(window_config) {
        Ok(w) => w,
        Err(e) => {
            error!("Failed to create Wayland window: {}", e);
            // Fallback to headless mode
            return run_headless_loop(from_network_rx, to_network_tx);
        }
    };

    // Set remote resolution for coordinate mapping
    window.set_remote_resolution(
        u32::from(desktop_size.width),
        u32::from(desktop_size.height),
    );

    // Story 3.4: Multi-monitor fullscreen mode
    // Need to dispatch events first to detect monitors
    // Dispatch a few rounds to let Wayland enumerate outputs
    for _ in 0..MONITOR_DETECTION_ROUNDS {
        let timeout = std::time::Duration::from_millis(MONITOR_DETECTION_TIMEOUT_MS);
        if event_loop.dispatch(timeout, &mut window).is_err() {
            warn!("Event loop dispatch failed during monitor detection");
        }
    }

    // Log detected monitors
    window.log_monitors();

    // Get queue handle for multi-monitor surface creation
    // We need to access the event loop's handle, which isn't directly exposed
    // So we use a workaround: dispatch with a callback that creates surfaces
    let multi_monitor_mode = if all_monitors && window.monitor_count() > 0 {
        info!(
            "Multi-monitor mode requested, {} monitor(s) detected",
            window.monitor_count()
        );

        // We cannot easily get QueueHandle here without restructuring WaylandWindow
        // For now, log the intent - actual surface creation happens in WaylandWindow
        // TODO: Implement proper multi-surface creation flow (requires QueueHandle access)
        // The create_surfaces_for_all_monitors method needs QueueHandle which we don't have here
        warn!("Multi-monitor fullscreen not yet fully implemented - using single window mode");
        warn!("To enable, refactor WaylandWindow to create multi-surfaces during construction");
        false
    } else if all_monitors {
        warn!("--all-monitors specified but no monitors detected yet, using single window mode");
        false
    } else {
        false
    };

    // Draw initial placeholder
    let (r, g, b) = PLACEHOLDER_COLOR;
    if multi_monitor_mode {
        window.draw_solid_all(r, g, b);
    } else {
        window.draw_solid(r, g, b);
    }
    info!(
        "Window created: {}x{} (remote: {}x{}, multi_monitor={})",
        window.dimensions().0,
        window.dimensions().1,
        desktop_size.width,
        desktop_size.height,
        multi_monitor_mode
    );

    // Track frame statistics
    let mut frame_count: u64 = 0;
    let start_time = std::time::Instant::now();

    // Story 6.2: Track session state for overlay display
    let session_start = std::time::Instant::now();

    // Set initial overlay content (connected state)
    window.update_overlay_content(build_connected_overlay(&server_name, session_start, None));

    // Story 5.4: File transfer manager for clipboard file downloads
    let mut file_transfer_manager: Option<FileTransferManager> = None;

    // Story 6.4: Track latest RTT for overlay display
    let mut latest_rtt_ms: Option<u32> = None;
    // Track last overlay update time for duration refresh
    let mut last_overlay_update = std::time::Instant::now();

    /// Story 6.4: Helper to build connected state overlay content.
    /// Reduces duplication across multiple update sites.
    fn build_connected_overlay(
        server_name: &str,
        session_start: std::time::Instant,
        rtt_ms: Option<u32>,
    ) -> OverlayContent {
        OverlayContent {
            status: ConnectionStatus::Connected,
            server_name: Some(server_name.to_string()),
            session_duration: Some(session_start.elapsed()),
            rtt_ms,
            reconnect_attempt: None,
            disconnect_reason: None,
        }
    }

    // Main event loop
    // NOTE: Network channel is polled manually via try_recv. Future optimization could
    // integrate it as a calloop source for true event-driven dispatch.
    // TODO: Add frame pacing via Wayland frame callbacks to avoid rendering faster
    // than the compositor can display (reduces CPU usage and potential tearing).
    loop {
        // Poll network messages (non-blocking) - process all available
        // TODO: Consider rate limiting if frames arrive faster than display refresh
        loop {
            match from_network_rx.try_recv() {
                Ok(FromNetwork::Disconnected) => {
                    info!("Disconnected after {} frames.", frame_count);
                    // Story 6.2: Update overlay to show disconnected state
                    window.update_overlay_content(OverlayContent {
                        status: ConnectionStatus::Disconnected,
                        server_name: Some(server_name.clone()),
                        session_duration: Some(session_start.elapsed()),
                        disconnect_reason: Some("Connection closed".to_string()),
                        ..Default::default()
                    });
                    return Ok(exit_codes::SUCCESS);
                }
                Ok(FromNetwork::Error(err)) => {
                    error!("{}", err);
                    // Story 6.2: Update overlay to show error state
                    window.update_overlay_content(OverlayContent {
                        status: ConnectionStatus::Disconnected,
                        server_name: Some(server_name.clone()),
                        session_duration: Some(session_start.elapsed()),
                        disconnect_reason: Some(err.to_string()),
                        ..Default::default()
                    });
                    return Ok(map_error_to_exit_code(&err));
                }
                Ok(FromNetwork::CertificateVerify { server, cert_info }) => {
                    let accepted = prompt_certificate_verification(&server, &cert_info);
                    if to_network_tx
                        .blocking_send(ToNetwork::CertificateDecision(accepted))
                        .is_err()
                    {
                        error!("Failed to send certificate decision");
                        return Ok(exit_codes::CONNECTION_ERROR);
                    }
                }
                Ok(FromNetwork::Frame(frame)) => {
                    // Render frame to window
                    if frame_count == 0 {
                        let elapsed = start_time.elapsed();
                        info!("First frame received in {:?}", elapsed);
                    }

                    // Use draw_frame_at_with_stride for partial updates with position
                    window.draw_frame_at_with_stride(
                        &frame.data,
                        frame.width,
                        frame.height,
                        frame.x,
                        frame.y,
                        frame.stride,
                    );
                    frame_count += 1;

                    if frame_count.is_multiple_of(300) {
                        debug!("Rendered {} frames", frame_count);
                    }
                }
                Ok(FromNetwork::Connecting) => {
                    // Already connected, ignore
                }
                Ok(FromNetwork::Connected(_)) => {
                    // Already handled before window creation
                }
                Ok(FromNetwork::MonitorLayoutAccepted { .. }) => {
                    // Monitor layout updates during session - could resize window
                    // For now, log and continue
                    debug!("Monitor layout updated during session");
                }
                Ok(FromNetwork::MultiMonitorNotSupported) => {
                    // Already warned during connection
                }
                // Story 6.4: Handle latency updates from network thread
                Ok(FromNetwork::LatencyUpdate { rtt_ms }) => {
                    latest_rtt_ms = Some(rtt_ms);
                    // Update overlay with new RTT if visible
                    if window.is_overlay_visible() {
                        window.update_overlay_content(build_connected_overlay(
                            &server_name,
                            session_start,
                            Some(rtt_ms),
                        ));
                    }
                }
                Ok(FromNetwork::ClipboardTextAvailable { formats }) => {
                    // Story 5.2: Server has text on clipboard
                    debug!("Clipboard text available ({} formats)", formats.len());
                }
                Ok(FromNetwork::ClipboardText { text }) => {
                    // Story 5.2: Clipboard text received from server
                    debug!("Clipboard text received: {} chars", text.len());
                    window.set_clipboard_text(text);
                }
                Ok(FromNetwork::ClipboardRequestFailed) => {
                    // Story 5.2: Clipboard request failed
                    warn!("Clipboard request failed - server could not provide data");
                }
                // Story 5.4: File clipboard messages
                Ok(FromNetwork::ClipboardFilesAvailable { formats }) => {
                    debug!("Clipboard files available ({} formats)", formats.len());
                }
                Ok(FromNetwork::ClipboardFilesReceived { files }) => {
                    debug!("Clipboard files received: {} files", files.len());
                    for file in &files {
                        debug!(
                            "  - {} ({} bytes, dir={})",
                            file.name,
                            file.size.unwrap_or(0),
                            file.is_directory
                        );
                    }

                    // Create file transfer manager and start downloads
                    match FileTransferManager::with_default_staging_dir() {
                        Ok(mut manager) => {
                            // Convert FileInfo to the tuple format expected by add_files
                            let file_tuples: Vec<(String, Option<u64>, bool)> = files
                                .iter()
                                .map(|f| (f.name.clone(), f.size, f.is_directory))
                                .collect();
                            manager.add_files(&file_tuples);

                            // Start first file transfer
                            if let Some((_, file_index)) = manager.start_next_transfer() {
                                if to_network_tx
                                    .blocking_send(ToNetwork::RequestFileSize { file_index })
                                    .is_err()
                                {
                                    error!("Failed to send file size request");
                                }
                            }

                            file_transfer_manager = Some(manager);
                        }
                        Err(e) => {
                            error!("Failed to create file transfer manager: {}", e);
                        }
                    }
                }
                Ok(FromNetwork::ClipboardFileSizeReceived { stream_id, size }) => {
                    debug!("File size received: stream_id={}, size={}", stream_id, size);

                    if let Some(ref mut manager) = file_transfer_manager {
                        match manager.handle_size_response(stream_id, size) {
                            Ok(Some((_, file_index, offset, length))) => {
                                // Request file content
                                if to_network_tx
                                    .blocking_send(ToNetwork::RequestFileContent {
                                        file_index,
                                        offset,
                                        length: length,
                                    })
                                    .is_err()
                                {
                                    error!("Failed to send file content request");
                                }
                            }
                            Ok(None) => {
                                // Empty file or error - check if we should start next file
                                if let Some((_, file_index)) = manager.start_next_transfer() {
                                    if to_network_tx
                                        .blocking_send(ToNetwork::RequestFileSize { file_index })
                                        .is_err()
                                    {
                                        error!("Failed to send file size request");
                                    }
                                } else if manager.pending_count() == 0
                                    && manager.active_count() == 0
                                {
                                    // All transfers complete
                                    let completed = manager.completed_files();
                                    if !completed.is_empty() {
                                        info!(
                                            "All {} files downloaded, setting clipboard",
                                            completed.len()
                                        );
                                        window.set_clipboard_files(completed);
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("Failed to handle size response: {}", e);
                            }
                        }
                    }
                }
                Ok(FromNetwork::ClipboardFileContentReceived { stream_id, data }) => {
                    debug!(
                        "File content received: stream_id={}, {} bytes",
                        stream_id,
                        data.len()
                    );

                    if let Some(ref mut manager) = file_transfer_manager {
                        match manager.handle_content_response(stream_id, &data) {
                            Ok(Some((_, file_index, offset, length))) => {
                                // Request more content
                                if to_network_tx
                                    .blocking_send(ToNetwork::RequestFileContent {
                                        file_index,
                                        offset,
                                        length: length,
                                    })
                                    .is_err()
                                {
                                    error!("Failed to send file content request");
                                }
                            }
                            Ok(None) => {
                                // Current file complete - check if we should start next file
                                if let Some((_, file_index)) = manager.start_next_transfer() {
                                    if to_network_tx
                                        .blocking_send(ToNetwork::RequestFileSize { file_index })
                                        .is_err()
                                    {
                                        error!("Failed to send file size request");
                                    }
                                } else if manager.pending_count() == 0
                                    && manager.active_count() == 0
                                {
                                    // All transfers complete
                                    let completed = manager.completed_files();
                                    if !completed.is_empty() {
                                        info!(
                                            "All {} files downloaded, setting clipboard",
                                            completed.len()
                                        );
                                        window.set_clipboard_files(completed);
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("Failed to handle content response: {}", e);
                            }
                        }
                    }
                }
                Ok(FromNetwork::ClipboardFileTransferFailed { stream_id }) => {
                    warn!("File transfer failed: stream_id={}", stream_id);

                    if let Some(ref mut manager) = file_transfer_manager {
                        manager.handle_failure(stream_id);

                        // Try to start next file transfer
                        if let Some((_, file_index)) = manager.start_next_transfer() {
                            if to_network_tx
                                .blocking_send(ToNetwork::RequestFileSize { file_index })
                                .is_err()
                            {
                                error!("Failed to send file size request");
                            }
                        }
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    error!("Network thread terminated");
                    return Ok(exit_codes::CONNECTION_ERROR);
                }
            }
        }

        // Dispatch Wayland events with timeout
        let timeout = std::time::Duration::from_millis(EVENT_LOOP_TIMEOUT_MS);
        if let Err(e) = event_loop.dispatch(timeout, &mut window) {
            error!("Event loop error: {}", e);
            return Ok(exit_codes::CONNECTION_ERROR);
        }

        // Story 6.1: Update overlay hide timer
        // This must be called periodically to check if the hide delay has expired
        window.update_overlay_timer();

        // Story 6.4: Refresh overlay duration every second while visible
        // This ensures the session duration updates in real-time (AC1)
        if window.is_overlay_visible() && last_overlay_update.elapsed().as_secs() >= 1 {
            window.update_overlay_content(build_connected_overlay(
                &server_name,
                session_start,
                latest_rtt_ms,
            ));
            last_overlay_update = std::time::Instant::now();
        }

        // Check window events (includes close request from WindowHandler)
        while let Ok(event) = event_rx.try_recv() {
            match event {
                WindowEvent::CloseRequested => {
                    info!("Window close requested after {} frames", frame_count);
                    let _ = to_network_tx.blocking_send(ToNetwork::Disconnect);
                    return Ok(exit_codes::SUCCESS);
                }
                WindowEvent::Resized { width, height } => {
                    debug!("Window resized to {}x{}", width, height);
                    // Only draw solid placeholder if no frames received yet
                    if frame_count == 0 {
                        window.draw_solid(r, g, b);
                    }
                }
                WindowEvent::RedrawRequested => {
                    // Only draw solid placeholder if no frames received yet
                    // Once frames start arriving, the compositor will handle
                    // redraw via damage
                    if frame_count == 0 {
                        window.draw_solid(r, g, b);
                    }
                }
                WindowEvent::FullscreenChanged { is_fullscreen } => {
                    info!(
                        "Fullscreen mode: {}",
                        if is_fullscreen { "entered" } else { "exited" }
                    );
                }
                WindowEvent::KeyboardShortcut(shortcut) => {
                    use yard_wayland::KeyboardShortcut;
                    match shortcut {
                        KeyboardShortcut::ToggleFullscreen => {
                            // Story 3.4 Task 6: Use multi-monitor toggle if in multi-monitor mode
                            if window.is_multi_monitor_mode() {
                                debug!("Toggle fullscreen shortcut - multi-monitor mode");
                                window.toggle_all_fullscreen();
                            } else {
                                debug!("Toggle fullscreen shortcut - single window mode");
                                window.toggle_fullscreen();
                            }
                        }
                        KeyboardShortcut::Disconnect => {
                            info!("Disconnect shortcut received (Ctrl+Alt+End)");
                            // Request graceful disconnect - exit the event loop
                            // The network thread will be notified when we drop the sender
                            return Ok(exit_codes::SUCCESS);
                        }
                    }
                }
                WindowEvent::KeyPressed { scancode } => {
                    // Send key press to remote server
                    if to_network_tx
                        .blocking_send(ToNetwork::KeyboardInput {
                            scancode,
                            pressed: true,
                        })
                        .is_err()
                    {
                        error!("Failed to send key press to network thread");
                    }
                }
                WindowEvent::KeyReleased { scancode } => {
                    // Send key release to remote server
                    if to_network_tx
                        .blocking_send(ToNetwork::KeyboardInput {
                            scancode,
                            pressed: false,
                        })
                        .is_err()
                    {
                        error!("Failed to send key release to network thread");
                    }
                }
                WindowEvent::UnicodeKeyPressed { character } => {
                    // Story 2.9: Send Unicode character for international keyboard support
                    if to_network_tx
                        .blocking_send(ToNetwork::UnicodeInput {
                            character,
                            pressed: true,
                        })
                        .is_err()
                    {
                        error!("Failed to send Unicode key press to network thread");
                    }
                }
                WindowEvent::UnicodeKeyReleased { character } => {
                    // Story 2.9: Send Unicode character release
                    if to_network_tx
                        .blocking_send(ToNetwork::UnicodeInput {
                            character,
                            pressed: false,
                        })
                        .is_err()
                    {
                        error!("Failed to send Unicode key release to network thread");
                    }
                }
                WindowEvent::MouseMove { x, y } => {
                    // Map window coordinates to remote desktop coordinates
                    let (remote_x, remote_y) =
                        map_to_remote_coords(x, y, window.dimensions(), window.remote_resolution());
                    if to_network_tx
                        .blocking_send(ToNetwork::MouseMove {
                            x: remote_x,
                            y: remote_y,
                        })
                        .is_err()
                    {
                        error!("Failed to send mouse move to network thread");
                    }
                }
                WindowEvent::MouseButton {
                    button,
                    pressed,
                    x,
                    y,
                } => {
                    // Map button code to MouseButton enum
                    // Linux evdev button codes: BTN_LEFT=0x110, BTN_RIGHT=0x111, BTN_MIDDLE=0x112
                    let rdp_button = match button {
                        0x110 => Some(MouseButton::Left),
                        0x111 => Some(MouseButton::Right),
                        0x112 => Some(MouseButton::Middle),
                        _ => {
                            debug!("Unknown mouse button: 0x{:x}", button);
                            None
                        }
                    };
                    if let Some(btn) = rdp_button {
                        let (remote_x, remote_y) = map_to_remote_coords(
                            x,
                            y,
                            window.dimensions(),
                            window.remote_resolution(),
                        );
                        if to_network_tx
                            .blocking_send(ToNetwork::MouseButton {
                                button: btn,
                                pressed,
                                x: remote_x,
                                y: remote_y,
                            })
                            .is_err()
                        {
                            error!("Failed to send mouse button to network thread");
                        }
                    }
                }
                WindowEvent::MouseAxis {
                    horizontal,
                    value,
                    x,
                    y,
                } => {
                    // Convert scroll value to RDP wheel delta
                    // RDP uses 120 units per wheel notch (Windows standard WHEEL_DELTA)
                    //
                    // Wayland scroll values:
                    // - Discrete (mouse wheel): small integers like -1, 0, +1 per notch
                    // - Continuous (touchpad): larger values representing pixel distance
                    //
                    // Threshold of 10.0 distinguishes these cases:
                    // - Mouse wheels rarely exceed ±3 per event
                    // - Touchpad scrolls are typically 15-100+ pixels per event
                    let delta = if value.abs() < 10.0 {
                        // Discrete scroll: multiply by RDP's WHEEL_DELTA (120)
                        (value * 120.0) as i16
                    } else {
                        // Continuous scroll: scale down for usable scroll speed
                        // Factor of 12 gives roughly 1 notch per 10 pixels
                        (value * 12.0) as i16
                    };

                    // Only send if there's actual scroll
                    if delta != 0 {
                        let (remote_x, remote_y) = map_to_remote_coords(
                            x,
                            y,
                            window.dimensions(),
                            window.remote_resolution(),
                        );
                        if to_network_tx
                            .blocking_send(ToNetwork::MouseWheel {
                                horizontal,
                                delta,
                                x: remote_x,
                                y: remote_y,
                            })
                            .is_err()
                        {
                            error!("Failed to send mouse wheel to network thread");
                        }
                    }
                }
                // Story 3.6: Handle monitor hot-plug events
                WindowEvent::MonitorConnected { monitor } => {
                    info!(
                        "Monitor connected: {} ({}x{} at {}, {})",
                        monitor.name, monitor.width, monitor.height, monitor.x, monitor.y
                    );
                }
                WindowEvent::MonitorDisconnected { monitor_id } => {
                    info!("Monitor disconnected: id={}", monitor_id);
                }
                WindowEvent::MonitorLayoutChanged { monitors } => {
                    // Story 3.6: Notify server of monitor layout change via DISPLAYCONTROL
                    info!("Monitor layout changed: {} monitor(s)", monitors.len());
                    // Convert MonitorInfo to RdpMonitorLayout
                    let rdp_monitors: Vec<yard_protocol::RdpMonitorLayout> = monitors
                        .iter()
                        .map(|m| yard_protocol::RdpMonitorLayout {
                            id: m.id,
                            width: m.width,
                            height: m.height,
                            x: m.x,
                            y: m.y,
                            is_primary: m.x == 0 && m.y == 0,
                        })
                        .collect();
                    if to_network_tx
                        .blocking_send(ToNetwork::UpdateMonitorLayout {
                            monitors: rdp_monitors,
                        })
                        .is_err()
                    {
                        error!("Failed to send monitor layout update to network thread");
                    }
                }
                // Story 3.7: Handle monitor resolution change events
                WindowEvent::MonitorResolutionChanged {
                    monitor_id,
                    old_width,
                    old_height,
                    new_width,
                    new_height,
                } => {
                    // Log the resolution change
                    info!(
                        "Monitor {} resolution changed: {}x{} -> {}x{}",
                        monitor_id, old_width, old_height, new_width, new_height
                    );
                    // Note: MonitorLayoutChanged event is emitted separately and will
                    // trigger DISPLAYCONTROL notification to server
                }
                // Story 5.3: Handle local clipboard changes
                WindowEvent::LocalClipboardChanged { text } => {
                    debug!("Local clipboard changed: {} chars", text.len());
                    if to_network_tx
                        .blocking_send(ToNetwork::LocalClipboardText { text })
                        .is_err()
                    {
                        error!("Failed to send local clipboard text to network thread");
                    }
                }
                // Story 5.5: Handle local file clipboard changes
                WindowEvent::LocalClipboardFilesChanged { files } => {
                    debug!("Local clipboard files changed: {} files", files.len());
                    if to_network_tx
                        .blocking_send(ToNetwork::LocalClipboardFiles { files })
                        .is_err()
                    {
                        error!("Failed to send local clipboard files to network thread");
                    }
                }
                // Story 6.1/6.2: Handle overlay visibility changes
                WindowEvent::OverlayVisibilityChanged {
                    visible,
                    monitor_id,
                } => {
                    debug!(
                        "Overlay visibility changed: visible={}, monitor={:?}",
                        visible, monitor_id
                    );
                    // Story 6.2: Update session duration when overlay becomes visible
                    // Story 6.4: Also include latest RTT measurement
                    if visible {
                        window.update_overlay_content(build_connected_overlay(
                            &server_name,
                            session_start,
                            latest_rtt_ms,
                        ));
                        // Reset timer so duration updates start from this point
                        last_overlay_update = std::time::Instant::now();
                    }
                }
                // Story 6.3: Handle disconnect button click
                WindowEvent::DisconnectRequested => {
                    info!("Disconnect requested via overlay button");

                    // Story 6.3 AC3: Clean up any active file transfers
                    if let Some(ref manager) = file_transfer_manager {
                        if manager.active_count() > 0 || manager.pending_count() > 0 {
                            info!(
                                "Aborting {} active and {} pending file transfers",
                                manager.active_count(),
                                manager.pending_count()
                            );
                        }
                        // Clean up staging directory (removes partial files)
                        if let Err(e) = manager.cleanup_staging_dir() {
                            warn!("Failed to cleanup staging directory: {}", e);
                        }
                    }

                    // Send disconnect to network thread
                    if let Err(e) = to_network_tx.blocking_send(ToNetwork::Disconnect) {
                        error!("Failed to send disconnect: {}", e);
                    }
                    // Exit the event loop (graceful shutdown will happen in main)
                    break;
                }
            }
        }
    }
}

/// Maps window-local coordinates to remote desktop coordinates.
///
/// Scales coordinates from window space to remote desktop space, handling
/// aspect ratio differences between local window and remote desktop.
#[cfg(target_os = "linux")]
fn map_to_remote_coords(
    local_x: f64,
    local_y: f64,
    window_size: (u32, u32),
    remote_size: (u32, u32),
) -> (u16, u16) {
    let (window_w, window_h) = window_size;
    let (remote_w, remote_h) = remote_size;

    // Avoid division by zero
    if window_w == 0 || window_h == 0 {
        return (0, 0);
    }

    // Scale and clamp coordinates
    let x = (local_x / window_w as f64 * remote_w as f64)
        .clamp(0.0, (remote_w.saturating_sub(1)) as f64) as u16;
    let y = (local_y / window_h as f64 * remote_h as f64)
        .clamp(0.0, (remote_h.saturating_sub(1)) as f64) as u16;

    (x, y)
}

/// Runs headless event loop (no window) - used as fallback.
#[cfg(target_os = "linux")]
fn run_headless_loop(
    from_network_rx: &mut mpsc::Receiver<FromNetwork>,
    to_network_tx: &mpsc::Sender<ToNetwork>,
) -> Result<u8> {
    loop {
        match from_network_rx.blocking_recv() {
            Some(FromNetwork::Disconnected) => {
                info!("Disconnected.");
                return Ok(exit_codes::SUCCESS);
            }
            Some(FromNetwork::Error(err)) => {
                error!("{}", err);
                return Ok(map_error_to_exit_code(&err));
            }
            Some(FromNetwork::CertificateVerify { server, cert_info }) => {
                let accepted = prompt_certificate_verification(&server, &cert_info);
                if to_network_tx
                    .blocking_send(ToNetwork::CertificateDecision(accepted))
                    .is_err()
                {
                    error!("Failed to send certificate decision");
                    return Ok(exit_codes::CONNECTION_ERROR);
                }
            }
            Some(_) => {}
            None => {
                error!("Network thread terminated unexpectedly");
                return Ok(exit_codes::CONNECTION_ERROR);
            }
        }
    }
}

/// Non-Linux event loop (simple blocking).
#[cfg(not(target_os = "linux"))]
fn run_event_loop(
    from_network_rx: &mut mpsc::Receiver<FromNetwork>,
    to_network_tx: &mpsc::Sender<ToNetwork>,
    _window_config: WindowConfig,
    _all_monitors: bool,
) -> Result<u8> {
    // On non-Linux, just use blocking loop (no Wayland window)
    let mut _desktop_size: Option<DesktopSize> = None;
    loop {
        match from_network_rx.blocking_recv() {
            Some(FromNetwork::Connecting) => {
                info!("Establishing connection...");
            }
            Some(FromNetwork::Connected(size)) => {
                info!(
                    "Connected successfully! Desktop: {}x{}",
                    size.width, size.height
                );
                info!("Note: Wayland window requires Linux");
                _desktop_size = Some(size);
            }
            Some(FromNetwork::Disconnected) => {
                info!("Disconnected.");
                return Ok(exit_codes::SUCCESS);
            }
            Some(FromNetwork::CertificateVerify { server, cert_info }) => {
                let accepted = prompt_certificate_verification(&server, &cert_info);
                if to_network_tx
                    .blocking_send(ToNetwork::CertificateDecision(accepted))
                    .is_err()
                {
                    error!("Failed to send certificate decision");
                    return Ok(exit_codes::CONNECTION_ERROR);
                }
            }
            Some(FromNetwork::Error(err)) => {
                error!("{}", err);
                return Ok(map_error_to_exit_code(&err));
            }
            Some(FromNetwork::Frame(_)) => {
                // Frames discarded on non-Linux (no window)
            }
            Some(FromNetwork::MonitorLayoutAccepted { desktop_size: size }) => {
                // Multi-monitor layout accepted - update desktop size
                info!(
                    "Multi-monitor layout accepted: {}x{}",
                    size.width, size.height
                );
                _desktop_size = Some(size);
            }
            Some(FromNetwork::MultiMonitorNotSupported) => {
                // Server doesn't support multi-monitor, continue with single monitor
                warn!("Server does not support multi-monitor mode");
            }
            // Story 6.4: Latency update (no UI on non-Linux)
            Some(FromNetwork::LatencyUpdate { rtt_ms }) => {
                debug!("RTT update: {}ms", rtt_ms);
            }
            Some(FromNetwork::ClipboardTextAvailable { formats }) => {
                // Story 5.2: Server has text on clipboard
                debug!("Clipboard text available ({} formats)", formats.len());
            }
            Some(FromNetwork::ClipboardText { text }) => {
                // Story 5.2: Clipboard text received from server
                debug!("Clipboard text received: {} chars", text.len());
                // On non-Linux, we just log it (no Wayland clipboard)
            }
            Some(FromNetwork::ClipboardRequestFailed) => {
                // Story 5.2: Clipboard request failed
                warn!("Clipboard request failed - server could not provide data");
            }
            // Story 5.4: File clipboard messages (non-Linux fallback - log only)
            Some(FromNetwork::ClipboardFilesAvailable { formats }) => {
                debug!("Clipboard files available ({} formats)", formats.len());
            }
            Some(FromNetwork::ClipboardFilesReceived { files }) => {
                debug!("Clipboard files received: {} files", files.len());
            }
            Some(FromNetwork::ClipboardFileSizeReceived { stream_id, size }) => {
                debug!("File size received: stream_id={}, size={}", stream_id, size);
            }
            Some(FromNetwork::ClipboardFileContentReceived { stream_id, data }) => {
                debug!(
                    "File content received: stream_id={}, {} bytes",
                    stream_id,
                    data.len()
                );
            }
            Some(FromNetwork::ClipboardFileTransferFailed { stream_id }) => {
                warn!("File transfer failed: stream_id={}", stream_id);
            }
            None => {
                error!("Network thread terminated unexpectedly");
                return Ok(exit_codes::CONNECTION_ERROR);
            }
        }
    }
}

/// Maps a ConnectionError to an exit code.
fn map_error_to_exit_code(err: &yard_protocol::ConnectionError) -> u8 {
    match err {
        yard_protocol::ConnectionError::AuthenticationFailed(_) => exit_codes::AUTH_ERROR,
        yard_protocol::ConnectionError::Protocol(_) => exit_codes::PROTOCOL_ERROR,
        _ => exit_codes::CONNECTION_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_truncate_string_short() {
        assert_eq!(truncate_string("hello", 10), "hello");
    }

    #[test]
    fn test_cli_no_audio_flag() {
        let cli =
            Cli::try_parse_from(["yard", "connect", "server.example.com", "--no-audio"]).unwrap();

        if let Some(Commands::Connect { no_audio, .. }) = cli.command {
            assert!(no_audio);
        } else {
            panic!("Expected Connect command");
        }
    }

    #[test]
    fn test_cli_no_microphone_flag() {
        let cli = Cli::try_parse_from(["yard", "connect", "server.example.com", "--no-microphone"])
            .unwrap();

        if let Some(Commands::Connect { no_microphone, .. }) = cli.command {
            assert!(no_microphone);
        } else {
            panic!("Expected Connect command");
        }
    }

    #[test]
    fn test_cli_audio_flags_default_false() {
        let cli = Cli::try_parse_from(["yard", "connect", "server.example.com"]).unwrap();

        if let Some(Commands::Connect {
            no_audio,
            no_microphone,
            ..
        }) = cli.command
        {
            assert!(!no_audio);
            assert!(!no_microphone);
        } else {
            panic!("Expected Connect command");
        }
    }

    #[test]
    fn test_cli_both_audio_flags() {
        let cli = Cli::try_parse_from([
            "yard",
            "connect",
            "server.example.com",
            "--no-audio",
            "--no-microphone",
        ])
        .unwrap();

        if let Some(Commands::Connect {
            no_audio,
            no_microphone,
            ..
        }) = cli.command
        {
            assert!(no_audio);
            assert!(no_microphone);
        } else {
            panic!("Expected Connect command");
        }
    }

    // Story 5.1: Clipboard flag tests
    #[test]
    fn test_cli_no_clipboard_flag() {
        let cli = Cli::try_parse_from(["yard", "connect", "server.example.com", "--no-clipboard"])
            .unwrap();

        if let Some(Commands::Connect { no_clipboard, .. }) = cli.command {
            assert!(no_clipboard);
        } else {
            panic!("Expected Connect command");
        }
    }

    #[test]
    fn test_cli_clipboard_flag_default_false() {
        let cli = Cli::try_parse_from(["yard", "connect", "server.example.com"]).unwrap();

        if let Some(Commands::Connect { no_clipboard, .. }) = cli.command {
            assert!(!no_clipboard);
        } else {
            panic!("Expected Connect command");
        }
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

    // Story 6.6: Profile connection tests
    mod profile_tests {
        use yard_core::Config;

        /// Helper to create a test config with profiles
        fn config_with_profile(name: &str, host: &str) -> Config {
            let toml = format!(
                r#"
[profiles.{}]
host = "{}"
"#,
                name, host
            );
            Config::parse(&toml).unwrap()
        }

        /// Helper to create a config with a fully-populated profile
        fn config_with_full_profile() -> Config {
            let toml = r#"
[profiles.work]
host = "work.example.com"
port = 13389
username = "john"
domain = "CORP"
fullscreen = true
all_monitors = true
audio = true
microphone = false
clipboard = true
"#;
            Config::parse(toml).unwrap()
        }

        #[test]
        fn test_profile_detection_exists() {
            let config = config_with_profile("work", "work.example.com");
            assert!(config.has_profile("work"));
            assert!(!config.has_profile("nonexistent"));
        }

        #[test]
        fn test_profile_detection_hostname_fallback() {
            // When no profile matches, the argument should be treated as hostname
            let config = Config::default();
            assert!(!config.has_profile("server.example.com"));
            assert!(!config.has_profile("192.168.1.100"));
        }

        #[test]
        fn test_profile_get_host() {
            let config = config_with_profile("myserver", "actual-host.example.com");
            let profile = config.get_profile("myserver").unwrap();
            assert_eq!(profile.host, "actual-host.example.com");
        }

        #[test]
        fn test_profile_all_fields_parsed() {
            let config = config_with_full_profile();
            let profile = config.get_profile("work").unwrap();

            assert_eq!(profile.host, "work.example.com");
            assert_eq!(profile.port, Some(13389));
            assert_eq!(profile.username, Some("john".to_string()));
            assert_eq!(profile.domain, Some("CORP".to_string()));
            assert_eq!(profile.fullscreen, Some(true));
            assert_eq!(profile.all_monitors, Some(true));
            assert_eq!(profile.audio, Some(true));
            assert_eq!(profile.microphone, Some(false));
            assert_eq!(profile.clipboard, Some(true));
        }

        #[test]
        fn test_profile_minimal_only_host() {
            let config = config_with_profile("minimal", "server.test.com");
            let profile = config.get_profile("minimal").unwrap();

            assert_eq!(profile.host, "server.test.com");
            assert!(profile.port.is_none());
            assert!(profile.username.is_none());
            assert!(profile.domain.is_none());
            assert!(profile.fullscreen.is_none());
            assert!(profile.all_monitors.is_none());
            assert!(profile.audio.is_none());
            assert!(profile.microphone.is_none());
            assert!(profile.clipboard.is_none());
        }

        // Test the merging logic (simulating what run() does)
        #[test]
        fn test_merge_cli_overrides_profile_port() {
            let config = config_with_full_profile();
            let profile = config.get_profile("work").unwrap();

            // CLI provides port, should override profile
            let cli_port: Option<u16> = Some(3390);
            let effective_port = cli_port
                .or(profile.port)
                .unwrap_or(config.defaults.effective_port());

            assert_eq!(effective_port, 3390); // CLI wins
        }

        #[test]
        fn test_merge_profile_provides_port_when_cli_none() {
            let config = config_with_full_profile();
            let profile = config.get_profile("work").unwrap();

            // CLI doesn't provide port, profile should be used
            let cli_port: Option<u16> = None;
            let effective_port = cli_port
                .or(profile.port)
                .unwrap_or(config.defaults.effective_port());

            assert_eq!(effective_port, 13389); // Profile wins
        }

        #[test]
        fn test_merge_default_port_when_neither_provides() {
            let config = config_with_profile("minimal", "server.test.com");
            let profile = config.get_profile("minimal").unwrap();

            let cli_port: Option<u16> = None;
            let effective_port = cli_port
                .or(profile.port)
                .unwrap_or(config.defaults.effective_port());

            assert_eq!(effective_port, 3389); // Default wins
        }

        #[test]
        fn test_merge_cli_overrides_profile_username() {
            let config = config_with_full_profile();
            let profile = config.get_profile("work").unwrap();

            let cli_username: Option<String> = Some("override-user".to_string());
            let effective_username = cli_username
                .or_else(|| profile.username.clone())
                .or_else(|| config.defaults.username.clone());

            assert_eq!(effective_username, Some("override-user".to_string()));
        }

        #[test]
        fn test_merge_fullscreen_cli_flag_additive() {
            let config = config_with_profile("no-fullscreen", "server.test.com");
            let profile = config.get_profile("no-fullscreen").unwrap();

            // CLI --fullscreen is set, profile has no fullscreen
            let cli_fullscreen = true;
            let effective_fullscreen = cli_fullscreen || profile.fullscreen.unwrap_or(false);

            assert!(effective_fullscreen); // CLI flag enables it
        }

        #[test]
        fn test_merge_fullscreen_from_profile() {
            let config = config_with_full_profile();
            let profile = config.get_profile("work").unwrap();

            // CLI --fullscreen is NOT set, but profile has fullscreen=true
            let cli_fullscreen = false;
            let effective_fullscreen = cli_fullscreen || profile.fullscreen.unwrap_or(false);

            assert!(effective_fullscreen); // Profile enables it
        }

        #[test]
        fn test_merge_no_audio_flag_overrides_profile() {
            let config = config_with_full_profile();
            let profile = config.get_profile("work").unwrap();

            // CLI --no-audio is set, profile has audio=true
            let no_audio = true;
            let audio_enabled = if no_audio {
                false
            } else {
                profile.audio.unwrap_or(config.audio.enabled)
            };

            assert!(!audio_enabled); // CLI --no-audio wins
        }

        #[test]
        fn test_merge_audio_from_profile_when_no_flag() {
            let config = config_with_full_profile();
            let profile = config.get_profile("work").unwrap();

            // CLI --no-audio is NOT set, profile has audio=true
            let no_audio = false;
            let audio_enabled = if no_audio {
                false
            } else {
                profile.audio.unwrap_or(config.audio.enabled)
            };

            assert!(audio_enabled); // Profile enables it
        }

        #[test]
        fn test_merge_microphone_disabled_by_profile() {
            let config = config_with_full_profile();
            let profile = config.get_profile("work").unwrap();

            // Profile has microphone=false
            let no_microphone = false;
            let microphone_enabled = if no_microphone {
                false
            } else {
                profile.microphone.unwrap_or(config.audio.microphone)
            };

            assert!(!microphone_enabled); // Profile disables it
        }

        #[test]
        fn test_empty_string_not_profile() {
            // Empty string should never match a profile
            let config = Config::default();
            // Empty profile name can't exist
            assert!(!config.has_profile(""));
        }

        #[test]
        fn test_profile_name_looks_like_hostname() {
            // Profile name that looks like a hostname (using quoted key in TOML)
            // TOML requires quoting keys with dots
            let toml = r#"
[profiles."server.local"]
host = "real-server.example.com"
"#;
            let config = Config::parse(toml).unwrap();
            assert!(config.has_profile("server.local"));

            let profile = config.get_profile("server.local").unwrap();
            assert_eq!(profile.host, "real-server.example.com");
        }

        #[test]
        fn test_merge_no_clipboard_flag_overrides_profile() {
            let config = config_with_full_profile();
            let profile = config.get_profile("work").unwrap();

            // CLI --no-clipboard is set, profile has clipboard=true
            let no_clipboard = true;
            let clipboard_enabled = if no_clipboard {
                false
            } else {
                profile.clipboard.unwrap_or(config.clipboard.enabled)
            };

            assert!(!clipboard_enabled); // CLI --no-clipboard wins
        }

        #[test]
        fn test_merge_clipboard_from_profile_when_no_flag() {
            let config = config_with_full_profile();
            let profile = config.get_profile("work").unwrap();

            // CLI --no-clipboard is NOT set, profile has clipboard=true
            let no_clipboard = false;
            let clipboard_enabled = if no_clipboard {
                false
            } else {
                profile.clipboard.unwrap_or(config.clipboard.enabled)
            };

            assert!(clipboard_enabled); // Profile enables it
        }

        #[test]
        fn test_merge_cli_overrides_profile_domain() {
            let config = config_with_full_profile();
            let profile = config.get_profile("work").unwrap();

            // CLI provides domain, should override profile
            let cli_domain: Option<String> = Some("OVERRIDE-DOMAIN".to_string());
            let effective_domain = cli_domain
                .or_else(|| profile.domain.clone())
                .or_else(|| config.defaults.domain.clone());

            assert_eq!(effective_domain, Some("OVERRIDE-DOMAIN".to_string()));
        }

        #[test]
        fn test_merge_domain_from_profile_when_cli_none() {
            let config = config_with_full_profile();
            let profile = config.get_profile("work").unwrap();

            // CLI doesn't provide domain, profile should be used
            let cli_domain: Option<String> = None;
            let effective_domain = cli_domain
                .or_else(|| profile.domain.clone())
                .or_else(|| config.defaults.domain.clone());

            assert_eq!(effective_domain, Some("CORP".to_string())); // Profile wins
        }

        #[test]
        fn test_merge_all_monitors_from_profile() {
            let config = config_with_full_profile();
            let profile = config.get_profile("work").unwrap();

            // CLI --all-monitors is NOT set, but profile has all_monitors=true
            let cli_all_monitors = false;
            let effective_all_monitors = cli_all_monitors || profile.all_monitors.unwrap_or(false);

            assert!(effective_all_monitors); // Profile enables it
        }

        #[test]
        fn test_merge_all_monitors_cli_flag_additive() {
            let config = config_with_profile("no-monitors", "server.test.com");
            let profile = config.get_profile("no-monitors").unwrap();

            // CLI --all-monitors is set, profile has no all_monitors
            let cli_all_monitors = true;
            let effective_all_monitors = cli_all_monitors || profile.all_monitors.unwrap_or(false);

            assert!(effective_all_monitors); // CLI flag enables it
        }

        #[test]
        fn test_hostname_fallback_preserves_cli_settings() {
            // When host is NOT a profile name, CLI settings should still work
            let config = Config::default();

            // Simulate: yard connect server.example.com -p 3390 -u testuser
            let host = "server.example.com";
            assert!(!config.has_profile(host)); // Not a profile

            // Without profile, CLI values should be used directly
            let cli_port: Option<u16> = Some(3390);
            let cli_username: Option<String> = Some("testuser".to_string());

            let effective_port = cli_port.unwrap_or(config.defaults.effective_port());
            let effective_username = cli_username.or(config.defaults.username.clone());

            assert_eq!(effective_port, 3390);
            assert_eq!(effective_username, Some("testuser".to_string()));
        }

        // Story 6.7: Default connection options tests
        #[test]
        fn test_defaults_fullscreen_applied_when_no_cli_or_profile() {
            // Config has defaults.fullscreen = true, no CLI flag, no profile
            let toml = r#"
[defaults]
fullscreen = true
"#;
            let config = Config::parse(toml).unwrap();

            // Simulate: no profile, no CLI flags
            let profile: Option<&yard_core::ConnectionProfile> = None;
            let cli_fullscreen = false;
            let cli_no_fullscreen = false;

            let effective_fullscreen = if cli_no_fullscreen {
                false
            } else if cli_fullscreen {
                true
            } else {
                profile
                    .and_then(|p| p.fullscreen)
                    .or(config.defaults.fullscreen)
                    .unwrap_or(false)
            };

            assert!(effective_fullscreen); // Defaults.fullscreen = true applied
        }

        #[test]
        fn test_defaults_all_monitors_applied_when_no_cli_or_profile() {
            // Config has defaults.all_monitors = true, no CLI flag, no profile
            let toml = r#"
[defaults]
all_monitors = true
"#;
            let config = Config::parse(toml).unwrap();

            let profile: Option<&yard_core::ConnectionProfile> = None;
            let cli_all_monitors = false;

            let effective_all_monitors = cli_all_monitors
                || profile
                    .and_then(|p| p.all_monitors)
                    .or(config.defaults.all_monitors)
                    .unwrap_or(false);

            assert!(effective_all_monitors); // Defaults.all_monitors = true applied
        }

        #[test]
        fn test_cli_fullscreen_flag_overrides_defaults_false() {
            // Config has defaults.fullscreen = false, CLI -f flag set
            let toml = r#"
[defaults]
fullscreen = false
"#;
            let config = Config::parse(toml).unwrap();

            let profile: Option<&yard_core::ConnectionProfile> = None;
            let cli_fullscreen = true; // CLI -f flag
            let cli_no_fullscreen = false;

            let effective_fullscreen = if cli_no_fullscreen {
                false
            } else if cli_fullscreen {
                true
            } else {
                profile
                    .and_then(|p| p.fullscreen)
                    .or(config.defaults.fullscreen)
                    .unwrap_or(false)
            };

            assert!(effective_fullscreen); // CLI -f wins
        }

        #[test]
        fn test_cli_no_fullscreen_overrides_defaults_true() {
            // Config has defaults.fullscreen = true, CLI --no-fullscreen set
            let toml = r#"
[defaults]
fullscreen = true
"#;
            let config = Config::parse(toml).unwrap();

            let profile: Option<&yard_core::ConnectionProfile> = None;
            let cli_fullscreen = false;
            let cli_no_fullscreen = true; // CLI --no-fullscreen flag

            let effective_fullscreen = if cli_no_fullscreen {
                false
            } else if cli_fullscreen {
                true
            } else {
                profile
                    .and_then(|p| p.fullscreen)
                    .or(config.defaults.fullscreen)
                    .unwrap_or(false)
            };

            assert!(!effective_fullscreen); // CLI --no-fullscreen wins
        }

        #[test]
        fn test_cli_no_fullscreen_overrides_profile_true() {
            // Profile has fullscreen = true, CLI --no-fullscreen should win
            let toml = r#"
[profiles.work]
host = "work.example.com"
fullscreen = true
"#;
            let config = Config::parse(toml).unwrap();
            let profile = config.get_profile("work").unwrap();

            let cli_fullscreen = false;
            let cli_no_fullscreen = true; // CLI --no-fullscreen flag

            let effective_fullscreen = if cli_no_fullscreen {
                false
            } else if cli_fullscreen {
                true
            } else {
                Some(profile)
                    .and_then(|p| p.fullscreen)
                    .or(config.defaults.fullscreen)
                    .unwrap_or(false)
            };

            assert!(!effective_fullscreen); // CLI --no-fullscreen wins over profile
        }

        #[test]
        fn test_no_fullscreen_with_defaults_all_monitors() {
            // Edge case: --no-fullscreen with defaults.all_monitors=true
            // all_monitors should still be computed but fullscreen=false takes precedence
            let toml = r#"
[defaults]
fullscreen = true
all_monitors = true
"#;
            let config = Config::parse(toml).unwrap();

            let profile: Option<&yard_core::ConnectionProfile> = None;
            let cli_fullscreen = false;
            let cli_no_fullscreen = true; // CLI --no-fullscreen
            let cli_all_monitors = false;

            let effective_fullscreen = if cli_no_fullscreen {
                false
            } else if cli_fullscreen {
                true
            } else {
                profile
                    .and_then(|p| p.fullscreen)
                    .or(config.defaults.fullscreen)
                    .unwrap_or(false)
            };

            let effective_all_monitors = cli_all_monitors
                || profile
                    .and_then(|p| p.all_monitors)
                    .or(config.defaults.all_monitors)
                    .unwrap_or(false);

            // --no-fullscreen disables fullscreen
            assert!(!effective_fullscreen);
            // all_monitors is computed as true from defaults, but meaningless without fullscreen
            // This documents the expected behavior: all_monitors is independent of fullscreen flag
            assert!(effective_all_monitors);
        }

        #[test]
        fn test_profile_fullscreen_overrides_defaults() {
            // Config has defaults.fullscreen = false, profile.fullscreen = true
            let toml = r#"
[defaults]
fullscreen = false

[profiles.work]
host = "work.example.com"
fullscreen = true
"#;
            let config = Config::parse(toml).unwrap();
            let profile = config.get_profile("work").unwrap();

            let cli_fullscreen = false;
            let cli_no_fullscreen = false;

            let effective_fullscreen = if cli_no_fullscreen {
                false
            } else if cli_fullscreen {
                true
            } else {
                Some(profile)
                    .and_then(|p| p.fullscreen)
                    .or(config.defaults.fullscreen)
                    .unwrap_or(false)
            };

            assert!(effective_fullscreen); // Profile wins over defaults
        }

        #[test]
        fn test_defaults_overrides_builtin_default() {
            // Config has no defaults section, built-in default should be false
            let config = Config::default();

            let profile: Option<&yard_core::ConnectionProfile> = None;
            let cli_fullscreen = false;
            let cli_no_fullscreen = false;

            let effective_fullscreen = if cli_no_fullscreen {
                false
            } else if cli_fullscreen {
                true
            } else {
                profile
                    .and_then(|p| p.fullscreen)
                    .or(config.defaults.fullscreen)
                    .unwrap_or(false) // Built-in default
            };

            assert!(!effective_fullscreen); // Built-in default = false
        }

        #[test]
        fn test_precedence_chain_cli_profile_defaults_builtin() {
            // Full precedence chain: CLI > profile > defaults > built-in
            let toml = r#"
[defaults]
fullscreen = true
all_monitors = true

[profiles.work]
host = "work.example.com"
fullscreen = false
"#;
            let config = Config::parse(toml).unwrap();
            let profile = config.get_profile("work").unwrap();

            // Profile says fullscreen = false, defaults says true
            // Profile should win
            let cli_fullscreen = false;
            let cli_no_fullscreen = false;

            let effective_fullscreen = if cli_no_fullscreen {
                false
            } else if cli_fullscreen {
                true
            } else {
                Some(profile)
                    .and_then(|p| p.fullscreen)
                    .or(config.defaults.fullscreen)
                    .unwrap_or(false)
            };

            assert!(!effective_fullscreen); // Profile fullscreen=false wins over defaults=true
        }

        #[test]
        fn test_defaults_backward_compat_no_fullscreen_field() {
            // Old config without fullscreen field should still work
            let toml = r#"
[defaults]
port = 3389
"#;
            let config = Config::parse(toml).unwrap();

            // defaults.fullscreen should be None
            assert!(config.defaults.fullscreen.is_none());
            assert!(config.defaults.all_monitors.is_none());

            // Should fall back to built-in default (false)
            let effective_fullscreen = config.defaults.fullscreen.unwrap_or(false);
            assert!(!effective_fullscreen);
        }
    }

    #[cfg(target_os = "linux")]
    mod coordinate_tests {
        use super::*;

        #[test]
        fn test_map_to_remote_coords_same_size() {
            // Same window and remote size - coordinates should be unchanged
            let (x, y) = map_to_remote_coords(100.0, 200.0, (1920, 1080), (1920, 1080));
            assert_eq!(x, 100);
            assert_eq!(y, 200);
        }

        #[test]
        fn test_map_to_remote_coords_scaled() {
            // Window is half the remote size - coordinates should double
            let (x, y) = map_to_remote_coords(100.0, 100.0, (960, 540), (1920, 1080));
            assert_eq!(x, 200);
            assert_eq!(y, 200);
        }

        #[test]
        fn test_map_to_remote_coords_origin() {
            let (x, y) = map_to_remote_coords(0.0, 0.0, (1920, 1080), (1920, 1080));
            assert_eq!(x, 0);
            assert_eq!(y, 0);
        }

        #[test]
        fn test_map_to_remote_coords_clamped_max() {
            // Coordinates at the edge should be clamped to remote_size - 1
            let (x, y) = map_to_remote_coords(1920.0, 1080.0, (1920, 1080), (1920, 1080));
            assert_eq!(x, 1919);
            assert_eq!(y, 1079);
        }

        #[test]
        fn test_map_to_remote_coords_negative() {
            // Negative coordinates should be clamped to 0
            let (x, y) = map_to_remote_coords(-10.0, -20.0, (1920, 1080), (1920, 1080));
            assert_eq!(x, 0);
            assert_eq!(y, 0);
        }

        #[test]
        fn test_map_to_remote_coords_zero_window() {
            // Zero window size should return (0, 0) to avoid division by zero
            let (x, y) = map_to_remote_coords(100.0, 200.0, (0, 0), (1920, 1080));
            assert_eq!(x, 0);
            assert_eq!(y, 0);
        }

        #[test]
        fn test_map_to_remote_coords_different_aspect_ratio() {
            // 4:3 window to 16:9 remote - coordinates scale independently
            let (x, y) = map_to_remote_coords(400.0, 300.0, (800, 600), (1920, 1080));
            // x: 400/800 * 1920 = 960
            // y: 300/600 * 1080 = 540
            assert_eq!(x, 960);
            assert_eq!(y, 540);
        }

        #[test]
        fn test_map_to_remote_coords_subpixel() {
            // Subpixel coordinates should be properly scaled and truncated
            let (x, y) = map_to_remote_coords(0.5, 0.5, (100, 100), (1000, 1000));
            assert_eq!(x, 5);
            assert_eq!(y, 5);
        }
    }
}
