//! Inter-thread message types for YARD.
//!
//! Defines the message types used for communication between the main thread
//! (calloop event loop) and the network thread (Tokio runtime).

use std::fmt;

/// Configuration for establishing an RDP connection.
///
/// Note: Password is intentionally excluded from Debug to prevent credential leakage in logs.
pub struct ConnectionConfig {
    /// Target hostname or IP address.
    pub host: String,
    /// Target port (default: 3389).
    pub port: u16,
    /// Username for authentication.
    pub username: Option<String>,
    /// Domain for authentication.
    pub domain: Option<String>,
    /// Password for authentication.
    /// SECURITY: Never log this field.
    pub password: Option<String>,
}

// Manual Debug implementation to exclude password from logs (NFR-S2)
impl std::fmt::Debug for ConnectionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("domain", &self.domain)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

// Manual Clone to handle password securely
impl Clone for ConnectionConfig {
    fn clone(&self) -> Self {
        Self {
            host: self.host.clone(),
            port: self.port,
            username: self.username.clone(),
            domain: self.domain.clone(),
            password: self.password.clone(),
        }
    }
}

impl ConnectionConfig {
    /// Creates a new connection configuration.
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            username: None,
            domain: None,
            password: None,
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

    /// Sets the password for authentication.
    ///
    /// SECURITY: The password is never logged or included in Debug output.
    pub fn with_password(mut self, password: impl Into<String>) -> Self {
        self.password = Some(password.into());
        self
    }

    /// Returns the full address as "host:port".
    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// Returns true if credentials are complete (username and password provided).
    pub fn has_credentials(&self) -> bool {
        self.username.is_some() && self.password.is_some()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_config_new() {
        let config = ConnectionConfig::new("example.com", 3389);
        assert_eq!(config.host, "example.com");
        assert_eq!(config.port, 3389);
        assert!(config.username.is_none());
        assert!(config.domain.is_none());
        assert!(config.password.is_none());
    }

    #[test]
    fn test_connection_config_with_username() {
        let config = ConnectionConfig::new("example.com", 3389).with_username("admin");
        assert_eq!(config.username, Some("admin".to_string()));
    }

    #[test]
    fn test_connection_config_with_domain() {
        let config = ConnectionConfig::new("example.com", 3389).with_domain("CORP");
        assert_eq!(config.domain, Some("CORP".to_string()));
    }

    #[test]
    fn test_connection_config_address() {
        let config = ConnectionConfig::new("server.local", 3390);
        assert_eq!(config.address(), "server.local:3390");
    }

    #[test]
    fn test_connection_config_builder_chain() {
        let config = ConnectionConfig::new("host", 3389)
            .with_username("user")
            .with_domain("domain");
        assert_eq!(config.host, "host");
        assert_eq!(config.username, Some("user".to_string()));
        assert_eq!(config.domain, Some("domain".to_string()));
    }

    #[test]
    fn test_connection_config_with_password() {
        let config = ConnectionConfig::new("host", 3389).with_password("secret123");
        assert_eq!(config.password, Some("secret123".to_string()));
    }

    #[test]
    fn test_connection_config_has_credentials() {
        let config = ConnectionConfig::new("host", 3389);
        assert!(!config.has_credentials());

        let config = config.with_username("user");
        assert!(!config.has_credentials());

        let config = config.with_password("pass");
        assert!(config.has_credentials());
    }

    #[test]
    fn test_connection_config_debug_redacts_password() {
        let config = ConnectionConfig::new("host", 3389)
            .with_username("user")
            .with_password("supersecret");
        let debug_output = format!("{:?}", config);
        assert!(debug_output.contains("[REDACTED]"));
        assert!(!debug_output.contains("supersecret"));
    }

    #[test]
    fn test_connection_error_display_dns() {
        let err = ConnectionError::DnsResolution("no such host".to_string());
        let msg = err.to_string();
        assert!(msg.contains("Cannot resolve hostname"));
        assert!(msg.contains("no such host"));
    }

    #[test]
    fn test_connection_error_display_refused() {
        let err = ConnectionError::ConnectionRefused("connection refused".to_string());
        let msg = err.to_string();
        assert!(msg.contains("Connection refused"));
        assert!(msg.contains("Server may not be running"));
    }

    #[test]
    fn test_connection_error_display_timeout() {
        let err = ConnectionError::Timeout("after 10s".to_string());
        let msg = err.to_string();
        assert!(msg.contains("Connection timeout"));
        assert!(msg.contains("after 10s"));
    }

    #[test]
    fn test_connection_error_display_tls() {
        let err = ConnectionError::TlsError("certificate expired".to_string());
        let msg = err.to_string();
        assert!(msg.contains("TLS error"));
        assert!(msg.contains("certificate"));
    }

    #[test]
    fn test_connection_error_display_auth() {
        let err = ConnectionError::AuthenticationFailed("bad password".to_string());
        let msg = err.to_string();
        assert!(msg.contains("Authentication failed"));
        assert!(msg.contains("credentials"));
    }

    #[test]
    fn test_connection_error_display_protocol() {
        let err = ConnectionError::Protocol("invalid packet".to_string());
        let msg = err.to_string();
        assert!(msg.contains("Protocol error"));
    }

    #[test]
    fn test_connection_error_display_io() {
        let err = ConnectionError::Io("broken pipe".to_string());
        let msg = err.to_string();
        assert!(msg.contains("I/O error"));
        assert!(msg.contains("broken pipe"));
    }

    #[test]
    fn test_to_network_debug() {
        let msg = ToNetwork::Connect(ConnectionConfig::new("test", 3389));
        let debug = format!("{:?}", msg);
        assert!(debug.contains("Connect"));
    }

    #[test]
    fn test_from_network_debug() {
        let msg = FromNetwork::Connecting;
        let debug = format!("{:?}", msg);
        assert!(debug.contains("Connecting"));
    }
}
