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

use ironrdp::core::AsAny;
use ironrdp::pdu::gcc::ChannelName;
use ironrdp::pdu::{Encode, EncodeResult, PduResult, WriteCursor};
use ironrdp::svc::{SvcClientProcessor, SvcEncode, SvcMessage, SvcProcessor};
use tracing::{debug, trace, warn};

pub mod pdu;

use pdu::{ClipCapsPdu, ClipboardFormat, CliprdrPdu, FormatListPdu, FormatListResponsePdu};

/// Channel name for CLIPRDR Static Virtual Channel.
pub const CLIPRDR_CHANNEL_NAME: &str = "cliprdr";

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
#[derive(Debug)]
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
}

impl YardCliprdrHandler {
    /// Creates a new CLIPRDR handler.
    pub fn new() -> Self {
        Self {
            state: CliprdrState::Initial,
            channel_id: None,
            use_long_format_names: true, // Default to long format names
            server_formats: Vec::new(),
            enabled: true,
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

        // Always respond with success
        let response = FormatListResponsePdu::ok();
        let response_data = response.encode();

        debug!("Sending Format List Response (OK)");

        Ok(vec![SvcMessage::from(CliprdrSvcMessage::new(
            response_data,
        ))])
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
}

impl Default for YardCliprdrHandler {
    fn default() -> Self {
        Self::new()
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
/// * `enabled` - Whether clipboard functionality should be enabled.
///
/// # Returns
///
/// A `YardCliprdrHandler` instance ready to be attached to the RDP connector.
pub fn create_cliprdr_client(enabled: bool) -> YardCliprdrHandler {
    if enabled {
        debug!("Creating CLIPRDR client for clipboard redirection");
        YardCliprdrHandler::new()
    } else {
        debug!("Creating disabled CLIPRDR client (clipboard disabled)");
        YardCliprdrHandler::disabled()
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
    fn test_create_cliprdr_client_enabled() {
        let handler = create_cliprdr_client(true);
        assert!(handler.is_enabled());
    }

    #[test]
    fn test_create_cliprdr_client_disabled() {
        let handler = create_cliprdr_client(false);
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
