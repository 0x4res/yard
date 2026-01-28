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

use yard_audio::AudioThread;
use yard_wayland::WindowConfig;

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
    Connect {
        /// Target hostname or IP address.
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
        #[arg(short = 'f', long)]
        fullscreen: bool,

        /// Enable multi-monitor fullscreen mode (Story 3.4).
        /// Creates a separate window on each connected monitor.
        #[arg(long)]
        all_monitors: bool,

        /// Disable audio output and input.
        #[arg(long)]
        no_audio: bool,

        /// Disable microphone input only.
        #[arg(long)]
        no_microphone: bool,
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
            all_monitors,
            no_audio,
            no_microphone,
        }) => {
            // User feedback (always visible, not affected by log level)
            eprintln!("YARD v{}", env!("CARGO_PKG_VERSION"));

            // Apply config defaults: CLI args take precedence over config file
            let effective_port = port.unwrap_or_else(|| app_config.defaults.effective_port());
            let effective_username = username.or(app_config.defaults.username.clone());
            let effective_domain = domain.or(app_config.defaults.domain.clone());

            // Build connection config, parsing domain from username if present
            // Supports: DOMAIN\user, user@domain.com, or plain username
            let mut config = if let Some(ref user) = effective_username {
                ConnectionConfig::from_username(&host, effective_port, user)
            } else {
                ConnectionConfig::new(&host, effective_port)
            };

            // Explicit domain (CLI or config) overrides parsed domain from username
            if let Some(dom) = effective_domain {
                config = config.with_domain(dom);
            }

            // Prompt for password if username is provided (secure, no echo)
            if config.username.is_some() {
                let password = prompt_password(&config)?;
                config = config.with_password(password);
            }

            // Determine audio settings: CLI flags override config file
            let audio_enabled = !no_audio && app_config.audio.enabled;
            let microphone_enabled = !no_microphone && app_config.audio.microphone;

            run_connection(
                config,
                fullscreen,
                all_monitors,
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
                    return Ok(exit_codes::SUCCESS);
                }
                Ok(FromNetwork::Error(err)) => {
                    error!("{}", err);
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
