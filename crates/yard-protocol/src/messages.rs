//! Inter-thread message types for YARD.
//!
//! Defines the message types used for communication between the main thread
//! (calloop event loop) and the network thread (Tokio runtime).

use std::fmt;

pub use yard_video::{DecodedFrame, VideoCodec};

/// Information about a monitor for RDP layout reporting (Story 3.2).
///
/// This struct is used to report the local monitor layout to the RDP server
/// via the DISPLAYCONTROL channel, allowing the server to configure the
/// remote desktop to span multiple monitors.
#[derive(Debug, Clone)]
pub struct RdpMonitorInfo {
    /// Position X offset in the virtual desktop coordinate space.
    pub x: i32,
    /// Position Y offset in the virtual desktop coordinate space.
    pub y: i32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Whether this is the primary monitor (at position 0,0).
    pub is_primary: bool,
    /// Desktop scale factor as percentage (100 = 100%, 200 = 200%).
    /// Valid range: 100-500.
    pub scale_percent: u32,
    /// Physical width in millimeters (optional).
    pub physical_width_mm: Option<u32>,
    /// Physical height in millimeters (optional).
    pub physical_height_mm: Option<u32>,
}

impl RdpMonitorInfo {
    /// Creates a new RdpMonitorInfo for a monitor.
    pub fn new(x: i32, y: i32, width: u32, height: u32, is_primary: bool) -> Self {
        Self {
            x,
            y,
            width,
            height,
            is_primary,
            scale_percent: 100,
            physical_width_mm: None,
            physical_height_mm: None,
        }
    }

    /// Sets the desktop scale factor.
    pub fn with_scale(mut self, scale_percent: u32) -> Self {
        self.scale_percent = scale_percent.clamp(100, 500);
        self
    }

    /// Sets the physical dimensions in millimeters.
    pub fn with_physical_size(mut self, width_mm: u32, height_mm: u32) -> Self {
        self.physical_width_mm = Some(width_mm);
        self.physical_height_mm = Some(height_mm);
        self
    }
}

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
    /// Monitor layout to report to the server (Story 3.2).
    /// If provided, DISPLAYCONTROL channel will be used to report the layout.
    pub monitor_layout: Option<Vec<RdpMonitorInfo>>,
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
            .field("monitor_layout", &self.monitor_layout)
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
            monitor_layout: None,
        }
    }

    /// Sets the monitor layout for multi-monitor support (Story 3.2).
    ///
    /// When provided, the DISPLAYCONTROL channel will be used to report the
    /// local monitor layout to the server, allowing the remote desktop to
    /// span all monitors.
    pub fn with_monitor_layout(mut self, monitors: Vec<RdpMonitorInfo>) -> Self {
        self.monitor_layout = Some(monitors);
        self
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
        if let Some((user, domain)) = input.rsplit_once('@')
            && domain.contains('.')
        {
            return (user.to_string(), Some(domain.to_string()));
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
        lines.push(format!(
            "Valid:       {} to {}",
            self.not_before, self.not_after
        ));

        lines.join("\n")
    }
}

/// Mouse button types for RDP input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    /// Left mouse button (primary).
    Left,
    /// Right mouse button (secondary/context).
    Right,
    /// Middle mouse button (wheel click).
    Middle,
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
    /// Keyboard input event to send to the remote server.
    ///
    /// The scancode is an RDP scancode (not a keysym or evdev keycode).
    /// Standard keys use scancodes 0x00-0x7F.
    /// Extended keys (arrows, Home, End, etc.) use 0xE0xx format where
    /// the high byte is 0xE0 and the low byte is the scancode.
    KeyboardInput {
        /// RDP scancode for the key.
        scancode: u16,
        /// True for key press, false for key release.
        pressed: bool,
    },
    /// Unicode character input for international keyboard support (Story 2.9).
    ///
    /// Used when the user types a character that may not have a direct scancode
    /// mapping, such as accented characters (é, ü, ñ) or characters from
    /// non-US keyboard layouts (AZERTY, QWERTZ, etc.).
    UnicodeInput {
        /// The Unicode character to send.
        character: char,
        /// True for key press, false for key release.
        pressed: bool,
    },
    /// Mouse movement event to send to the remote server.
    ///
    /// Coordinates are absolute positions in the remote desktop coordinate space.
    MouseMove {
        /// X position in remote desktop pixels.
        x: u16,
        /// Y position in remote desktop pixels.
        y: u16,
    },
    /// Mouse button event to send to the remote server.
    MouseButton {
        /// Which button was pressed/released.
        button: MouseButton,
        /// True for button press, false for button release.
        pressed: bool,
        /// X position at time of click (for click accuracy).
        x: u16,
        /// Y position at time of click (for click accuracy).
        y: u16,
    },
    /// Mouse wheel scroll event to send to the remote server.
    ///
    /// Delta is in wheel rotation units (positive = up/left, negative = down/right).
    MouseWheel {
        /// True for horizontal scroll, false for vertical scroll.
        horizontal: bool,
        /// Scroll delta (positive = up/left, negative = down/right).
        /// Standard wheel delta is 120 units per notch.
        delta: i16,
        /// X position at time of scroll.
        x: u16,
        /// Y position at time of scroll.
        y: u16,
    },
}

/// Desktop size information from the RDP server.
#[derive(Debug, Clone, Copy)]
pub struct DesktopSize {
    /// Width in pixels.
    pub width: u16,
    /// Height in pixels.
    pub height: u16,
}

impl DesktopSize {
    /// Creates a new DesktopSize.
    pub fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }
}

/// Messages sent from the network thread to the main thread.
#[derive(Debug)]
pub enum FromNetwork {
    /// Connection attempt is in progress.
    Connecting,
    /// Connection established successfully with desktop dimensions.
    Connected(DesktopSize),
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
    /// Multi-monitor layout was accepted by the server (Story 3.2).
    MonitorLayoutAccepted {
        /// The final desktop size spanning all monitors.
        desktop_size: DesktopSize,
    },
    /// Server does not support multi-monitor (Story 3.2).
    /// The client should fall back to single-monitor mode.
    MultiMonitorNotSupported,
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
    fn test_mouse_button_enum() {
        // Verify MouseButton variants exist and are Debug-able
        let left = MouseButton::Left;
        let right = MouseButton::Right;
        let middle = MouseButton::Middle;

        assert_eq!(format!("{:?}", left), "Left");
        assert_eq!(format!("{:?}", right), "Right");
        assert_eq!(format!("{:?}", middle), "Middle");
    }

    #[test]
    fn test_mouse_button_equality() {
        assert_eq!(MouseButton::Left, MouseButton::Left);
        assert_ne!(MouseButton::Left, MouseButton::Right);
        assert_ne!(MouseButton::Right, MouseButton::Middle);
    }

    #[test]
    fn test_to_network_mouse_move_debug() {
        let msg = ToNetwork::MouseMove { x: 100, y: 200 };
        let debug = format!("{:?}", msg);
        assert!(debug.contains("MouseMove"));
        assert!(debug.contains("100"));
        assert!(debug.contains("200"));
    }

    #[test]
    fn test_to_network_mouse_button_debug() {
        let msg = ToNetwork::MouseButton {
            button: MouseButton::Left,
            pressed: true,
            x: 50,
            y: 75,
        };
        let debug = format!("{:?}", msg);
        assert!(debug.contains("MouseButton"));
        assert!(debug.contains("Left"));
        assert!(debug.contains("true"));
    }

    #[test]
    fn test_to_network_mouse_wheel_debug() {
        let msg = ToNetwork::MouseWheel {
            horizontal: false,
            delta: -120,
            x: 100,
            y: 100,
        };
        let debug = format!("{:?}", msg);
        assert!(debug.contains("MouseWheel"));
        assert!(debug.contains("-120"));
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
        let cert =
            CertificateInfo::new("SHA256:AB:CD:EF", "Self-signed", "2024-01-01", "2025-01-01");
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

    // Story 3.2: RdpMonitorInfo tests
    #[test]
    fn test_rdp_monitor_info_new() {
        let monitor = RdpMonitorInfo::new(0, 0, 1920, 1080, true);
        assert_eq!(monitor.x, 0);
        assert_eq!(monitor.y, 0);
        assert_eq!(monitor.width, 1920);
        assert_eq!(monitor.height, 1080);
        assert!(monitor.is_primary);
        assert_eq!(monitor.scale_percent, 100);
        assert!(monitor.physical_width_mm.is_none());
        assert!(monitor.physical_height_mm.is_none());
    }

    #[test]
    fn test_rdp_monitor_info_secondary() {
        let monitor = RdpMonitorInfo::new(1920, 0, 1920, 1080, false);
        assert_eq!(monitor.x, 1920);
        assert_eq!(monitor.y, 0);
        assert!(!monitor.is_primary);
    }

    #[test]
    fn test_rdp_monitor_info_with_scale() {
        let monitor = RdpMonitorInfo::new(0, 0, 3840, 2160, true).with_scale(200);
        assert_eq!(monitor.scale_percent, 200);
    }

    #[test]
    fn test_rdp_monitor_info_scale_clamped() {
        // Scale should be clamped to 100-500
        let monitor_low = RdpMonitorInfo::new(0, 0, 1920, 1080, true).with_scale(50);
        let monitor_high = RdpMonitorInfo::new(0, 0, 1920, 1080, true).with_scale(600);
        assert_eq!(monitor_low.scale_percent, 100);
        assert_eq!(monitor_high.scale_percent, 500);
    }

    #[test]
    fn test_rdp_monitor_info_with_physical_size() {
        let monitor = RdpMonitorInfo::new(0, 0, 1920, 1080, true).with_physical_size(527, 296);
        assert_eq!(monitor.physical_width_mm, Some(527));
        assert_eq!(monitor.physical_height_mm, Some(296));
    }

    #[test]
    fn test_connection_config_with_monitor_layout() {
        let monitors = vec![
            RdpMonitorInfo::new(0, 0, 1920, 1080, true),
            RdpMonitorInfo::new(1920, 0, 1920, 1080, false),
        ];
        let config = ConnectionConfig::new("host", 3389).with_monitor_layout(monitors);
        assert!(config.monitor_layout.is_some());
        assert_eq!(config.monitor_layout.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_connection_config_debug_includes_monitor_layout() {
        let monitors = vec![RdpMonitorInfo::new(0, 0, 1920, 1080, true)];
        let config = ConnectionConfig::new("host", 3389)
            .with_monitor_layout(monitors)
            .with_password("secret");
        let debug = format!("{:?}", config);
        assert!(debug.contains("monitor_layout"));
        assert!(debug.contains("1920"));
        // Password should still be redacted
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("secret"));
    }

    #[test]
    fn test_from_network_monitor_layout_accepted() {
        let msg = FromNetwork::MonitorLayoutAccepted {
            desktop_size: DesktopSize::new(3840, 1080),
        };
        let debug = format!("{:?}", msg);
        assert!(debug.contains("MonitorLayoutAccepted"));
        assert!(debug.contains("3840"));
    }

    #[test]
    fn test_from_network_multi_monitor_not_supported() {
        let msg = FromNetwork::MultiMonitorNotSupported;
        let debug = format!("{:?}", msg);
        assert!(debug.contains("MultiMonitorNotSupported"));
    }
}
