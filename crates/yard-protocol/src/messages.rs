//! Inter-thread message types for YARD.
//!
//! Defines the message types used for communication between the main thread
//! (calloop event loop) and the network thread (Tokio runtime).

use std::fmt;

/// Configuration for establishing an RDP connection.
#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    /// Target hostname or IP address.
    pub host: String,
    /// Target port (default: 3389).
    pub port: u16,
    /// Username for authentication.
    pub username: Option<String>,
    /// Domain for authentication.
    pub domain: Option<String>,
}

impl ConnectionConfig {
    /// Creates a new connection configuration.
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            username: None,
            domain: None,
        }
    }

    /// Sets the username for authentication.
    pub fn with_username(mut self, username: impl Into<String>) -> Self {
        self.username = Some(username.into());
        self
    }

    /// Sets the domain for authentication.
    pub fn with_domain(mut self, domain: impl Into<String>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    /// Returns the full address as "host:port".
    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// Messages sent from the main thread to the network thread.
#[derive(Debug)]
pub enum ToNetwork {
    /// Request to establish a connection.
    Connect(ConnectionConfig),
    /// Request to disconnect gracefully.
    Disconnect,
}

/// Messages sent from the network thread to the main thread.
#[derive(Debug)]
pub enum FromNetwork {
    /// Connection attempt is in progress.
    Connecting,
    /// Connection established successfully.
    Connected,
    /// Disconnected (gracefully or due to error).
    Disconnected,
    /// An error occurred.
    Error(ConnectionError),
}

/// Error types for connection failures.
#[derive(Debug, Clone)]
pub enum ConnectionError {
    /// Could not resolve the hostname.
    DnsResolution(String),
    /// Connection was refused by the server.
    ConnectionRefused(String),
    /// Connection timed out.
    Timeout(String),
    /// TLS handshake failed.
    TlsError(String),
    /// Authentication failed.
    AuthenticationFailed(String),
    /// Protocol error.
    Protocol(String),
    /// Generic I/O error.
    Io(String),
}

impl fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DnsResolution(msg) => {
                write!(
                    f,
                    "Cannot resolve hostname: {msg}. Check the server address."
                )
            }
            Self::ConnectionRefused(msg) => {
                write!(
                    f,
                    "Connection refused: {msg}. Server may not be running or port is blocked."
                )
            }
            Self::Timeout(msg) => {
                write!(f, "Connection timeout: {msg}. Server may be unreachable.")
            }
            Self::TlsError(msg) => {
                write!(f, "TLS error: {msg}. Server certificate may be invalid.")
            }
            Self::AuthenticationFailed(msg) => {
                write!(f, "Authentication failed: {msg}. Check your credentials.")
            }
            Self::Protocol(msg) => {
                write!(f, "Protocol error: {msg}.")
            }
            Self::Io(msg) => {
                write!(f, "I/O error: {msg}.")
            }
        }
    }
}

impl std::error::Error for ConnectionError {}
