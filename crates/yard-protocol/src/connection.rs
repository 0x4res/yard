//! RDP connection handling for the network thread.
//!
//! This module provides the async connection logic that runs in a dedicated
//! Tokio thread, communicating with the main thread via message channels.

use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::ServerName;
use tokio::net::{TcpStream, lookup_host};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tracing::{debug, warn};

use crate::messages::{CertificateInfo, ConnectionConfig, ConnectionError, FromNetwork, ToNetwork};

/// Default connection timeout in seconds.
const CONNECTION_TIMEOUT_SECS: u64 = 10;

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

/// Handles a connection request with TLS and certificate verification.
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
    let stream = match attempt_tcp_connection(&config).await {
        Ok(stream) => stream,
        Err(err) => {
            let _ = tx.send(FromNetwork::Error(err)).await;
            return;
        }
    };

    debug!("TCP connection established to {}", config.address());

    // Perform TLS upgrade with certificate verification
    let _tls_stream = match perform_tls_upgrade(stream, &config, to_network_rx, tx).await {
        Ok(stream) => stream,
        Err(err) => {
            let _ = tx.send(FromNetwork::Error(err)).await;
            return;
        }
    };

    debug!("TLS connection established");

    // Connection established (TLS level)
    // TODO: Continue with RDP handshake (CredSSP/NLA) in future stories
    let _ = tx.send(FromNetwork::Connected).await;

    // For now, just disconnect after successful TLS connection
    // Full RDP session handling will come in later stories
    let _ = tx.send(FromNetwork::Disconnected).await;
}

/// Attempts to establish a TCP connection to the RDP server.
async fn attempt_tcp_connection(config: &ConnectionConfig) -> Result<TcpStream, ConnectionError> {
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

    Ok(stream)
}

/// Performs TLS upgrade with certificate verification.
///
/// This function:
/// 1. Upgrades the TCP connection to TLS
/// 2. Extracts certificate information
/// 3. Sends CertificateVerify to main thread for user decision
/// 4. Waits for CertificateDecision response
/// 5. Returns the TLS stream if accepted, or error if rejected
async fn perform_tls_upgrade(
    stream: TcpStream,
    config: &ConnectionConfig,
    to_network_rx: &mut mpsc::Receiver<ToNetwork>,
    tx: &mpsc::Sender<FromNetwork>,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, ConnectionError> {
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

    // Extract certificate information from the TLS connection
    let cert_info = extract_certificate_info(&tls_stream)?;

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
        return Err(ConnectionError::Io("Failed to send certificate info".to_string()));
    }

    // Wait for user's decision
    let accepted = wait_for_certificate_decision(to_network_rx).await?;

    if accepted {
        debug!("Certificate accepted by user");
        Ok(tls_stream)
    } else {
        Err(ConnectionError::CertificateRejected(
            "User rejected the server certificate".to_string(),
        ))
    }
}

/// Extracts certificate information from a TLS connection.
fn extract_certificate_info(
    tls_stream: &tokio_rustls::client::TlsStream<TcpStream>,
) -> Result<CertificateInfo, ConnectionError> {
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
    use sha2::{Sha256, Digest};
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

    Ok(cert_info)
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
    // Wait for the decision with a timeout
    let decision_timeout = Duration::from_secs(60); // 1 minute for user to decide

    match timeout(decision_timeout, to_network_rx.recv()).await {
        Ok(Some(ToNetwork::CertificateDecision(accepted))) => Ok(accepted),
        Ok(Some(ToNetwork::Disconnect)) => {
            Err(ConnectionError::Io("Connection cancelled by user".to_string()))
        }
        Ok(Some(_)) => {
            // Unexpected message, keep waiting
            warn!("Received unexpected message while waiting for certificate decision");
            Err(ConnectionError::Io("Unexpected message during certificate verification".to_string()))
        }
        Ok(None) => Err(ConnectionError::Io("Channel closed".to_string())),
        Err(_) => Err(ConnectionError::Timeout(
            "Certificate decision timeout (60s)".to_string(),
        )),
    }
}

/// A certificate verifier that accepts all certificates.
/// We verify certificates manually by prompting the user.
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
