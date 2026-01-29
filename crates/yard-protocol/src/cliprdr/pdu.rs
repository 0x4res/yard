//! CLIPRDR PDU structures per MS-RDPECLIP specification.
//!
//! This module implements the Protocol Data Units for the Clipboard
//! Virtual Channel Extension (CLIPRDR).
//!
//! Reference: [MS-RDPECLIP] Remote Desktop Protocol: Clipboard Virtual
//! Channel Extension

use thiserror::Error;

/// CLIPRDR-specific error type.
#[derive(Debug, Error)]
pub enum CliprdrError {
    /// Invalid message type.
    #[error("invalid CLIPRDR message type: {0:#x}")]
    InvalidMessageType(u16),
    /// Payload too short.
    #[error("CLIPRDR payload too short for {context}: expected {expected}, got {actual}")]
    PayloadTooShort {
        context: &'static str,
        expected: usize,
        actual: usize,
    },
    /// Invalid PDU flags.
    #[error("invalid CLIPRDR PDU flags: {0:#x}")]
    InvalidFlags(u16),
    /// Format list parsing error.
    #[error("failed to parse format list: {0}")]
    FormatListParse(String),
}

/// CLIPRDR PDU header size in bytes.
/// Header: msgType (2) + msgFlags (2) + dataLen (4) = 8 bytes
pub const CLIPRDR_HEADER_SIZE: usize = 8;

/// CLIPRDR PDU message types (msgType field).
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    /// Monitor Ready PDU (CB_MONITOR_READY) - section 2.2.2.2
    MonitorReady = 0x0001,
    /// Format List PDU (CB_FORMAT_LIST) - section 2.2.3.1
    FormatList = 0x0002,
    /// Format List Response PDU (CB_FORMAT_LIST_RESPONSE) - section 2.2.3.2
    FormatListResponse = 0x0003,
    /// Format Data Request PDU (CB_FORMAT_DATA_REQUEST) - section 2.2.5.1
    FormatDataRequest = 0x0004,
    /// Format Data Response PDU (CB_FORMAT_DATA_RESPONSE) - section 2.2.5.2
    FormatDataResponse = 0x0005,
    /// Temporary Directory PDU (CB_TEMP_DIRECTORY) - section 2.2.4.1
    TempDirectory = 0x0006,
    /// Clip Capabilities PDU (CB_CLIP_CAPS) - section 2.2.2.1
    ClipCaps = 0x0007,
    /// File Contents Request PDU (CB_FILECONTENTS_REQUEST) - section 2.2.5.3
    FileContentsRequest = 0x0008,
    /// File Contents Response PDU (CB_FILECONTENTS_RESPONSE) - section 2.2.5.4
    FileContentsResponse = 0x0009,
    /// Lock Clipboard Data PDU (CB_LOCK_CLIPDATA) - section 2.2.4.2
    LockClipData = 0x000A,
    /// Unlock Clipboard Data PDU (CB_UNLOCK_CLIPDATA) - section 2.2.4.3
    UnlockClipData = 0x000B,
}

impl TryFrom<u16> for MessageType {
    type Error = CliprdrError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            0x0001 => Ok(Self::MonitorReady),
            0x0002 => Ok(Self::FormatList),
            0x0003 => Ok(Self::FormatListResponse),
            0x0004 => Ok(Self::FormatDataRequest),
            0x0005 => Ok(Self::FormatDataResponse),
            0x0006 => Ok(Self::TempDirectory),
            0x0007 => Ok(Self::ClipCaps),
            0x0008 => Ok(Self::FileContentsRequest),
            0x0009 => Ok(Self::FileContentsResponse),
            0x000A => Ok(Self::LockClipData),
            0x000B => Ok(Self::UnlockClipData),
            _ => Err(CliprdrError::InvalidMessageType(value)),
        }
    }
}

/// CLIPRDR PDU message flags (msgFlags field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageFlags(u16);

impl MessageFlags {
    /// CB_RESPONSE_OK - Request completed successfully.
    pub const RESPONSE_OK: Self = Self(0x0001);
    /// CB_RESPONSE_FAIL - Request failed.
    pub const RESPONSE_FAIL: Self = Self(0x0002);
    /// CB_ASCII_NAMES - Format list uses ASCII names (short format names).
    pub const ASCII_NAMES: Self = Self(0x0004);

    /// Creates empty flags.
    pub fn empty() -> Self {
        Self(0)
    }

    /// Creates flags from raw value.
    pub fn from_bits(bits: u16) -> Self {
        Self(bits)
    }

    /// Returns the raw flags value.
    pub fn bits(&self) -> u16 {
        self.0
    }

    /// Returns true if RESPONSE_OK flag is set.
    pub fn is_ok(&self) -> bool {
        self.0 & Self::RESPONSE_OK.0 != 0
    }

    /// Returns true if RESPONSE_FAIL flag is set.
    pub fn is_fail(&self) -> bool {
        self.0 & Self::RESPONSE_FAIL.0 != 0
    }

    /// Returns true if ASCII_NAMES flag is set.
    pub fn has_ascii_names(&self) -> bool {
        self.0 & Self::ASCII_NAMES.0 != 0
    }
}

/// Standard clipboard format identifiers (per Windows API).
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardFormat {
    /// CF_TEXT - ANSI text.
    Text = 1,
    /// CF_BITMAP - Bitmap.
    Bitmap = 2,
    /// CF_METAFILEPICT - Metafile picture.
    MetafilePict = 3,
    /// CF_SYLK - Symbolic link.
    Sylk = 4,
    /// CF_DIF - Data interchange format.
    Dif = 5,
    /// CF_TIFF - TIFF image.
    Tiff = 6,
    /// CF_OEMTEXT - OEM text.
    OemText = 7,
    /// CF_DIB - Device-independent bitmap.
    Dib = 8,
    /// CF_PALETTE - Color palette.
    Palette = 9,
    /// CF_PENDATA - Pen data.
    PenData = 10,
    /// CF_RIFF - RIFF audio.
    Riff = 11,
    /// CF_WAVE - Wave audio.
    Wave = 12,
    /// CF_UNICODETEXT - Unicode text.
    UnicodeText = 13,
    /// CF_ENHMETAFILE - Enhanced metafile.
    EnhMetafile = 14,
    /// CF_HDROP - File list (for file transfers).
    Hdrop = 15,
    /// CF_LOCALE - Locale identifier.
    Locale = 16,
    /// CF_DIBV5 - Device-independent bitmap v5.
    DibV5 = 17,
}

impl TryFrom<u32> for StandardFormat {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Text),
            2 => Ok(Self::Bitmap),
            3 => Ok(Self::MetafilePict),
            4 => Ok(Self::Sylk),
            5 => Ok(Self::Dif),
            6 => Ok(Self::Tiff),
            7 => Ok(Self::OemText),
            8 => Ok(Self::Dib),
            9 => Ok(Self::Palette),
            10 => Ok(Self::PenData),
            11 => Ok(Self::Riff),
            12 => Ok(Self::Wave),
            13 => Ok(Self::UnicodeText),
            14 => Ok(Self::EnhMetafile),
            15 => Ok(Self::Hdrop),
            16 => Ok(Self::Locale),
            17 => Ok(Self::DibV5),
            _ => Err(()),
        }
    }
}

/// A clipboard format entry with ID and optional name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardFormat {
    /// Format ID (standard or registered).
    pub id: u32,
    /// Format name (for registered formats, empty for standard formats).
    pub name: String,
}

impl ClipboardFormat {
    /// Creates a new clipboard format.
    pub fn new(id: u32, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
        }
    }

    /// Creates a standard format (no name).
    pub fn standard(format: StandardFormat) -> Self {
        Self {
            id: format as u32,
            name: String::new(),
        }
    }

    /// Returns true if this is a text format (CF_TEXT or CF_UNICODETEXT).
    pub fn is_text(&self) -> bool {
        self.id == StandardFormat::Text as u32 || self.id == StandardFormat::UnicodeText as u32
    }

    /// Returns true if this is a file list format (CF_HDROP).
    pub fn is_file_list(&self) -> bool {
        self.id == StandardFormat::Hdrop as u32
    }
}

/// General capability set type.
const CB_CAPSTYPE_GENERAL: u16 = 0x0001;

/// General capability set version.
const CB_CAPS_VERSION_2: u32 = 0x00000002;

/// Capability flags for CB_GENERAL_CAPABILITY_SET.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeneralCapabilityFlags(u32);

impl GeneralCapabilityFlags {
    /// CB_USE_LONG_FORMAT_NAMES - Support long format names.
    pub const USE_LONG_FORMAT_NAMES: Self = Self(0x00000002);
    /// CB_STREAM_FILECLIP_ENABLED - Support file clipboard operations.
    pub const STREAM_FILECLIP_ENABLED: Self = Self(0x00000004);
    /// CB_FILECLIP_NO_FILE_PATHS - Don't include file paths in file operations.
    pub const FILECLIP_NO_FILE_PATHS: Self = Self(0x00000008);
    /// CB_CAN_LOCK_CLIPDATA - Support clipboard locking.
    pub const CAN_LOCK_CLIPDATA: Self = Self(0x00000010);
    /// CB_HUGE_FILE_SUPPORT_ENABLED - Support files larger than 4GB.
    pub const HUGE_FILE_SUPPORT_ENABLED: Self = Self(0x00000020);

    /// Creates empty flags.
    pub fn empty() -> Self {
        Self(0)
    }

    /// Creates flags from raw value.
    pub fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Returns the raw flags value.
    pub fn bits(&self) -> u32 {
        self.0
    }

    /// Returns true if long format names are supported.
    pub fn use_long_format_names(&self) -> bool {
        self.0 & Self::USE_LONG_FORMAT_NAMES.0 != 0
    }

    /// Returns true if file clipboard is enabled.
    pub fn stream_fileclip_enabled(&self) -> bool {
        self.0 & Self::STREAM_FILECLIP_ENABLED.0 != 0
    }

    /// Combines two capability flags.
    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Clip Capabilities PDU (CB_CLIP_CAPS) - section 2.2.2.1.
///
/// Structure:
/// - Header (8 bytes): msgType=0x0007, msgFlags=0, dataLen
/// - cCapabilitiesSets (2 bytes): Number of capability sets
/// - pad1 (2 bytes): Padding
/// - capabilitySets (variable): Array of capability sets
#[derive(Debug, Clone)]
pub struct ClipCapsPdu {
    /// General capability flags.
    pub general_flags: GeneralCapabilityFlags,
}

impl ClipCapsPdu {
    /// Creates a new Clip Capabilities PDU with the given flags.
    pub fn new(flags: GeneralCapabilityFlags) -> Self {
        Self {
            general_flags: flags,
        }
    }

    /// Creates default client capabilities.
    pub fn default_client() -> Self {
        // Support long format names for better format identification
        Self::new(GeneralCapabilityFlags::USE_LONG_FORMAT_NAMES)
    }

    /// Decodes a Clip Capabilities PDU from payload (after header).
    pub fn decode(payload: &[u8]) -> Result<Self, CliprdrError> {
        if payload.len() < 4 {
            return Err(CliprdrError::PayloadTooShort {
                context: "ClipCapsPdu",
                expected: 4,
                actual: payload.len(),
            });
        }

        let n_capabilities = u16::from_le_bytes([payload[0], payload[1]]);
        // Skip pad1 (2 bytes)
        let mut offset = 4;

        let mut general_flags = GeneralCapabilityFlags::empty();

        for _ in 0..n_capabilities {
            if offset + 4 > payload.len() {
                break;
            }

            let cap_type = u16::from_le_bytes([payload[offset], payload[offset + 1]]);
            let cap_len = u16::from_le_bytes([payload[offset + 2], payload[offset + 3]]) as usize;

            if cap_type == CB_CAPSTYPE_GENERAL && cap_len >= 12 && offset + cap_len <= payload.len()
            {
                // CB_GENERAL_CAPABILITY_SET: capabilitySetType (2) + lengthCapability (2) +
                // version (4) + generalFlags (4)
                let flags_offset = offset + 8;
                if flags_offset + 4 <= payload.len() {
                    let flags = u32::from_le_bytes([
                        payload[flags_offset],
                        payload[flags_offset + 1],
                        payload[flags_offset + 2],
                        payload[flags_offset + 3],
                    ]);
                    general_flags = GeneralCapabilityFlags::from_bits(flags);
                }
            }

            offset += cap_len;
        }

        Ok(Self { general_flags })
    }

    /// Encodes the Clip Capabilities PDU to bytes (without header).
    pub fn encode_payload(&self) -> Vec<u8> {
        // CB_GENERAL_CAPABILITY_SET: type (2) + len (2) + version (4) + flags (4) = 12 bytes
        // Plus: cCapabilitiesSets (2) + pad1 (2) = 4 bytes header
        let mut buf = Vec::with_capacity(16);

        // cCapabilitiesSets = 1
        buf.extend_from_slice(&1u16.to_le_bytes());
        // pad1 = 0
        buf.extend_from_slice(&0u16.to_le_bytes());

        // CB_GENERAL_CAPABILITY_SET
        buf.extend_from_slice(&CB_CAPSTYPE_GENERAL.to_le_bytes()); // capabilitySetType
        buf.extend_from_slice(&12u16.to_le_bytes()); // lengthCapability
        buf.extend_from_slice(&CB_CAPS_VERSION_2.to_le_bytes()); // version
        buf.extend_from_slice(&self.general_flags.bits().to_le_bytes()); // generalFlags

        buf
    }

    /// Encodes the full PDU with header.
    pub fn encode(&self) -> Vec<u8> {
        let payload = self.encode_payload();
        encode_pdu(MessageType::ClipCaps, MessageFlags::empty(), &payload)
    }
}

/// Monitor Ready PDU (CB_MONITOR_READY) - section 2.2.2.2.
///
/// Server sends this to indicate the clipboard channel is ready.
/// Client should respond with Clip Capabilities PDU followed by
/// a Format List PDU if it has clipboard data.
#[derive(Debug, Clone, Copy)]
pub struct MonitorReadyPdu;

impl MonitorReadyPdu {
    /// Decodes a Monitor Ready PDU (no payload expected).
    pub fn decode(_payload: &[u8]) -> Result<Self, CliprdrError> {
        // Monitor Ready has no payload
        Ok(Self)
    }
}

/// Format List PDU (CB_FORMAT_LIST) - section 2.2.3.1.
///
/// Announces available clipboard formats.
#[derive(Debug, Clone)]
pub struct FormatListPdu {
    /// List of available formats.
    pub formats: Vec<ClipboardFormat>,
}

impl FormatListPdu {
    /// Creates a new Format List PDU.
    pub fn new(formats: Vec<ClipboardFormat>) -> Self {
        Self { formats }
    }

    /// Creates an empty Format List PDU (no formats available).
    pub fn empty() -> Self {
        Self {
            formats: Vec::new(),
        }
    }

    /// Decodes a Format List PDU from payload (after header).
    ///
    /// Uses long format names (CB_USE_LONG_FORMAT_NAMES) format.
    pub fn decode_long(payload: &[u8]) -> Result<Self, CliprdrError> {
        let mut formats = Vec::new();
        let mut offset = 0;

        while offset + 4 <= payload.len() {
            let format_id = u32::from_le_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ]);
            offset += 4;

            // Format name is null-terminated UTF-16LE string
            let mut name = String::new();
            while offset + 2 <= payload.len() {
                let ch = u16::from_le_bytes([payload[offset], payload[offset + 1]]);
                offset += 2;
                if ch == 0 {
                    break;
                }
                // Convert UTF-16 to char
                if let Some(c) = char::from_u32(ch as u32) {
                    name.push(c);
                }
            }

            formats.push(ClipboardFormat::new(format_id, name));
        }

        Ok(Self { formats })
    }

    /// Decodes a Format List PDU from payload using short format names.
    pub fn decode_short(payload: &[u8]) -> Result<Self, CliprdrError> {
        let mut formats = Vec::new();
        let mut offset = 0;

        // Short format: formatId (4) + formatName[32] (32 bytes ASCII, null-padded)
        while offset + 36 <= payload.len() {
            let format_id = u32::from_le_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ]);
            offset += 4;

            // Read 32-byte ASCII name
            let name_bytes = &payload[offset..offset + 32];
            offset += 32;

            // Find null terminator
            let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(32);
            let name = String::from_utf8_lossy(&name_bytes[..name_len]).to_string();

            formats.push(ClipboardFormat::new(format_id, name));
        }

        Ok(Self { formats })
    }

    /// Encodes the Format List PDU payload using long format names.
    pub fn encode_payload_long(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        for format in &self.formats {
            buf.extend_from_slice(&format.id.to_le_bytes());

            // Encode name as null-terminated UTF-16LE
            for ch in format.name.encode_utf16() {
                buf.extend_from_slice(&ch.to_le_bytes());
            }
            // Null terminator
            buf.extend_from_slice(&0u16.to_le_bytes());
        }

        buf
    }

    /// Encodes the full PDU with header.
    pub fn encode(&self) -> Vec<u8> {
        let payload = self.encode_payload_long();
        encode_pdu(MessageType::FormatList, MessageFlags::empty(), &payload)
    }

    /// Story 5.3: Creates a Format List PDU announcing text formats.
    ///
    /// This announces that the client has text available in both
    /// CF_UNICODETEXT and CF_TEXT formats.
    pub fn text_formats() -> Self {
        Self {
            formats: vec![
                ClipboardFormat::new(StandardFormat::UnicodeText as u32, ""),
                ClipboardFormat::new(StandardFormat::Text as u32, ""),
            ],
        }
    }
}

/// Format List Response PDU (CB_FORMAT_LIST_RESPONSE) - section 2.2.3.2.
///
/// Response to Format List PDU indicating success or failure.
#[derive(Debug, Clone, Copy)]
pub struct FormatListResponsePdu {
    /// Whether the format list was accepted.
    pub success: bool,
}

impl FormatListResponsePdu {
    /// Creates a successful response.
    pub fn ok() -> Self {
        Self { success: true }
    }

    /// Creates a failed response.
    pub fn fail() -> Self {
        Self { success: false }
    }

    /// Decodes from message flags.
    pub fn from_flags(flags: MessageFlags) -> Self {
        Self {
            success: flags.is_ok(),
        }
    }

    /// Encodes the full PDU with header.
    pub fn encode(&self) -> Vec<u8> {
        let flags = if self.success {
            MessageFlags::RESPONSE_OK
        } else {
            MessageFlags::RESPONSE_FAIL
        };
        encode_pdu(MessageType::FormatListResponse, flags, &[])
    }
}

/// Format Data Request PDU (CB_FORMAT_DATA_REQUEST) - section 2.2.5.1.
///
/// Client sends this to request clipboard data in a specific format.
///
/// Structure:
/// - Header (8 bytes): msgType=0x0004, msgFlags=0, dataLen=4
/// - requestedFormatId (4 bytes): The clipboard format ID to request
#[derive(Debug, Clone, Copy)]
pub struct FormatDataRequestPdu {
    /// The format ID being requested (from Format List PDU).
    pub requested_format_id: u32,
}

impl FormatDataRequestPdu {
    /// Creates a new Format Data Request PDU.
    pub fn new(format_id: u32) -> Self {
        Self {
            requested_format_id: format_id,
        }
    }

    /// Creates a request for CF_UNICODETEXT.
    pub fn unicode_text() -> Self {
        Self::new(StandardFormat::UnicodeText as u32)
    }

    /// Creates a request for CF_TEXT.
    pub fn text() -> Self {
        Self::new(StandardFormat::Text as u32)
    }

    /// Decodes a Format Data Request PDU from payload (after header).
    pub fn decode(payload: &[u8]) -> Result<Self, CliprdrError> {
        if payload.len() < 4 {
            return Err(CliprdrError::PayloadTooShort {
                context: "FormatDataRequestPdu",
                expected: 4,
                actual: payload.len(),
            });
        }

        let requested_format_id =
            u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);

        Ok(Self {
            requested_format_id,
        })
    }

    /// Encodes the Format Data Request PDU payload.
    pub fn encode_payload(&self) -> Vec<u8> {
        self.requested_format_id.to_le_bytes().to_vec()
    }

    /// Encodes the full PDU with header.
    pub fn encode(&self) -> Vec<u8> {
        let payload = self.encode_payload();
        encode_pdu(
            MessageType::FormatDataRequest,
            MessageFlags::empty(),
            &payload,
        )
    }
}

/// Format Data Response PDU (CB_FORMAT_DATA_RESPONSE) - section 2.2.5.2.
///
/// Server sends this in response to a Format Data Request, containing the
/// actual clipboard data.
///
/// Structure:
/// - Header (8 bytes): msgType=0x0005, msgFlags, dataLen
/// - requestedFormatData (variable): The clipboard data bytes
#[derive(Debug, Clone)]
pub struct FormatDataResponsePdu {
    /// Whether the request was successful.
    pub success: bool,
    /// The clipboard data (empty if failed).
    pub data: Vec<u8>,
}

impl FormatDataResponsePdu {
    /// Creates a successful response with data.
    pub fn ok(data: Vec<u8>) -> Self {
        Self {
            success: true,
            data,
        }
    }

    /// Creates a failed response.
    pub fn fail() -> Self {
        Self {
            success: false,
            data: Vec::new(),
        }
    }

    /// Decodes a Format Data Response PDU from payload and flags.
    pub fn decode(payload: &[u8], flags: MessageFlags) -> Result<Self, CliprdrError> {
        let success = flags.is_ok();
        let data = if success {
            payload.to_vec()
        } else {
            Vec::new()
        };

        Ok(Self { success, data })
    }

    /// Returns the data as UTF-8 string, assuming CF_UNICODETEXT format.
    ///
    /// CF_UNICODETEXT is UTF-16LE encoded, so this converts to UTF-8.
    pub fn as_utf8_from_unicode(&self) -> Option<String> {
        if !self.success || self.data.is_empty() {
            return None;
        }

        // CF_UNICODETEXT is UTF-16LE with optional BOM
        let mut data = &self.data[..];

        // Skip BOM if present (0xFF 0xFE for little-endian)
        if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xFE {
            data = &data[2..];
        }

        // Convert UTF-16LE to UTF-8
        // Data should be pairs of bytes (even length)
        if !data.len().is_multiple_of(2) {
            return None;
        }

        let utf16_units: Vec<u16> = data
            .chunks(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .take_while(|&c| c != 0) // Stop at null terminator
            .collect();

        String::from_utf16(&utf16_units).ok()
    }

    /// Returns the data as UTF-8 string, assuming CF_TEXT format.
    ///
    /// CF_TEXT is ANSI/ASCII encoded.
    pub fn as_utf8_from_ansi(&self) -> Option<String> {
        if !self.success || self.data.is_empty() {
            return None;
        }

        // Find null terminator
        let end = self
            .data
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.data.len());

        // CF_TEXT is typically Windows-1252 or similar, but we'll treat as UTF-8/ASCII
        String::from_utf8(self.data[..end].to_vec()).ok()
    }

    /// Encodes the full PDU with header.
    pub fn encode(&self) -> Vec<u8> {
        let flags = if self.success {
            MessageFlags::RESPONSE_OK
        } else {
            MessageFlags::RESPONSE_FAIL
        };
        encode_pdu(MessageType::FormatDataResponse, flags, &self.data)
    }
}

/// Parsed CLIPRDR PDU.
#[derive(Debug, Clone)]
pub enum CliprdrPdu {
    /// Monitor Ready PDU from server.
    MonitorReady(MonitorReadyPdu),
    /// Clip Capabilities PDU.
    ClipCaps(ClipCapsPdu),
    /// Format List PDU.
    FormatList(FormatListPdu),
    /// Format List Response PDU.
    FormatListResponse(FormatListResponsePdu),
    /// Format Data Request PDU.
    FormatDataRequest(FormatDataRequestPdu),
    /// Format Data Response PDU.
    FormatDataResponse(FormatDataResponsePdu),
}

impl CliprdrPdu {
    /// Decodes a CLIPRDR PDU from raw bytes including header.
    pub fn decode(data: &[u8], use_long_format_names: bool) -> Result<Self, CliprdrError> {
        if data.len() < CLIPRDR_HEADER_SIZE {
            return Err(CliprdrError::PayloadTooShort {
                context: "CLIPRDR header",
                expected: CLIPRDR_HEADER_SIZE,
                actual: data.len(),
            });
        }

        let msg_type = u16::from_le_bytes([data[0], data[1]]);
        let msg_flags = MessageFlags::from_bits(u16::from_le_bytes([data[2], data[3]]));
        let data_len = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;

        let payload = if data_len > 0 && data.len() >= CLIPRDR_HEADER_SIZE + data_len {
            &data[CLIPRDR_HEADER_SIZE..CLIPRDR_HEADER_SIZE + data_len]
        } else {
            &[]
        };

        let msg_type = MessageType::try_from(msg_type)?;

        match msg_type {
            MessageType::MonitorReady => Ok(Self::MonitorReady(MonitorReadyPdu::decode(payload)?)),
            MessageType::ClipCaps => Ok(Self::ClipCaps(ClipCapsPdu::decode(payload)?)),
            MessageType::FormatList => {
                let format_list = if use_long_format_names || !msg_flags.has_ascii_names() {
                    FormatListPdu::decode_long(payload)?
                } else {
                    FormatListPdu::decode_short(payload)?
                };
                Ok(Self::FormatList(format_list))
            }
            MessageType::FormatListResponse => Ok(Self::FormatListResponse(
                FormatListResponsePdu::from_flags(msg_flags),
            )),
            MessageType::FormatDataRequest => Ok(Self::FormatDataRequest(
                FormatDataRequestPdu::decode(payload)?,
            )),
            MessageType::FormatDataResponse => Ok(Self::FormatDataResponse(
                FormatDataResponsePdu::decode(payload, msg_flags)?,
            )),
            _ => {
                // Other message types not yet implemented (File transfers in Story 5.4+)
                Err(CliprdrError::InvalidMessageType(msg_type as u16))
            }
        }
    }
}

/// Encodes a CLIPRDR PDU with header.
fn encode_pdu(msg_type: MessageType, flags: MessageFlags, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(CLIPRDR_HEADER_SIZE + payload.len());

    buf.extend_from_slice(&(msg_type as u16).to_le_bytes());
    buf.extend_from_slice(&flags.bits().to_le_bytes());
    buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buf.extend_from_slice(payload);

    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_type_conversion() {
        assert_eq!(
            MessageType::try_from(0x0001).unwrap(),
            MessageType::MonitorReady
        );
        assert_eq!(
            MessageType::try_from(0x0002).unwrap(),
            MessageType::FormatList
        );
        assert_eq!(
            MessageType::try_from(0x0003).unwrap(),
            MessageType::FormatListResponse
        );
        assert_eq!(
            MessageType::try_from(0x0007).unwrap(),
            MessageType::ClipCaps
        );
        assert!(MessageType::try_from(0xFFFF).is_err());
    }

    #[test]
    fn test_message_flags() {
        let flags = MessageFlags::RESPONSE_OK;
        assert!(flags.is_ok());
        assert!(!flags.is_fail());

        let flags = MessageFlags::RESPONSE_FAIL;
        assert!(!flags.is_ok());
        assert!(flags.is_fail());

        let flags = MessageFlags::ASCII_NAMES;
        assert!(flags.has_ascii_names());
    }

    #[test]
    fn test_standard_format_conversion() {
        assert_eq!(StandardFormat::try_from(1).unwrap(), StandardFormat::Text);
        assert_eq!(
            StandardFormat::try_from(13).unwrap(),
            StandardFormat::UnicodeText
        );
        assert_eq!(StandardFormat::try_from(15).unwrap(), StandardFormat::Hdrop);
        assert!(StandardFormat::try_from(255).is_err());
    }

    #[test]
    fn test_clipboard_format() {
        let text = ClipboardFormat::standard(StandardFormat::Text);
        assert!(text.is_text());
        assert!(!text.is_file_list());

        let unicode = ClipboardFormat::standard(StandardFormat::UnicodeText);
        assert!(unicode.is_text());

        let files = ClipboardFormat::standard(StandardFormat::Hdrop);
        assert!(files.is_file_list());
        assert!(!files.is_text());

        let custom = ClipboardFormat::new(0xC001, "CustomFormat");
        assert!(!custom.is_text());
        assert!(!custom.is_file_list());
        assert_eq!(custom.name, "CustomFormat");
    }

    #[test]
    fn test_general_capability_flags() {
        let flags = GeneralCapabilityFlags::USE_LONG_FORMAT_NAMES;
        assert!(flags.use_long_format_names());
        assert!(!flags.stream_fileclip_enabled());

        let combined = flags.union(GeneralCapabilityFlags::STREAM_FILECLIP_ENABLED);
        assert!(combined.use_long_format_names());
        assert!(combined.stream_fileclip_enabled());
    }

    #[test]
    fn test_clip_caps_encode_decode() {
        let caps = ClipCapsPdu::default_client();
        let encoded = caps.encode();

        // Header check
        assert_eq!(
            u16::from_le_bytes([encoded[0], encoded[1]]),
            MessageType::ClipCaps as u16
        );
        assert_eq!(u16::from_le_bytes([encoded[2], encoded[3]]), 0); // flags

        // Decode
        let decoded = CliprdrPdu::decode(&encoded, true).unwrap();
        if let CliprdrPdu::ClipCaps(caps) = decoded {
            assert!(caps.general_flags.use_long_format_names());
        } else {
            panic!("Expected ClipCaps PDU");
        }
    }

    #[test]
    fn test_format_list_encode_decode() {
        let formats = vec![
            ClipboardFormat::standard(StandardFormat::UnicodeText),
            ClipboardFormat::new(0xC001, "HTML"),
        ];
        let format_list = FormatListPdu::new(formats);
        let encoded = format_list.encode();

        // Header check
        assert_eq!(
            u16::from_le_bytes([encoded[0], encoded[1]]),
            MessageType::FormatList as u16
        );

        // Decode
        let decoded = CliprdrPdu::decode(&encoded, true).unwrap();
        if let CliprdrPdu::FormatList(list) = decoded {
            assert_eq!(list.formats.len(), 2);
            assert_eq!(list.formats[0].id, StandardFormat::UnicodeText as u32);
            assert_eq!(list.formats[1].id, 0xC001);
            assert_eq!(list.formats[1].name, "HTML");
        } else {
            panic!("Expected FormatList PDU");
        }
    }

    #[test]
    fn test_format_list_empty() {
        let format_list = FormatListPdu::empty();
        let encoded = format_list.encode();

        let decoded = CliprdrPdu::decode(&encoded, true).unwrap();
        if let CliprdrPdu::FormatList(list) = decoded {
            assert!(list.formats.is_empty());
        } else {
            panic!("Expected FormatList PDU");
        }
    }

    #[test]
    fn test_format_list_response_ok() {
        let response = FormatListResponsePdu::ok();
        let encoded = response.encode();

        // Header check
        assert_eq!(
            u16::from_le_bytes([encoded[0], encoded[1]]),
            MessageType::FormatListResponse as u16
        );
        // Flags should have RESPONSE_OK
        let flags = MessageFlags::from_bits(u16::from_le_bytes([encoded[2], encoded[3]]));
        assert!(flags.is_ok());

        let decoded = CliprdrPdu::decode(&encoded, true).unwrap();
        if let CliprdrPdu::FormatListResponse(resp) = decoded {
            assert!(resp.success);
        } else {
            panic!("Expected FormatListResponse PDU");
        }
    }

    #[test]
    fn test_format_list_response_fail() {
        let response = FormatListResponsePdu::fail();
        let encoded = response.encode();

        let flags = MessageFlags::from_bits(u16::from_le_bytes([encoded[2], encoded[3]]));
        assert!(flags.is_fail());

        let decoded = CliprdrPdu::decode(&encoded, true).unwrap();
        if let CliprdrPdu::FormatListResponse(resp) = decoded {
            assert!(!resp.success);
        } else {
            panic!("Expected FormatListResponse PDU");
        }
    }

    #[test]
    fn test_monitor_ready_decode() {
        // Monitor Ready PDU with empty payload
        let data = [
            0x01, 0x00, // msgType = MonitorReady
            0x00, 0x00, // msgFlags
            0x00, 0x00, 0x00, 0x00, // dataLen = 0
        ];

        let decoded = CliprdrPdu::decode(&data, true).unwrap();
        assert!(matches!(decoded, CliprdrPdu::MonitorReady(_)));
    }

    #[test]
    fn test_decode_short_payload_error() {
        let data = [0x01, 0x00]; // Too short for header
        let result = CliprdrPdu::decode(&data, true);
        assert!(result.is_err());
    }

    #[test]
    fn test_format_data_request_unicode() {
        let request = FormatDataRequestPdu::unicode_text();
        assert_eq!(
            request.requested_format_id,
            StandardFormat::UnicodeText as u32
        );

        let encoded = request.encode();

        // Header check
        assert_eq!(
            u16::from_le_bytes([encoded[0], encoded[1]]),
            MessageType::FormatDataRequest as u16
        );
        // dataLen should be 4
        assert_eq!(
            u32::from_le_bytes([encoded[4], encoded[5], encoded[6], encoded[7]]),
            4
        );

        // Decode
        let decoded = CliprdrPdu::decode(&encoded, true).unwrap();
        if let CliprdrPdu::FormatDataRequest(req) = decoded {
            assert_eq!(req.requested_format_id, StandardFormat::UnicodeText as u32);
        } else {
            panic!("Expected FormatDataRequest PDU");
        }
    }

    #[test]
    fn test_format_data_request_text() {
        let request = FormatDataRequestPdu::text();
        assert_eq!(request.requested_format_id, StandardFormat::Text as u32);
    }

    #[test]
    fn test_format_data_request_custom() {
        let request = FormatDataRequestPdu::new(0xC001);
        assert_eq!(request.requested_format_id, 0xC001);
    }

    #[test]
    fn test_format_data_response_ok() {
        let data = vec![0x48, 0x00, 0x69, 0x00, 0x00, 0x00]; // "Hi" in UTF-16LE + null
        let response = FormatDataResponsePdu::ok(data.clone());
        assert!(response.success);
        assert_eq!(response.data, data);

        let encoded = response.encode();

        // Header check
        assert_eq!(
            u16::from_le_bytes([encoded[0], encoded[1]]),
            MessageType::FormatDataResponse as u16
        );
        // Flags should have RESPONSE_OK
        let flags = MessageFlags::from_bits(u16::from_le_bytes([encoded[2], encoded[3]]));
        assert!(flags.is_ok());

        // Decode
        let decoded = CliprdrPdu::decode(&encoded, true).unwrap();
        if let CliprdrPdu::FormatDataResponse(resp) = decoded {
            assert!(resp.success);
            assert_eq!(resp.data, data);
        } else {
            panic!("Expected FormatDataResponse PDU");
        }
    }

    #[test]
    fn test_format_data_response_fail() {
        let response = FormatDataResponsePdu::fail();
        assert!(!response.success);
        assert!(response.data.is_empty());

        let encoded = response.encode();

        let flags = MessageFlags::from_bits(u16::from_le_bytes([encoded[2], encoded[3]]));
        assert!(flags.is_fail());

        let decoded = CliprdrPdu::decode(&encoded, true).unwrap();
        if let CliprdrPdu::FormatDataResponse(resp) = decoded {
            assert!(!resp.success);
        } else {
            panic!("Expected FormatDataResponse PDU");
        }
    }

    #[test]
    fn test_format_data_response_utf16_to_utf8() {
        // "Hello" in UTF-16LE: H(0x48 0x00) e(0x65 0x00) l(0x6C 0x00) l(0x6C 0x00) o(0x6F 0x00) null(0x00 0x00)
        let data = vec![
            0x48, 0x00, 0x65, 0x00, 0x6C, 0x00, 0x6C, 0x00, 0x6F, 0x00, 0x00, 0x00,
        ];
        let response = FormatDataResponsePdu::ok(data);

        let text = response.as_utf8_from_unicode().unwrap();
        assert_eq!(text, "Hello");
    }

    #[test]
    fn test_format_data_response_utf16_with_bom() {
        // UTF-16LE BOM (0xFF 0xFE) + "Hi" + null
        let data = vec![
            0xFF, 0xFE, // BOM
            0x48, 0x00, // H
            0x69, 0x00, // i
            0x00, 0x00, // null
        ];
        let response = FormatDataResponsePdu::ok(data);

        let text = response.as_utf8_from_unicode().unwrap();
        assert_eq!(text, "Hi");
    }

    #[test]
    fn test_format_data_response_unicode_special_chars() {
        // "Héllo" with é (U+00E9) in UTF-16LE
        let data = vec![
            0x48, 0x00, // H
            0xE9, 0x00, // é (U+00E9)
            0x6C, 0x00, // l
            0x6C, 0x00, // l
            0x6F, 0x00, // o
            0x00, 0x00, // null
        ];
        let response = FormatDataResponsePdu::ok(data);

        let text = response.as_utf8_from_unicode().unwrap();
        assert_eq!(text, "Héllo");
    }

    #[test]
    fn test_format_data_response_ansi() {
        let data = vec![0x48, 0x65, 0x6C, 0x6C, 0x6F, 0x00]; // "Hello" + null
        let response = FormatDataResponsePdu::ok(data);

        let text = response.as_utf8_from_ansi().unwrap();
        assert_eq!(text, "Hello");
    }

    #[test]
    fn test_format_data_response_empty_fails() {
        let response = FormatDataResponsePdu::fail();
        assert!(response.as_utf8_from_unicode().is_none());
        assert!(response.as_utf8_from_ansi().is_none());
    }
}
