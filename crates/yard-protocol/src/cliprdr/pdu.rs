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

/// File Contents Request flags (dwFlags field).
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileContentsFlags {
    /// Request file size only (returns 8 bytes: u64 file size).
    Size = 0x0001,
    /// Request file content range.
    Range = 0x0002,
}

/// Size of FILEDESCRIPTORW structure in bytes.
pub const FILEDESCRIPTORW_SIZE: usize = 592;

/// FILEDESCRIPTORW flags (dwFlags field).
pub mod fd_flags {
    /// clsid field is valid.
    pub const FD_CLSID: u32 = 0x0001;
    /// sizel/pointl fields are valid.
    pub const FD_SIZEPOINT: u32 = 0x0002;
    /// dwFileAttributes field is valid.
    pub const FD_ATTRIBUTES: u32 = 0x0004;
    /// ftCreationTime field is valid.
    pub const FD_CREATETIME: u32 = 0x0008;
    /// ftLastAccessTime field is valid.
    pub const FD_ACCESSTIME: u32 = 0x0010;
    /// ftLastWriteTime field is valid.
    pub const FD_WRITETIME: u32 = 0x0020;
    /// nFileSizeHigh/Low fields are valid.
    pub const FD_FILESIZE: u32 = 0x0040;
    /// Show progress UI during transfer.
    pub const FD_PROGRESSUI: u32 = 0x4000;
    /// Treat as shortcut link.
    pub const FD_LINKUI: u32 = 0x8000;
}

/// Windows file attribute constants (for FILEDESCRIPTORW.dwFileAttributes).
pub mod file_attributes {
    /// Read-only file.
    pub const FILE_ATTRIBUTE_READONLY: u32 = 0x0001;
    /// Hidden file.
    pub const FILE_ATTRIBUTE_HIDDEN: u32 = 0x0002;
    /// System file.
    pub const FILE_ATTRIBUTE_SYSTEM: u32 = 0x0004;
    /// Directory.
    pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0010;
    /// Archive (file modified since last backup).
    pub const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x0020;
    /// Normal file (no other attributes set).
    pub const FILE_ATTRIBUTE_NORMAL: u32 = 0x0080;
}

/// Story 5.5: Converts Unix timestamp to Windows FILETIME.
///
/// FILETIME is 100-nanosecond intervals since January 1, 1601.
/// Unix timestamp is seconds since January 1, 1970.
///
/// Conversion: FILETIME = (unix_timestamp + 11644473600) * 10000000
pub fn unix_to_filetime(unix_timestamp: u64) -> u64 {
    // Seconds between 1601-01-01 and 1970-01-01
    const UNIX_EPOCH_OFFSET: u64 = 11_644_473_600;
    // FILETIME units per second (100ns intervals)
    const FILETIME_UNITS_PER_SECOND: u64 = 10_000_000;

    unix_timestamp
        .saturating_add(UNIX_EPOCH_OFFSET)
        .saturating_mul(FILETIME_UNITS_PER_SECOND)
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
    ///
    /// Story 5.4: Enables file clipboard support (STREAM_FILECLIP_ENABLED).
    pub fn default_client() -> Self {
        // Support long format names and file clipboard operations
        let flags = GeneralCapabilityFlags::USE_LONG_FORMAT_NAMES
            .union(GeneralCapabilityFlags::STREAM_FILECLIP_ENABLED);
        Self::new(flags)
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

    /// Story 5.5: Creates a Format List PDU announcing file formats.
    ///
    /// This announces that the client has files available for clipboard transfer.
    /// The format IDs must be in the registered format range (>= 0xC000).
    pub fn file_formats(
        file_group_descriptor_format_id: u32,
        file_contents_format_id: u32,
    ) -> Self {
        Self {
            formats: vec![
                ClipboardFormat::new(
                    file_group_descriptor_format_id,
                    "FileGroupDescriptorW".to_string(),
                ),
                ClipboardFormat::new(file_contents_format_id, "FileContents".to_string()),
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

/// File Contents Request PDU (CB_FILECONTENTS_REQUEST) - section 2.2.5.3.
///
/// Client or server sends this to request file size or content.
///
/// Structure:
/// - Header (8 bytes): msgType=0x0008, msgFlags=0, dataLen
/// - streamId (4 bytes): Request identifier
/// - lindex (4 bytes): File index (0-based) in the file list
/// - dwFlags (4 bytes): FILECONTENTS_SIZE or FILECONTENTS_RANGE
/// - nPositionLow (4 bytes): Low 32 bits of byte offset
/// - nPositionHigh (4 bytes): High 32 bits of byte offset
/// - cbRequested (4 bytes): Number of bytes to request (for RANGE)
/// - clipDataId (4 bytes): Optional clipboard data ID (if locking supported)
#[derive(Debug, Clone, Copy)]
pub struct FileContentsRequestPdu {
    /// Request identifier for matching response.
    pub stream_id: u32,
    /// File index in the file list (0-based).
    pub lindex: u32,
    /// Request type: Size or Range.
    pub flags: FileContentsFlags,
    /// Byte offset for range requests (low 32 bits).
    pub position_low: u32,
    /// Byte offset for range requests (high 32 bits).
    pub position_high: u32,
    /// Number of bytes to request (for range requests).
    pub cb_requested: u32,
    /// Optional clipboard data ID (for locking).
    pub clip_data_id: Option<u32>,
}

impl FileContentsRequestPdu {
    /// Creates a new request for file size.
    pub fn size(stream_id: u32, file_index: u32) -> Self {
        Self {
            stream_id,
            lindex: file_index,
            flags: FileContentsFlags::Size,
            position_low: 0,
            position_high: 0,
            cb_requested: 8, // Size response is 8 bytes
            clip_data_id: None,
        }
    }

    /// Creates a new request for file content range.
    pub fn range(stream_id: u32, file_index: u32, offset: u64, length: u32) -> Self {
        Self {
            stream_id,
            lindex: file_index,
            flags: FileContentsFlags::Range,
            position_low: offset as u32,
            position_high: (offset >> 32) as u32,
            cb_requested: length,
            clip_data_id: None,
        }
    }

    /// Decodes a File Contents Request PDU from payload (after header).
    pub fn decode(payload: &[u8]) -> Result<Self, CliprdrError> {
        // Minimum size: streamId + lindex + dwFlags + nPositionLow + nPositionHigh + cbRequested = 24 bytes
        if payload.len() < 24 {
            return Err(CliprdrError::PayloadTooShort {
                context: "FileContentsRequestPdu",
                expected: 24,
                actual: payload.len(),
            });
        }

        let stream_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let lindex = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
        let flags_raw = u32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
        let position_low = u32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
        let position_high =
            u32::from_le_bytes([payload[16], payload[17], payload[18], payload[19]]);
        let cb_requested = u32::from_le_bytes([payload[20], payload[21], payload[22], payload[23]]);

        let flags = match flags_raw {
            0x0001 => FileContentsFlags::Size,
            0x0002 => FileContentsFlags::Range,
            _ => return Err(CliprdrError::InvalidFlags(flags_raw as u16)),
        };

        // Optional clipDataId at offset 24
        let clip_data_id = if payload.len() >= 28 {
            Some(u32::from_le_bytes([
                payload[24],
                payload[25],
                payload[26],
                payload[27],
            ]))
        } else {
            None
        };

        Ok(Self {
            stream_id,
            lindex,
            flags,
            position_low,
            position_high,
            cb_requested,
            clip_data_id,
        })
    }

    /// Encodes the File Contents Request PDU payload.
    pub fn encode_payload(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(28);

        buf.extend_from_slice(&self.stream_id.to_le_bytes());
        buf.extend_from_slice(&self.lindex.to_le_bytes());
        buf.extend_from_slice(&(self.flags as u32).to_le_bytes());
        buf.extend_from_slice(&self.position_low.to_le_bytes());
        buf.extend_from_slice(&self.position_high.to_le_bytes());
        buf.extend_from_slice(&self.cb_requested.to_le_bytes());

        if let Some(clip_data_id) = self.clip_data_id {
            buf.extend_from_slice(&clip_data_id.to_le_bytes());
        }

        buf
    }

    /// Encodes the full PDU with header.
    pub fn encode(&self) -> Vec<u8> {
        let payload = self.encode_payload();
        encode_pdu(
            MessageType::FileContentsRequest,
            MessageFlags::empty(),
            &payload,
        )
    }

    /// Returns the byte offset as u64.
    pub fn offset(&self) -> u64 {
        (self.position_high as u64) << 32 | self.position_low as u64
    }
}

/// File Contents Response PDU (CB_FILECONTENTS_RESPONSE) - section 2.2.5.4.
///
/// Response containing file size or content data.
///
/// Structure:
/// - Header (8 bytes): msgType=0x0009, msgFlags, dataLen
/// - streamId (4 bytes): Matches the request stream ID
/// - requestedFileContentsData (variable): File size (8 bytes) or file data
#[derive(Debug, Clone)]
pub struct FileContentsResponsePdu {
    /// Whether the request was successful.
    pub success: bool,
    /// Request identifier (matches request).
    pub stream_id: u32,
    /// Response data: file size (8 bytes for Size requests) or file content.
    pub data: Vec<u8>,
}

impl FileContentsResponsePdu {
    /// Creates a successful response with data.
    pub fn ok(stream_id: u32, data: Vec<u8>) -> Self {
        Self {
            success: true,
            stream_id,
            data,
        }
    }

    /// Creates a failed response.
    pub fn fail(stream_id: u32) -> Self {
        Self {
            success: false,
            stream_id,
            data: Vec::new(),
        }
    }

    /// Decodes a File Contents Response PDU from payload and flags.
    pub fn decode(payload: &[u8], flags: MessageFlags) -> Result<Self, CliprdrError> {
        if payload.len() < 4 {
            return Err(CliprdrError::PayloadTooShort {
                context: "FileContentsResponsePdu",
                expected: 4,
                actual: payload.len(),
            });
        }

        let stream_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let success = flags.is_ok();
        let data = if success && payload.len() > 4 {
            payload[4..].to_vec()
        } else {
            Vec::new()
        };

        Ok(Self {
            success,
            stream_id,
            data,
        })
    }

    /// Returns the file size if this is a size response.
    ///
    /// Size responses contain exactly 8 bytes (u64 little-endian).
    pub fn as_file_size(&self) -> Option<u64> {
        if !self.success || self.data.len() != 8 {
            return None;
        }

        Some(u64::from_le_bytes([
            self.data[0],
            self.data[1],
            self.data[2],
            self.data[3],
            self.data[4],
            self.data[5],
            self.data[6],
            self.data[7],
        ]))
    }

    /// Encodes the full PDU with header.
    pub fn encode(&self) -> Vec<u8> {
        let flags = if self.success {
            MessageFlags::RESPONSE_OK
        } else {
            MessageFlags::RESPONSE_FAIL
        };

        let mut payload = Vec::with_capacity(4 + self.data.len());
        payload.extend_from_slice(&self.stream_id.to_le_bytes());
        payload.extend_from_slice(&self.data);

        encode_pdu(MessageType::FileContentsResponse, flags, &payload)
    }
}

/// FILEDESCRIPTORW structure (592 bytes) - describes a file in clipboard.
///
/// This structure is part of the CLIPRDR_FILELIST in Format Data Response
/// when file formats (FileGroupDescriptorW) are requested.
#[derive(Debug, Clone)]
pub struct FileDescriptorW {
    /// Validity flags indicating which fields are valid.
    pub flags: u32,
    /// File attributes (FILE_ATTRIBUTE_* constants).
    pub file_attributes: u32,
    /// File creation time (FILETIME format).
    pub creation_time: u64,
    /// File last access time (FILETIME format).
    pub last_access_time: u64,
    /// File last write time (FILETIME format).
    pub last_write_time: u64,
    /// File size in bytes.
    pub file_size: u64,
    /// File name (UTF-16LE, max 260 chars including null).
    pub file_name: String,
}

impl FileDescriptorW {
    /// Decodes a FILEDESCRIPTORW structure from bytes.
    pub fn decode(data: &[u8]) -> Result<Self, CliprdrError> {
        if data.len() < FILEDESCRIPTORW_SIZE {
            return Err(CliprdrError::PayloadTooShort {
                context: "FileDescriptorW",
                expected: FILEDESCRIPTORW_SIZE,
                actual: data.len(),
            });
        }

        // Offsets in FILEDESCRIPTORW:
        // 0-3: dwFlags (4 bytes)
        // 4-19: clsid (16 bytes) - reserved
        // 20-27: sizel (8 bytes) - reserved
        // 28-35: pointl (8 bytes) - reserved
        // 36-39: dwFileAttributes (4 bytes)
        // 40-47: ftCreationTime (8 bytes)
        // 48-55: ftLastAccessTime (8 bytes)
        // 56-63: ftLastWriteTime (8 bytes)
        // 64-67: nFileSizeHigh (4 bytes)
        // 68-71: nFileSizeLow (4 bytes)
        // 72-591: cFileName (520 bytes, 260 UTF-16 chars)

        let flags = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let file_attributes = u32::from_le_bytes([data[36], data[37], data[38], data[39]]);
        let creation_time = u64::from_le_bytes([
            data[40], data[41], data[42], data[43], data[44], data[45], data[46], data[47],
        ]);
        let last_access_time = u64::from_le_bytes([
            data[48], data[49], data[50], data[51], data[52], data[53], data[54], data[55],
        ]);
        let last_write_time = u64::from_le_bytes([
            data[56], data[57], data[58], data[59], data[60], data[61], data[62], data[63],
        ]);
        let file_size_high = u32::from_le_bytes([data[64], data[65], data[66], data[67]]);
        let file_size_low = u32::from_le_bytes([data[68], data[69], data[70], data[71]]);
        let file_size = (file_size_high as u64) << 32 | file_size_low as u64;

        // Parse filename (UTF-16LE, null-terminated, 520 bytes max)
        let filename_bytes = &data[72..592];
        let utf16_units: Vec<u16> = filename_bytes
            .chunks(2)
            .take_while(|chunk| chunk.len() == 2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .take_while(|&c| c != 0) // Stop at null terminator
            .collect();
        let file_name = String::from_utf16_lossy(&utf16_units);

        Ok(Self {
            flags,
            file_attributes,
            creation_time,
            last_access_time,
            last_write_time,
            file_size,
            file_name,
        })
    }

    /// Returns true if the file size is valid.
    pub fn has_file_size(&self) -> bool {
        self.flags & fd_flags::FD_FILESIZE != 0
    }

    /// Returns true if file attributes are valid.
    pub fn has_attributes(&self) -> bool {
        self.flags & fd_flags::FD_ATTRIBUTES != 0
    }

    /// Returns true if this is a directory.
    pub fn is_directory(&self) -> bool {
        // FILE_ATTRIBUTE_DIRECTORY = 0x10
        self.has_attributes() && (self.file_attributes & 0x10) != 0
    }

    /// Story 5.5: Encodes a FILEDESCRIPTORW structure to bytes (592 bytes).
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = vec![0u8; FILEDESCRIPTORW_SIZE];

        // Offsets in FILEDESCRIPTORW:
        // 0-3: dwFlags (4 bytes)
        // 4-19: clsid (16 bytes) - reserved, leave as zeros
        // 20-27: sizel (8 bytes) - reserved, leave as zeros
        // 28-35: pointl (8 bytes) - reserved, leave as zeros
        // 36-39: dwFileAttributes (4 bytes)
        // 40-47: ftCreationTime (8 bytes)
        // 48-55: ftLastAccessTime (8 bytes)
        // 56-63: ftLastWriteTime (8 bytes)
        // 64-67: nFileSizeHigh (4 bytes)
        // 68-71: nFileSizeLow (4 bytes)
        // 72-591: cFileName (520 bytes, 260 UTF-16 chars)

        buf[0..4].copy_from_slice(&self.flags.to_le_bytes());
        buf[36..40].copy_from_slice(&self.file_attributes.to_le_bytes());
        buf[40..48].copy_from_slice(&self.creation_time.to_le_bytes());
        buf[48..56].copy_from_slice(&self.last_access_time.to_le_bytes());
        buf[56..64].copy_from_slice(&self.last_write_time.to_le_bytes());

        // Split file_size into high and low 32-bit parts
        let file_size_high = (self.file_size >> 32) as u32;
        let file_size_low = self.file_size as u32;
        buf[64..68].copy_from_slice(&file_size_high.to_le_bytes());
        buf[68..72].copy_from_slice(&file_size_low.to_le_bytes());

        // Encode filename as null-terminated UTF-16LE (max 260 chars = 520 bytes)
        let utf16_chars: Vec<u16> = self.file_name.encode_utf16().take(259).collect();
        let filename_offset = 72;
        for (i, ch) in utf16_chars.iter().enumerate() {
            let byte_offset = filename_offset + i * 2;
            if byte_offset + 2 <= FILEDESCRIPTORW_SIZE {
                buf[byte_offset..byte_offset + 2].copy_from_slice(&ch.to_le_bytes());
            }
        }
        // Null terminator (already zeroed from initialization)

        buf
    }

    /// Story 5.5: Creates a FileDescriptorW from local file metadata.
    pub fn from_local_file(
        file_name: String,
        file_size: u64,
        modified_time: u64,
        is_directory: bool,
    ) -> Self {
        let flags = fd_flags::FD_ATTRIBUTES | fd_flags::FD_FILESIZE | fd_flags::FD_WRITETIME;

        let file_attributes = if is_directory {
            file_attributes::FILE_ATTRIBUTE_DIRECTORY
        } else {
            file_attributes::FILE_ATTRIBUTE_NORMAL
        };

        let filetime = unix_to_filetime(modified_time);

        Self {
            flags,
            file_attributes,
            creation_time: 0,    // We don't have creation time on Unix
            last_access_time: 0, // We don't typically track this
            last_write_time: filetime,
            file_size,
            file_name,
        }
    }
}

/// CLIPRDR_FILELIST structure - list of file descriptors.
///
/// This is the payload of Format Data Response when FileGroupDescriptorW
/// format is requested.
#[derive(Debug, Clone)]
pub struct CliprdrFileList {
    /// List of file descriptors.
    pub files: Vec<FileDescriptorW>,
}

impl CliprdrFileList {
    /// Decodes a CLIPRDR_FILELIST from Format Data Response payload.
    ///
    /// Structure:
    /// - cItems (4 bytes): Number of files
    /// - fileDescriptorArray (variable): Array of FILEDESCRIPTORW structures
    pub fn decode(data: &[u8]) -> Result<Self, CliprdrError> {
        if data.len() < 4 {
            return Err(CliprdrError::PayloadTooShort {
                context: "CliprdrFileList",
                expected: 4,
                actual: data.len(),
            });
        }

        let c_items = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let expected_size = 4 + c_items * FILEDESCRIPTORW_SIZE;

        if data.len() < expected_size {
            return Err(CliprdrError::PayloadTooShort {
                context: "CliprdrFileList files",
                expected: expected_size,
                actual: data.len(),
            });
        }

        let mut files = Vec::with_capacity(c_items);
        for i in 0..c_items {
            let offset = 4 + i * FILEDESCRIPTORW_SIZE;
            let file_data = &data[offset..offset + FILEDESCRIPTORW_SIZE];
            files.push(FileDescriptorW::decode(file_data)?);
        }

        Ok(Self { files })
    }

    /// Returns the number of files.
    pub fn count(&self) -> usize {
        self.files.len()
    }

    /// Returns an iterator over file names.
    pub fn file_names(&self) -> impl Iterator<Item = &str> {
        self.files.iter().map(|f| f.file_name.as_str())
    }

    /// Story 5.5: Creates a new CliprdrFileList from file descriptors.
    pub fn new(files: Vec<FileDescriptorW>) -> Self {
        Self { files }
    }

    /// Story 5.5: Encodes the CLIPRDR_FILELIST to bytes.
    ///
    /// Structure:
    /// - cItems (4 bytes): Number of files
    /// - fileDescriptorArray (variable): Array of FILEDESCRIPTORW structures
    pub fn encode(&self) -> Vec<u8> {
        let count = self.files.len() as u32;
        let total_size = 4 + self.files.len() * FILEDESCRIPTORW_SIZE;
        let mut buf = Vec::with_capacity(total_size);

        // cItems (4 bytes)
        buf.extend_from_slice(&count.to_le_bytes());

        // fileDescriptorArray
        for file in &self.files {
            buf.extend_from_slice(&file.encode());
        }

        buf
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
    /// File Contents Request PDU (Story 5.4).
    FileContentsRequest(FileContentsRequestPdu),
    /// File Contents Response PDU (Story 5.4).
    FileContentsResponse(FileContentsResponsePdu),
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
            MessageType::FileContentsRequest => Ok(Self::FileContentsRequest(
                FileContentsRequestPdu::decode(payload)?,
            )),
            MessageType::FileContentsResponse => Ok(Self::FileContentsResponse(
                FileContentsResponsePdu::decode(payload, msg_flags)?,
            )),
            _ => {
                // Other message types not yet implemented (Lock/Unlock, TempDirectory)
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
    fn test_format_data_request_roundtrip() {
        // Test that encode then decode produces the same result
        let original = FormatDataRequestPdu::new(0xC123);
        let encoded = original.encode();

        // Decode the full PDU
        let decoded = CliprdrPdu::decode(&encoded, true).unwrap();
        if let CliprdrPdu::FormatDataRequest(req) = decoded {
            assert_eq!(req.requested_format_id, original.requested_format_id);
        } else {
            panic!("Expected FormatDataRequest PDU");
        }
    }

    #[test]
    fn test_format_data_response_roundtrip() {
        // Test success response round-trip
        let original_data = vec![0x48, 0x65, 0x6C, 0x6C, 0x6F]; // "Hello"
        let original = FormatDataResponsePdu::ok(original_data.clone());
        let encoded = original.encode();

        let decoded = CliprdrPdu::decode(&encoded, true).unwrap();
        if let CliprdrPdu::FormatDataResponse(resp) = decoded {
            assert!(resp.success);
            assert_eq!(resp.data, original_data);
        } else {
            panic!("Expected FormatDataResponse PDU");
        }

        // Test fail response round-trip
        let fail_response = FormatDataResponsePdu::fail();
        let encoded_fail = fail_response.encode();

        let decoded_fail = CliprdrPdu::decode(&encoded_fail, true).unwrap();
        if let CliprdrPdu::FormatDataResponse(resp) = decoded_fail {
            assert!(!resp.success);
        } else {
            panic!("Expected FormatDataResponse PDU");
        }
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
    fn test_format_data_response_unicode_cjk() {
        // "日本語" (Japanese) in UTF-16LE
        // 日 = U+65E5, 本 = U+672C, 語 = U+8A9E
        let data = vec![
            0xE5, 0x65, // 日
            0x2C, 0x67, // 本
            0x9E, 0x8A, // 語
            0x00, 0x00, // null
        ];
        let response = FormatDataResponsePdu::ok(data);

        let text = response.as_utf8_from_unicode().unwrap();
        assert_eq!(text, "日本語");
    }

    #[test]
    fn test_format_data_response_unicode_emoji() {
        // "Hi👋" with waving hand emoji (U+1F44B) - requires surrogate pair in UTF-16
        // U+1F44B = D83D DC4B in UTF-16 surrogate pair
        let data = vec![
            0x48, 0x00, // H
            0x69, 0x00, // i
            0x3D, 0xD8, // High surrogate D83D
            0x4B, 0xDC, // Low surrogate DC4B
            0x00, 0x00, // null
        ];
        let response = FormatDataResponsePdu::ok(data);

        let text = response.as_utf8_from_unicode().unwrap();
        assert_eq!(text, "Hi👋");
    }

    #[test]
    fn test_format_data_response_unicode_rtl() {
        // "שלום" (Hebrew "Shalom") in UTF-16LE
        // ש = U+05E9, ל = U+05DC, ו = U+05D5, ם = U+05DD
        let data = vec![
            0xE9, 0x05, // ש
            0xDC, 0x05, // ל
            0xD5, 0x05, // ו
            0xDD, 0x05, // ם
            0x00, 0x00, // null
        ];
        let response = FormatDataResponsePdu::ok(data);

        let text = response.as_utf8_from_unicode().unwrap();
        assert_eq!(text, "שלום");
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

    // Story 5.4: File Contents PDU tests

    #[test]
    fn test_file_contents_request_size() {
        let request = FileContentsRequestPdu::size(1, 0);
        assert_eq!(request.stream_id, 1);
        assert_eq!(request.lindex, 0);
        assert!(matches!(request.flags, FileContentsFlags::Size));
        assert_eq!(request.offset(), 0);

        let encoded = request.encode();

        // Header check
        assert_eq!(
            u16::from_le_bytes([encoded[0], encoded[1]]),
            MessageType::FileContentsRequest as u16
        );

        // Decode
        let decoded = CliprdrPdu::decode(&encoded, true).unwrap();
        if let CliprdrPdu::FileContentsRequest(req) = decoded {
            assert_eq!(req.stream_id, 1);
            assert_eq!(req.lindex, 0);
            assert!(matches!(req.flags, FileContentsFlags::Size));
        } else {
            panic!("Expected FileContentsRequest PDU");
        }
    }

    #[test]
    fn test_file_contents_request_range() {
        let request = FileContentsRequestPdu::range(42, 3, 0x100000000, 65536);
        assert_eq!(request.stream_id, 42);
        assert_eq!(request.lindex, 3);
        assert!(matches!(request.flags, FileContentsFlags::Range));
        assert_eq!(request.offset(), 0x100000000);
        assert_eq!(request.cb_requested, 65536);
        assert_eq!(request.position_high, 1);
        assert_eq!(request.position_low, 0);

        let encoded = request.encode();
        let decoded = CliprdrPdu::decode(&encoded, true).unwrap();
        if let CliprdrPdu::FileContentsRequest(req) = decoded {
            assert_eq!(req.stream_id, 42);
            assert_eq!(req.lindex, 3);
            assert_eq!(req.offset(), 0x100000000);
            assert_eq!(req.cb_requested, 65536);
        } else {
            panic!("Expected FileContentsRequest PDU");
        }
    }

    #[test]
    fn test_file_contents_response_size() {
        // File size response: 8 bytes containing u64 file size
        let file_size: u64 = 1_234_567_890;
        let size_bytes = file_size.to_le_bytes().to_vec();
        let response = FileContentsResponsePdu::ok(1, size_bytes);

        assert!(response.success);
        assert_eq!(response.stream_id, 1);
        assert_eq!(response.as_file_size(), Some(1_234_567_890));

        let encoded = response.encode();

        // Header check
        assert_eq!(
            u16::from_le_bytes([encoded[0], encoded[1]]),
            MessageType::FileContentsResponse as u16
        );
        let flags = MessageFlags::from_bits(u16::from_le_bytes([encoded[2], encoded[3]]));
        assert!(flags.is_ok());

        // Decode
        let decoded = CliprdrPdu::decode(&encoded, true).unwrap();
        if let CliprdrPdu::FileContentsResponse(resp) = decoded {
            assert!(resp.success);
            assert_eq!(resp.stream_id, 1);
            assert_eq!(resp.as_file_size(), Some(1_234_567_890));
        } else {
            panic!("Expected FileContentsResponse PDU");
        }
    }

    #[test]
    fn test_file_contents_response_data() {
        // File content response
        let content = vec![0x48, 0x65, 0x6C, 0x6C, 0x6F]; // "Hello"
        let response = FileContentsResponsePdu::ok(2, content.clone());

        assert!(response.success);
        assert_eq!(response.stream_id, 2);
        assert_eq!(response.data, content);
        // Should not be a valid file size (wrong length)
        assert_eq!(response.as_file_size(), None);

        let encoded = response.encode();
        let decoded = CliprdrPdu::decode(&encoded, true).unwrap();
        if let CliprdrPdu::FileContentsResponse(resp) = decoded {
            assert_eq!(resp.data, content);
        } else {
            panic!("Expected FileContentsResponse PDU");
        }
    }

    #[test]
    fn test_file_contents_response_fail() {
        let response = FileContentsResponsePdu::fail(5);

        assert!(!response.success);
        assert_eq!(response.stream_id, 5);
        assert!(response.data.is_empty());

        let encoded = response.encode();
        let flags = MessageFlags::from_bits(u16::from_le_bytes([encoded[2], encoded[3]]));
        assert!(flags.is_fail());

        let decoded = CliprdrPdu::decode(&encoded, true).unwrap();
        if let CliprdrPdu::FileContentsResponse(resp) = decoded {
            assert!(!resp.success);
            assert_eq!(resp.stream_id, 5);
        } else {
            panic!("Expected FileContentsResponse PDU");
        }
    }

    #[test]
    fn test_file_descriptor_decode() {
        // Create a minimal FILEDESCRIPTORW (592 bytes)
        let mut data = vec![0u8; FILEDESCRIPTORW_SIZE];

        // dwFlags = FD_FILESIZE | FD_ATTRIBUTES (0x44)
        data[0..4].copy_from_slice(&0x44u32.to_le_bytes());

        // dwFileAttributes = FILE_ATTRIBUTE_NORMAL (0x80)
        data[36..40].copy_from_slice(&0x80u32.to_le_bytes());

        // nFileSizeHigh = 0, nFileSizeLow = 12345
        data[64..68].copy_from_slice(&0u32.to_le_bytes());
        data[68..72].copy_from_slice(&12345u32.to_le_bytes());

        // cFileName = "test.txt" in UTF-16LE
        let filename = "test.txt";
        let mut offset = 72;
        for ch in filename.encode_utf16() {
            data[offset..offset + 2].copy_from_slice(&ch.to_le_bytes());
            offset += 2;
        }
        // Null terminator already there (vec initialized with zeros)

        let descriptor = FileDescriptorW::decode(&data).unwrap();

        assert_eq!(descriptor.flags, 0x44);
        assert!(descriptor.has_file_size());
        assert!(descriptor.has_attributes());
        assert!(!descriptor.is_directory());
        assert_eq!(descriptor.file_size, 12345);
        assert_eq!(descriptor.file_name, "test.txt");
    }

    #[test]
    fn test_file_descriptor_directory() {
        let mut data = vec![0u8; FILEDESCRIPTORW_SIZE];

        // dwFlags = FD_ATTRIBUTES (0x04)
        data[0..4].copy_from_slice(&0x04u32.to_le_bytes());

        // dwFileAttributes = FILE_ATTRIBUTE_DIRECTORY (0x10)
        data[36..40].copy_from_slice(&0x10u32.to_le_bytes());

        // cFileName = "Documents" in UTF-16LE
        let filename = "Documents";
        let mut offset = 72;
        for ch in filename.encode_utf16() {
            data[offset..offset + 2].copy_from_slice(&ch.to_le_bytes());
            offset += 2;
        }

        let descriptor = FileDescriptorW::decode(&data).unwrap();

        assert!(descriptor.is_directory());
        assert_eq!(descriptor.file_name, "Documents");
    }

    #[test]
    fn test_cliprdr_file_list_decode() {
        // Create a file list with 2 files
        let mut data = Vec::new();

        // cItems = 2
        data.extend_from_slice(&2u32.to_le_bytes());

        // File 1: "file1.txt"
        let mut file1 = vec![0u8; FILEDESCRIPTORW_SIZE];
        file1[0..4].copy_from_slice(&0x40u32.to_le_bytes()); // FD_FILESIZE
        file1[68..72].copy_from_slice(&100u32.to_le_bytes()); // size = 100
        let name1 = "file1.txt";
        let mut offset = 72;
        for ch in name1.encode_utf16() {
            file1[offset..offset + 2].copy_from_slice(&ch.to_le_bytes());
            offset += 2;
        }
        data.extend_from_slice(&file1);

        // File 2: "file2.pdf"
        let mut file2 = vec![0u8; FILEDESCRIPTORW_SIZE];
        file2[0..4].copy_from_slice(&0x40u32.to_le_bytes()); // FD_FILESIZE
        file2[68..72].copy_from_slice(&500u32.to_le_bytes()); // size = 500
        let name2 = "file2.pdf";
        offset = 72;
        for ch in name2.encode_utf16() {
            file2[offset..offset + 2].copy_from_slice(&ch.to_le_bytes());
            offset += 2;
        }
        data.extend_from_slice(&file2);

        let file_list = CliprdrFileList::decode(&data).unwrap();

        assert_eq!(file_list.count(), 2);
        let names: Vec<&str> = file_list.file_names().collect();
        assert_eq!(names, vec!["file1.txt", "file2.pdf"]);
        assert_eq!(file_list.files[0].file_size, 100);
        assert_eq!(file_list.files[1].file_size, 500);
    }

    #[test]
    fn test_cliprdr_file_list_empty() {
        // Empty file list
        let data = vec![0, 0, 0, 0]; // cItems = 0

        let file_list = CliprdrFileList::decode(&data).unwrap();
        assert_eq!(file_list.count(), 0);
    }

    #[test]
    fn test_file_descriptor_unicode_filename() {
        let mut data = vec![0u8; FILEDESCRIPTORW_SIZE];

        // dwFlags = FD_FILESIZE
        data[0..4].copy_from_slice(&0x40u32.to_le_bytes());
        data[68..72].copy_from_slice(&1000u32.to_le_bytes());

        // cFileName = "文書.txt" (Japanese "document.txt") in UTF-16LE
        let filename = "文書.txt";
        let mut offset = 72;
        for ch in filename.encode_utf16() {
            data[offset..offset + 2].copy_from_slice(&ch.to_le_bytes());
            offset += 2;
        }

        let descriptor = FileDescriptorW::decode(&data).unwrap();
        assert_eq!(descriptor.file_name, "文書.txt");
    }

    #[test]
    fn test_file_contents_flags() {
        assert_eq!(FileContentsFlags::Size as u32, 0x0001);
        assert_eq!(FileContentsFlags::Range as u32, 0x0002);
    }

    #[test]
    fn test_fd_flags_constants() {
        assert_eq!(fd_flags::FD_CLSID, 0x0001);
        assert_eq!(fd_flags::FD_SIZEPOINT, 0x0002);
        assert_eq!(fd_flags::FD_ATTRIBUTES, 0x0004);
        assert_eq!(fd_flags::FD_CREATETIME, 0x0008);
        assert_eq!(fd_flags::FD_ACCESSTIME, 0x0010);
        assert_eq!(fd_flags::FD_WRITETIME, 0x0020);
        assert_eq!(fd_flags::FD_FILESIZE, 0x0040);
        assert_eq!(fd_flags::FD_PROGRESSUI, 0x4000);
        assert_eq!(fd_flags::FD_LINKUI, 0x8000);
    }

    // Story 5.5: Tests for encoding functions

    #[test]
    fn test_unix_to_filetime() {
        // Unix epoch (1970-01-01 00:00:00) should convert correctly
        let filetime = unix_to_filetime(0);
        // Expected: 11644473600 * 10000000 = 116444736000000000
        assert_eq!(filetime, 116_444_736_000_000_000);

        // A known timestamp: 2024-01-01 00:00:00 UTC = 1704067200
        let filetime2 = unix_to_filetime(1_704_067_200);
        // This should be > Unix epoch conversion
        assert!(filetime2 > filetime);
    }

    #[test]
    fn test_file_descriptor_encode_normal_file() {
        let fd = FileDescriptorW::from_local_file(
            "test.txt".to_string(),
            1234,
            1_704_067_200, // 2024-01-01 00:00:00 UTC
            false,
        );

        let encoded = fd.encode();
        assert_eq!(encoded.len(), FILEDESCRIPTORW_SIZE);

        // Verify flags (FD_ATTRIBUTES | FD_FILESIZE | FD_WRITETIME = 0x64)
        let flags = u32::from_le_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]);
        assert_eq!(
            flags,
            fd_flags::FD_ATTRIBUTES | fd_flags::FD_FILESIZE | fd_flags::FD_WRITETIME
        );

        // Verify attributes (FILE_ATTRIBUTE_NORMAL = 0x80)
        let attrs = u32::from_le_bytes([encoded[36], encoded[37], encoded[38], encoded[39]]);
        assert_eq!(attrs, file_attributes::FILE_ATTRIBUTE_NORMAL);

        // Verify file size (low DWORD at offset 68)
        let size_low = u32::from_le_bytes([encoded[68], encoded[69], encoded[70], encoded[71]]);
        assert_eq!(size_low, 1234);
    }

    #[test]
    fn test_file_descriptor_encode_directory() {
        let fd = FileDescriptorW::from_local_file("my_folder".to_string(), 0, 1_704_067_200, true);

        let encoded = fd.encode();
        assert_eq!(encoded.len(), FILEDESCRIPTORW_SIZE);

        // Verify attributes (FILE_ATTRIBUTE_DIRECTORY = 0x10)
        let attrs = u32::from_le_bytes([encoded[36], encoded[37], encoded[38], encoded[39]]);
        assert_eq!(attrs, file_attributes::FILE_ATTRIBUTE_DIRECTORY);
    }

    #[test]
    fn test_file_descriptor_encode_decode_roundtrip() {
        let original = FileDescriptorW::from_local_file(
            "document.pdf".to_string(),
            1_048_576, // 1MB
            1_704_067_200,
            false,
        );

        let encoded = original.encode();
        let decoded = FileDescriptorW::decode(&encoded).unwrap();

        assert_eq!(decoded.file_name, "document.pdf");
        assert_eq!(decoded.file_size, 1_048_576);
        assert!(decoded.has_file_size());
        assert!(decoded.has_attributes());
        assert!(!decoded.is_directory());
    }

    #[test]
    fn test_file_descriptor_encode_unicode_filename() {
        let fd = FileDescriptorW::from_local_file(
            "日本語ファイル.txt".to_string(),
            100,
            1_704_067_200,
            false,
        );

        let encoded = fd.encode();
        let decoded = FileDescriptorW::decode(&encoded).unwrap();

        assert_eq!(decoded.file_name, "日本語ファイル.txt");
    }

    #[test]
    fn test_file_descriptor_encode_large_file() {
        // Test file larger than 4GB (requires both high and low DWORDs)
        let fd = FileDescriptorW::from_local_file(
            "large_file.iso".to_string(),
            5_000_000_000, // ~4.7GB
            1_704_067_200,
            false,
        );

        let encoded = fd.encode();
        let decoded = FileDescriptorW::decode(&encoded).unwrap();

        assert_eq!(decoded.file_size, 5_000_000_000);
    }

    #[test]
    fn test_cliprdr_file_list_encode_single() {
        let files = vec![FileDescriptorW::from_local_file(
            "file.txt".to_string(),
            1000,
            1_704_067_200,
            false,
        )];

        let file_list = CliprdrFileList::new(files);
        let encoded = file_list.encode();

        // Should be 4 bytes (count) + 592 bytes (1 file)
        assert_eq!(encoded.len(), 4 + FILEDESCRIPTORW_SIZE);

        // Verify count
        let count = u32::from_le_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]);
        assert_eq!(count, 1);
    }

    #[test]
    fn test_cliprdr_file_list_encode_multiple() {
        let files = vec![
            FileDescriptorW::from_local_file("file1.txt".to_string(), 100, 0, false),
            FileDescriptorW::from_local_file("file2.txt".to_string(), 200, 0, false),
            FileDescriptorW::from_local_file("folder".to_string(), 0, 0, true),
        ];

        let file_list = CliprdrFileList::new(files);
        let encoded = file_list.encode();

        // Should be 4 bytes (count) + 3 * 592 bytes
        assert_eq!(encoded.len(), 4 + 3 * FILEDESCRIPTORW_SIZE);

        // Verify count
        let count = u32::from_le_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]);
        assert_eq!(count, 3);
    }

    #[test]
    fn test_cliprdr_file_list_encode_decode_roundtrip() {
        let files = vec![
            FileDescriptorW::from_local_file("doc.pdf".to_string(), 50_000, 1_704_067_200, false),
            FileDescriptorW::from_local_file("photos".to_string(), 0, 1_704_067_200, true),
        ];

        let original = CliprdrFileList::new(files);
        let encoded = original.encode();
        let decoded = CliprdrFileList::decode(&encoded).unwrap();

        assert_eq!(decoded.count(), 2);
        let names: Vec<&str> = decoded.file_names().collect();
        assert_eq!(names, vec!["doc.pdf", "photos"]);
        assert!(!decoded.files[0].is_directory());
        assert!(decoded.files[1].is_directory());
    }

    #[test]
    fn test_file_attributes_constants() {
        assert_eq!(file_attributes::FILE_ATTRIBUTE_READONLY, 0x0001);
        assert_eq!(file_attributes::FILE_ATTRIBUTE_HIDDEN, 0x0002);
        assert_eq!(file_attributes::FILE_ATTRIBUTE_SYSTEM, 0x0004);
        assert_eq!(file_attributes::FILE_ATTRIBUTE_DIRECTORY, 0x0010);
        assert_eq!(file_attributes::FILE_ATTRIBUTE_ARCHIVE, 0x0020);
        assert_eq!(file_attributes::FILE_ATTRIBUTE_NORMAL, 0x0080);
    }

    #[test]
    fn test_format_list_file_formats() {
        let format_list = FormatListPdu::file_formats(0xC100, 0xC101);

        assert_eq!(format_list.formats.len(), 2);
        assert_eq!(format_list.formats[0].id, 0xC100);
        assert_eq!(format_list.formats[0].name, "FileGroupDescriptorW");
        assert_eq!(format_list.formats[1].id, 0xC101);
        assert_eq!(format_list.formats[1].name, "FileContents");

        // Test encoding/decoding roundtrip
        let encoded = format_list.encode_payload_long();
        let decoded = FormatListPdu::decode_long(&encoded).unwrap();
        assert_eq!(decoded.formats.len(), 2);
        assert_eq!(decoded.formats[0].name, "FileGroupDescriptorW");
    }
}
