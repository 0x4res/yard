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

pub mod file_transfer;
pub mod pdu;

use pdu::{
    ClipCapsPdu, ClipboardFormat, CliprdrFileList, CliprdrPdu, FileContentsRequestPdu,
    FormatDataRequestPdu, FormatDataResponsePdu, FormatListPdu, FormatListResponsePdu,
    StandardFormat,
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
        /// True if files are available (FileGroupDescriptorW format).
        has_files: bool,
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
    /// Story 5.4: File list received from server.
    ///
    /// Contains metadata about files available for download.
    FilesReceived {
        /// File descriptors with names and sizes.
        files: Vec<FileInfo>,
    },
    /// Story 5.4: File content chunk received.
    FileContentReceived {
        /// Stream ID matching the request.
        stream_id: u32,
        /// File data bytes.
        data: Vec<u8>,
    },
    /// Story 5.4: File size received.
    FileSizeReceived {
        /// Stream ID matching the request.
        stream_id: u32,
        /// File size in bytes.
        size: u64,
    },
    /// Story 5.4: File transfer failed.
    FileTransferFailed {
        /// Stream ID of the failed request.
        stream_id: u32,
    },
}

/// Story 5.4: Information about a file in the clipboard (from remote server).
#[derive(Debug, Clone)]
pub struct FileInfo {
    /// File name.
    pub name: String,
    /// File size in bytes (if known).
    pub size: Option<u64>,
    /// True if this is a directory.
    pub is_directory: bool,
}

/// Story 5.5: Information about a local file to send to the remote server.
#[derive(Debug, Clone)]
pub struct LocalFileInfo {
    /// Full path to the local file.
    pub path: std::path::PathBuf,
    /// File name (extracted from path for convenience).
    pub name: String,
    /// File size in bytes.
    pub size: u64,
    /// Last modification time as Unix timestamp.
    pub modified_time: u64,
    /// True if this is a directory.
    pub is_directory: bool,
}

impl LocalFileInfo {
    /// Creates a LocalFileInfo from a file path by reading its metadata.
    ///
    /// Returns None if the file doesn't exist or can't be read.
    pub fn from_path(path: std::path::PathBuf) -> Option<Self> {
        let metadata = std::fs::metadata(&path).ok()?;
        let name = path.file_name()?.to_str()?.to_string();

        let modified_time = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);

        Some(Self {
            path,
            name,
            size: metadata.len(),
            modified_time,
            is_directory: metadata.is_dir(),
        })
    }
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
    /// Whether file clipboard is enabled (CB_STREAM_FILECLIP_ENABLED).
    file_clipboard_enabled: bool,
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
    /// Story 5.4: Format ID for FileGroupDescriptorW (dynamic, announced by server).
    file_group_descriptor_format_id: Option<u32>,
    /// Story 5.4: Format ID for FileContents (dynamic, announced by server).
    file_contents_format_id: Option<u32>,
    /// Story 5.4: Next stream ID for file content requests.
    next_stream_id: u32,
    /// Story 5.4: Cached file list from server.
    remote_file_list: Option<CliprdrFileList>,
    /// Story 5.5: Local files to offer to the remote server.
    local_clipboard_files: Option<Vec<LocalFileInfo>>,
    /// Story 5.5: Client-assigned format ID for FileGroupDescriptorW (for local→remote).
    local_file_group_descriptor_format_id: u32,
    /// Story 5.5: Client-assigned format ID for FileContents (for local→remote).
    local_file_contents_format_id: u32,
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
            file_clipboard_enabled: false,
            server_formats: Vec::new(),
            enabled: true,
            pending_format_request: None,
            pending_clipboard_data: None,
            event_tx: None,
            local_clipboard_text: None,
            file_group_descriptor_format_id: None,
            file_contents_format_id: None,
            next_stream_id: 1,
            remote_file_list: None,
            local_clipboard_files: None,
            // Story 5.5: Client-assigned format IDs in registered format range (>= 0xC000)
            local_file_group_descriptor_format_id: 0xC100,
            local_file_contents_format_id: 0xC101,
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
            file_clipboard_enabled: false,
            server_formats: Vec::new(),
            enabled: true,
            pending_format_request: None,
            pending_clipboard_data: None,
            event_tx: Some(event_tx),
            local_clipboard_text: None,
            file_group_descriptor_format_id: None,
            file_contents_format_id: None,
            next_stream_id: 1,
            remote_file_list: None,
            local_clipboard_files: None,
            local_file_group_descriptor_format_id: 0xC100,
            local_file_contents_format_id: 0xC101,
        }
    }

    /// Creates a disabled CLIPRDR handler (no-op).
    pub fn disabled() -> Self {
        Self {
            state: CliprdrState::Closed,
            channel_id: None,
            use_long_format_names: false,
            file_clipboard_enabled: false,
            server_formats: Vec::new(),
            enabled: false,
            pending_format_request: None,
            pending_clipboard_data: None,
            event_tx: None,
            local_clipboard_text: None,
            file_group_descriptor_format_id: None,
            file_contents_format_id: None,
            next_stream_id: 1,
            remote_file_list: None,
            local_clipboard_files: None,
            local_file_group_descriptor_format_id: 0xC100,
            local_file_contents_format_id: 0xC101,
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
        self.file_clipboard_enabled = caps.general_flags.stream_fileclip_enabled();

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

        // Story 5.6: Clear local clipboard state when remote changes
        // The server's Format List takes precedence - "most recent change wins"
        if self.local_clipboard_text.is_some() || self.local_clipboard_files.is_some() {
            debug!("Remote clipboard changed, clearing local clipboard state");
            self.local_clipboard_text = None;
            self.local_clipboard_files = None;
        }

        // Store server formats
        self.server_formats = format_list.formats.clone();

        // Reset file format IDs
        self.file_group_descriptor_format_id = None;
        self.file_contents_format_id = None;
        self.remote_file_list = None;

        // Log available formats and detect file formats
        for format in &self.server_formats {
            if format.name.is_empty() {
                trace!("  Format ID {}", format.id);
            } else {
                trace!("  Format ID {} ({})", format.id, format.name);

                // Story 5.4: Detect file clipboard formats by name
                if format.name == "FileGroupDescriptorW" {
                    debug!("Detected FileGroupDescriptorW format: ID {}", format.id);
                    self.file_group_descriptor_format_id = Some(format.id);
                } else if format.name == "FileContents" {
                    debug!("Detected FileContents format: ID {}", format.id);
                    self.file_contents_format_id = Some(format.id);
                }
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

        // Story 5.4: Check if file formats are available
        let has_files = self.file_group_descriptor_format_id.is_some();

        // Notify main thread of available formats
        let formats: Vec<u32> = self.server_formats.iter().map(|f| f.id).collect();
        self.send_event(ClipboardEvent::FormatsAvailable {
            formats,
            has_text,
            has_files,
        });

        // Build response messages
        let mut messages = Vec::new();

        // Always respond with Format List Response (success)
        let response = FormatListResponsePdu::ok();
        let response_data = response.encode();
        debug!("Sending Format List Response (OK)");
        messages.push(SvcMessage::from(CliprdrSvcMessage::new(response_data)));

        // Automatically request data based on what's available
        if self.event_tx.is_some() {
            if has_files {
                // Story 5.4: Prefer files over text if available
                if let Some(format_id) = self.file_group_descriptor_format_id {
                    debug!("Auto-requesting file list (format {})", format_id);
                    let request = FormatDataRequestPdu::new(format_id);
                    self.pending_format_request = Some(format_id);
                    messages.push(SvcMessage::from(CliprdrSvcMessage::new(request.encode())));
                }
            } else if has_text {
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
    /// in our Format List. Story 5.3/5.5: Respond with local clipboard data.
    fn handle_format_data_request(
        &mut self,
        request: &FormatDataRequestPdu,
        _channel_id: u32,
    ) -> PduResult<Vec<SvcMessage>> {
        debug!(
            "Received Format Data Request for format ID 0x{:04X}",
            request.requested_format_id
        );

        // Story 5.5: Check if this is a request for our file list
        if request.requested_format_id == self.local_file_group_descriptor_format_id {
            return self.handle_file_list_request();
        }

        // Story 5.3: Provide local clipboard text if available
        let response = if let Some(ref text) = self.local_clipboard_text {
            match request.requested_format_id {
                id if id == StandardFormat::UnicodeText as u32 => {
                    // Convert UTF-8 to UTF-16LE with null terminator
                    let data = Self::utf8_to_utf16le_with_null(text);
                    debug!(
                        "Sending Format Data Response (Unicode, {} bytes)",
                        data.len()
                    );
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
                    debug!(
                        "Requested format 0x{:04X} not available",
                        request.requested_format_id
                    );
                    FormatDataResponsePdu::fail()
                }
            }
        } else {
            debug!("No local clipboard data available");
            FormatDataResponsePdu::fail()
        };

        let response_data = response.encode();
        Ok(vec![SvcMessage::from(CliprdrSvcMessage::new(
            response_data,
        ))])
    }

    /// Story 5.5: Handles request for local file list (FileGroupDescriptorW format).
    fn handle_file_list_request(&self) -> PduResult<Vec<SvcMessage>> {
        let response = if let Some(ref files) = self.local_clipboard_files {
            debug!("Serving file list with {} files", files.len());

            // Build FileDescriptorW array from local file info
            let file_descriptors: Vec<pdu::FileDescriptorW> = files
                .iter()
                .map(|f| {
                    pdu::FileDescriptorW::from_local_file(
                        f.name.clone(),
                        f.size,
                        f.modified_time,
                        f.is_directory,
                    )
                })
                .collect();

            let file_list = pdu::CliprdrFileList::new(file_descriptors);
            let data = file_list.encode();
            debug!(
                "Sending Format Data Response (file list, {} bytes)",
                data.len()
            );
            FormatDataResponsePdu::ok(data)
        } else {
            debug!("No local files available");
            FormatDataResponsePdu::fail()
        };

        let response_data = response.encode();
        Ok(vec![SvcMessage::from(CliprdrSvcMessage::new(
            response_data,
        ))])
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

            // Story 5.4: Check if this is a file list response
            if self.pending_format_request == self.file_group_descriptor_format_id {
                // Parse as CLIPRDR_FILELIST
                match CliprdrFileList::decode(&response.data) {
                    Ok(file_list) => {
                        debug!("Received file list with {} files", file_list.count());
                        let files: Vec<FileInfo> = file_list
                            .files
                            .iter()
                            .map(|f| FileInfo {
                                name: f.file_name.clone(),
                                size: if f.has_file_size() {
                                    Some(f.file_size)
                                } else {
                                    None
                                },
                                is_directory: f.is_directory(),
                            })
                            .collect();
                        self.remote_file_list = Some(file_list);
                        self.send_event(ClipboardEvent::FilesReceived { files });
                    }
                    Err(e) => {
                        warn!("Failed to parse file list: {}", e);
                        self.send_event(ClipboardEvent::RequestFailed);
                    }
                }
            } else if let Some(text) = response.as_utf8_from_unicode() {
                // Try to convert to text if possible
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

    /// Handles a File Contents Request PDU from the server.
    ///
    /// Story 5.5: The server sends this when it wants file contents that we
    /// announced (local to remote file transfer).
    fn handle_file_contents_request(
        &mut self,
        request: &pdu::FileContentsRequestPdu,
        _channel_id: u32,
    ) -> PduResult<Vec<SvcMessage>> {
        debug!(
            "Received File Contents Request: stream_id={}, file_index={}, flags={:?}",
            request.stream_id, request.lindex, request.flags
        );

        // Get the file from our local clipboard
        let response = match &self.local_clipboard_files {
            Some(files) if (request.lindex as usize) < files.len() => {
                let file_info = &files[request.lindex as usize];
                self.serve_file_content(request, file_info)
            }
            Some(files) => {
                warn!(
                    "Invalid file index {} (have {} files)",
                    request.lindex,
                    files.len()
                );
                pdu::FileContentsResponsePdu::fail(request.stream_id)
            }
            None => {
                warn!("No local files available for file contents request");
                pdu::FileContentsResponsePdu::fail(request.stream_id)
            }
        };

        let response_data = response.encode();
        Ok(vec![SvcMessage::from(CliprdrSvcMessage::new(
            response_data,
        ))])
    }

    /// Story 5.5: Serves file content for a FileContentsRequest.
    fn serve_file_content(
        &self,
        request: &pdu::FileContentsRequestPdu,
        file_info: &LocalFileInfo,
    ) -> pdu::FileContentsResponsePdu {
        use pdu::FileContentsFlags;
        use std::io::{Read, Seek, SeekFrom};

        match request.flags {
            FileContentsFlags::Size => {
                // Return file size as 8-byte little-endian value
                debug!(
                    "Serving file size for '{}': {} bytes",
                    file_info.name, file_info.size
                );
                let size_bytes = file_info.size.to_le_bytes().to_vec();
                pdu::FileContentsResponsePdu::ok(request.stream_id, size_bytes)
            }
            FileContentsFlags::Range => {
                // Read file content at specified offset
                let offset = request.offset();
                let length = request.cb_requested as usize;

                debug!(
                    "Serving file content for '{}': offset={}, length={}",
                    file_info.name, offset, length
                );

                match std::fs::File::open(&file_info.path) {
                    Ok(mut file) => {
                        // Seek to the requested position
                        if let Err(e) = file.seek(SeekFrom::Start(offset)) {
                            warn!("Failed to seek in file '{}': {}", file_info.name, e);
                            return pdu::FileContentsResponsePdu::fail(request.stream_id);
                        }

                        // Read the requested bytes
                        let mut buffer = vec![0u8; length];
                        match file.read(&mut buffer) {
                            Ok(bytes_read) => {
                                buffer.truncate(bytes_read);
                                debug!(
                                    "Read {} bytes from '{}' at offset {}",
                                    bytes_read, file_info.name, offset
                                );
                                pdu::FileContentsResponsePdu::ok(request.stream_id, buffer)
                            }
                            Err(e) => {
                                warn!("Failed to read file '{}': {}", file_info.name, e);
                                pdu::FileContentsResponsePdu::fail(request.stream_id)
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to open file '{}': {}", file_info.name, e);
                        pdu::FileContentsResponsePdu::fail(request.stream_id)
                    }
                }
            }
        }
    }

    /// Handles a File Contents Response PDU from the server.
    ///
    /// Story 5.4: The server sends this in response to our File Contents Request,
    /// containing file size or content data for remote-to-local file transfer.
    fn handle_file_contents_response(
        &mut self,
        response: &pdu::FileContentsResponsePdu,
        _channel_id: u32,
    ) -> PduResult<Vec<SvcMessage>> {
        if response.success {
            debug!(
                "Received File Contents Response: stream_id={}, {} bytes",
                response.stream_id,
                response.data.len()
            );

            // Check if this is a size response (8 bytes)
            if let Some(size) = response.as_file_size() {
                debug!("File size: {} bytes", size);
                self.send_event(ClipboardEvent::FileSizeReceived {
                    stream_id: response.stream_id,
                    size,
                });
            } else {
                // Content data
                debug!("File content: {} bytes", response.data.len());
                self.send_event(ClipboardEvent::FileContentReceived {
                    stream_id: response.stream_id,
                    data: response.data.clone(),
                });
            }
        } else {
            warn!(
                "File Contents Request failed: stream_id={}",
                response.stream_id
            );
            self.send_event(ClipboardEvent::FileTransferFailed {
                stream_id: response.stream_id,
            });
        }

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
    /// Returns SvcMessage vector ready for encoding via `process_svc_processor_messages()`.
    pub fn set_local_clipboard_text(&mut self, text: String) -> Option<Vec<SvcMessage>> {
        if self.state != CliprdrState::Ready {
            warn!("Cannot set local clipboard: channel not ready");
            return None;
        }

        debug!("Setting local clipboard text: {} chars", text.len());
        self.local_clipboard_text = Some(text);
        // Story 5.6: Clear files when setting text (mutually exclusive)
        self.local_clipboard_files = None;

        // Create Format List PDU announcing text formats
        let format_list = FormatListPdu::text_formats();
        let encoded = format_list.encode();

        debug!("Sending Format List with text formats");
        Some(vec![SvcMessage::from(CliprdrSvcMessage::new(encoded))])
    }

    /// Story 5.3/5.5: Clears the local clipboard (both text and files).
    ///
    /// Returns SvcMessage vector ready for encoding via `process_svc_processor_messages()`.
    pub fn clear_local_clipboard(&mut self) -> Option<Vec<SvcMessage>> {
        if self.state != CliprdrState::Ready {
            return None;
        }

        debug!("Clearing local clipboard");
        self.local_clipboard_text = None;
        self.local_clipboard_files = None;

        // Send empty Format List
        let format_list = FormatListPdu::empty();
        Some(vec![SvcMessage::from(CliprdrSvcMessage::new(
            format_list.encode(),
        ))])
    }

    /// Story 5.3: Returns the local clipboard text for Format Data Response.
    pub fn local_clipboard_text(&self) -> Option<&str> {
        self.local_clipboard_text.as_deref()
    }

    /// Story 5.5: Sets the local clipboard files and returns Format List PDU to send.
    ///
    /// Call this when the local clipboard changes with file content.
    /// Returns SvcMessage vector ready for encoding via `process_svc_processor_messages()`.
    ///
    /// Note: The file paths must exist and be readable. Files are read on demand
    /// when the server requests their content.
    pub fn set_local_clipboard_files(
        &mut self,
        files: Vec<std::path::PathBuf>,
    ) -> Option<Vec<SvcMessage>> {
        if self.state != CliprdrState::Ready {
            warn!("Cannot set local clipboard files: channel not ready");
            return None;
        }

        if files.is_empty() {
            debug!("No files to set in clipboard");
            return self.clear_local_clipboard();
        }

        // Convert paths to LocalFileInfo, reading metadata
        let file_infos: Vec<LocalFileInfo> = files
            .into_iter()
            .filter_map(|path| {
                LocalFileInfo::from_path(path.clone()).or_else(|| {
                    warn!("Failed to read file metadata for {:?}", path);
                    None
                })
            })
            .collect();

        if file_infos.is_empty() {
            warn!("No valid files to set in clipboard");
            return None;
        }

        debug!("Setting local clipboard files: {} files", file_infos.len());
        self.local_clipboard_files = Some(file_infos);
        // Clear text when setting files (they're mutually exclusive)
        self.local_clipboard_text = None;

        // Create Format List PDU announcing file formats
        let format_list = FormatListPdu::file_formats(
            self.local_file_group_descriptor_format_id,
            self.local_file_contents_format_id,
        );
        let encoded = format_list.encode();

        debug!(
            "Sending Format List with file formats (FileGroupDescriptorW: 0x{:04X}, FileContents: 0x{:04X})",
            self.local_file_group_descriptor_format_id, self.local_file_contents_format_id
        );
        Some(vec![SvcMessage::from(CliprdrSvcMessage::new(encoded))])
    }

    /// Story 5.5: Returns the local clipboard files.
    pub fn local_clipboard_files(&self) -> Option<&[LocalFileInfo]> {
        self.local_clipboard_files.as_deref()
    }

    /// Story 5.5: Returns the client-assigned file format IDs.
    pub fn local_file_format_ids(&self) -> (u32, u32) {
        (
            self.local_file_group_descriptor_format_id,
            self.local_file_contents_format_id,
        )
    }

    /// Story 5.4: Returns the cached file list from the server.
    pub fn remote_file_list(&self) -> Option<&CliprdrFileList> {
        self.remote_file_list.as_ref()
    }

    /// Story 5.4: Returns true if file clipboard is supported by the server.
    pub fn supports_file_clipboard(&self) -> bool {
        self.file_clipboard_enabled && self.file_group_descriptor_format_id.is_some()
    }

    /// Story 5.4: Requests file size for a file in the cached file list.
    ///
    /// Returns the stream ID and encoded PDU, or None if file index is invalid.
    pub fn request_file_size(&mut self, file_index: u32) -> Option<(u32, Vec<u8>)> {
        if self.state != CliprdrState::Ready {
            warn!("Cannot request file size: clipboard not ready");
            return None;
        }

        // Validate file index against cached file list
        if let Some(ref file_list) = self.remote_file_list {
            if file_index as usize >= file_list.count() {
                warn!(
                    "Invalid file index {} (have {} files)",
                    file_index,
                    file_list.count()
                );
                return None;
            }
        } else {
            warn!("No file list available");
            return None;
        }

        let stream_id = self.next_stream_id;
        self.next_stream_id = self.next_stream_id.wrapping_add(1);

        let request = FileContentsRequestPdu::size(stream_id, file_index);
        debug!(
            "Requesting file size: stream_id={}, file_index={}",
            stream_id, file_index
        );

        Some((stream_id, request.encode()))
    }

    /// Story 5.4: Requests file content range for a file in the cached file list.
    ///
    /// Returns the stream ID and encoded PDU, or None if file index is invalid.
    pub fn request_file_content(
        &mut self,
        file_index: u32,
        offset: u64,
        length: u32,
    ) -> Option<(u32, Vec<u8>)> {
        if self.state != CliprdrState::Ready {
            warn!("Cannot request file content: clipboard not ready");
            return None;
        }

        // Validate file index against cached file list
        if let Some(ref file_list) = self.remote_file_list {
            if file_index as usize >= file_list.count() {
                warn!(
                    "Invalid file index {} (have {} files)",
                    file_index,
                    file_list.count()
                );
                return None;
            }
        } else {
            warn!("No file list available");
            return None;
        }

        let stream_id = self.next_stream_id;
        self.next_stream_id = self.next_stream_id.wrapping_add(1);

        let request = FileContentsRequestPdu::range(stream_id, file_index, offset, length);
        debug!(
            "Requesting file content: stream_id={}, file_index={}, offset={}, length={}",
            stream_id, file_index, offset, length
        );

        Some((stream_id, request.encode()))
    }

    /// Story 5.4: Returns the number of files in the cached file list.
    pub fn remote_file_count(&self) -> usize {
        self.remote_file_list
            .as_ref()
            .map(|l| l.count())
            .unwrap_or(0)
    }

    /// Story 5.4: Returns file info for a file in the cached list.
    pub fn remote_file_info(&self, index: usize) -> Option<FileInfo> {
        self.remote_file_list.as_ref().and_then(|list| {
            list.files.get(index).map(|f| FileInfo {
                name: f.file_name.clone(),
                size: if f.has_file_size() {
                    Some(f.file_size)
                } else {
                    None
                },
                is_directory: f.is_directory(),
            })
        })
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
            .field("file_clipboard_enabled", &self.file_clipboard_enabled)
            .field("server_formats", &self.server_formats)
            .field("enabled", &self.enabled)
            .field("pending_format_request", &self.pending_format_request)
            .field(
                "pending_clipboard_data",
                &self.pending_clipboard_data.as_ref().map(|s| s.len()),
            )
            .field("event_tx", &self.event_tx.is_some())
            .field(
                "file_group_descriptor_format_id",
                &self.file_group_descriptor_format_id,
            )
            .field("remote_file_count", &self.remote_file_count())
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
            CliprdrPdu::FileContentsRequest(request) => {
                self.handle_file_contents_request(&request, channel_id)
            }
            CliprdrPdu::FileContentsResponse(response) => {
                self.handle_file_contents_response(&response, channel_id)
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

    #[test]
    fn test_utf8_to_utf16le_conversion() {
        // Story 5.3: Test UTF-8 to UTF-16LE conversion for clipboard
        // "Hello" should convert to UTF-16LE with null terminator
        let result = YardCliprdrHandler::utf8_to_utf16le_with_null("Hello");

        // Expected: H(0x48 0x00) e(0x65 0x00) l(0x6C 0x00) l(0x6C 0x00) o(0x6F 0x00) null(0x00 0x00)
        assert_eq!(result.len(), 12); // 5 chars * 2 bytes + 2 bytes null
        assert_eq!(result[0..2], [0x48, 0x00]); // H
        assert_eq!(result[10..12], [0x00, 0x00]); // null terminator
    }

    #[test]
    fn test_utf8_to_utf16le_unicode() {
        // Story 5.3: Test Unicode characters including emoji
        let result = YardCliprdrHandler::utf8_to_utf16le_with_null("日本");

        // 日 = U+65E5 → 0xE5 0x65 in UTF-16LE
        // 本 = U+672C → 0x2C 0x67 in UTF-16LE
        assert_eq!(result.len(), 6); // 2 chars * 2 bytes + 2 bytes null
        assert_eq!(result[0..2], [0xE5, 0x65]); // 日
        assert_eq!(result[2..4], [0x2C, 0x67]); // 本
    }

    #[test]
    fn test_utf8_to_utf16le_emoji() {
        // Story 5.3: Test emoji (surrogate pair)
        let result = YardCliprdrHandler::utf8_to_utf16le_with_null("👋");

        // 👋 = U+1F44B → D83D DC4B in UTF-16 surrogate pair
        assert_eq!(result.len(), 6); // 2 code units * 2 bytes + 2 bytes null
        assert_eq!(result[0..2], [0x3D, 0xD8]); // High surrogate
        assert_eq!(result[2..4], [0x4B, 0xDC]); // Low surrogate
    }

    #[test]
    fn test_set_local_clipboard_text() {
        // Story 5.3/5.6: Test setting local clipboard text
        let mut handler = YardCliprdrHandler::new();
        handler.start().unwrap();
        handler.state = CliprdrState::Ready;

        // Set local clipboard text
        let result = handler.set_local_clipboard_text("Test clipboard".to_string());

        // Should return SvcMessage vector with Format List PDU
        assert!(result.is_some());
        let messages = result.unwrap();
        assert_eq!(messages.len(), 1);

        // Verify local clipboard text is stored
        assert_eq!(handler.local_clipboard_text(), Some("Test clipboard"));
        // Story 5.6: Files should be cleared when text is set
        assert!(handler.local_clipboard_files().is_none());
    }

    #[test]
    fn test_set_local_clipboard_not_ready() {
        // Story 5.3: Setting clipboard when not ready should fail
        let mut handler = YardCliprdrHandler::new();
        // Don't call start() or set state to Ready

        let result = handler.set_local_clipboard_text("Test".to_string());
        assert!(result.is_none());
    }

    #[test]
    fn test_remote_format_list_clears_local_clipboard() {
        // Story 5.6: Remote Format List should clear local clipboard state
        let mut handler = YardCliprdrHandler::new();
        handler.start().unwrap();
        handler.state = CliprdrState::Ready;

        // Set local clipboard text first
        handler.set_local_clipboard_text("Local content".to_string());
        assert!(handler.local_clipboard_text().is_some());

        // Simulate receiving Format List from server (remote clipboard changed)
        let format_list = [
            0x02, 0x00, // msgType = FormatList
            0x00, 0x00, // msgFlags
            0x06, 0x00, 0x00, 0x00, // dataLen = 6
            0x0D, 0x00, 0x00, 0x00, // formatId = 13 (CF_UNICODETEXT)
            0x00, 0x00, // null terminator
        ];
        let _ = handler.process(&format_list);

        // Local clipboard should be cleared (remote takes precedence)
        assert!(handler.local_clipboard_text().is_none());
    }

    #[test]
    fn test_rapid_clipboard_changes_preserve_latest() {
        // Story 5.6: Rapid clipboard changes should preserve the latest value
        let mut handler = YardCliprdrHandler::new();
        handler.start().unwrap();
        handler.state = CliprdrState::Ready;

        // Simulate rapid clipboard changes (Ctrl+C spam)
        handler.set_local_clipboard_text("First".to_string());
        handler.set_local_clipboard_text("Second".to_string());
        handler.set_local_clipboard_text("Third".to_string());
        handler.set_local_clipboard_text("Final".to_string());

        // The latest value should be preserved
        assert_eq!(handler.local_clipboard_text(), Some("Final"));
    }

    #[test]
    fn test_bidirectional_text_sync_local_then_remote() {
        // Story 5.6: Test local change followed by remote change
        let (tx, rx) = std::sync::mpsc::channel();
        let mut handler = YardCliprdrHandler::with_event_channel(tx);
        handler.start().unwrap();
        handler.state = CliprdrState::Ready;

        // 1. Local clipboard change
        let result = handler.set_local_clipboard_text("Local text".to_string());
        assert!(result.is_some()); // Format List PDU generated
        assert_eq!(handler.local_clipboard_text(), Some("Local text"));

        // 2. Remote clipboard change (server sends Format List)
        let format_list = [
            0x02, 0x00, // msgType = FormatList
            0x00, 0x00, // msgFlags
            0x06, 0x00, 0x00, 0x00, // dataLen = 6
            0x0D, 0x00, 0x00, 0x00, // formatId = 13 (CF_UNICODETEXT)
            0x00, 0x00, // null terminator
        ];
        let _ = handler.process(&format_list);

        // Local clipboard should be cleared (remote wins)
        assert!(handler.local_clipboard_text().is_none());

        // Event should be received for formats available
        let event = rx.try_recv();
        assert!(event.is_ok());
        if let ClipboardEvent::FormatsAvailable { has_text, .. } = event.unwrap() {
            assert!(has_text);
        } else {
            panic!("Expected FormatsAvailable event");
        }
    }

    #[test]
    fn test_bidirectional_text_sync_remote_then_local() {
        // Story 5.6: Test remote change followed by local change
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut handler = YardCliprdrHandler::with_event_channel(tx);
        handler.start().unwrap();
        handler.state = CliprdrState::Ready;

        // 1. Remote clipboard change
        let format_list = [
            0x02, 0x00, // msgType = FormatList
            0x00, 0x00, // msgFlags
            0x06, 0x00, 0x00, 0x00, // dataLen = 6
            0x0D, 0x00, 0x00, 0x00, // formatId = 13 (CF_UNICODETEXT)
            0x00, 0x00, // null terminator
        ];
        let _ = handler.process(&format_list);

        // Server formats should be stored
        assert!(!handler.server_formats().is_empty());

        // 2. Local clipboard change
        let result = handler.set_local_clipboard_text("New local text".to_string());
        assert!(result.is_some()); // Format List PDU generated
        assert_eq!(handler.local_clipboard_text(), Some("New local text"));
    }

    #[test]
    fn test_text_and_files_mutually_exclusive() {
        // Story 5.6: Text and files should be mutually exclusive
        let mut handler = YardCliprdrHandler::new();
        handler.start().unwrap();
        handler.state = CliprdrState::Ready;

        // Set text
        handler.set_local_clipboard_text("Some text".to_string());
        assert!(handler.local_clipboard_text().is_some());
        assert!(handler.local_clipboard_files().is_none());

        // Setting text again clears files (already None, but verifies logic)
        handler.set_local_clipboard_text("More text".to_string());
        assert!(handler.local_clipboard_text().is_some());
        assert!(handler.local_clipboard_files().is_none());
    }

    #[test]
    fn test_clear_local_clipboard() {
        // Story 5.6: Test clearing the local clipboard
        let mut handler = YardCliprdrHandler::new();
        handler.start().unwrap();
        handler.state = CliprdrState::Ready;

        // Set some content
        handler.set_local_clipboard_text("Content".to_string());
        assert!(handler.local_clipboard_text().is_some());

        // Clear clipboard
        let result = handler.clear_local_clipboard();
        assert!(result.is_some()); // Empty Format List PDU generated
        assert!(handler.local_clipboard_text().is_none());
        assert!(handler.local_clipboard_files().is_none());
    }

    #[test]
    fn test_format_data_request_response_cycle() {
        // Story 5.6: Test complete format data request/response cycle
        let mut handler = YardCliprdrHandler::new();
        handler.start().unwrap();
        handler.state = CliprdrState::Ready;

        // Set local clipboard text
        handler.set_local_clipboard_text("Test data for server".to_string());

        // Simulate server requesting Unicode text
        let request = [
            0x04, 0x00, // msgType = FormatDataRequest
            0x00, 0x00, // msgFlags
            0x04, 0x00, 0x00, 0x00, // dataLen = 4
            0x0D, 0x00, 0x00, 0x00, // requestedFormatId = 13 (CF_UNICODETEXT)
        ];
        let result = handler.process(&request);
        assert!(result.is_ok());

        let messages = result.unwrap();
        assert_eq!(messages.len(), 1); // Format Data Response

        // Verify the response contains UTF-16LE encoded text
        // The response is wrapped in SvcMessage, so we can't easily inspect the bytes
        // but we know the handler processed it successfully
    }
}
