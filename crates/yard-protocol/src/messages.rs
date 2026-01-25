//! Inter-thread message types for YARD.
//!
//! Defines the message types used for communication between the main thread
//! (calloop event loop) and the network thread (Tokio runtime).

use std::fmt;

pub use yard_video::{DecodedFrame, VideoCodec};

/// Configuration for establishing an RDP connection.
///
/// Note: Password is intentionally excluded from Debug to prevent credential leakage in logs.
#[derive(Clone)]
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

    /// Parses a username string and extracts domain if present.
    ///
    /// Supports two formats:
    /// - NT-style: `DOMAIN\username` or `.\username` (local)
    /// - UPN-style: `user@domain.com`
    ///
    /// Returns (username, Option<domain>).
    pub fn parse_username(input: &str) -> (String, Option<String>) {
        // Check for NT-style: DOMAIN\username
        if let Some((domain, user)) = input.split_once('\\') {
            let domain = if domain == "." {
                None // Local machine, no domain
            } else {
                Some(domain.to_string())
            };
            return (user.to_string(), domain);
        }

        // Check for UPN-style: user@domain.com
        // Only treat as UPN if domain part contains a dot (looks like FQDN)
        if let Some((user, domain)) = input.rsplit_once('@') {
            if domain.contains('.') {
                return (user.to_string(), Some(domain.to_string()));
            }
        }

        // Plain username, no domain
        (input.to_string(), None)
    }

    /// Creates a ConnectionConfig from a username string, parsing domain if present.
    ///
    /// This handles both NT-style (`DOMAIN\user`) and UPN-style (`user@domain.com`)
    /// username formats automatically.
    pub fn from_username(host: impl Into<String>, port: u16, username: &str) -> Self {
        let (user, domain) = Self::parse_username(username);
        let mut config = Self::new(host, port).with_username(user);
        if let Some(dom) = domain {
            config = config.with_domain(dom);
        }
        config
    }
}

/// Information about a server certificate for user verification.
#[derive(Debug, Clone, PartialEq)]
pub struct CertificateInfo {
    /// SHA-256 fingerprint of the certificate (hex encoded with colons).
    pub fingerprint: String,
    /// Common Name (CN) from the certificate subject.
    pub common_name: Option<String>,
    /// Organization from the certificate subject.
    pub organization: Option<String>,
    /// Certificate issuer (CN or organization).
    pub issuer: String,
    /// Certificate validity start date (ISO 8601 format).
    pub not_before: String,
    /// Certificate validity end date (ISO 8601 format).
    pub not_after: String,
}

impl CertificateInfo {
    /// Creates a new CertificateInfo with the given details.
    pub fn new(
        fingerprint: impl Into<String>,
        issuer: impl Into<String>,
        not_before: impl Into<String>,
        not_after: impl Into<String>,
    ) -> Self {
        Self {
            fingerprint: fingerprint.into(),
            common_name: None,
            organization: None,
            issuer: issuer.into(),
            not_before: not_before.into(),
            not_after: not_after.into(),
        }
    }

    /// Sets the Common Name (CN) from the certificate subject.
    pub fn with_common_name(mut self, cn: impl Into<String>) -> Self {
        self.common_name = Some(cn.into());
        self
    }

    /// Sets the Organization from the certificate subject.
    pub fn with_organization(mut self, org: impl Into<String>) -> Self {
        self.organization = Some(org.into());
        self
    }

    /// Returns a formatted display of the certificate for user prompts.
    pub fn display(&self) -> String {
        let mut lines = Vec::new();

        // Truncate fingerprint if too long (SHA-256 = 95 chars with colons)
        // Keep prefix "SHA256:" and first ~40 chars of hash for UI fit
        let fp_display = if self.fingerprint.len() > 50 {
            format!("{}...", &self.fingerprint[..47])
        } else {
            self.fingerprint.clone()
        };
        lines.push(format!("Fingerprint: {}", fp_display));

        if let Some(ref cn) = self.common_name {
            lines.push(format!("Subject:     CN={}", cn));
        }
        if let Some(ref org) = self.organization {
            lines.push(format!("Org:         {}", org));
        }

        lines.push(format!("Issuer:      {}", self.issuer));
        lines.push(format!("Valid:       {} to {}", self.not_before, self.not_after));

        lines.join("\n")
    }
}

/// Messages sent from the main thread to the network thread.
#[derive(Debug)]
pub enum ToNetwork {
    /// Request to establish a connection.
    Connect(ConnectionConfig),
    /// Request to disconnect gracefully.
    Disconnect,
    /// Response to a certificate verification request.
    CertificateDecision(bool),
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
    /// Server certificate needs user verification.
    /// The main thread should display the certificate info and send back
    /// a CertificateDecision via ToNetwork.
    CertificateVerify {
        /// The server hostname being connected to.
        server: String,
        /// Certificate information to display to the user.
        cert_info: CertificateInfo,
    },
    /// A decoded video frame ready for rendering.
    /// The main thread should pass this to the window for display.
    Frame(DecodedFrame),
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
    /// Server certificate was rejected by user.
    CertificateRejected(String),
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
            Self::CertificateRejected(msg) => {
                write!(f, "Certificate rejected: {msg}. Connection aborted.")
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

    #[test]
    fn test_from_network_frame_debug() {
        let frame = DecodedFrame::new(vec![0u8; 16], 2, 2);
        let msg = FromNetwork::Frame(frame);
        let debug = format!("{:?}", msg);
        assert!(debug.contains("Frame"));
        assert!(debug.contains("DecodedFrame"));
    }

    #[test]
    fn test_parse_username_plain() {
        let (user, domain) = ConnectionConfig::parse_username("john");
        assert_eq!(user, "john");
        assert!(domain.is_none());
    }

    #[test]
    fn test_parse_username_nt_style() {
        let (user, domain) = ConnectionConfig::parse_username("CORP\\john");
        assert_eq!(user, "john");
        assert_eq!(domain, Some("CORP".to_string()));
    }

    #[test]
    fn test_parse_username_nt_style_local() {
        let (user, domain) = ConnectionConfig::parse_username(".\\admin");
        assert_eq!(user, "admin");
        assert!(domain.is_none()); // Local machine, no domain
    }

    #[test]
    fn test_parse_username_upn_style() {
        let (user, domain) = ConnectionConfig::parse_username("john@corp.example.com");
        assert_eq!(user, "john");
        assert_eq!(domain, Some("corp.example.com".to_string()));
    }

    #[test]
    fn test_parse_username_email_not_upn() {
        // Email-like but without dots in domain part - not treated as UPN
        let (user, domain) = ConnectionConfig::parse_username("john@localhost");
        assert_eq!(user, "john@localhost");
        assert!(domain.is_none());
    }

    #[test]
    fn test_from_username_nt_style() {
        let config = ConnectionConfig::from_username("server", 3389, "DOMAIN\\user");
        assert_eq!(config.username, Some("user".to_string()));
        assert_eq!(config.domain, Some("DOMAIN".to_string()));
        assert_eq!(config.host, "server");
    }

    #[test]
    fn test_from_username_upn_style() {
        let config = ConnectionConfig::from_username("server", 3389, "user@domain.com");
        assert_eq!(config.username, Some("user".to_string()));
        assert_eq!(config.domain, Some("domain.com".to_string()));
    }

    #[test]
    fn test_from_username_plain() {
        let config = ConnectionConfig::from_username("server", 3389, "admin");
        assert_eq!(config.username, Some("admin".to_string()));
        assert!(config.domain.is_none());
    }

    #[test]
    fn test_parse_username_empty() {
        let (user, domain) = ConnectionConfig::parse_username("");
        assert_eq!(user, "");
        assert!(domain.is_none());
    }

    #[test]
    fn test_parse_username_double_backslash() {
        // Double backslash edge case - split_once takes first backslash only
        // This results in domain="DOMAIN", user="\\user" which is technically
        // what the user typed after the first backslash
        let (user, domain) = ConnectionConfig::parse_username("DOMAIN\\\\user");
        assert_eq!(domain, Some("DOMAIN".to_string()));
        assert_eq!(user, "\\user"); // Preserves the second backslash
    }

    #[test]
    fn test_parse_username_at_in_nt_style() {
        // NT-style takes precedence over UPN - backslash checked first
        let (user, domain) = ConnectionConfig::parse_username("DOMAIN\\user@email.com");
        assert_eq!(domain, Some("DOMAIN".to_string()));
        assert_eq!(user, "user@email.com");
    }

    #[test]
    fn test_certificate_info_new() {
        let cert = CertificateInfo::new(
            "SHA256:AB:CD:EF",
            "Self-signed",
            "2024-01-01",
            "2025-01-01",
        );
        assert_eq!(cert.fingerprint, "SHA256:AB:CD:EF");
        assert_eq!(cert.issuer, "Self-signed");
        assert!(cert.common_name.is_none());
        assert!(cert.organization.is_none());
    }

    #[test]
    fn test_certificate_info_builder() {
        let cert = CertificateInfo::new("FP", "Issuer", "2024", "2025")
            .with_common_name("server.example.com")
            .with_organization("Example Corp");
        assert_eq!(cert.common_name, Some("server.example.com".to_string()));
        assert_eq!(cert.organization, Some("Example Corp".to_string()));
    }

    #[test]
    fn test_certificate_info_display() {
        let cert = CertificateInfo::new("SHA256:AB:CD", "CA", "2024-01-01", "2025-12-31")
            .with_common_name("example.com");
        let display = cert.display();
        assert!(display.contains("SHA256:AB:CD"));
        assert!(display.contains("CN=example.com"));
        assert!(display.contains("CA"));
        assert!(display.contains("2024-01-01"));
        assert!(display.contains("2025-12-31"));
    }

    #[test]
    fn test_connection_error_display_certificate_rejected() {
        let err = ConnectionError::CertificateRejected("user declined".to_string());
        let msg = err.to_string();
        assert!(msg.contains("Certificate rejected"));
        assert!(msg.contains("user declined"));
    }

    #[test]
    fn test_certificate_info_display_long_fingerprint() {
        // SHA-256 fingerprint with colons = 95 characters
        let long_fp = "SHA256:AB:CD:EF:12:34:56:78:90:AB:CD:EF:12:34:56:78:90:AB:CD:EF:12:34:56:78:90:AB:CD:EF:12:34:56:78";
        let cert = CertificateInfo::new(long_fp, "CA", "2024", "2025");
        let display = cert.display();
        // Should be truncated with "..."
        assert!(display.contains("..."));
        // Should not contain the full fingerprint
        assert!(!display.contains(long_fp));
        // Should contain the beginning
        assert!(display.contains("SHA256:AB:CD:EF"));
    }

    #[test]
    fn test_certificate_info_partial_eq() {
        let cert1 = CertificateInfo::new("FP1", "Issuer", "2024", "2025");
        let cert2 = CertificateInfo::new("FP1", "Issuer", "2024", "2025");
        let cert3 = CertificateInfo::new("FP2", "Issuer", "2024", "2025");
        assert_eq!(cert1, cert2);
        assert_ne!(cert1, cert3);
    }
}
