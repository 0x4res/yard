//! RDP connection handling for the network thread.
//!
//! This module provides the async connection logic that runs in a dedicated
//! Tokio thread, communicating with the main thread via message channels.

use std::time::Duration;

use tokio::net::{TcpStream, lookup_host};
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::messages::{ConnectionConfig, ConnectionError, FromNetwork, ToNetwork};

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
                handle_connect(config, &from_network_tx).await;
            }
            ToNetwork::Disconnect => {
                let _ = from_network_tx.send(FromNetwork::Disconnected).await;
                break;
            }
            ToNetwork::CertificateDecision(_accepted) => {
                // Certificate decisions are handled during TLS handshake
                // This message is received asynchronously after CertificateVerify is sent
                // TODO: Integrate with actual TLS handshake when IronRDP is available
            }
        }
    }
}

/// Handles a connection request.
async fn handle_connect(config: ConnectionConfig, tx: &mpsc::Sender<FromNetwork>) {
    // Notify main thread that we're connecting
    if tx.send(FromNetwork::Connecting).await.is_err() {
        return;
    }

    // Attempt to connect
    match attempt_connection(&config).await {
        Ok(_stream) => {
            // Connection established (TCP level)
            // TODO: Implement full RDP handshake in Story 1.5+
            let _ = tx.send(FromNetwork::Connected).await;

            // For now, just disconnect after successful TCP connection
            // Full RDP session handling will come in later stories
            let _ = tx.send(FromNetwork::Disconnected).await;
        }
        Err(err) => {
            let _ = tx.send(FromNetwork::Error(err)).await;
        }
    }
}

/// Attempts to establish a TCP connection to the RDP server.
async fn attempt_connection(config: &ConnectionConfig) -> Result<TcpStream, ConnectionError> {
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
