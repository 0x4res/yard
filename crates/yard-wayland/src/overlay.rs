//! Overlay UI component for YARD (Story 6.1).
//!
//! This module implements a minimal overlay that appears when the user hovers
//! at the top edge of the screen. The overlay provides client controls like
//! disconnect, connection status, and session information.
//!
//! # Implementation
//!
//! The overlay uses wlr-layer-shell when available (Tier 1 compositors like
//! Hyprland and Sway) to create a surface that renders above the main window.
//! On compositors without layer shell support, the overlay functionality is
//! disabled.
//!
//! # State Machine
//!
//! The overlay follows a state machine:
//! - `Hidden`: Overlay is not visible
//! - `Visible`: Overlay is shown at the top of the screen
//! - `Hiding`: Overlay is scheduled to hide after a delay
//!
//! Transitions:
//! - Hidden → Visible: Mouse enters hover zone (y < threshold)
//! - Visible → Hiding: Mouse leaves hover zone
//! - Hiding → Hidden: Hide delay expires
//! - Hiding → Visible: Mouse re-enters hover zone before delay expires

#[cfg(target_os = "linux")]
mod linux {
    use std::time::{Duration, Instant};

    /// Default threshold in pixels from the top edge to trigger overlay.
    pub const DEFAULT_HOVER_THRESHOLD: f64 = 10.0;

    /// Default delay before hiding the overlay after mouse leaves.
    pub const DEFAULT_HIDE_DELAY: Duration = Duration::from_millis(300);

    /// Overlay height in pixels.
    pub const OVERLAY_HEIGHT: u32 = 40;

    /// Overlay background color (semi-transparent black).
    /// Format: BGRA (for Wayland's ARGB8888/XRGB8888)
    pub const OVERLAY_BG_COLOR: [u8; 4] = [0x00, 0x00, 0x00, 0xB0]; // ~70% opacity black

    // Story 6.2: Status indicator colors (BGRA format)
    /// Green color for connected status.
    pub const COLOR_GREEN: [u8; 4] = [0x64, 0xFF, 0x00, 0xFF]; // Bright green
    /// Yellow color for reconnecting status.
    pub const COLOR_YELLOW: [u8; 4] = [0x00, 0xC8, 0xFF, 0xFF]; // Yellow (BGRA)
    /// Red color for disconnected status.
    pub const COLOR_RED: [u8; 4] = [0x64, 0x64, 0xFF, 0xFF]; // Red (BGRA)
    /// White color for text.
    pub const COLOR_WHITE: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];
    /// Gray color for secondary text.
    pub const COLOR_GRAY: [u8; 4] = [0xAA, 0xAA, 0xAA, 0xFF];

    // Story 6.2: Font constants
    /// Font character width in pixels.
    pub const FONT_WIDTH: u32 = 6;
    /// Font character height in pixels.
    pub const FONT_HEIGHT: u32 = 10;

    /// Connection status for overlay display (Story 6.2).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub enum ConnectionStatus {
        /// Disconnected from the remote server (default state before connection).
        #[default]
        Disconnected,
        /// Connected to the remote server.
        Connected,
        /// Attempting to reconnect after connection loss.
        Reconnecting,
    }

    /// Content to display in the overlay (Story 6.2).
    ///
    /// This struct holds all the information needed to render the overlay content,
    /// including connection status, server name, latency, and session duration.
    #[derive(Debug, Clone, Default)]
    pub struct OverlayContent {
        /// Current connection status.
        pub status: ConnectionStatus,
        /// Server name/hostname (shown when connected).
        pub server_name: Option<String>,
        /// Round-trip time in milliseconds (shown when connected).
        pub rtt_ms: Option<u32>,
        /// Session duration (shown when connected).
        pub session_duration: Option<Duration>,
        /// Reconnection attempt info: (current_attempt, max_attempts).
        pub reconnect_attempt: Option<(u32, u32)>,
        /// Reason for disconnection (shown when disconnected).
        pub disconnect_reason: Option<String>,
    }

    impl OverlayContent {
        /// Creates content for a connected state.
        pub fn connected(server_name: impl Into<String>) -> Self {
            Self {
                status: ConnectionStatus::Connected,
                server_name: Some(server_name.into()),
                ..Default::default()
            }
        }

        /// Creates content for a reconnecting state.
        pub fn reconnecting(attempt: u32, max_attempts: u32, last_error: Option<String>) -> Self {
            Self {
                status: ConnectionStatus::Reconnecting,
                reconnect_attempt: Some((attempt, max_attempts)),
                disconnect_reason: last_error,
                ..Default::default()
            }
        }

        /// Creates content for a disconnected state.
        pub fn disconnected(reason: impl Into<String>) -> Self {
            Self {
                status: ConnectionStatus::Disconnected,
                disconnect_reason: Some(reason.into()),
                ..Default::default()
            }
        }

        /// Sets the RTT (round-trip time) in milliseconds.
        pub fn with_rtt(mut self, rtt_ms: u32) -> Self {
            self.rtt_ms = Some(rtt_ms);
            self
        }

        /// Sets the session duration.
        pub fn with_duration(mut self, duration: Duration) -> Self {
            self.session_duration = Some(duration);
            self
        }

        /// Formats the content as a display string.
        pub fn format_status_text(&self) -> String {
            match self.status {
                ConnectionStatus::Connected => {
                    let mut parts = Vec::new();

                    // Server name
                    if let Some(ref name) = self.server_name {
                        parts.push(format!("Connected to {}", name));
                    } else {
                        parts.push("Connected".to_string());
                    }

                    // RTT
                    if let Some(rtt) = self.rtt_ms {
                        parts.push(format!("{}ms", rtt));
                    }

                    // Duration
                    if let Some(dur) = self.session_duration {
                        parts.push(format_duration(dur));
                    }

                    parts.join(" | ")
                }
                ConnectionStatus::Reconnecting => {
                    let mut text = "Reconnecting...".to_string();
                    if let Some((current, max)) = self.reconnect_attempt {
                        text.push_str(&format!(" ({}/{})", current, max));
                    }
                    if let Some(ref reason) = self.disconnect_reason {
                        text.push_str(&format!(" | {}", reason));
                    }
                    text
                }
                ConnectionStatus::Disconnected => {
                    if let Some(ref reason) = self.disconnect_reason {
                        format!("Disconnected: {}", reason)
                    } else {
                        "Disconnected".to_string()
                    }
                }
            }
        }
    }

    /// Formats a duration as HH:MM:SS.
    pub fn format_duration(duration: Duration) -> String {
        let total_secs = duration.as_secs();
        let hours = total_secs / 3600;
        let minutes = (total_secs % 3600) / 60;
        let seconds = total_secs % 60;
        format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
    }

    // =========================================================================
    // Story 6.2: Bitmap Font Rendering
    // =========================================================================
    //
    // Simple 6x10 bitmap font for overlay text rendering.
    // Covers ASCII 32-126 (printable characters).
    // Each character is stored as 10 bytes, where each byte represents one row
    // and the 6 least significant bits represent the pixels (1 = on, 0 = off).

    /// 6x10 bitmap font data for ASCII 32-126 (95 characters).
    /// Each character is 10 bytes (one per row), with 6 bits per row.
    #[rustfmt::skip]
    const FONT_DATA: &[u8] = &[
        // Space (32)
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        // ! (33)
        0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04, 0x00, 0x00,
        // " (34)
        0x0A, 0x0A, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        // # (35)
        0x0A, 0x0A, 0x1F, 0x0A, 0x1F, 0x0A, 0x0A, 0x00, 0x00, 0x00,
        // $ (36)
        0x04, 0x0F, 0x14, 0x0E, 0x05, 0x1E, 0x04, 0x00, 0x00, 0x00,
        // % (37)
        0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03, 0x00, 0x00, 0x00,
        // & (38)
        0x08, 0x14, 0x14, 0x08, 0x15, 0x12, 0x0D, 0x00, 0x00, 0x00,
        // ' (39)
        0x04, 0x04, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        // ( (40)
        0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02, 0x00, 0x00, 0x00,
        // ) (41)
        0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08, 0x00, 0x00, 0x00,
        // * (42)
        0x00, 0x04, 0x15, 0x0E, 0x15, 0x04, 0x00, 0x00, 0x00, 0x00,
        // + (43)
        0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00, 0x00, 0x00, 0x00,
        // , (44)
        0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x04, 0x08, 0x00, 0x00,
        // - (45)
        0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        // . (46)
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00,
        // / (47)
        0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10, 0x00, 0x00, 0x00,
        // 0 (48)
        0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E, 0x00, 0x00, 0x00,
        // 1 (49)
        0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E, 0x00, 0x00, 0x00,
        // 2 (50)
        0x0E, 0x11, 0x01, 0x06, 0x08, 0x10, 0x1F, 0x00, 0x00, 0x00,
        // 3 (51)
        0x0E, 0x11, 0x01, 0x06, 0x01, 0x11, 0x0E, 0x00, 0x00, 0x00,
        // 4 (52)
        0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02, 0x00, 0x00, 0x00,
        // 5 (53)
        0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E, 0x00, 0x00, 0x00,
        // 6 (54)
        0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E, 0x00, 0x00, 0x00,
        // 7 (55)
        0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08, 0x00, 0x00, 0x00,
        // 8 (56)
        0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E, 0x00, 0x00, 0x00,
        // 9 (57)
        0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C, 0x00, 0x00, 0x00,
        // : (58)
        0x00, 0x00, 0x04, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00,
        // ; (59)
        0x00, 0x00, 0x04, 0x00, 0x00, 0x04, 0x04, 0x08, 0x00, 0x00,
        // < (60)
        0x02, 0x04, 0x08, 0x10, 0x08, 0x04, 0x02, 0x00, 0x00, 0x00,
        // = (61)
        0x00, 0x00, 0x1F, 0x00, 0x1F, 0x00, 0x00, 0x00, 0x00, 0x00,
        // > (62)
        0x08, 0x04, 0x02, 0x01, 0x02, 0x04, 0x08, 0x00, 0x00, 0x00,
        // ? (63)
        0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04, 0x00, 0x00, 0x00,
        // @ (64)
        0x0E, 0x11, 0x17, 0x15, 0x17, 0x10, 0x0E, 0x00, 0x00, 0x00,
        // A (65)
        0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11, 0x00, 0x00, 0x00,
        // B (66)
        0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E, 0x00, 0x00, 0x00,
        // C (67)
        0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E, 0x00, 0x00, 0x00,
        // D (68)
        0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E, 0x00, 0x00, 0x00,
        // E (69)
        0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F, 0x00, 0x00, 0x00,
        // F (70)
        0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10, 0x00, 0x00, 0x00,
        // G (71)
        0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F, 0x00, 0x00, 0x00,
        // H (72)
        0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11, 0x00, 0x00, 0x00,
        // I (73)
        0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E, 0x00, 0x00, 0x00,
        // J (74)
        0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C, 0x00, 0x00, 0x00,
        // K (75)
        0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11, 0x00, 0x00, 0x00,
        // L (76)
        0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F, 0x00, 0x00, 0x00,
        // M (77)
        0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11, 0x00, 0x00, 0x00,
        // N (78)
        0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11, 0x00, 0x00, 0x00,
        // O (79)
        0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E, 0x00, 0x00, 0x00,
        // P (80)
        0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10, 0x00, 0x00, 0x00,
        // Q (81)
        0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D, 0x00, 0x00, 0x00,
        // R (82)
        0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11, 0x00, 0x00, 0x00,
        // S (83)
        0x0E, 0x11, 0x10, 0x0E, 0x01, 0x11, 0x0E, 0x00, 0x00, 0x00,
        // T (84)
        0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x00, 0x00,
        // U (85)
        0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E, 0x00, 0x00, 0x00,
        // V (86)
        0x11, 0x11, 0x11, 0x11, 0x0A, 0x0A, 0x04, 0x00, 0x00, 0x00,
        // W (87)
        0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11, 0x00, 0x00, 0x00,
        // X (88)
        0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11, 0x00, 0x00, 0x00,
        // Y (89)
        0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04, 0x00, 0x00, 0x00,
        // Z (90)
        0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F, 0x00, 0x00, 0x00,
        // [ (91)
        0x0E, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0E, 0x00, 0x00, 0x00,
        // \ (92)
        0x10, 0x10, 0x08, 0x04, 0x02, 0x01, 0x01, 0x00, 0x00, 0x00,
        // ] (93)
        0x0E, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0E, 0x00, 0x00, 0x00,
        // ^ (94)
        0x04, 0x0A, 0x11, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        // _ (95)
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00,
        // ` (96)
        0x08, 0x04, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        // a (97)
        0x00, 0x00, 0x0E, 0x01, 0x0F, 0x11, 0x0F, 0x00, 0x00, 0x00,
        // b (98)
        0x10, 0x10, 0x1E, 0x11, 0x11, 0x11, 0x1E, 0x00, 0x00, 0x00,
        // c (99)
        0x00, 0x00, 0x0F, 0x10, 0x10, 0x10, 0x0F, 0x00, 0x00, 0x00,
        // d (100)
        0x01, 0x01, 0x0F, 0x11, 0x11, 0x11, 0x0F, 0x00, 0x00, 0x00,
        // e (101)
        0x00, 0x00, 0x0E, 0x11, 0x1F, 0x10, 0x0E, 0x00, 0x00, 0x00,
        // f (102)
        0x06, 0x08, 0x1E, 0x08, 0x08, 0x08, 0x08, 0x00, 0x00, 0x00,
        // g (103)
        0x00, 0x00, 0x0F, 0x11, 0x11, 0x0F, 0x01, 0x0E, 0x00, 0x00,
        // h (104)
        0x10, 0x10, 0x1E, 0x11, 0x11, 0x11, 0x11, 0x00, 0x00, 0x00,
        // i (105)
        0x04, 0x00, 0x0C, 0x04, 0x04, 0x04, 0x0E, 0x00, 0x00, 0x00,
        // j (106)
        0x02, 0x00, 0x06, 0x02, 0x02, 0x02, 0x12, 0x0C, 0x00, 0x00,
        // k (107)
        0x10, 0x10, 0x12, 0x14, 0x18, 0x14, 0x12, 0x00, 0x00, 0x00,
        // l (108)
        0x0C, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E, 0x00, 0x00, 0x00,
        // m (109)
        0x00, 0x00, 0x1A, 0x15, 0x15, 0x15, 0x15, 0x00, 0x00, 0x00,
        // n (110)
        0x00, 0x00, 0x1E, 0x11, 0x11, 0x11, 0x11, 0x00, 0x00, 0x00,
        // o (111)
        0x00, 0x00, 0x0E, 0x11, 0x11, 0x11, 0x0E, 0x00, 0x00, 0x00,
        // p (112)
        0x00, 0x00, 0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x00, 0x00,
        // q (113)
        0x00, 0x00, 0x0F, 0x11, 0x11, 0x0F, 0x01, 0x01, 0x00, 0x00,
        // r (114)
        0x00, 0x00, 0x16, 0x19, 0x10, 0x10, 0x10, 0x00, 0x00, 0x00,
        // s (115)
        0x00, 0x00, 0x0F, 0x10, 0x0E, 0x01, 0x1E, 0x00, 0x00, 0x00,
        // t (116)
        0x08, 0x08, 0x1E, 0x08, 0x08, 0x08, 0x06, 0x00, 0x00, 0x00,
        // u (117)
        0x00, 0x00, 0x11, 0x11, 0x11, 0x11, 0x0F, 0x00, 0x00, 0x00,
        // v (118)
        0x00, 0x00, 0x11, 0x11, 0x11, 0x0A, 0x04, 0x00, 0x00, 0x00,
        // w (119)
        0x00, 0x00, 0x11, 0x11, 0x15, 0x15, 0x0A, 0x00, 0x00, 0x00,
        // x (120)
        0x00, 0x00, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x00, 0x00, 0x00,
        // y (121)
        0x00, 0x00, 0x11, 0x11, 0x11, 0x0F, 0x01, 0x0E, 0x00, 0x00,
        // z (122)
        0x00, 0x00, 0x1F, 0x02, 0x04, 0x08, 0x1F, 0x00, 0x00, 0x00,
        // { (123)
        0x02, 0x04, 0x04, 0x08, 0x04, 0x04, 0x02, 0x00, 0x00, 0x00,
        // | (124)
        0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x00, 0x00,
        // } (125)
        0x08, 0x04, 0x04, 0x02, 0x04, 0x04, 0x08, 0x00, 0x00, 0x00,
        // ~ (126)
        0x00, 0x08, 0x15, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    /// Renders a single character to the buffer at the specified position.
    ///
    /// # Arguments
    /// * `buffer` - The BGRA buffer to render to
    /// * `buffer_width` - Width of the buffer in pixels
    /// * `x` - X position in pixels
    /// * `y` - Y position in pixels
    /// * `ch` - Character to render (ASCII 32-126)
    /// * `color` - BGRA color for the character
    fn render_char(buffer: &mut [u8], buffer_width: u32, x: u32, y: u32, ch: char, color: [u8; 4]) {
        // Only render printable ASCII
        if ch < ' ' || ch > '~' {
            return;
        }

        let glyph_idx = (ch as usize) - 32;
        let glyph_offset = glyph_idx * (FONT_HEIGHT as usize);

        // Check if glyph data exists
        if glyph_offset + (FONT_HEIGHT as usize) > FONT_DATA.len() {
            return;
        }

        for row in 0..FONT_HEIGHT {
            let row_data = FONT_DATA[glyph_offset + row as usize];
            for col in 0..FONT_WIDTH {
                // Check if this pixel should be drawn (bit is set)
                // Bits are stored with MSB on the left
                let bit_mask = 1 << (5 - col);
                if row_data & bit_mask != 0 {
                    let px = x + col;
                    let py = y + row;

                    // Bounds check
                    if px < buffer_width {
                        let buffer_height = buffer.len() as u32 / (buffer_width * 4);
                        if py < buffer_height {
                            let offset = ((py * buffer_width + px) * 4) as usize;
                            if offset + 4 <= buffer.len() {
                                buffer[offset..offset + 4].copy_from_slice(&color);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Renders a text string to the buffer at the specified position.
    ///
    /// # Arguments
    /// * `buffer` - The BGRA buffer to render to
    /// * `buffer_width` - Width of the buffer in pixels
    /// * `x` - X position in pixels
    /// * `y` - Y position in pixels
    /// * `text` - Text string to render
    /// * `color` - BGRA color for the text
    pub fn render_text(buffer: &mut [u8], buffer_width: u32, x: u32, y: u32, text: &str, color: [u8; 4]) {
        for (i, ch) in text.chars().enumerate() {
            let char_x = x + (i as u32 * FONT_WIDTH);
            render_char(buffer, buffer_width, char_x, y, ch, color);
        }
    }

    /// Renders a filled circle (status indicator) to the buffer.
    ///
    /// # Arguments
    /// * `buffer` - The BGRA buffer to render to
    /// * `buffer_width` - Width of the buffer in pixels
    /// * `cx` - Center X position
    /// * `cy` - Center Y position
    /// * `radius` - Radius in pixels
    /// * `color` - BGRA color for the circle
    pub fn render_circle(buffer: &mut [u8], buffer_width: u32, cx: u32, cy: u32, radius: u32, color: [u8; 4]) {
        let buffer_height = buffer.len() as u32 / (buffer_width * 4);
        let r2 = (radius * radius) as i32;

        for dy in 0..=radius {
            for dx in 0..=radius {
                let dist2 = (dx * dx + dy * dy) as i32;
                if dist2 <= r2 {
                    // Draw all four quadrants
                    let points = [
                        (cx + dx, cy + dy),
                        (cx.saturating_sub(dx), cy + dy),
                        (cx + dx, cy.saturating_sub(dy)),
                        (cx.saturating_sub(dx), cy.saturating_sub(dy)),
                    ];

                    for (px, py) in points {
                        if px < buffer_width && py < buffer_height {
                            let offset = ((py * buffer_width + px) * 4) as usize;
                            if offset + 4 <= buffer.len() {
                                buffer[offset..offset + 4].copy_from_slice(&color);
                            }
                        }
                    }
                }
            }
        }
    }

    /// State of the overlay visibility.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum OverlayState {
        /// Overlay is not visible.
        Hidden,
        /// Overlay is visible and active.
        Visible,
        /// Overlay is scheduled to hide after a delay.
        /// Contains the instant when the hide was triggered.
        Hiding(Instant),
    }

    impl Default for OverlayState {
        fn default() -> Self {
            Self::Hidden
        }
    }

    /// Configuration for the overlay behavior.
    #[derive(Debug, Clone)]
    pub struct OverlayConfig {
        /// Pixels from top edge to trigger overlay appearance.
        pub hover_threshold: f64,
        /// Delay before hiding after mouse leaves.
        pub hide_delay: Duration,
        /// Height of the overlay bar in pixels.
        pub height: u32,
    }

    impl Default for OverlayConfig {
        fn default() -> Self {
            Self {
                hover_threshold: DEFAULT_HOVER_THRESHOLD,
                hide_delay: DEFAULT_HIDE_DELAY,
                height: OVERLAY_HEIGHT,
            }
        }
    }

    /// Overlay controller that manages visibility state and timing.
    ///
    /// This struct is independent of the rendering mechanism (layer shell vs subsurface)
    /// and handles the state machine logic for showing/hiding the overlay.
    #[derive(Debug)]
    pub struct OverlayController {
        /// Current overlay state.
        state: OverlayState,
        /// Configuration for overlay behavior.
        config: OverlayConfig,
        /// Monitor ID where the overlay should appear (for multi-monitor support).
        /// None means overlay is not targeting any specific monitor.
        active_monitor: Option<u32>,
    }

    impl OverlayController {
        /// Creates a new overlay controller with default configuration.
        pub fn new() -> Self {
            Self {
                state: OverlayState::Hidden,
                config: OverlayConfig::default(),
                active_monitor: None,
            }
        }

        /// Creates a new overlay controller with custom configuration.
        pub fn with_config(config: OverlayConfig) -> Self {
            Self {
                state: OverlayState::Hidden,
                config,
                active_monitor: None,
            }
        }

        /// Returns the current overlay state.
        pub fn state(&self) -> OverlayState {
            self.state
        }

        /// Returns the overlay configuration.
        pub fn config(&self) -> &OverlayConfig {
            &self.config
        }

        /// Returns true if the overlay is currently visible.
        pub fn is_visible(&self) -> bool {
            matches!(self.state, OverlayState::Visible | OverlayState::Hiding(_))
        }

        /// Returns true if the overlay is in the hiding state.
        pub fn is_hiding(&self) -> bool {
            matches!(self.state, OverlayState::Hiding(_))
        }

        /// Returns the monitor ID where the overlay is active.
        pub fn active_monitor(&self) -> Option<u32> {
            self.active_monitor
        }

        /// Check if pointer position should trigger overlay (y < threshold).
        ///
        /// Call this on every pointer motion event.
        ///
        /// # Arguments
        /// * `y` - Pointer Y position in surface-local coordinates
        /// * `monitor_id` - ID of the monitor where the pointer is
        ///
        /// # Returns
        /// `true` if the overlay state changed and needs to be rendered/updated
        pub fn check_pointer_position(&mut self, y: f64, monitor_id: u32) -> bool {
            let in_hover_zone = y < self.config.hover_threshold;
            let previous_visible = self.is_visible();

            if in_hover_zone {
                // Pointer is in hover zone - show overlay on this monitor
                if self.state != OverlayState::Visible || self.active_monitor != Some(monitor_id) {
                    self.state = OverlayState::Visible;
                    self.active_monitor = Some(monitor_id);
                    return true;
                }
            } else if self.is_visible() && self.active_monitor == Some(monitor_id) {
                // Pointer left hover zone on the active monitor - start hide timer
                if !self.is_hiding() {
                    self.state = OverlayState::Hiding(Instant::now());
                    return true;
                }
            }

            // Return true if visibility changed from hidden to visible
            !previous_visible && self.is_visible()
        }

        /// Called when pointer enters a different monitor.
        ///
        /// If the overlay was showing on a different monitor, transition to that monitor.
        ///
        /// # Arguments
        /// * `y` - Pointer Y position on the new monitor
        /// * `monitor_id` - ID of the monitor the pointer entered
        ///
        /// # Returns
        /// Tuple of (old_monitor_id, should_show_on_new) for handling surface updates
        pub fn pointer_entered_monitor(&mut self, y: f64, monitor_id: u32) -> (Option<u32>, bool) {
            let old_monitor = self.active_monitor;
            let in_hover_zone = y < self.config.hover_threshold;

            if in_hover_zone {
                self.state = OverlayState::Visible;
                self.active_monitor = Some(monitor_id);
                (old_monitor, true)
            } else {
                // Not in hover zone on new monitor - hide if was showing
                if self.is_visible() {
                    self.state = OverlayState::Hidden;
                    self.active_monitor = None;
                }
                (old_monitor, false)
            }
        }

        /// Updates the hide timer state.
        ///
        /// Call this periodically (e.g., on every event loop iteration) to check
        /// if the hide delay has expired.
        ///
        /// # Returns
        /// `true` if the overlay should now be hidden (delay expired)
        pub fn update_hide_timer(&mut self) -> bool {
            if let OverlayState::Hiding(start) = self.state {
                if start.elapsed() >= self.config.hide_delay {
                    self.state = OverlayState::Hidden;
                    self.active_monitor = None;
                    return true;
                }
            }
            false
        }

        /// Forces the overlay to hide immediately.
        pub fn hide(&mut self) {
            self.state = OverlayState::Hidden;
            self.active_monitor = None;
        }

        /// Forces the overlay to show on a specific monitor.
        pub fn show(&mut self, monitor_id: u32) {
            self.state = OverlayState::Visible;
            self.active_monitor = Some(monitor_id);
        }

        /// Resets the overlay state (e.g., when disconnecting).
        pub fn reset(&mut self) {
            self.state = OverlayState::Hidden;
            self.active_monitor = None;
        }
    }

    impl Default for OverlayController {
        fn default() -> Self {
            Self::new()
        }
    }

    /// Generates a solid-color BGRA buffer for the overlay background.
    ///
    /// # Arguments
    /// * `width` - Buffer width in pixels
    /// * `height` - Buffer height in pixels
    /// * `color` - BGRA color (4 bytes)
    ///
    /// # Returns
    /// Vec containing the pixel data in BGRA format
    pub fn generate_overlay_buffer(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        let pixel_count = (width * height) as usize;
        let mut buffer = Vec::with_capacity(pixel_count * 4);

        for _ in 0..pixel_count {
            buffer.extend_from_slice(&color);
        }

        buffer
    }

    /// Generates an overlay buffer with content (Story 6.2).
    ///
    /// Renders the background color, status indicator, and status text.
    ///
    /// # Arguments
    /// * `width` - Buffer width in pixels
    /// * `height` - Buffer height in pixels
    /// * `bg_color` - BGRA background color
    /// * `content` - Optional overlay content to render
    ///
    /// # Returns
    /// Vec containing the pixel data in BGRA format
    pub fn generate_overlay_buffer_with_content(
        width: u32,
        height: u32,
        bg_color: [u8; 4],
        content: Option<&OverlayContent>,
    ) -> Vec<u8> {
        // Start with background
        let mut buffer = generate_overlay_buffer(width, height, bg_color);

        // If no content, return just the background
        let Some(content) = content else {
            return buffer;
        };

        // Calculate vertical center for text
        let text_y = (height.saturating_sub(FONT_HEIGHT)) / 2;
        let indicator_radius = 5;
        let indicator_y = height / 2;
        let left_margin = 12;

        // Render status indicator circle
        let indicator_color = match content.status {
            ConnectionStatus::Connected => COLOR_GREEN,
            ConnectionStatus::Reconnecting => COLOR_YELLOW,
            ConnectionStatus::Disconnected => COLOR_RED,
        };
        render_circle(
            &mut buffer,
            width,
            left_margin + indicator_radius,
            indicator_y,
            indicator_radius,
            indicator_color,
        );

        // Render status text
        let text = content.format_status_text();
        let text_x = left_margin + (indicator_radius * 2) + 10;
        render_text(&mut buffer, width, text_x, text_y, &text, COLOR_WHITE);

        buffer
    }

    /// Returns the color for an RTT value (Story 6.2).
    ///
    /// - < 50ms: Green (good)
    /// - 50-150ms: Yellow (moderate)
    /// - > 150ms: Red (poor)
    pub fn rtt_color(rtt_ms: u32) -> [u8; 4] {
        if rtt_ms < 50 {
            COLOR_GREEN
        } else if rtt_ms < 150 {
            COLOR_YELLOW
        } else {
            COLOR_RED
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_overlay_state_default() {
            let state = OverlayState::default();
            assert_eq!(state, OverlayState::Hidden);
        }

        #[test]
        fn test_overlay_controller_new() {
            let controller = OverlayController::new();
            assert_eq!(controller.state(), OverlayState::Hidden);
            assert!(!controller.is_visible());
            assert!(controller.active_monitor().is_none());
        }

        #[test]
        fn test_overlay_show_on_hover() {
            let mut controller = OverlayController::new();

            // Pointer in hover zone should show overlay
            let changed = controller.check_pointer_position(5.0, 1);
            assert!(changed);
            assert!(controller.is_visible());
            assert_eq!(controller.active_monitor(), Some(1));
            assert_eq!(controller.state(), OverlayState::Visible);
        }

        #[test]
        fn test_overlay_hide_on_leave() {
            // Use a very short hide delay for faster, more reliable testing
            let config = OverlayConfig {
                hover_threshold: DEFAULT_HOVER_THRESHOLD,
                hide_delay: Duration::from_millis(1), // 1ms for fast test
                height: OVERLAY_HEIGHT,
            };
            let mut controller = OverlayController::with_config(config);

            // Show overlay
            controller.check_pointer_position(5.0, 1);
            assert!(controller.is_visible());

            // Move pointer out of hover zone
            let changed = controller.check_pointer_position(50.0, 1);
            assert!(changed);
            assert!(controller.is_hiding());

            // Wait for hide timer with minimal sleep (1ms delay + small margin)
            std::thread::sleep(Duration::from_millis(5));
            let hidden = controller.update_hide_timer();
            assert!(hidden);
            assert!(!controller.is_visible());
            assert_eq!(controller.state(), OverlayState::Hidden);
        }

        #[test]
        fn test_overlay_cancel_hide_on_reenter() {
            let mut controller = OverlayController::new();

            // Show overlay
            controller.check_pointer_position(5.0, 1);

            // Start hiding
            controller.check_pointer_position(50.0, 1);
            assert!(controller.is_hiding());

            // Re-enter hover zone before timer expires
            let changed = controller.check_pointer_position(5.0, 1);
            assert!(changed);
            assert_eq!(controller.state(), OverlayState::Visible);
            assert!(!controller.is_hiding());
        }

        #[test]
        fn test_overlay_multi_monitor_switch() {
            let mut controller = OverlayController::new();

            // Show on monitor 1
            controller.check_pointer_position(5.0, 1);
            assert_eq!(controller.active_monitor(), Some(1));

            // Enter monitor 2 in hover zone
            let (old_monitor, should_show) = controller.pointer_entered_monitor(5.0, 2);
            assert_eq!(old_monitor, Some(1));
            assert!(should_show);
            assert_eq!(controller.active_monitor(), Some(2));
        }

        #[test]
        fn test_overlay_buffer_generation() {
            let buffer = generate_overlay_buffer(2, 2, [0xFF, 0x00, 0x00, 0xFF]);
            assert_eq!(buffer.len(), 16); // 2x2 pixels * 4 bytes
            assert_eq!(&buffer[0..4], &[0xFF, 0x00, 0x00, 0xFF]);
            assert_eq!(&buffer[12..16], &[0xFF, 0x00, 0x00, 0xFF]);
        }

        #[test]
        fn test_overlay_force_hide() {
            let mut controller = OverlayController::new();

            controller.show(1);
            assert!(controller.is_visible());

            controller.hide();
            assert!(!controller.is_visible());
            assert!(controller.active_monitor().is_none());
        }

        #[test]
        fn test_overlay_no_change_when_already_visible() {
            let mut controller = OverlayController::new();

            // First hover - should change
            let changed1 = controller.check_pointer_position(5.0, 1);
            assert!(changed1);

            // Second hover on same monitor - should not change
            let changed2 = controller.check_pointer_position(3.0, 1);
            assert!(!changed2);
        }

        #[test]
        fn test_overlay_config_custom() {
            let config = OverlayConfig {
                hover_threshold: 20.0,
                hide_delay: Duration::from_millis(500),
                height: 60,
            };
            let controller = OverlayController::with_config(config.clone());

            assert_eq!(controller.config().hover_threshold, 20.0);
            assert_eq!(controller.config().hide_delay, Duration::from_millis(500));
            assert_eq!(controller.config().height, 60);
        }

        // Story 6.2: OverlayContent tests

        #[test]
        fn test_connection_status_default() {
            let status = ConnectionStatus::default();
            assert_eq!(status, ConnectionStatus::Disconnected);
        }

        #[test]
        fn test_overlay_content_connected() {
            let content = OverlayContent::connected("server.example.com");
            assert_eq!(content.status, ConnectionStatus::Connected);
            assert_eq!(content.server_name, Some("server.example.com".to_string()));
            assert!(content.rtt_ms.is_none());
            assert!(content.session_duration.is_none());
        }

        #[test]
        fn test_overlay_content_connected_with_rtt() {
            let content = OverlayContent::connected("server").with_rtt(25);
            assert_eq!(content.rtt_ms, Some(25));
        }

        #[test]
        fn test_overlay_content_connected_with_duration() {
            let content = OverlayContent::connected("server")
                .with_duration(Duration::from_secs(3661)); // 1h 1m 1s
            assert_eq!(content.session_duration, Some(Duration::from_secs(3661)));
        }

        #[test]
        fn test_overlay_content_reconnecting() {
            let content = OverlayContent::reconnecting(3, 10, Some("timeout".to_string()));
            assert_eq!(content.status, ConnectionStatus::Reconnecting);
            assert_eq!(content.reconnect_attempt, Some((3, 10)));
            assert_eq!(content.disconnect_reason, Some("timeout".to_string()));
        }

        #[test]
        fn test_overlay_content_disconnected() {
            let content = OverlayContent::disconnected("Connection refused");
            assert_eq!(content.status, ConnectionStatus::Disconnected);
            assert_eq!(content.disconnect_reason, Some("Connection refused".to_string()));
        }

        #[test]
        fn test_format_duration_zero() {
            let duration = Duration::from_secs(0);
            assert_eq!(format_duration(duration), "00:00:00");
        }

        #[test]
        fn test_format_duration_seconds() {
            let duration = Duration::from_secs(45);
            assert_eq!(format_duration(duration), "00:00:45");
        }

        #[test]
        fn test_format_duration_minutes() {
            let duration = Duration::from_secs(125); // 2m 5s
            assert_eq!(format_duration(duration), "00:02:05");
        }

        #[test]
        fn test_format_duration_hours() {
            let duration = Duration::from_secs(3661); // 1h 1m 1s
            assert_eq!(format_duration(duration), "01:01:01");
        }

        #[test]
        fn test_format_duration_long() {
            let duration = Duration::from_secs(36000); // 10 hours
            assert_eq!(format_duration(duration), "10:00:00");
        }

        #[test]
        fn test_format_status_text_connected() {
            let content = OverlayContent::connected("server.example.com");
            let text = content.format_status_text();
            assert!(text.contains("Connected to server.example.com"));
        }

        #[test]
        fn test_format_status_text_connected_with_rtt() {
            let content = OverlayContent::connected("server").with_rtt(42);
            let text = content.format_status_text();
            assert!(text.contains("Connected to server"));
            assert!(text.contains("42ms"));
        }

        #[test]
        fn test_format_status_text_connected_with_duration() {
            let content = OverlayContent::connected("server")
                .with_duration(Duration::from_secs(3661));
            let text = content.format_status_text();
            assert!(text.contains("01:01:01"));
        }

        #[test]
        fn test_format_status_text_reconnecting() {
            let content = OverlayContent::reconnecting(2, 10, None);
            let text = content.format_status_text();
            assert!(text.contains("Reconnecting..."));
            assert!(text.contains("(2/10)"));
        }

        #[test]
        fn test_format_status_text_reconnecting_with_reason() {
            let content = OverlayContent::reconnecting(1, 5, Some("timeout".to_string()));
            let text = content.format_status_text();
            assert!(text.contains("Reconnecting..."));
            assert!(text.contains("timeout"));
        }

        #[test]
        fn test_format_status_text_disconnected() {
            let content = OverlayContent::disconnected("Connection refused");
            let text = content.format_status_text();
            assert!(text.contains("Disconnected: Connection refused"));
        }

        #[test]
        fn test_format_status_text_disconnected_no_reason() {
            let content = OverlayContent {
                status: ConnectionStatus::Disconnected,
                disconnect_reason: None,
                ..Default::default()
            };
            let text = content.format_status_text();
            assert_eq!(text, "Disconnected");
        }

        #[test]
        fn test_rtt_color_good() {
            assert_eq!(rtt_color(10), COLOR_GREEN);
            assert_eq!(rtt_color(49), COLOR_GREEN);
        }

        #[test]
        fn test_rtt_color_moderate() {
            assert_eq!(rtt_color(50), COLOR_YELLOW);
            assert_eq!(rtt_color(100), COLOR_YELLOW);
            assert_eq!(rtt_color(149), COLOR_YELLOW);
        }

        #[test]
        fn test_rtt_color_poor() {
            assert_eq!(rtt_color(150), COLOR_RED);
            assert_eq!(rtt_color(500), COLOR_RED);
        }

        #[test]
        fn test_render_text_basic() {
            // Create a small buffer and render text
            let width = 100u32;
            let height = 20u32;
            let mut buffer = vec![0u8; (width * height * 4) as usize];

            render_text(&mut buffer, width, 0, 5, "Hi", COLOR_WHITE);

            // Check that some pixels were modified (at least one white pixel)
            let has_white_pixel = buffer.chunks(4).any(|pixel| pixel == COLOR_WHITE);
            assert!(has_white_pixel, "Text should have rendered some white pixels");
        }

        #[test]
        fn test_render_text_bounds_check() {
            // Render text at the edge - should not panic
            let width = 50u32;
            let height = 20u32;
            let mut buffer = vec![0u8; (width * height * 4) as usize];

            // This should not panic even if text extends beyond buffer
            render_text(&mut buffer, width, 45, 5, "Hello World", COLOR_WHITE);
        }

        #[test]
        fn test_render_circle_basic() {
            let width = 30u32;
            let height = 30u32;
            let mut buffer = vec![0u8; (width * height * 4) as usize];

            render_circle(&mut buffer, width, 15, 15, 5, COLOR_GREEN);

            // Check center pixel is green
            let center_offset = ((15 * width + 15) * 4) as usize;
            assert_eq!(&buffer[center_offset..center_offset + 4], &COLOR_GREEN);
        }

        #[test]
        fn test_generate_overlay_buffer_with_content_connected() {
            let width = 200u32;
            let height = OVERLAY_HEIGHT;
            let content = OverlayContent::connected("test-server").with_rtt(50);

            let buffer = generate_overlay_buffer_with_content(
                width, height, OVERLAY_BG_COLOR, Some(&content)
            );

            assert_eq!(buffer.len(), (width * height * 4) as usize);

            // Buffer should have some non-background pixels (indicator and text)
            let has_green = buffer.chunks(4).any(|p| p == COLOR_GREEN);
            let has_white = buffer.chunks(4).any(|p| p == COLOR_WHITE);
            assert!(has_green, "Should have green status indicator");
            assert!(has_white, "Should have white text pixels");
        }

        #[test]
        fn test_generate_overlay_buffer_with_content_none() {
            let width = 100u32;
            let height = 40u32;

            let buffer = generate_overlay_buffer_with_content(width, height, OVERLAY_BG_COLOR, None);
            let plain_buffer = generate_overlay_buffer(width, height, OVERLAY_BG_COLOR);

            // Should be identical when no content
            assert_eq!(buffer, plain_buffer);
        }

        #[test]
        fn test_font_data_complete() {
            // Font should have data for all 95 printable ASCII characters (32-126)
            let expected_chars = 95;
            let expected_size = expected_chars * (FONT_HEIGHT as usize);
            assert_eq!(FONT_DATA.len(), expected_size, "Font data should cover ASCII 32-126");
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::*;

// Stub for non-Linux platforms
#[cfg(not(target_os = "linux"))]
mod stub {
    use std::time::Duration;

    /// Overlay state stub for non-Linux platforms.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub enum OverlayState {
        #[default]
        Hidden,
        Visible,
    }

    /// Overlay configuration stub for non-Linux platforms.
    #[derive(Debug, Clone)]
    pub struct OverlayConfig {
        pub hover_threshold: f64,
        pub hide_delay: Duration,
        pub height: u32,
    }

    impl Default for OverlayConfig {
        fn default() -> Self {
            Self {
                hover_threshold: DEFAULT_HOVER_THRESHOLD,
                hide_delay: DEFAULT_HIDE_DELAY,
                height: OVERLAY_HEIGHT,
            }
        }
    }

    /// Connection status stub (Story 6.2).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub enum ConnectionStatus {
        #[default]
        Connected,
        Reconnecting,
        Disconnected,
    }

    /// Overlay content stub (Story 6.2).
    #[derive(Debug, Clone, Default)]
    pub struct OverlayContent {
        pub status: ConnectionStatus,
        pub server_name: Option<String>,
        pub rtt_ms: Option<u32>,
        pub session_duration: Option<Duration>,
        pub reconnect_attempt: Option<(u32, u32)>,
        pub disconnect_reason: Option<String>,
    }

    impl OverlayContent {
        pub fn connected(_server_name: impl Into<String>) -> Self {
            Self::default()
        }
        pub fn reconnecting(_attempt: u32, _max: u32, _reason: Option<String>) -> Self {
            Self { status: ConnectionStatus::Reconnecting, ..Default::default() }
        }
        pub fn disconnected(_reason: impl Into<String>) -> Self {
            Self { status: ConnectionStatus::Disconnected, ..Default::default() }
        }
        pub fn with_rtt(self, _rtt: u32) -> Self { self }
        pub fn with_duration(self, _dur: Duration) -> Self { self }
        pub fn format_status_text(&self) -> String { String::new() }
    }

    /// Overlay controller stub for non-Linux platforms.
    #[derive(Debug, Default)]
    pub struct OverlayController;

    impl OverlayController {
        pub fn new() -> Self {
            Self
        }

        pub fn with_config(_config: OverlayConfig) -> Self {
            Self
        }

        pub fn state(&self) -> OverlayState {
            OverlayState::Hidden
        }

        pub fn config(&self) -> &OverlayConfig {
            // Return a static default config for stub
            static DEFAULT_CONFIG: std::sync::OnceLock<OverlayConfig> = std::sync::OnceLock::new();
            DEFAULT_CONFIG.get_or_init(OverlayConfig::default)
        }

        pub fn is_visible(&self) -> bool {
            false
        }

        pub fn is_hiding(&self) -> bool {
            false
        }

        pub fn active_monitor(&self) -> Option<u32> {
            None
        }

        pub fn check_pointer_position(&mut self, _y: f64, _monitor_id: u32) -> bool {
            false
        }

        pub fn pointer_entered_monitor(&mut self, _y: f64, _monitor_id: u32) -> (Option<u32>, bool) {
            (None, false)
        }

        pub fn update_hide_timer(&mut self) -> bool {
            false
        }

        pub fn hide(&mut self) {}

        pub fn show(&mut self, _monitor_id: u32) {}

        pub fn reset(&mut self) {}
    }

    pub fn generate_overlay_buffer(_width: u32, _height: u32, _color: [u8; 4]) -> Vec<u8> {
        Vec::new()
    }

    pub fn generate_overlay_buffer_with_content(
        _width: u32,
        _height: u32,
        _bg_color: [u8; 4],
        _content: Option<&OverlayContent>,
    ) -> Vec<u8> {
        Vec::new()
    }

    pub fn format_duration(_duration: Duration) -> String {
        String::new()
    }

    pub fn render_text(_buffer: &mut [u8], _width: u32, _x: u32, _y: u32, _text: &str, _color: [u8; 4]) {}
    pub fn render_circle(_buffer: &mut [u8], _width: u32, _cx: u32, _cy: u32, _radius: u32, _color: [u8; 4]) {}
    pub fn rtt_color(_rtt_ms: u32) -> [u8; 4] { [0, 0, 0, 0] }

    pub const DEFAULT_HOVER_THRESHOLD: f64 = 10.0;
    pub const DEFAULT_HIDE_DELAY: Duration = Duration::from_millis(300);
    pub const OVERLAY_BG_COLOR: [u8; 4] = [0, 0, 0, 0];
    pub const OVERLAY_HEIGHT: u32 = 40;
    pub const FONT_WIDTH: u32 = 6;
    pub const FONT_HEIGHT: u32 = 10;
    pub const COLOR_GREEN: [u8; 4] = [0, 0, 0, 0];
    pub const COLOR_YELLOW: [u8; 4] = [0, 0, 0, 0];
    pub const COLOR_RED: [u8; 4] = [0, 0, 0, 0];
    pub const COLOR_WHITE: [u8; 4] = [0, 0, 0, 0];
    pub const COLOR_GRAY: [u8; 4] = [0, 0, 0, 0];
}

#[cfg(not(target_os = "linux"))]
pub use stub::*;
