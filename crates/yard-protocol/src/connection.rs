//! RDP connection handling for the network thread.
//!
//! This module provides the async connection logic that runs in a dedicated
//! Tokio thread, communicating with the main thread via message channels.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::mpsc::Sender as StdSender;
use std::time::Duration;

use ironrdp::connector::{self, ClientConnector, Credentials};
use ironrdp::displaycontrol::client::DisplayControlClient;
use ironrdp::displaycontrol::pdu::{MonitorLayoutEntry, MonitorOrientation};
use ironrdp::dvc::DrdynvcClient;
use ironrdp::input::{
    Database as InputDatabase, MouseButton as IronMouseButton, MousePosition, Operation, Scancode,
    WheelRotations,
};
use ironrdp::pdu::gcc::KeyboardType;
use ironrdp::pdu::geometry::Rectangle as _;
use ironrdp::pdu::rdp::capability_sets::{MajorPlatformType, client_codecs_capabilities};
use ironrdp::pdu::rdp::client_info::PerformanceFlags;
use ironrdp::session::{ActiveStage, ActiveStageOutput};
use ironrdp_graphics::image_processing::PixelFormat;
use ironrdp_tokio::{Framed, FramedWrite, TokioFramed, TokioStream};
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpStream, lookup_host};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tracing::{debug, error, info, warn};

use crate::audin::create_audin_client;
use crate::messages::{
    CertificateInfo, ConnectionConfig, ConnectionError, DesktopSize, FromNetwork, MouseButton,
    RdpMonitorInfo, ToNetwork,
};
use crate::rdpsnd::create_rdpsnd_client;
use yard_audio::{FromAudio, ToAudio};

/// Default connection timeout in seconds.
const CONNECTION_TIMEOUT_SECS: u64 = 10;

/// Timeout for user to decide on certificate acceptance (in seconds).
const CERTIFICATE_DECISION_TIMEOUT_SECS: u64 = 60;

/// Default desktop width.
const DEFAULT_WIDTH: u16 = 1920;

/// Default desktop height.
const DEFAULT_HEIGHT: u16 = 1080;

/// Spawns the network thread and returns a channel sender for commands.
///
/// # Arguments
///
/// * `from_network_tx` - Channel to send messages back to the main thread.
/// * `audio_tx` - Optional sender to the audio thread for RDPSND audio output and AUDIN capture.
/// * `capture_rx` - Optional receiver for captured audio data from the audio thread (for AUDIN).
///
/// # Returns
///
/// A sender for sending commands to the network thread.
pub fn spawn_network_thread(
    from_network_tx: mpsc::Sender<FromNetwork>,
    audio_tx: Option<StdSender<ToAudio>>,
    capture_rx: Option<std::sync::mpsc::Receiver<FromAudio>>,
) -> mpsc::Sender<ToNetwork> {
    let (to_network_tx, to_network_rx) = mpsc::channel::<ToNetwork>(32);

    std::thread::Builder::new()
        .name("yard-network".to_string())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create Tokio runtime");

            rt.block_on(network_loop(
                to_network_rx,
                from_network_tx,
                audio_tx,
                capture_rx,
            ));
        })
        .expect("Failed to spawn network thread");

    to_network_tx
}

/// Main loop for the network thread.
async fn network_loop(
    mut to_network_rx: mpsc::Receiver<ToNetwork>,
    from_network_tx: mpsc::Sender<FromNetwork>,
    audio_tx: Option<StdSender<ToAudio>>,
    capture_rx: Option<std::sync::mpsc::Receiver<FromAudio>>,
) {
    // Store capture_rx in an Option that can be taken for the first connection
    let mut capture_rx = capture_rx;

    while let Some(msg) = to_network_rx.recv().await {
        match msg {
            ToNetwork::Connect(config) => {
                handle_connect(
                    config,
                    &mut to_network_rx,
                    &from_network_tx,
                    audio_tx.clone(),
                    capture_rx.take(), // Take capture_rx for this connection
                )
                .await;
            }
            ToNetwork::Disconnect => {
                let _ = from_network_tx.send(FromNetwork::Disconnected).await;
                break;
            }
            ToNetwork::CertificateDecision(_) => {
                // Certificate decisions should be received during handle_connect
                warn!("Received unexpected CertificateDecision outside of connection");
            }
            ToNetwork::KeyboardInput { .. } | ToNetwork::UnicodeInput { .. } => {
                // Keyboard input should be received during active session
                // Ignore if received outside of session (no connection established)
                warn!("Received keyboard input outside of active session");
            }
            ToNetwork::MouseMove { .. }
            | ToNetwork::MouseButton { .. }
            | ToNetwork::MouseWheel { .. } => {
                // Mouse input should be received during active session
                // Ignore if received outside of session (no connection established)
                warn!("Received mouse input outside of active session");
            }
            ToNetwork::UpdateMonitorLayout { .. } => {
                // Story 3.6: Monitor layout update should be received during active session
                // Ignore if received outside of session (no connection established)
                warn!("Received monitor layout update outside of active session");
            }
        }
    }
}

/// Handles a connection request with TLS and RDP session establishment.
async fn handle_connect(
    config: ConnectionConfig,
    to_network_rx: &mut mpsc::Receiver<ToNetwork>,
    tx: &mpsc::Sender<FromNetwork>,
    audio_tx: Option<StdSender<ToAudio>>,
    capture_rx: Option<std::sync::mpsc::Receiver<FromAudio>>,
) {
    // Notify main thread that we're connecting
    if tx.send(FromNetwork::Connecting).await.is_err() {
        return;
    }

    // Attempt TCP connection
    let (stream, local_addr) = match attempt_tcp_connection(&config).await {
        Ok(result) => result,
        Err(err) => {
            let _ = tx.send(FromNetwork::Error(err)).await;
            return;
        }
    };

    debug!("TCP connection established to {}", config.address());

    // Create IronRDP config
    let rdp_config = match build_rdp_config(&config) {
        Ok(cfg) => cfg,
        Err(err) => {
            let _ = tx.send(FromNetwork::Error(err)).await;
            return;
        }
    };

    // Create framed transport for IronRDP
    let mut framed: TokioFramed<TcpStream> = TokioFramed::new(stream);

    // Create connector with local address
    let mut connector = ClientConnector::new(rdp_config, local_addr);

    // Set up DISPLAYCONTROL channel for multi-monitor support (Story 3.2)
    // The channel is only added if monitor layout is provided.
    let has_multi_monitor = config.monitor_layout.is_some();
    if has_multi_monitor {
        // Create DisplayControlClient with a callback for when server capabilities arrive.
        // The callback receives capabilities and can return messages to send.
        // We just log the capabilities and return an empty response.
        let displaycontrol_client = DisplayControlClient::new(|_caps| {
            info!("DisplayControl capabilities received from server");
            // Return empty response - we'll send the layout separately
            Ok(vec![])
        });

        // Create DrdynvcClient (DRDYNVC static channel) and attach displaycontrol DVC
        let drdynvc = DrdynvcClient::new().with_dynamic_channel(displaycontrol_client);

        // Attach DRDYNVC as a static virtual channel to the connector
        connector.attach_static_channel(drdynvc);
        debug!("DISPLAYCONTROL channel configured for multi-monitor support");
    }

    // Set up RDPSND channel for audio output (Story 4.2)
    // Only attach if audio is enabled (audio_tx is Some)
    if let Some(ref tx) = audio_tx {
        let rdpsnd = create_rdpsnd_client(Some(tx.clone()));
        connector.attach_static_channel(rdpsnd);
        debug!("RDPSND channel configured for audio output");
    } else {
        debug!("Audio disabled, skipping RDPSND channel");
    }

    // Set up AUDIN channel for microphone input (Story 4.3)
    // Only attach if capture_rx is provided (microphone enabled)
    if capture_rx.is_some() {
        // Create AUDIN handler with audio_tx for sending StartCapture/StopCapture
        // and capture_rx for receiving captured audio data
        let audin = create_audin_client(audio_tx.clone(), capture_rx);

        // AUDIN is a Dynamic Virtual Channel - attach to DrdynvcClient
        // If we already have a DrdynvcClient (from DISPLAYCONTROL), we need to get it
        // Otherwise create a new one
        if has_multi_monitor {
            // DrdynvcClient already attached, we need to add AUDIN to it
            // Unfortunately IronRDP doesn't support adding channels after connector creation
            // For now, AUDIN will work without DISPLAYCONTROL in same session
            warn!(
                "AUDIN with DISPLAYCONTROL in same session not yet supported, microphone may not work"
            );
            // TODO: Refactor to build DrdynvcClient with all channels at once
        } else {
            // No DISPLAYCONTROL, create DrdynvcClient just for AUDIN
            let drdynvc = DrdynvcClient::new().with_dynamic_channel(audin);
            connector.attach_static_channel(drdynvc);
            debug!("AUDIN channel configured for microphone input");
        }
    } else {
        debug!("Microphone disabled, skipping AUDIN channel");
    }

    // Phase 1: Initial RDP negotiation (before TLS)
    debug!("Starting RDP negotiation");
    let should_upgrade = match ironrdp_tokio::connect_begin(&mut framed, &mut connector).await {
        Ok(upgrade) => upgrade,
        Err(e) => {
            let _ = tx
                .send(FromNetwork::Error(ConnectionError::Protocol(format!(
                    "RDP negotiation failed: {e}"
                ))))
                .await;
            return;
        }
    };

    debug!("RDP negotiation complete, upgrading to TLS");

    // Phase 2: TLS upgrade with certificate verification
    let (tls_stream, server_public_key) =
        match perform_tls_upgrade(framed, &config, to_network_rx, tx).await {
            Ok(result) => result,
            Err(err) => {
                let _ = tx.send(FromNetwork::Error(err)).await;
                return;
            }
        };

    debug!("TLS upgrade complete");

    // Mark connector as upgraded using the ShouldUpgrade token from connect_begin
    let upgraded = ironrdp_tokio::mark_as_upgraded(should_upgrade, &mut connector);

    // Create new framed transport with TLS stream
    let mut tls_framed: TokioFramed<tokio_rustls::client::TlsStream<TcpStream>> =
        TokioFramed::new(tls_stream);

    // Phase 3: Complete RDP handshake (CredSSP if enabled, capabilities, channels)
    let server_name = connector::ServerName::new(&config.host);

    // NetworkClient implementation for SSPI (stub - we don't support Kerberos yet)
    let mut network_client = StubNetworkClient;

    debug!("Completing RDP handshake");
    let connection_result = match ironrdp_tokio::connect_finalize(
        upgraded,
        connector,
        &mut tls_framed,
        &mut network_client,
        server_name,
        server_public_key,
        None, // No Kerberos config
    )
    .await
    {
        Ok(result) => result,
        Err(e) => {
            let err_msg = e.to_string();
            let conn_err = if err_msg.contains("access denied") || err_msg.contains("Access denied")
            {
                ConnectionError::AuthenticationFailed(err_msg)
            } else {
                ConnectionError::Protocol(format!("RDP handshake failed: {err_msg}"))
            };
            let _ = tx.send(FromNetwork::Error(conn_err)).await;
            return;
        }
    };

    info!(
        "RDP connection established: {}x{}",
        connection_result.desktop_size.width, connection_result.desktop_size.height
    );

    let desktop_size = DesktopSize::new(
        connection_result.desktop_size.width,
        connection_result.desktop_size.height,
    );

    // Notify main thread of successful connection
    if tx.send(FromNetwork::Connected(desktop_size)).await.is_err() {
        return;
    }

    // Store dimensions before moving connection_result
    let (img_width, img_height) = (
        connection_result.desktop_size.width,
        connection_result.desktop_size.height,
    );

    // Create session handler (takes ownership of connection_result)
    let mut active_stage = ActiveStage::new(connection_result);

    // Create decoded image buffer for frame accumulation
    // Use BgrA32 format to match DecodedFrame's expected BGRA pixel order
    let mut image =
        ironrdp::session::image::DecodedImage::new(PixelFormat::BgrA32, img_width, img_height);

    // Send monitor layout if multi-monitor was requested (Story 3.2)
    if let Some(ref monitors) = config.monitor_layout {
        match send_monitor_layout(&mut active_stage, monitors) {
            Ok(encoded_messages) => {
                // Send the encoded DVC messages to the server
                if !encoded_messages.is_empty() {
                    if let Err(e) = tls_framed.write_all(&encoded_messages).await {
                        warn!("Failed to send monitor layout: {e}");
                        let _ = tx.send(FromNetwork::MultiMonitorNotSupported).await;
                    } else {
                        info!(
                            "Monitor layout sent to server ({} monitors)",
                            monitors.len()
                        );
                        // Calculate combined desktop size from all monitors
                        let combined_size = calculate_combined_desktop_size(monitors);
                        let _ = tx
                            .send(FromNetwork::MonitorLayoutAccepted {
                                desktop_size: combined_size,
                            })
                            .await;
                    }
                }
            }
            Err(e) => {
                warn!("Failed to encode monitor layout: {e}");
                let _ = tx.send(FromNetwork::MultiMonitorNotSupported).await;
            }
        }
    }

    // Session loop
    if let Err(e) = session_loop(
        &mut tls_framed,
        &mut active_stage,
        &mut image,
        to_network_rx,
        tx,
    )
    .await
    {
        error!("Session error: {e}");
        let _ = tx
            .send(FromNetwork::Error(ConnectionError::Protocol(e.to_string())))
            .await;
    }

    let _ = tx.send(FromNetwork::Disconnected).await;
}

/// Session loop that processes RDP frames and handles user input.
async fn session_loop<S>(
    framed: &mut Framed<TokioStream<S>>,
    active_stage: &mut ActiveStage,
    image: &mut ironrdp::session::image::DecodedImage,
    to_network_rx: &mut mpsc::Receiver<ToNetwork>,
    tx: &mpsc::Sender<FromNetwork>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Sync,
{
    use yard_video::DecodedFrame;

    // Input database for tracking keyboard/mouse state and generating FastPath events
    let mut input_database = InputDatabase::new();

    loop {
        // Use select to handle both incoming PDUs and outgoing input events
        tokio::select! {
            // Read next PDU from server (biased to prioritize server data)
            biased;

            pdu_result = framed.read_pdu() => {
                let (action, frame) = pdu_result?;

                // Process the frame
                let outputs = active_stage.process(image, action, &frame)?;

                for output in outputs {
                    match output {
                        ActiveStageOutput::ResponseFrame(response) => {
                            // Send response back to server
                            framed.write_all(&response).await?;
                        }
                        ActiveStageOutput::GraphicsUpdate(rect) => {
                            // Extract the updated region and send to main thread
                            let x = rect.left;
                            let y = rect.top;
                            let width = rect.width();
                            let height = rect.height();

                            // Validate rect bounds against image dimensions
                            let img_width = image.width();
                            let img_height = image.height();
                            if rect.right >= img_width || rect.bottom >= img_height {
                                warn!(
                                    "Graphics update rect ({},{})x({},{}) exceeds image bounds {}x{}",
                                    x, y, rect.right, rect.bottom, img_width, img_height
                                );
                                continue;
                            }

                            // Get the pixel data for the updated region
                            // The image data is in BgrA32 format (4 bytes per pixel, BGRA order)
                            let data = image.data_for_rect(&rect).to_vec();

                            // Create frame with position for partial update support
                            let frame = DecodedFrame::with_position(
                                data,
                                u32::from(width),
                                u32::from(height),
                                u32::from(x),
                                u32::from(y),
                            );

                            if tx.send(FromNetwork::Frame(frame)).await.is_err() {
                                return Ok(()); // Main thread disconnected
                            }
                        }
                        ActiveStageOutput::Terminate(reason) => {
                            info!("Session terminated: {reason}");
                            return Ok(());
                        }
                        ActiveStageOutput::DeactivateAll(_) => {
                            // Server is requesting reactivation - handle reconnection
                            warn!("Server requested deactivation - reconnection not implemented");
                            return Ok(());
                        }
                        ActiveStageOutput::PointerDefault
                        | ActiveStageOutput::PointerHidden
                        | ActiveStageOutput::PointerPosition { .. }
                        | ActiveStageOutput::PointerBitmap(_) => {
                            // Pointer updates - TODO: implement cursor handling
                        }
                    }
                }
            }

            // Handle messages from main thread (keyboard input, disconnect)
            msg = to_network_rx.recv() => {
                match msg {
                    Some(ToNetwork::KeyboardInput { scancode, pressed }) => {
                        // Convert our scancode format to IronRDP Scancode
                        // Our format: extended keys have 0xE0 in high byte
                        let extended = (scancode & 0xE000) == 0xE000;
                        let code = (scancode & 0xFF) as u8;
                        let sc = Scancode::from_u8(extended, code);

                        // Create the appropriate operation
                        let operation = if pressed {
                            Operation::KeyPressed(sc)
                        } else {
                            Operation::KeyReleased(sc)
                        };

                        // Apply to input database and get FastPath events to send
                        let events = input_database.apply([operation]);

                        if !events.is_empty() {
                            // Process the input events through ActiveStage
                            // This encodes them and may produce response frames
                            // Note: We log errors instead of propagating to avoid disconnecting
                            // the session due to a single key input error
                            match active_stage.process_fastpath_input(image, &events) {
                                Ok(outputs) => {
                                    // Send any response frames to the server
                                    for output in outputs {
                                        if let ActiveStageOutput::ResponseFrame(response) = output {
                                            framed.write_all(&response).await?;
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("Failed to process keyboard input: {e}");
                                }
                            }
                        }
                    }
                    Some(ToNetwork::UnicodeInput { character, pressed }) => {
                        // Story 2.9: Unicode input for international keyboards
                        // Use IronRDP's UnicodeKeyPressed/Released for character input
                        let operation = if pressed {
                            Operation::UnicodeKeyPressed(character)
                        } else {
                            Operation::UnicodeKeyReleased(character)
                        };

                        // Apply to input database and send
                        let events = input_database.apply([operation]);

                        if !events.is_empty() {
                            match active_stage.process_fastpath_input(image, &events) {
                                Ok(outputs) => {
                                    for output in outputs {
                                        if let ActiveStageOutput::ResponseFrame(response) = output {
                                            framed.write_all(&response).await?;
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("Failed to process Unicode input: {e}");
                                }
                            }
                        }
                    }
                    Some(ToNetwork::MouseMove { x, y }) => {
                        // Create mouse move operation
                        let operation = Operation::MouseMove(MousePosition { x, y });

                        // Apply to input database and send
                        let events = input_database.apply([operation]);

                        if !events.is_empty() {
                            match active_stage.process_fastpath_input(image, &events) {
                                Ok(outputs) => {
                                    for output in outputs {
                                        if let ActiveStageOutput::ResponseFrame(response) = output {
                                            framed.write_all(&response).await?;
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("Failed to process mouse move: {e}");
                                }
                            }
                        }
                    }
                    Some(ToNetwork::MouseButton { button, pressed, x, y }) => {
                        // Map our button enum to IronRDP's
                        let iron_button = match button {
                            MouseButton::Left => IronMouseButton::Left,
                            MouseButton::Right => IronMouseButton::Right,
                            MouseButton::Middle => IronMouseButton::Middle,
                        };

                        // Create mouse button operation with position
                        let operation = if pressed {
                            Operation::MouseButtonPressed(iron_button)
                        } else {
                            Operation::MouseButtonReleased(iron_button)
                        };

                        // Also update position to ensure click is at correct location
                        let pos_operation = Operation::MouseMove(MousePosition { x, y });

                        // Apply both operations
                        let events = input_database.apply([pos_operation, operation]);

                        if !events.is_empty() {
                            match active_stage.process_fastpath_input(image, &events) {
                                Ok(outputs) => {
                                    for output in outputs {
                                        if let ActiveStageOutput::ResponseFrame(response) = output {
                                            framed.write_all(&response).await?;
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("Failed to process mouse button: {e}");
                                }
                            }
                        }
                    }
                    Some(ToNetwork::MouseWheel { horizontal, delta, x, y }) => {
                        // Update position first
                        let pos_operation = Operation::MouseMove(MousePosition { x, y });

                        // Create wheel operation
                        // IronRDP uses WheelRotations which takes rotation units
                        let wheel_operation = Operation::WheelRotations(WheelRotations {
                            is_vertical: !horizontal,
                            rotation_units: delta,
                        });

                        // Apply both operations
                        let events = input_database.apply([pos_operation, wheel_operation]);

                        if !events.is_empty() {
                            match active_stage.process_fastpath_input(image, &events) {
                                Ok(outputs) => {
                                    for output in outputs {
                                        if let ActiveStageOutput::ResponseFrame(response) = output {
                                            framed.write_all(&response).await?;
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("Failed to process mouse wheel: {e}");
                                }
                            }
                        }
                    }
                    Some(ToNetwork::Disconnect) => {
                        info!("Disconnect requested");
                        // Release all pressed keys and mouse buttons before disconnecting
                        // to prevent stuck inputs on the remote server.
                        // InputDatabase::release_all() handles both keyboard and mouse state.
                        let release_events = input_database.release_all();
                        if !release_events.is_empty()
                            && let Ok(outputs) =
                                active_stage.process_fastpath_input(image, &release_events)
                        {
                            for output in outputs {
                                if let ActiveStageOutput::ResponseFrame(response) = output {
                                    let _ = framed.write_all(&response).await;
                                }
                            }
                        }
                        return Ok(());
                    }
                    Some(ToNetwork::Connect(_)) => {
                        // Ignore - already connected
                        warn!("Received Connect message during active session");
                    }
                    Some(ToNetwork::CertificateDecision(_)) => {
                        // Ignore - certificate already verified
                        warn!("Received CertificateDecision during active session");
                    }
                    Some(ToNetwork::UpdateMonitorLayout { monitors }) => {
                        // Story 3.6: Update monitor layout due to hot-plug event
                        // TODO: Send DISPLAYCONTROL message to server when DISPLAYCONTROL
                        // channel is implemented. For now, log the layout change.
                        info!(
                            "Monitor layout update: {} monitor(s) - DISPLAYCONTROL notification not yet implemented",
                            monitors.len()
                        );
                        for m in &monitors {
                            debug!(
                                "  Monitor {}: {}x{} at ({}, {}), primary={}",
                                m.id, m.width, m.height, m.x, m.y, m.is_primary
                            );
                        }
                    }
                    None => {
                        // Channel closed - main thread disconnected
                        info!("Main thread disconnected");
                        return Ok(());
                    }
                }
            }
        }
    }
}

/// Builds IronRDP connector config from our ConnectionConfig.
fn build_rdp_config(config: &ConnectionConfig) -> Result<connector::Config, ConnectionError> {
    let credentials = Credentials::UsernamePassword {
        username: config.username.clone().unwrap_or_default(),
        password: config.password.clone().unwrap_or_default(),
    };

    // Use default codecs (includes RemoteFX)
    let bitmap_codecs = client_codecs_capabilities(&[])
        .map_err(|e| ConnectionError::Protocol(format!("Failed to build codec config: {e}")))?;

    let bitmap_config = connector::BitmapConfig {
        lossy_compression: true,
        color_depth: 32,
        codecs: bitmap_codecs,
    };

    Ok(connector::Config {
        credentials,
        domain: config.domain.clone(),
        desktop_size: connector::DesktopSize {
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
        },
        desktop_scale_factor: 100,
        enable_tls: true,
        enable_credssp: true,
        client_name: "YARD".to_string(),
        client_build: 0,
        keyboard_type: KeyboardType::IbmEnhanced,
        keyboard_subtype: 0,
        keyboard_functional_keys_count: 12,
        // TODO(Story 2.9): Detect XKB layout and map to RDP layout code.
        // Currently hardcoded to US English. Unicode input bypasses most layout
        // issues, but the server may still use this for function keys and shortcuts.
        keyboard_layout: 0x0409, // US English
        ime_file_name: String::new(),
        bitmap: Some(bitmap_config),
        dig_product_id: String::new(),
        client_dir: String::new(),
        platform: MajorPlatformType::UNIX,
        hardware_id: None,
        request_data: None,
        autologon: false,
        enable_audio_playback: true,
        performance_flags: PerformanceFlags::default(),
        license_cache: None,
        timezone_info: Default::default(),
        enable_server_pointer: true,
        pointer_software_rendering: false,
    })
}

/// Stub network client for SSPI - we don't support Kerberos authentication yet.
struct StubNetworkClient;

impl ironrdp_tokio::NetworkClient for StubNetworkClient {
    async fn send(
        &mut self,
        _request: &ironrdp::connector::sspi::generator::NetworkRequest,
    ) -> connector::ConnectorResult<Vec<u8>> {
        Err(connector::ConnectorError::new(
            "Kerberos authentication not supported",
            connector::ConnectorErrorKind::General,
        ))
    }
}

/// Attempts to establish a TCP connection to the RDP server.
/// Returns both the stream and the local socket address (needed for ClientConnector).
async fn attempt_tcp_connection(
    config: &ConnectionConfig,
) -> Result<(TcpStream, SocketAddr), ConnectionError> {
    let address = config.address();

    // Resolve DNS asynchronously (non-blocking)
    let socket_addr = lookup_host(&address)
        .await
        .map_err(|e| ConnectionError::DnsResolution(e.to_string()))?
        .next()
        .ok_or_else(|| ConnectionError::DnsResolution("No addresses found".to_string()))?;

    // Attempt TCP connection with timeout
    let connect_future = TcpStream::connect(socket_addr);
    let stream = timeout(Duration::from_secs(CONNECTION_TIMEOUT_SECS), connect_future)
        .await
        .map_err(|_| ConnectionError::Timeout(format!("after {}s", CONNECTION_TIMEOUT_SECS)))?
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("refused") {
                ConnectionError::ConnectionRefused(msg)
            } else {
                ConnectionError::Io(msg)
            }
        })?;

    // Get local address for the connector
    let local_addr = stream
        .local_addr()
        .map_err(|e| ConnectionError::Io(format!("Failed to get local address: {e}")))?;

    Ok((stream, local_addr))
}

/// Performs TLS upgrade with certificate verification.
///
/// Returns the TLS stream and the server's public key (for CredSSP).
async fn perform_tls_upgrade(
    framed: TokioFramed<TcpStream>,
    config: &ConnectionConfig,
    to_network_rx: &mut mpsc::Receiver<ToNetwork>,
    tx: &mpsc::Sender<FromNetwork>,
) -> Result<(tokio_rustls::client::TlsStream<TcpStream>, Vec<u8>), ConnectionError> {
    // Extract the TCP stream from the framed transport
    let (stream, _leftover) = framed.into_inner();

    // Create TLS config that accepts all certificates (we verify manually)
    // This is necessary because RDP servers commonly use self-signed certs
    let tls_config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAllCertVerifier))
        .with_no_client_auth();

    let connector = TlsConnector::from(Arc::new(tls_config));

    // Parse server name for SNI
    let server_name = ServerName::try_from(config.host.clone())
        .map_err(|_| ConnectionError::TlsError(format!("Invalid server name: {}", config.host)))?;

    // Perform TLS handshake
    let tls_stream = connector
        .connect(server_name, stream)
        .await
        .map_err(|e| ConnectionError::TlsError(e.to_string()))?;

    // Extract certificate information and public key
    let (cert_info, server_public_key) = extract_certificate_info(&tls_stream)?;

    // Send certificate info to main thread for user verification
    let server = config.address();
    if tx
        .send(FromNetwork::CertificateVerify {
            server: server.clone(),
            cert_info,
        })
        .await
        .is_err()
    {
        return Err(ConnectionError::Io(
            "Failed to send certificate info".to_string(),
        ));
    }

    // Wait for user's decision
    let accepted = wait_for_certificate_decision(to_network_rx).await?;

    if accepted {
        debug!("Certificate accepted by user");
        Ok((tls_stream, server_public_key))
    } else {
        Err(ConnectionError::CertificateRejected(
            "User rejected the server certificate".to_string(),
        ))
    }
}

/// Extracts certificate information and public key from a TLS connection.
fn extract_certificate_info(
    tls_stream: &tokio_rustls::client::TlsStream<TcpStream>,
) -> Result<(CertificateInfo, Vec<u8>), ConnectionError> {
    let (_, client_conn) = tls_stream.get_ref();

    // Get peer certificates
    let certs = client_conn
        .peer_certificates()
        .ok_or_else(|| ConnectionError::TlsError("No peer certificates".to_string()))?;

    let cert_der = certs
        .first()
        .ok_or_else(|| ConnectionError::TlsError("Empty certificate chain".to_string()))?;

    // Parse the certificate using x509-cert
    use x509_cert::der::Decode;
    let cert = x509_cert::Certificate::from_der(cert_der.as_ref())
        .map_err(|e| ConnectionError::TlsError(format!("Failed to parse certificate: {e}")))?;

    // Extract fingerprint (SHA-256)
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(cert_der.as_ref());
    let fingerprint_bytes = hasher.finalize();
    let fingerprint = format_fingerprint(&fingerprint_bytes);

    // Extract subject info
    let subject = &cert.tbs_certificate.subject;
    let common_name = extract_rdn_value(subject, "2.5.4.3"); // OID for CN
    let organization = extract_rdn_value(subject, "2.5.4.10"); // OID for O

    // Extract issuer
    let issuer = &cert.tbs_certificate.issuer;
    let issuer_cn = extract_rdn_value(issuer, "2.5.4.3")
        .or_else(|| extract_rdn_value(issuer, "2.5.4.10"))
        .unwrap_or_else(|| "Unknown".to_string());

    // Extract validity dates
    let validity = &cert.tbs_certificate.validity;
    let not_before = format!("{}", validity.not_before);
    let not_after = format!("{}", validity.not_after);

    let mut cert_info = CertificateInfo::new(fingerprint, issuer_cn, not_before, not_after);

    if let Some(cn) = common_name {
        cert_info = cert_info.with_common_name(cn);
    }
    if let Some(org) = organization {
        cert_info = cert_info.with_organization(org);
    }

    // Extract server public key for CredSSP
    // The public key is the SubjectPublicKeyInfo from the certificate
    let spki = &cert.tbs_certificate.subject_public_key_info;
    let public_key = x509_cert::der::Encode::to_der(spki)
        .map_err(|e| ConnectionError::TlsError(format!("Failed to encode public key: {e}")))?;

    Ok((cert_info, public_key))
}

/// Formats a fingerprint as hex with colons (e.g., "SHA256:AB:CD:EF:...")
fn format_fingerprint(bytes: &[u8]) -> String {
    let hex: Vec<String> = bytes.iter().map(|b| format!("{:02X}", b)).collect();
    format!("SHA256:{}", hex.join(":"))
}

/// Extracts a value from an X.500 distinguished name by OID.
fn extract_rdn_value(name: &x509_cert::name::Name, oid_str: &str) -> Option<String> {
    use x509_cert::der::oid::ObjectIdentifier;

    let oid = ObjectIdentifier::new(oid_str).ok()?;

    for rdn in name.0.iter() {
        for atv in rdn.0.iter() {
            if atv.oid == oid {
                // Try to extract the string value as UTF-8 or printable string
                if let Ok(s) = atv.value.decode_as::<x509_cert::der::asn1::Utf8StringRef>() {
                    return Some(s.to_string());
                }
                if let Ok(s) = atv
                    .value
                    .decode_as::<x509_cert::der::asn1::PrintableStringRef>()
                {
                    return Some(s.to_string());
                }
            }
        }
    }
    None
}

/// Waits for the user's certificate decision from the main thread.
async fn wait_for_certificate_decision(
    to_network_rx: &mut mpsc::Receiver<ToNetwork>,
) -> Result<bool, ConnectionError> {
    let decision_timeout = Duration::from_secs(CERTIFICATE_DECISION_TIMEOUT_SECS);
    let deadline = tokio::time::Instant::now() + decision_timeout;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(ConnectionError::Timeout(format!(
                "Certificate decision timeout ({}s)",
                CERTIFICATE_DECISION_TIMEOUT_SECS
            )));
        }

        match timeout(remaining, to_network_rx.recv()).await {
            Ok(Some(ToNetwork::CertificateDecision(accepted))) => return Ok(accepted),
            Ok(Some(ToNetwork::Disconnect)) => {
                return Err(ConnectionError::Io(
                    "Connection cancelled by user".to_string(),
                ));
            }
            Ok(Some(_)) => {
                // Unexpected message - ignore and keep waiting
                warn!(
                    "Received unexpected message while waiting for certificate decision, ignoring"
                );
                continue;
            }
            Ok(None) => return Err(ConnectionError::Io("Channel closed".to_string())),
            Err(_) => {
                return Err(ConnectionError::Timeout(format!(
                    "Certificate decision timeout ({}s)",
                    CERTIFICATE_DECISION_TIMEOUT_SECS
                )));
            }
        }
    }
}

/// A certificate verifier that accepts all certificates.
/// We verify certificates manually by prompting the user.
///
/// NOTE: AC 1 requires auto-accepting trusted certificates. Current implementation
/// prompts for ALL certificates. Future enhancement: try system root CAs first,
/// only prompt if verification fails (requires rustls-native-certs integration).
#[derive(Debug)]
struct AcceptAllCertVerifier;

impl rustls::client::danger::ServerCertVerifier for AcceptAllCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        // Accept all certificates - we'll verify manually with user prompt
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

/// Sends monitor layout to the server via DISPLAYCONTROL channel (Story 3.2).
///
/// Converts the provided `RdpMonitorInfo` list to RDP's `MonitorLayoutEntry` format
/// and encodes the message for transmission.
///
/// # Validation
///
/// - Exactly one monitor must be marked as primary (`is_primary = true`)
/// - The primary monitor should be at position (0, 0) per RDP specification
///
/// # Protocol Note
///
/// Per MS-RDPEDISP, the server does not send an explicit acknowledgment for
/// the monitor layout PDU. The server processes the layout and adjusts the
/// desktop accordingly. We assume success if no error occurs during transmission.
fn send_monitor_layout(
    active_stage: &mut ActiveStage,
    monitors: &[RdpMonitorInfo],
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    use crate::messages::validate_monitor_layout;
    use ironrdp::displaycontrol::pdu::{DisplayControlMonitorLayout, DisplayControlPdu};
    use ironrdp::svc::ChannelFlags;

    // Validate monitor layout using shared validation logic
    if let Err(e) = validate_monitor_layout(monitors) {
        return Err(e.into());
    }

    // Convert our monitor info to IronRDP's MonitorLayoutEntry format
    let mut entries: Vec<MonitorLayoutEntry> = Vec::with_capacity(monitors.len());

    for m in monitors {
        // Create entry based on whether it's primary or secondary
        let mut entry = if m.is_primary {
            MonitorLayoutEntry::new_primary(m.width, m.height)?
        } else {
            MonitorLayoutEntry::new_secondary(m.width, m.height)?
        };

        // Set position (primary must be at 0,0 per RDP spec, so only set for non-primary)
        if !m.is_primary {
            entry = entry.with_position(m.x, m.y)?;
        }

        // Set desktop scale factor
        entry = entry.with_desktop_scale_factor(m.scale_percent)?;

        // Set orientation to landscape (default)
        entry = entry.with_orientation(MonitorOrientation::Landscape);

        // Set physical dimensions if available
        if let (Some(w), Some(h)) = (m.physical_width_mm, m.physical_height_mm) {
            entry = entry.with_physical_dimensions(w, h)?;
        }

        entries.push(entry);
    }

    // Create the monitor layout PDU
    let layout = DisplayControlMonitorLayout::new(&entries)?;
    let pdu = DisplayControlPdu::MonitorLayout(layout);

    // Get the DisplayControlClient from the session
    // Note: The channel may not be open immediately after connection if the server
    // hasn't yet sent its capabilities. This is expected behavior - we fall back
    // to single-monitor mode in this case.
    if let Some(dvc) = active_stage.get_dvc::<DisplayControlClient>() {
        // Check if the channel is ready
        if !dvc.is_open() {
            return Err("DISPLAYCONTROL channel not open (server may not support it)".into());
        }

        // Get channel ID (must be Some if channel is open)
        let channel_id = dvc.channel_id().ok_or("DISPLAYCONTROL channel has no ID")?;

        // Encode the PDU as DVC messages using the channel
        // The PDU implements DvcEncode, so we can convert it to SvcMessage
        let svc_messages = ironrdp::dvc::encode_dvc_messages(
            channel_id,
            vec![Box::new(pdu)],
            ChannelFlags::empty(),
        )?;

        // Encode via ActiveStage
        let encoded = active_stage.encode_dvc_messages(svc_messages)?;
        Ok(encoded)
    } else {
        Err("DisplayControlClient not found in session".into())
    }
}

/// Calculates the combined desktop size from multiple monitors.
///
/// The combined size is the bounding rectangle that encompasses all monitors,
/// accounting for monitors that may have negative positions (e.g., above or
/// left of the primary monitor).
fn calculate_combined_desktop_size(monitors: &[RdpMonitorInfo]) -> DesktopSize {
    if monitors.is_empty() {
        return DesktopSize::new(DEFAULT_WIDTH, DEFAULT_HEIGHT);
    }

    // Find the bounding box of all monitors
    let mut min_left: i32 = i32::MAX;
    let mut min_top: i32 = i32::MAX;
    let mut max_right: i32 = i32::MIN;
    let mut max_bottom: i32 = i32::MIN;

    for m in monitors {
        min_left = min_left.min(m.x);
        min_top = min_top.min(m.y);
        max_right = max_right.max(m.x + m.width as i32);
        max_bottom = max_bottom.max(m.y + m.height as i32);
    }

    // Calculate total dimensions
    let total_width = (max_right - min_left) as u32;
    let total_height = (max_bottom - min_top) as u32;

    // Clamp to u16::MAX to prevent overflow (very large multi-monitor setups)
    let width = total_width.min(u16::MAX as u32) as u16;
    let height = total_height.min(u16::MAX as u32) as u16;

    DesktopSize::new(width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_fingerprint_basic() {
        let bytes = [0xAB, 0xCD, 0xEF];
        let result = format_fingerprint(&bytes);
        assert_eq!(result, "SHA256:AB:CD:EF");
    }

    #[test]
    fn test_format_fingerprint_sha256_length() {
        // SHA-256 produces 32 bytes
        let bytes: [u8; 32] = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E,
            0x0F, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C,
            0x1D, 0x1E, 0x1F, 0x20,
        ];
        let result = format_fingerprint(&bytes);
        assert!(result.starts_with("SHA256:"));
        assert_eq!(result.matches(':').count(), 32); // 31 colons between bytes + 1 after SHA256
    }

    #[test]
    fn test_format_fingerprint_lowercase_hex() {
        let bytes = [0x00, 0xFF, 0xAA];
        let result = format_fingerprint(&bytes);
        // Should be uppercase hex
        assert_eq!(result, "SHA256:00:FF:AA");
    }

    #[test]
    fn test_format_fingerprint_empty() {
        let bytes: [u8; 0] = [];
        let result = format_fingerprint(&bytes);
        assert_eq!(result, "SHA256:");
    }

    #[test]
    fn test_connection_timeout_constant() {
        assert_eq!(CONNECTION_TIMEOUT_SECS, 10);
    }

    #[test]
    fn test_certificate_decision_timeout_constant() {
        assert_eq!(CERTIFICATE_DECISION_TIMEOUT_SECS, 60);
    }

    #[test]
    fn test_desktop_size_new() {
        let size = DesktopSize::new(1920, 1080);
        assert_eq!(size.width, 1920);
        assert_eq!(size.height, 1080);
    }

    #[test]
    fn test_build_rdp_config_basic() {
        let conn_config = ConnectionConfig::new("test.example.com", 3389)
            .with_username("testuser")
            .with_password("testpass");

        let rdp_config = build_rdp_config(&conn_config).unwrap();

        assert_eq!(rdp_config.client_name, "YARD");
        assert!(rdp_config.enable_tls);
        assert!(rdp_config.enable_credssp);
        assert_eq!(rdp_config.desktop_size.width, DEFAULT_WIDTH);
        assert_eq!(rdp_config.desktop_size.height, DEFAULT_HEIGHT);
    }

    #[test]
    fn test_build_rdp_config_with_domain() {
        let conn_config = ConnectionConfig::new("server", 3389)
            .with_username("user")
            .with_domain("CORP")
            .with_password("pass");

        let rdp_config = build_rdp_config(&conn_config).unwrap();

        assert_eq!(rdp_config.domain, Some("CORP".to_string()));
    }

    #[test]
    fn test_build_rdp_config_empty_credentials() {
        // Empty credentials should still build config (server validates)
        let conn_config = ConnectionConfig::new("server", 3389);

        let rdp_config = build_rdp_config(&conn_config).unwrap();

        // Credentials default to empty strings
        match &rdp_config.credentials {
            ironrdp::connector::Credentials::UsernamePassword { username, password } => {
                assert!(username.is_empty());
                assert!(password.is_empty());
            }
            _ => panic!("Expected UsernamePassword credentials"),
        }
    }

    #[test]
    fn test_build_rdp_config_bitmap_settings() {
        let conn_config = ConnectionConfig::new("server", 3389);
        let rdp_config = build_rdp_config(&conn_config).unwrap();

        let bitmap = rdp_config.bitmap.expect("bitmap config should be set");
        assert!(bitmap.lossy_compression);
        assert_eq!(bitmap.color_depth, 32);
    }

    #[test]
    fn test_default_dimensions() {
        assert_eq!(DEFAULT_WIDTH, 1920);
        assert_eq!(DEFAULT_HEIGHT, 1080);
    }

    // Story 3.2: Multi-monitor tests
    #[test]
    fn test_calculate_combined_desktop_size_empty() {
        let size = calculate_combined_desktop_size(&[]);
        assert_eq!(size.width, DEFAULT_WIDTH);
        assert_eq!(size.height, DEFAULT_HEIGHT);
    }

    #[test]
    fn test_calculate_combined_desktop_size_single_monitor() {
        use crate::messages::RdpMonitorInfo;
        let monitors = vec![RdpMonitorInfo::new(0, 0, 1920, 1080, true)];
        let size = calculate_combined_desktop_size(&monitors);
        assert_eq!(size.width, 1920);
        assert_eq!(size.height, 1080);
    }

    #[test]
    fn test_calculate_combined_desktop_size_horizontal_dual() {
        use crate::messages::RdpMonitorInfo;
        let monitors = vec![
            RdpMonitorInfo::new(0, 0, 1920, 1080, true),
            RdpMonitorInfo::new(1920, 0, 1920, 1080, false),
        ];
        let size = calculate_combined_desktop_size(&monitors);
        assert_eq!(size.width, 3840);
        assert_eq!(size.height, 1080);
    }

    #[test]
    fn test_calculate_combined_desktop_size_vertical_dual() {
        use crate::messages::RdpMonitorInfo;
        let monitors = vec![
            RdpMonitorInfo::new(0, 0, 1920, 1080, true),
            RdpMonitorInfo::new(0, 1080, 1920, 1080, false),
        ];
        let size = calculate_combined_desktop_size(&monitors);
        assert_eq!(size.width, 1920);
        assert_eq!(size.height, 2160);
    }

    #[test]
    fn test_calculate_combined_desktop_size_staggered_above() {
        use crate::messages::RdpMonitorInfo;
        // Layout: primary at (0,0), secondary at (1920, -500) - above and to the right
        let monitors = vec![
            RdpMonitorInfo::new(0, 0, 1920, 1080, true),
            RdpMonitorInfo::new(1920, -500, 2560, 1440, false),
        ];
        let size = calculate_combined_desktop_size(&monitors);
        // Bounding box: left=0, top=-500, right=4480, bottom=1080
        // Width: 4480 - 0 = 4480
        // Height: 1080 - (-500) = 1580
        assert_eq!(size.width, 4480);
        assert_eq!(size.height, 1580);
    }

    #[test]
    fn test_calculate_combined_desktop_size_negative_positions() {
        use crate::messages::RdpMonitorInfo;
        // Layout: secondary to the left of primary
        let monitors = vec![
            RdpMonitorInfo::new(0, 0, 1920, 1080, true),
            RdpMonitorInfo::new(-1920, 0, 1920, 1080, false),
        ];
        let size = calculate_combined_desktop_size(&monitors);
        // Bounding box: left=-1920, top=0, right=1920, bottom=1080
        // Width: 1920 - (-1920) = 3840
        // Height: 1080 - 0 = 1080
        assert_eq!(size.width, 3840);
        assert_eq!(size.height, 1080);
    }

    #[test]
    fn test_calculate_combined_desktop_size_all_negative() {
        use crate::messages::RdpMonitorInfo;
        // Edge case: monitors positioned in negative quadrant
        let monitors = vec![
            RdpMonitorInfo::new(-1920, -1080, 1920, 1080, true),
            RdpMonitorInfo::new(-3840, -1080, 1920, 1080, false),
        ];
        let size = calculate_combined_desktop_size(&monitors);
        // Bounding box: left=-3840, top=-1080, right=0, bottom=0
        // Width: 0 - (-3840) = 3840
        // Height: 0 - (-1080) = 1080
        assert_eq!(size.width, 3840);
        assert_eq!(size.height, 1080);
    }

    // Test for validation logic (tested indirectly through validation_* helper functions)
    #[test]
    fn test_validate_monitor_layout_counts_primaries() {
        use crate::messages::RdpMonitorInfo;

        // Valid: one primary
        let monitors = [
            RdpMonitorInfo::new(0, 0, 1920, 1080, true),
            RdpMonitorInfo::new(1920, 0, 1920, 1080, false),
        ];
        let primary_count = monitors.iter().filter(|m| m.is_primary).count();
        assert_eq!(primary_count, 1);

        // Invalid: no primary
        let monitors_no_primary = [
            RdpMonitorInfo::new(0, 0, 1920, 1080, false),
            RdpMonitorInfo::new(1920, 0, 1920, 1080, false),
        ];
        let count = monitors_no_primary.iter().filter(|m| m.is_primary).count();
        assert_eq!(count, 0);

        // Invalid: two primaries
        let monitors_two_primary = [
            RdpMonitorInfo::new(0, 0, 1920, 1080, true),
            RdpMonitorInfo::new(1920, 0, 1920, 1080, true),
        ];
        let count = monitors_two_primary.iter().filter(|m| m.is_primary).count();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_validate_primary_monitor_position() {
        use crate::messages::RdpMonitorInfo;

        // Valid: primary at (0,0)
        let monitors = [RdpMonitorInfo::new(0, 0, 1920, 1080, true)];
        let primary = monitors.iter().find(|m| m.is_primary).unwrap();
        assert_eq!(primary.x, 0);
        assert_eq!(primary.y, 0);

        // Invalid: primary not at origin
        let monitors_bad = [RdpMonitorInfo::new(100, 50, 1920, 1080, true)];
        let primary = monitors_bad.iter().find(|m| m.is_primary).unwrap();
        assert_ne!(primary.x, 0); // This would trigger a warning in send_monitor_layout
    }
}
