//! CLIPRDR clipboard channel handler for RDP clipboard redirection.
//!
//! This module implements the CLIPRDR Static Virtual Channel for clipboard
//! synchronization between local and remote systems.
//!
//! Clipboard data flow (remote to local):
//! 1. Server sends Format List PDU announcing available formats
//! 2. Client responds with Format List Response
//! 3. Client can request data via Format Data Request (Story 5.2+)
//!
//! Protocol: MS-RDPECLIP (Remote Desktop Protocol: Clipboard Virtual Channel Extension)

use std::any::Any;
use std::sync::mpsc::Sender;

use ironrdp::core::AsAny;
use ironrdp::pdu::gcc::ChannelName;
use ironrdp::pdu::{Encode, EncodeResult, PduResult, WriteCursor};
use ironrdp::svc::{SvcClientProcessor, SvcEncode, SvcMessage, SvcProcessor};
use tracing::{debug, trace, warn};

pub mod pdu;

use pdu::{
    ClipCapsPdu, ClipboardFormat, CliprdrPdu, FormatDataRequestPdu, FormatDataResponsePdu,
    FormatListPdu, FormatListResponsePdu, StandardFormat,
};

/// Channel name for CLIPRDR Static Virtual Channel.
pub const CLIPRDR_CHANNEL_NAME: &str = "cliprdr";

/// Maximum byte value for ASCII characters (exclusive).
/// Used when converting UTF-8 to ANSI format (lossy).
const ASCII_MAX: u8 = 128;

/// Events emitted by the CLIPRDR handler for clipboard synchronization.
///
/// These events are sent to the main thread when clipboard state changes
/// on the remote server.
#[derive(Debug, Clone)]
pub enum ClipboardEvent {
    /// Server clipboard content changed (Format List received).
    ///
    /// The formats list contains the IDs of available clipboard formats.
    /// Use `StandardFormat` to check for text formats.
    FormatsAvailable {
        /// Available clipboard format IDs.
        formats: Vec<u32>,
        /// True if text data is available (CF_UNICODETEXT or CF_TEXT).
        has_text: bool,
    },
    /// Clipboard text data received from server.
    ///
    /// Response to a format data request.
    TextReceived {
        /// The clipboard text content (UTF-8).
        text: String,
    },
    /// Format data request failed.
    RequestFailed,
}

/// CLIPRDR protocol state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliprdrState {
    /// Waiting for Monitor Ready PDU from server.
    Initial,
    /// Monitor Ready received, capabilities exchanged.
    Ready,
    /// Channel closed.
    Closed,
}

/// CLIPRDR Static Virtual Channel handler.
///
/// Implements MS-RDPECLIP protocol for clipboard redirection.
/// This handler manages the clipboard channel state and format negotiation.
pub struct YardCliprdrHandler {
    /// Current protocol state.
    state: CliprdrState,
    /// Channel ID assigned by SVC.
    channel_id: Option<u32>,
    /// Whether long format names are supported (CB_USE_LONG_FORMAT_NAMES).
    use_long_format_names: bool,
    /// Formats available on the server's clipboard.
    server_formats: Vec<ClipboardFormat>,
    /// Whether clipboard functionality is enabled.
    enabled: bool,
    /// Pending format data request (format ID we're waiting for).
    pending_format_request: Option<u32>,
    /// Received clipboard data (text), waiting to be consumed.
    pending_clipboard_data: Option<String>,
    /// Optional channel for sending clipboard events to the main thread.
    event_tx: Option<Sender<ClipboardEvent>>,
    /// Story 5.3: Local clipboard text to provide when server requests.
    local_clipboard_text: Option<String>,
}

impl YardCliprdrHandler {
    /// Creates a new CLIPRDR handler without event channel.
    ///
    /// This handler will not send clipboard events to the main thread.
    /// Use `with_event_channel` for clipboard synchronization.
    pub fn new() -> Self {
        Self {
            state: CliprdrState::Initial,
            channel_id: None,
            use_long_format_names: true, // Default to long format names
            server_formats: Vec::new(),
            enabled: true,
            pending_format_request: None,
            pending_clipboard_data: None,
            event_tx: None,
            local_clipboard_text: None,
        }
    }

    /// Creates a new CLIPRDR handler with an event channel.
    ///
    /// Clipboard events (format list updates, data received) will be sent
    /// through the provided channel for integration with the main thread.
    pub fn with_event_channel(event_tx: Sender<ClipboardEvent>) -> Self {
        Self {
            state: CliprdrState::Initial,
            channel_id: None,
            use_long_format_names: true,
            server_formats: Vec::new(),
            enabled: true,
            pending_format_request: None,
            pending_clipboard_data: None,
            event_tx: Some(event_tx),
            local_clipboard_text: None,
        }
    }

    /// Creates a disabled CLIPRDR handler (no-op).
    pub fn disabled() -> Self {
        Self {
            state: CliprdrState::Closed,
            channel_id: None,
            use_long_format_names: false,
            server_formats: Vec::new(),
            enabled: false,
            pending_format_request: None,
            pending_clipboard_data: None,
            event_tx: None,
            local_clipboard_text: None,
        }
    }

    /// Sends a clipboard event to the main thread if a channel is configured.
    fn send_event(&self, event: ClipboardEvent) {
        if let Some(ref tx) = self.event_tx
            && let Err(e) = tx.send(event)
        {
            warn!("Failed to send clipboard event: {e}");
        }
    }

    /// Returns whether clipboard functionality is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Returns the formats available on the server's clipboard.
    pub fn server_formats(&self) -> &[ClipboardFormat] {
        &self.server_formats
    }

    /// Handles a Monitor Ready PDU from the server.
    fn handle_monitor_ready(&mut self, _channel_id: u32) -> PduResult<Vec<SvcMessage>> {
        debug!("Received CLIPRDR Monitor Ready PDU");

        // Respond with Clip Capabilities PDU
        let caps = ClipCapsPdu::default_client();
        let caps_data = caps.encode();

        debug!(
            "Sending Clip Capabilities (long_format_names={})",
            caps.general_flags.use_long_format_names()
        );

        // Also send an empty Format List to indicate we have no data initially
        let format_list = FormatListPdu::empty();
        let format_list_data = format_list.encode();

        self.state = CliprdrState::Ready;

        Ok(vec![
            SvcMessage::from(CliprdrSvcMessage::new(caps_data)),
            SvcMessage::from(CliprdrSvcMessage::new(format_list_data)),
        ])
    }

    /// Handles a Clip Capabilities PDU from the server.
    fn handle_clip_caps(
        &mut self,
        caps: &ClipCapsPdu,
        _channel_id: u32,
    ) -> PduResult<Vec<SvcMessage>> {
        debug!(
            "Received CLIPRDR Clip Capabilities: long_format_names={}, fileclip={}",
            caps.general_flags.use_long_format_names(),
            caps.general_flags.stream_fileclip_enabled()
        );

        // Update our understanding of server capabilities
        self.use_long_format_names = caps.general_flags.use_long_format_names();

        Ok(Vec::new())
    }

    /// Handles a Format List PDU from the server.
    fn handle_format_list(
        &mut self,
        format_list: &FormatListPdu,
        _channel_id: u32,
    ) -> PduResult<Vec<SvcMessage>> {
        debug!(
            "Received CLIPRDR Format List with {} formats",
            format_list.formats.len()
        );

        // Store server formats
        self.server_formats = format_list.formats.clone();

        // Log available formats for debugging
        for format in &self.server_formats {
            if format.name.is_empty() {
                trace!("  Format ID {}", format.id);
            } else {
                trace!("  Format ID {} ({})", format.id, format.name);
            }
        }

        // Check if text formats are available
        let has_unicode = self
            .server_formats
            .iter()
            .any(|f| f.id == StandardFormat::UnicodeText as u32);
        let has_ansi = self
            .server_formats
            .iter()
            .any(|f| f.id == StandardFormat::Text as u32);
        let has_text = has_unicode || has_ansi;

        // Notify main thread of available formats
        let formats: Vec<u32> = self.server_formats.iter().map(|f| f.id).collect();
        self.send_event(ClipboardEvent::FormatsAvailable {
            formats,
            has_text,
        });

        // Build response messages
        let mut messages = Vec::new();

        // Always respond with Format List Response (success)
        let response = FormatListResponsePdu::ok();
        let response_data = response.encode();
        debug!("Sending Format List Response (OK)");
        messages.push(SvcMessage::from(CliprdrSvcMessage::new(response_data)));

        // Automatically request text data if available (Story 5.2)
        // This is the "pull" model where we fetch text immediately when announced
        if has_text && self.event_tx.is_some() {
            // Prefer Unicode over ANSI
            let format_id = if has_unicode {
                StandardFormat::UnicodeText as u32
            } else {
                StandardFormat::Text as u32
            };

            debug!("Auto-requesting clipboard text (format {})", format_id);
            let request = FormatDataRequestPdu::new(format_id);
            self.pending_format_request = Some(format_id);
            messages.push(SvcMessage::from(CliprdrSvcMessage::new(request.encode())));
        }

        Ok(messages)
    }

    /// Handles a Format List Response PDU from the server.
    fn handle_format_list_response(
        &mut self,
        response: &FormatListResponsePdu,
        _channel_id: u32,
    ) -> PduResult<Vec<SvcMessage>> {
        if response.success {
            trace!("Server accepted our Format List");
        } else {
            warn!("Server rejected our Format List");
        }

        Ok(Vec::new())
    }

    /// Handles a Format Data Request PDU from the server.
    ///
    /// The server sends this when it wants to paste data that we announced
    /// in our Format List. Story 5.3: Respond with local clipboard data.
    fn handle_format_data_request(
        &mut self,
        request: &FormatDataRequestPdu,
        _channel_id: u32,
    ) -> PduResult<Vec<SvcMessage>> {
        debug!(
            "Received Format Data Request for format ID {}",
            request.requested_format_id
        );

        // Story 5.3: Provide local clipboard data if available
        let response = if let Some(ref text) = self.local_clipboard_text {
            match request.requested_format_id {
                id if id == StandardFormat::UnicodeText as u32 => {
                    // Convert UTF-8 to UTF-16LE with null terminator
                    let data = Self::utf8_to_utf16le_with_null(text);
                    debug!("Sending Format Data Response (Unicode, {} bytes)", data.len());
                    FormatDataResponsePdu::ok(data)
                }
                id if id == StandardFormat::Text as u32 => {
                    // Convert to ANSI (lossy conversion, just use ASCII bytes)
                    let mut data: Vec<u8> = text.bytes().filter(|&b| b < ASCII_MAX).collect();
                    data.push(0); // Null terminator
                    debug!("Sending Format Data Response (ANSI, {} bytes)", data.len());
                    FormatDataResponsePdu::ok(data)
                }
                _ => {
                    debug!("Requested format {} not available", request.requested_format_id);
                    FormatDataResponsePdu::fail()
                }
            }
        } else {
            debug!("No local clipboard data available");
            FormatDataResponsePdu::fail()
        };

        let response_data = response.encode();
        Ok(vec![SvcMessage::from(CliprdrSvcMessage::new(response_data))])
    }

    /// Converts UTF-8 string to UTF-16LE bytes with null terminator.
    fn utf8_to_utf16le_with_null(text: &str) -> Vec<u8> {
        let mut data = Vec::with_capacity((text.len() + 1) * 2);
        for code_unit in text.encode_utf16() {
            data.extend_from_slice(&code_unit.to_le_bytes());
        }
        // Add null terminator
        data.extend_from_slice(&0u16.to_le_bytes());
        data
    }

    /// Handles a Format Data Response PDU from the server.
    ///
    /// The server sends this in response to our Format Data Request,
    /// containing the actual clipboard data.
    fn handle_format_data_response(
        &mut self,
        response: &FormatDataResponsePdu,
        _channel_id: u32,
    ) -> PduResult<Vec<SvcMessage>> {
        if response.success {
            debug!(
                "Received Format Data Response with {} bytes",
                response.data.len()
            );

            // Try to convert to text if possible
            if let Some(text) = response.as_utf8_from_unicode() {
                debug!("Clipboard text (Unicode): {} chars", text.len());
                // Store the received data for the clipboard bridge
                self.pending_clipboard_data = Some(text.clone());
                // Notify main thread
                self.send_event(ClipboardEvent::TextReceived { text });
            } else if let Some(text) = response.as_utf8_from_ansi() {
                debug!("Clipboard text (ANSI): {} chars", text.len());
                self.pending_clipboard_data = Some(text.clone());
                // Notify main thread
                self.send_event(ClipboardEvent::TextReceived { text });
            } else {
                debug!("Clipboard data is not text or failed to decode");
                self.send_event(ClipboardEvent::RequestFailed);
            }
        } else {
            warn!("Format Data Request failed");
            self.send_event(ClipboardEvent::RequestFailed);
        }

        // Clear the pending request
        self.pending_format_request = None;

        Ok(Vec::new())
    }

    /// Requests clipboard data in the specified format.
    ///
    /// Sends a Format Data Request PDU to the server. The response will be
    /// received asynchronously via `handle_format_data_response`.
    pub fn request_format_data(&mut self, format_id: u32) -> Option<Vec<u8>> {
        if self.state != CliprdrState::Ready {
            warn!("Cannot request format data: clipboard not ready");
            return None;
        }

        // Check if the format is available
        if !self.server_formats.iter().any(|f| f.id == format_id) {
            warn!("Requested format {} not available on server", format_id);
            return None;
        }

        debug!("Requesting clipboard format {}", format_id);

        let request = FormatDataRequestPdu::new(format_id);
        self.pending_format_request = Some(format_id);

        Some(request.encode())
    }

    /// Requests text clipboard data, preferring Unicode format.
    ///
    /// Returns the encoded PDU to send, or None if not available.
    pub fn request_text_data(&mut self) -> Option<Vec<u8>> {
        // Prefer Unicode text
        if self
            .server_formats
            .iter()
            .any(|f| f.id == StandardFormat::UnicodeText as u32)
        {
            return self.request_format_data(StandardFormat::UnicodeText as u32);
        }

        // Fall back to ANSI text
        if self
            .server_formats
            .iter()
            .any(|f| f.id == StandardFormat::Text as u32)
        {
            return self.request_format_data(StandardFormat::Text as u32);
        }

        warn!("No text format available on server clipboard");
        None
    }

    /// Returns the pending clipboard data, if any.
    ///
    /// This is set when we receive a successful Format Data Response.
    pub fn take_clipboard_data(&mut self) -> Option<String> {
        self.pending_clipboard_data.take()
    }

    /// Returns true if there's a pending format data request.
    pub fn has_pending_request(&self) -> bool {
        self.pending_format_request.is_some()
    }

    /// Story 5.3: Sets the local clipboard text and returns Format List PDU to send.
    ///
    /// Call this when the local clipboard changes with text content.
    /// Send the returned bytes via the CLIPRDR static channel.
    pub fn set_local_clipboard_text(&mut self, text: String) -> Option<Vec<u8>> {
        if self.state != CliprdrState::Ready {
            warn!("Cannot set local clipboard: channel not ready");
            return None;
        }

        debug!("Setting local clipboard text: {} chars", text.len());
        self.local_clipboard_text = Some(text);

        // Create Format List PDU announcing text formats
        let format_list = FormatListPdu::text_formats();
        let encoded = format_list.encode();

        debug!("Sending Format List with text formats");
        Some(encoded)
    }

    /// Story 5.3: Clears the local clipboard.
    pub fn clear_local_clipboard(&mut self) -> Option<Vec<u8>> {
        if self.state != CliprdrState::Ready {
            return None;
        }

        debug!("Clearing local clipboard");
        self.local_clipboard_text = None;

        // Send empty Format List
        let format_list = FormatListPdu::empty();
        Some(format_list.encode())
    }

    /// Story 5.3: Returns the local clipboard text for Format Data Response.
    pub fn local_clipboard_text(&self) -> Option<&str> {
        self.local_clipboard_text.as_deref()
    }
}

impl Default for YardCliprdrHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for YardCliprdrHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("YardCliprdrHandler")
            .field("state", &self.state)
            .field("channel_id", &self.channel_id)
            .field("use_long_format_names", &self.use_long_format_names)
            .field("server_formats", &self.server_formats)
            .field("enabled", &self.enabled)
            .field("pending_format_request", &self.pending_format_request)
            .field(
                "pending_clipboard_data",
                &self.pending_clipboard_data.as_ref().map(|s| s.len()),
            )
            .field("event_tx", &self.event_tx.is_some())
            .finish()
    }
}

impl SvcProcessor for YardCliprdrHandler {
    fn channel_name(&self) -> ChannelName {
        ChannelName::from_static(b"cliprdr\0")
    }

    fn compression_condition(&self) -> ironrdp::svc::CompressionCondition {
        ironrdp::svc::CompressionCondition::Never
    }

    fn start(&mut self) -> PduResult<Vec<SvcMessage>> {
        debug!("CLIPRDR channel started");
        self.state = CliprdrState::Initial;
        Ok(Vec::new())
    }

    fn process(&mut self, payload: &[u8]) -> PduResult<Vec<SvcMessage>> {
        if !self.enabled {
            return Ok(Vec::new());
        }

        if payload.is_empty() {
            return Ok(Vec::new());
        }

        let channel_id = self.channel_id.unwrap_or(0);

        // Parse the PDU
        let pdu = CliprdrPdu::decode(payload, self.use_long_format_names)
            .map_err(|e| ironrdp::pdu::other_err!("CLIPRDR", source: e))?;

        match pdu {
            CliprdrPdu::MonitorReady(_) => self.handle_monitor_ready(channel_id),
            CliprdrPdu::ClipCaps(caps) => self.handle_clip_caps(&caps, channel_id),
            CliprdrPdu::FormatList(format_list) => {
                self.handle_format_list(&format_list, channel_id)
            }
            CliprdrPdu::FormatListResponse(response) => {
                self.handle_format_list_response(&response, channel_id)
            }
            CliprdrPdu::FormatDataRequest(request) => {
                self.handle_format_data_request(&request, channel_id)
            }
            CliprdrPdu::FormatDataResponse(response) => {
                self.handle_format_data_response(&response, channel_id)
            }
        }
    }
}

impl SvcClientProcessor for YardCliprdrHandler {}

impl AsAny for YardCliprdrHandler {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Wrapper for CLIPRDR PDU that implements SvcEncode for sending via SVC.
#[derive(Clone, Debug)]
pub struct CliprdrSvcMessage {
    data: Vec<u8>,
}

impl CliprdrSvcMessage {
    /// Creates a new CLIPRDR SVC message from raw bytes.
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }
}

impl Encode for CliprdrSvcMessage {
    fn encode(&self, dst: &mut WriteCursor<'_>) -> EncodeResult<()> {
        dst.write_slice(&self.data);
        Ok(())
    }

    fn name(&self) -> &'static str {
        "CliprdrSvcMessage"
    }

    fn size(&self) -> usize {
        self.data.len()
    }
}

impl SvcEncode for CliprdrSvcMessage {}

/// Creates a CLIPRDR client for the RDP connection.
///
/// # Arguments
///
/// * `event_tx` - Optional channel for clipboard events. If provided, clipboard
///   events (format list updates, data received) will be sent through this channel.
///   If `None`, clipboard is disabled.
///
/// # Returns
///
/// A `YardCliprdrHandler` instance ready to be attached to the RDP connector.
pub fn create_cliprdr_client(event_tx: Option<Sender<ClipboardEvent>>) -> YardCliprdrHandler {
    match event_tx {
        Some(tx) => {
            debug!("Creating CLIPRDR client with event channel");
            YardCliprdrHandler::with_event_channel(tx)
        }
        None => {
            debug!("Creating disabled CLIPRDR client (clipboard disabled)");
            YardCliprdrHandler::disabled()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handler_creation() {
        let handler = YardCliprdrHandler::new();
        assert_eq!(handler.state, CliprdrState::Initial);
        assert!(handler.is_enabled());
        assert!(handler.channel_id.is_none());
    }

    #[test]
    fn test_handler_disabled() {
        let handler = YardCliprdrHandler::disabled();
        assert!(!handler.is_enabled());
        assert_eq!(handler.state, CliprdrState::Closed);
    }

    #[test]
    fn test_handler_channel_name() {
        let handler = YardCliprdrHandler::new();
        assert_eq!(handler.channel_name().as_str(), Some(CLIPRDR_CHANNEL_NAME));
    }

    #[test]
    fn test_handler_start() {
        let mut handler = YardCliprdrHandler::new();
        let result = handler.start();
        assert!(result.is_ok());
        assert_eq!(handler.state, CliprdrState::Initial);
    }

    #[test]
    fn test_process_empty_payload() {
        let mut handler = YardCliprdrHandler::new();
        handler.start().unwrap();

        let result = handler.process(&[]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_process_disabled() {
        let mut handler = YardCliprdrHandler::disabled();

        // Even with valid data, disabled handler returns empty
        let monitor_ready = [
            0x01, 0x00, // msgType = MonitorReady
            0x00, 0x00, // msgFlags
            0x00, 0x00, 0x00, 0x00, // dataLen = 0
        ];
        let result = handler.process(&monitor_ready);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_process_monitor_ready() {
        let mut handler = YardCliprdrHandler::new();
        handler.start().unwrap();

        // Monitor Ready PDU
        let monitor_ready = [
            0x01, 0x00, // msgType = MonitorReady
            0x00, 0x00, // msgFlags
            0x00, 0x00, 0x00, 0x00, // dataLen = 0
        ];

        let result = handler.process(&monitor_ready);
        assert!(result.is_ok());

        // Should respond with Clip Capabilities and Format List
        let messages = result.unwrap();
        assert_eq!(messages.len(), 2);

        // State should be Ready
        assert_eq!(handler.state, CliprdrState::Ready);
    }

    #[test]
    fn test_process_format_list() {
        let mut handler = YardCliprdrHandler::new();
        handler.start().unwrap();
        handler.state = CliprdrState::Ready;

        // Format List PDU with CF_UNICODETEXT
        let format_list = [
            0x02, 0x00, // msgType = FormatList
            0x00, 0x00, // msgFlags
            0x06, 0x00, 0x00, 0x00, // dataLen = 6
            // Format entry: ID=13 (CF_UNICODETEXT), empty name (null-terminated)
            0x0D, 0x00, 0x00, 0x00, // formatId = 13
            0x00, 0x00, // null terminator (UTF-16)
        ];

        let result = handler.process(&format_list);
        assert!(result.is_ok());

        // Should respond with Format List Response
        let messages = result.unwrap();
        assert_eq!(messages.len(), 1);

        // Should have stored the format
        assert_eq!(handler.server_formats.len(), 1);
        assert_eq!(handler.server_formats[0].id, 13);
    }

    #[test]
    fn test_create_cliprdr_client_with_channel() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let handler = create_cliprdr_client(Some(tx));
        assert!(handler.is_enabled());
    }

    #[test]
    fn test_create_cliprdr_client_disabled() {
        let handler = create_cliprdr_client(None);
        assert!(!handler.is_enabled());
    }

    #[test]
    fn test_cliprdr_svc_message() {
        let data = vec![0x01, 0x02, 0x03];
        let msg = CliprdrSvcMessage::new(data.clone());

        assert_eq!(msg.size(), 3);
        assert_eq!(msg.name(), "CliprdrSvcMessage");
    }

    #[test]
    fn test_server_formats_accessor() {
        let mut handler = YardCliprdrHandler::new();
        assert!(handler.server_formats().is_empty());

        handler.server_formats = vec![ClipboardFormat::new(13, "")];
        assert_eq!(handler.server_formats().len(), 1);
    }
}
