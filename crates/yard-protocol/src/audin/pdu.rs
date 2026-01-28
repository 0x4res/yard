//! AUDIN PDU structures per MS-RDPEAI specification.
//!
//! This module implements the Protocol Data Units for the Audio Input
//! Redirection Virtual Channel Extension.
//!
//! Reference: [MS-RDPEAI] Remote Desktop Protocol: Audio Input Redirection
//! Virtual Channel Extension

use ironrdp::dvc::DvcEncode;
use ironrdp::pdu::{Encode, EncodeResult, PduResult};
use thiserror::Error;
use yard_audio::AudioFormat;

/// AUDIN-specific error type.
#[derive(Debug, Error)]
pub enum AudinError {
    /// Invalid message type.
    #[error("invalid AUDIN message type: {0:#x}")]
    InvalidMessageType(u8),
    /// Payload too short.
    #[error("AUDIN payload too short for {context}: expected {expected}, got {actual}")]
    PayloadTooShort {
        context: &'static str,
        expected: usize,
        actual: usize,
    },
    /// Unexpected PDU from server.
    #[error("unexpected AUDIN PDU from server: {0:?}")]
    UnexpectedPdu(MessageType),
}

/// AUDIN protocol version.
pub const AUDIN_VERSION: u32 = 1;

/// AUDIN PDU message types (MessageId field).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    /// Version PDU (MSG_SNDIN_VERSION) - section 2.2.2.1
    Version = 0x01,
    /// Sound Formats PDU (MSG_SNDIN_FORMATS) - section 2.2.2.2
    SoundFormats = 0x02,
    /// Open PDU (MSG_SNDIN_OPEN) - section 2.2.2.3
    Open = 0x03,
    /// Open Reply PDU (MSG_SNDIN_OPEN_REPLY) - section 2.2.3.1
    OpenReply = 0x04,
    /// Data Incoming PDU (MSG_SNDIN_DATA_INCOMING) - section 2.2.2.5
    DataIncoming = 0x05,
    /// Data PDU (MSG_SNDIN_DATA) - section 2.2.2.6
    Data = 0x06,
    /// Format Change PDU (MSG_SNDIN_FORMATCHANGE) - section 2.2.2.4
    FormatChange = 0x07,
}

impl TryFrom<u8> for MessageType {
    type Error = AudinError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x01 => Ok(Self::Version),
            0x02 => Ok(Self::SoundFormats),
            0x03 => Ok(Self::Open),
            0x04 => Ok(Self::OpenReply),
            0x05 => Ok(Self::DataIncoming),
            0x06 => Ok(Self::Data),
            0x07 => Ok(Self::FormatChange),
            _ => Err(AudinError::InvalidMessageType(value)),
        }
    }
}

/// WAVE_FORMAT_PCM constant.
pub const WAVE_FORMAT_PCM: u16 = 0x0001;

/// Parsed AUDIN PDU.
#[derive(Debug, Clone)]
pub enum AudinPdu {
    /// Version PDU from server.
    Version(VersionPdu),
    /// Sound Formats PDU from server.
    SoundFormats(Vec<AudioFormat>),
    /// Open PDU from server (start recording).
    Open {
        format: AudioFormat,
        frames_per_packet: u32,
    },
    /// Format Change PDU from server.
    FormatChange { new_format: u32 },
    /// Data PDU (unexpected from server).
    Data(Vec<u8>),
}

impl AudinPdu {
    /// Decodes an AUDIN PDU from raw bytes.
    pub fn decode(data: &[u8]) -> PduResult<Self> {
        if data.is_empty() {
            return Err(ironrdp::pdu::other_err!("AUDIN", "empty payload"));
        }

        let msg_type = MessageType::try_from(data[0])
            .map_err(|e| ironrdp::pdu::other_err!("AUDIN", source: e))?;
        let payload = &data[1..];

        match msg_type {
            MessageType::Version => {
                let pdu = VersionPdu::decode(payload)?;
                Ok(Self::Version(pdu))
            }
            MessageType::SoundFormats => {
                let formats = ServerSoundFormats::decode(payload)?;
                Ok(Self::SoundFormats(formats.formats))
            }
            MessageType::Open => {
                let open = OpenPdu::decode(payload)?;
                Ok(Self::Open {
                    format: open.format,
                    frames_per_packet: open.frames_per_packet,
                })
            }
            MessageType::FormatChange => {
                let fc = FormatChange::decode(payload)?;
                Ok(Self::FormatChange {
                    new_format: fc.new_format,
                })
            }
            MessageType::Data => Ok(Self::Data(payload.to_vec())),
            MessageType::OpenReply | MessageType::DataIncoming => {
                // These are client->server PDUs, unexpected from server
                Err(ironrdp::pdu::other_err!("AUDIN", source: AudinError::UnexpectedPdu(msg_type)))
            }
        }
    }
}

/// Version PDU (MSG_SNDIN_VERSION) - section 2.2.2.1
///
/// Structure:
/// - Header (1 byte): MessageId = 0x01
/// - Version (4 bytes): Protocol version
#[derive(Debug, Clone, Copy)]
pub struct VersionPdu {
    /// Protocol version.
    pub version: u32,
}

impl VersionPdu {
    /// Creates a new Version PDU.
    pub fn new(version: u32) -> Self {
        Self { version }
    }

    /// Decodes a Version PDU from payload (after header).
    pub fn decode(payload: &[u8]) -> PduResult<Self> {
        if payload.len() < 4 {
            return Err(ironrdp::pdu::other_err!(
                "AUDIN",
                source: AudinError::PayloadTooShort {
                    context: "VersionPdu",
                    expected: 4,
                    actual: payload.len(),
                }
            ));
        }
        let version = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        Ok(Self { version })
    }

    /// Encodes the Version PDU to bytes.
    pub fn encode(&self) -> PduResult<Vec<u8>> {
        let mut buf = Vec::with_capacity(5);
        buf.push(MessageType::Version as u8);
        buf.extend_from_slice(&self.version.to_le_bytes());
        Ok(buf)
    }
}

/// Server Sound Formats PDU (MSG_SNDIN_FORMATS) - section 2.2.2.2
///
/// Structure:
/// - Header (1 byte): MessageId = 0x02
/// - NumFormats (4 bytes): Number of formats
/// - cbSizeFormatsPacket (4 bytes): Size of packet (minus ExtraData)
/// - SoundFormats (variable): Array of AUDIO_FORMAT structures
#[derive(Debug, Clone)]
pub struct ServerSoundFormats {
    /// Supported audio formats.
    pub formats: Vec<AudioFormat>,
}

impl ServerSoundFormats {
    /// Decodes server sound formats from payload (after header).
    pub fn decode(payload: &[u8]) -> PduResult<Self> {
        if payload.len() < 8 {
            return Err(ironrdp::pdu::other_err!(
                "AUDIN",
                source: AudinError::PayloadTooShort {
                    context: "ServerSoundFormats",
                    expected: 8,
                    actual: payload.len(),
                }
            ));
        }

        let num_formats = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let _cb_size = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);

        let mut formats = Vec::with_capacity(num_formats as usize);
        let mut offset = 8;

        for _ in 0..num_formats {
            if offset + 18 > payload.len() {
                break; // Not enough data for another format
            }

            // AUDIO_FORMAT structure (18 bytes minimum):
            // wFormatTag (2), nChannels (2), nSamplesPerSec (4),
            // nAvgBytesPerSec (4), nBlockAlign (2), wBitsPerSample (2), cbSize (2)
            let w_format_tag = u16::from_le_bytes([payload[offset], payload[offset + 1]]);
            let n_channels = u16::from_le_bytes([payload[offset + 2], payload[offset + 3]]);
            let n_samples_per_sec = u32::from_le_bytes([
                payload[offset + 4],
                payload[offset + 5],
                payload[offset + 6],
                payload[offset + 7],
            ]);
            let _n_avg_bytes_per_sec = u32::from_le_bytes([
                payload[offset + 8],
                payload[offset + 9],
                payload[offset + 10],
                payload[offset + 11],
            ]);
            let _n_block_align = u16::from_le_bytes([payload[offset + 12], payload[offset + 13]]);
            let w_bits_per_sample =
                u16::from_le_bytes([payload[offset + 14], payload[offset + 15]]);
            let cb_size = u16::from_le_bytes([payload[offset + 16], payload[offset + 17]]);

            // Only support PCM for now
            if w_format_tag == WAVE_FORMAT_PCM {
                formats.push(AudioFormat {
                    sample_rate: n_samples_per_sec,
                    channels: n_channels as u8,
                    bits_per_sample: w_bits_per_sample as u8,
                });
            }

            offset += 18 + cb_size as usize;
        }

        Ok(Self { formats })
    }
}

/// Client Sound Formats PDU - section 2.2.3.2
///
/// Structure:
/// - Header (1 byte): MessageId = 0x02
/// - NumFormats (4 bytes): Number of formats
/// - cbSizeFormatsPacket (4 bytes): Size of packet
/// - SoundFormats (variable): Array of AUDIO_FORMAT structures
#[derive(Debug, Clone)]
pub struct ClientSoundFormats {
    /// Supported audio formats.
    pub formats: Vec<AudioFormat>,
}

impl ClientSoundFormats {
    /// Creates a new Client Sound Formats PDU.
    pub fn new(formats: &[AudioFormat]) -> Self {
        Self {
            formats: formats.to_vec(),
        }
    }

    /// Encodes the Client Sound Formats PDU.
    pub fn encode(&self) -> PduResult<Vec<u8>> {
        // Calculate size: header (1) + num_formats (4) + cb_size (4) + formats (18 each)
        let format_size = 18 * self.formats.len();
        let total_size = 1 + 4 + 4 + format_size;
        let mut buf = Vec::with_capacity(total_size);

        // Header
        buf.push(MessageType::SoundFormats as u8);

        // NumFormats
        buf.extend_from_slice(&(self.formats.len() as u32).to_le_bytes());

        // cbSizeFormatsPacket (size of packet minus ExtraData, which we don't have)
        buf.extend_from_slice(&(total_size as u32).to_le_bytes());

        // SoundFormats
        for format in &self.formats {
            // wFormatTag = WAVE_FORMAT_PCM
            buf.extend_from_slice(&WAVE_FORMAT_PCM.to_le_bytes());
            // nChannels
            buf.extend_from_slice(&(format.channels as u16).to_le_bytes());
            // nSamplesPerSec
            buf.extend_from_slice(&format.sample_rate.to_le_bytes());
            // nAvgBytesPerSec = sample_rate * channels * bytes_per_sample
            let bytes_per_sample = format.bits_per_sample as u32 / 8;
            let avg_bytes_per_sec = format.sample_rate * format.channels as u32 * bytes_per_sample;
            buf.extend_from_slice(&avg_bytes_per_sec.to_le_bytes());
            // nBlockAlign = channels * bytes_per_sample
            let block_align = format.channels as u16 * (format.bits_per_sample / 8) as u16;
            buf.extend_from_slice(&block_align.to_le_bytes());
            // wBitsPerSample
            buf.extend_from_slice(&(format.bits_per_sample as u16).to_le_bytes());
            // cbSize = 0 (no extra data for PCM)
            buf.extend_from_slice(&0u16.to_le_bytes());
        }

        Ok(buf)
    }
}

/// Open PDU (MSG_SNDIN_OPEN) - section 2.2.2.3
///
/// Server sends this to request the client to start recording.
#[derive(Debug, Clone)]
pub struct OpenPdu {
    /// Frames per packet to send.
    pub frames_per_packet: u32,
    /// Initial format index.
    pub initial_format: u32,
    /// Audio format to use.
    pub format: AudioFormat,
}

impl OpenPdu {
    /// Decodes an Open PDU from payload (after header).
    pub fn decode(payload: &[u8]) -> PduResult<Self> {
        if payload.len() < 26 {
            return Err(ironrdp::pdu::other_err!(
                "AUDIN",
                source: AudinError::PayloadTooShort {
                    context: "OpenPdu",
                    expected: 26,
                    actual: payload.len(),
                }
            ));
        }

        let frames_per_packet =
            u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let initial_format = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);

        // Audio format fields starting at offset 8
        let _w_format_tag = u16::from_le_bytes([payload[8], payload[9]]);
        let n_channels = u16::from_le_bytes([payload[10], payload[11]]);
        let n_samples_per_sec =
            u32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
        let _n_avg_bytes_per_sec =
            u32::from_le_bytes([payload[16], payload[17], payload[18], payload[19]]);
        let _n_block_align = u16::from_le_bytes([payload[20], payload[21]]);
        let w_bits_per_sample = u16::from_le_bytes([payload[22], payload[23]]);
        // cbSize at offset 24-25

        Ok(Self {
            frames_per_packet,
            initial_format,
            format: AudioFormat {
                sample_rate: n_samples_per_sec,
                channels: n_channels as u8,
                bits_per_sample: w_bits_per_sample as u8,
            },
        })
    }
}

/// Open Reply PDU (MSG_SNDIN_OPEN_REPLY) - section 2.2.3.1
///
/// Client sends this in response to Open PDU.
///
/// Structure:
/// - Header (1 byte): MessageId = 0x04
/// - Result (4 bytes): HRESULT (0 = success)
#[derive(Debug, Clone, Copy)]
pub struct OpenReply {
    /// Result code (0 = success).
    pub result: u32,
}

impl OpenReply {
    /// Creates a successful Open Reply.
    pub fn success() -> Self {
        Self { result: 0 }
    }

    /// Creates a failed Open Reply.
    pub fn failure(result: u32) -> Self {
        Self { result }
    }

    /// Encodes the Open Reply PDU.
    pub fn encode(&self) -> PduResult<Vec<u8>> {
        let mut buf = Vec::with_capacity(5);
        buf.push(MessageType::OpenReply as u8);
        buf.extend_from_slice(&self.result.to_le_bytes());
        Ok(buf)
    }
}

/// Data Incoming PDU (MSG_SNDIN_DATA_INCOMING) - section 2.2.2.5
///
/// Client sends this before sending a Data PDU.
///
/// Structure:
/// - Header (1 byte): MessageId = 0x05
#[derive(Debug, Clone, Copy)]
pub struct DataIncoming;

impl DataIncoming {
    /// Encodes the Data Incoming PDU.
    pub fn encode(&self) -> PduResult<Vec<u8>> {
        Ok(vec![MessageType::DataIncoming as u8])
    }
}

/// Data PDU (MSG_SNDIN_DATA) - section 2.2.2.6
///
/// Client sends this with captured audio data.
///
/// Structure:
/// - Header (1 byte): MessageId = 0x06
/// - Data (variable): Audio data
#[derive(Debug, Clone)]
pub struct DataPdu {
    /// Audio data.
    pub data: Vec<u8>,
}

impl DataPdu {
    /// Creates a new Data PDU.
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Encodes the Data PDU.
    pub fn encode(&self) -> PduResult<Vec<u8>> {
        let mut buf = Vec::with_capacity(1 + self.data.len());
        buf.push(MessageType::Data as u8);
        buf.extend_from_slice(&self.data);
        Ok(buf)
    }
}

/// Format Change PDU (MSG_SNDIN_FORMATCHANGE) - section 2.2.2.4
///
/// Structure:
/// - Header (1 byte): MessageId = 0x07
/// - NewFormat (4 bytes): Index into format list
#[derive(Debug, Clone, Copy)]
pub struct FormatChange {
    /// New format index.
    pub new_format: u32,
}

impl FormatChange {
    /// Creates a new Format Change PDU.
    pub fn new(new_format: u32) -> Self {
        Self { new_format }
    }

    /// Decodes a Format Change PDU from payload (after header).
    pub fn decode(payload: &[u8]) -> PduResult<Self> {
        if payload.len() < 4 {
            return Err(ironrdp::pdu::other_err!(
                "AUDIN",
                source: AudinError::PayloadTooShort {
                    context: "FormatChange",
                    expected: 4,
                    actual: payload.len(),
                }
            ));
        }
        let new_format = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        Ok(Self { new_format })
    }

    /// Encodes the Format Change PDU.
    pub fn encode(&self) -> PduResult<Vec<u8>> {
        let mut buf = Vec::with_capacity(5);
        buf.push(MessageType::FormatChange as u8);
        buf.extend_from_slice(&self.new_format.to_le_bytes());
        Ok(buf)
    }
}

/// Wrapper for AUDIN PDU that implements DvcEncode.
///
/// This allows AUDIN PDUs to be sent as DVC messages.
#[derive(Clone)]
pub struct AudinDvcMessage {
    data: Vec<u8>,
}

impl AudinDvcMessage {
    /// Creates a new AUDIN DVC message from raw bytes.
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Creates a DVC message from a Version PDU.
    pub fn from_version(pdu: &VersionPdu) -> PduResult<Self> {
        Ok(Self::new(pdu.encode()?))
    }

    /// Creates a DVC message from a Client Sound Formats PDU.
    pub fn from_sound_formats(pdu: &ClientSoundFormats) -> PduResult<Self> {
        Ok(Self::new(pdu.encode()?))
    }

    /// Creates a DVC message from an Open Reply PDU.
    pub fn from_open_reply(pdu: &OpenReply) -> PduResult<Self> {
        Ok(Self::new(pdu.encode()?))
    }

    /// Creates a DVC message from a Data Incoming PDU.
    pub fn from_data_incoming(pdu: &DataIncoming) -> PduResult<Self> {
        Ok(Self::new(pdu.encode()?))
    }

    /// Creates a DVC message from a Data PDU.
    pub fn from_data(pdu: &DataPdu) -> PduResult<Self> {
        Ok(Self::new(pdu.encode()?))
    }

    /// Creates a DVC message from a Format Change PDU.
    pub fn from_format_change(pdu: &FormatChange) -> PduResult<Self> {
        Ok(Self::new(pdu.encode()?))
    }
}

impl Encode for AudinDvcMessage {
    fn encode(&self, dst: &mut ironrdp::pdu::WriteCursor<'_>) -> EncodeResult<()> {
        dst.write_slice(&self.data);
        Ok(())
    }

    fn name(&self) -> &'static str {
        "AudinDvcMessage"
    }

    fn size(&self) -> usize {
        self.data.len()
    }
}

// SAFETY: AudinDvcMessage only contains Vec<u8> which is Send
unsafe impl Send for AudinDvcMessage {}

impl DvcEncode for AudinDvcMessage {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_type_conversion() {
        assert_eq!(MessageType::try_from(0x01).unwrap(), MessageType::Version);
        assert_eq!(
            MessageType::try_from(0x02).unwrap(),
            MessageType::SoundFormats
        );
        assert_eq!(MessageType::try_from(0x03).unwrap(), MessageType::Open);
        assert_eq!(MessageType::try_from(0x04).unwrap(), MessageType::OpenReply);
        assert_eq!(
            MessageType::try_from(0x05).unwrap(),
            MessageType::DataIncoming
        );
        assert_eq!(MessageType::try_from(0x06).unwrap(), MessageType::Data);
        assert_eq!(
            MessageType::try_from(0x07).unwrap(),
            MessageType::FormatChange
        );
        assert!(MessageType::try_from(0xFF).is_err());
    }

    #[test]
    fn test_version_pdu_encode_decode() {
        let pdu = VersionPdu::new(1);
        let encoded = pdu.encode().unwrap();
        assert_eq!(encoded[0], MessageType::Version as u8);
        assert_eq!(encoded.len(), 5);

        let decoded = VersionPdu::decode(&encoded[1..]).unwrap();
        assert_eq!(decoded.version, 1);
    }

    #[test]
    fn test_open_reply_encode() {
        let reply = OpenReply::success();
        let encoded = reply.encode().unwrap();
        assert_eq!(encoded[0], MessageType::OpenReply as u8);
        assert_eq!(encoded.len(), 5);
        assert_eq!(
            u32::from_le_bytes([encoded[1], encoded[2], encoded[3], encoded[4]]),
            0
        );
    }

    #[test]
    fn test_data_incoming_encode() {
        let pdu = DataIncoming;
        let encoded = pdu.encode().unwrap();
        assert_eq!(encoded.len(), 1);
        assert_eq!(encoded[0], MessageType::DataIncoming as u8);
    }

    #[test]
    fn test_data_pdu_encode() {
        let data = vec![0x01, 0x02, 0x03, 0x04];
        let pdu = DataPdu::new(data.clone());
        let encoded = pdu.encode().unwrap();
        assert_eq!(encoded[0], MessageType::Data as u8);
        assert_eq!(&encoded[1..], &data);
    }

    #[test]
    fn test_format_change_encode_decode() {
        let pdu = FormatChange::new(42);
        let encoded = pdu.encode().unwrap();
        assert_eq!(encoded[0], MessageType::FormatChange as u8);
        assert_eq!(encoded.len(), 5);

        let decoded = FormatChange::decode(&encoded[1..]).unwrap();
        assert_eq!(decoded.new_format, 42);
    }

    #[test]
    fn test_client_sound_formats_encode() {
        let formats = vec![
            AudioFormat::new(48000, 1, 16),
            AudioFormat::new(44100, 2, 16),
        ];
        let pdu = ClientSoundFormats::new(&formats);
        let encoded = pdu.encode().unwrap();

        assert_eq!(encoded[0], MessageType::SoundFormats as u8);
        // NumFormats should be 2
        let num_formats = u32::from_le_bytes([encoded[1], encoded[2], encoded[3], encoded[4]]);
        assert_eq!(num_formats, 2);
    }

    #[test]
    fn test_audin_pdu_decode_version() {
        let data = vec![0x01, 0x01, 0x00, 0x00, 0x00]; // Version PDU, version=1
        let pdu = AudinPdu::decode(&data).unwrap();
        if let AudinPdu::Version(v) = pdu {
            assert_eq!(v.version, 1);
        } else {
            panic!("Expected Version PDU");
        }
    }

    #[test]
    fn test_audin_pdu_decode_format_change() {
        let data = vec![0x07, 0x05, 0x00, 0x00, 0x00]; // FormatChange PDU, new_format=5
        let pdu = AudinPdu::decode(&data).unwrap();
        if let AudinPdu::FormatChange { new_format } = pdu {
            assert_eq!(new_format, 5);
        } else {
            panic!("Expected FormatChange PDU");
        }
    }
}
