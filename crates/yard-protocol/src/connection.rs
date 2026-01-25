//! RDP connection handling for the network thread.
//!
//! This module provides the async connection logic that runs in a dedicated
//! Tokio thread, communicating with the main thread via message channels.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ironrdp::connector::{self, ClientConnector, Credentials};
use ironrdp::pdu::gcc::KeyboardType;
use ironrdp::pdu::geometry::Rectangle as _;
use ironrdp::pdu::rdp::capability_sets::{client_codecs_capabilities, MajorPlatformType};
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

use crate::messages::{
    CertificateInfo, ConnectionConfig, ConnectionError, DesktopSize, FromNetwork, ToNetwork,
};

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
///
/// # Returns
///
/// A sender for sending commands to the network thread.
pub fn spawn_network_thread(from_network_tx: mpsc::Sender<FromNetwork>) -> mpsc::Sender<ToNetwork> {
    let (to_network_tx, to_network_rx) = mpsc::channel::<ToNetwork>(32);

    std::thread::Builder::new()
        .name("yard-network".to_string())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create Tokio runtime");

            rt.block_on(network_loop(to_network_rx, from_network_tx));
        })
        .expect("Failed to spawn network thread");

    to_network_tx
}

/// Main loop for the network thread.
async fn network_loop(
    mut to_network_rx: mpsc::Receiver<ToNetwork>,
    from_network_tx: mpsc::Sender<FromNetwork>,
) {
    while let Some(msg) = to_network_rx.recv().await {
        match msg {
            ToNetwork::Connect(config) => {
                handle_connect(config, &mut to_network_rx, &from_network_tx).await;
            }
            ToNetwork::Disconnect => {
                let _ = from_network_tx.send(FromNetwork::Disconnected).await;
                break;
            }
            ToNetwork::CertificateDecision(_) => {
                // Certificate decisions should be received during handle_connect
                warn!("Received unexpected CertificateDecision outside of connection");
            }
        }
    }
}

/// Handles a connection request with TLS and RDP session establishment.
async fn handle_connect(
    config: ConnectionConfig,
    to_network_rx: &mut mpsc::Receiver<ToNetwork>,
    tx: &mpsc::Sender<FromNetwork>,
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
            let conn_err =
                if err_msg.contains("access denied") || err_msg.contains("Access denied") {
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

    // Session loop
    if let Err(e) = session_loop(&mut tls_framed, &mut active_stage, &mut image, tx).await {
        error!("Session error: {e}");
        let _ = tx
            .send(FromNetwork::Error(ConnectionError::Protocol(e.to_string())))
            .await;
    }

    let _ = tx.send(FromNetwork::Disconnected).await;
}

/// Session loop that processes RDP frames.
async fn session_loop<S>(
    framed: &mut Framed<TokioStream<S>>,
    active_stage: &mut ActiveStage,
    image: &mut ironrdp::session::image::DecodedImage,
    tx: &mpsc::Sender<FromNetwork>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Sync,
{
    use yard_video::DecodedFrame;

    loop {
        // Read next PDU from server
        let (action, frame) = framed.read_pdu().await?;

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
        keyboard_layout: 0x0409, // US English
        ime_file_name: String::new(),
        bitmap: Some(bitmap_config),
        dig_product_id: String::new(),
        client_dir: String::new(),
        platform: MajorPlatformType::UNIX,
        hardware_id: None,
        request_data: None,
        autologon: false,
        enable_audio_playback: false,
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
    fn send(
        &mut self,
        _request: &ironrdp::connector::sspi::generator::NetworkRequest,
    ) -> impl std::future::Future<Output = connector::ConnectorResult<Vec<u8>>> {
        async {
            Err(connector::ConnectorError::new(
                "Kerberos authentication not supported",
                connector::ConnectorErrorKind::General,
            ))
        }
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
                if let Ok(s) = atv.value.decode_as::<x509_cert::der::asn1::PrintableStringRef>() {
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
}
