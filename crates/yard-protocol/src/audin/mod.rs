//! AUDIN audio input channel handler for RDP microphone redirection.
//!
//! This module implements the AUDIN Dynamic Virtual Channel for capturing
//! audio from the local microphone and sending it to the RDP server.
//!
//! Audio data flow:
//! 1. Server sends `Open PDU` requesting audio capture
//! 2. `YardAudinHandler` signals audio thread to start capture via `ToAudio::StartCapture`
//! 3. Audio thread captures via PipeWire, sends `FromAudio::CapturedData`
//! 4. `YardAudinHandler::poll()` packages data into `Data PDU`
//! 5. Data PDU sent to server via DVC channel
//!
//! Protocol: MS-RDPEAI (Remote Desktop Protocol: Audio Input Redirection)

use std::any::Any;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use ironrdp::core::AsAny;
use ironrdp::dvc::{DvcClientProcessor, DvcMessage, DvcProcessor};
use ironrdp::pdu::PduResult;
use tracing::{debug, trace, warn};

use yard_audio::{AudioFormat, FromAudio, ToAudio};

use crate::audin::pdu::{
    AUDIN_VERSION, AudinDvcMessage, AudinPdu, ClientSoundFormats, DataIncoming, DataPdu, OpenReply,
    VersionPdu,
};

pub mod pdu;

/// Channel name for AUDIN Dynamic Virtual Channel.
pub const AUDIN_CHANNEL_NAME: &str = "AUDIO_INPUT";

/// Default supported audio formats for YARD microphone capture.
///
/// We support PCM audio in common configurations for voice.
const SUPPORTED_FORMATS: &[AudioFormat] = &[
    // PCM 48kHz mono 16-bit (preferred for voice - low bandwidth)
    AudioFormat {
        sample_rate: 48000,
        channels: 1,
        bits_per_sample: 16,
    },
    // PCM 48kHz stereo 16-bit (for higher quality)
    AudioFormat {
        sample_rate: 48000,
        channels: 2,
        bits_per_sample: 16,
    },
    // PCM 44.1kHz mono 16-bit (CD quality mono)
    AudioFormat {
        sample_rate: 44100,
        channels: 1,
        bits_per_sample: 16,
    },
];

/// AUDIN protocol state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AudinState {
    /// Waiting for Version PDU from server.
    Initial,
    /// Version exchanged, waiting for Sound Formats PDU.
    VersionExchanged,
    /// Formats negotiated, waiting for Open PDU.
    FormatsNegotiated,
    /// Actively capturing and sending audio data.
    Capturing,
    /// Channel closed.
    Closed,
}

/// AUDIN Dynamic Virtual Channel handler.
///
/// Implements MS-RDPEAI protocol for audio input redirection.
/// This handler receives commands from the server and coordinates
/// with the audio thread for microphone capture.
pub struct YardAudinHandler {
    /// Current protocol state.
    state: AudinState,
    /// Sender to control audio capture (sends ToAudio messages).
    audio_tx: Option<Sender<ToAudio>>,
    /// Receiver for captured audio data.
    capture_rx: Option<Receiver<FromAudio>>,
    /// Current audio format being used.
    current_format: Option<AudioFormat>,
    /// Frames per packet as specified by server.
    frames_per_packet: u32,
    /// Channel ID assigned by DVC.
    channel_id: Option<u32>,
}

impl YardAudinHandler {
    /// Creates a new AUDIN handler.
    ///
    /// # Arguments
    ///
    /// * `audio_tx` - Sender to control audio capture in audio thread (via ToAudio messages).
    /// * `capture_rx` - Receiver for captured audio data from audio thread.
    pub fn new(audio_tx: Option<Sender<ToAudio>>, capture_rx: Option<Receiver<FromAudio>>) -> Self {
        Self {
            state: AudinState::Initial,
            audio_tx,
            capture_rx,
            current_format: None,
            frames_per_packet: 0,
            channel_id: None,
        }
    }

    /// Polls for captured audio data and returns any pending DVC messages.
    ///
    /// This should be called regularly from the network loop to check for
    /// captured audio data that needs to be sent to the server.
    pub fn poll(&mut self) -> Vec<DvcMessage> {
        let mut messages: Vec<DvcMessage> = Vec::new();

        // Only poll for captured data if we're actively capturing
        if self.state != AudinState::Capturing {
            return messages;
        }

        let Some(_channel_id) = self.channel_id else {
            return messages;
        };

        let Some(ref capture_rx) = self.capture_rx else {
            return messages;
        };

        // Check for captured audio data
        loop {
            match capture_rx.try_recv() {
                Ok(FromAudio::CapturedData { data, format: _ }) => {
                    trace!("Received {} bytes of captured audio", data.len());

                    // Send Data Incoming PDU to notify server
                    let incoming = DataIncoming;
                    if let Ok(msg) = AudinDvcMessage::from_data_incoming(&incoming) {
                        messages.push(Box::new(msg));
                    }

                    // Send Data PDU with audio data
                    let data_pdu = DataPdu::new(data);
                    if let Ok(msg) = AudinDvcMessage::from_data(&data_pdu) {
                        messages.push(Box::new(msg));
                    }
                }
                Ok(FromAudio::CaptureError(err)) => {
                    warn!("Audio capture error: {}", err);
                    self.state = AudinState::FormatsNegotiated;
                }
                Ok(FromAudio::CaptureStopped) => {
                    debug!("Audio capture stopped");
                    self.state = AudinState::FormatsNegotiated;
                }
                Ok(FromAudio::BufferStats(_)) => {
                    // Buffer stats are for playback, not capture - ignore here
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    warn!("Audio capture channel disconnected");
                    self.state = AudinState::FormatsNegotiated;
                    break;
                }
            }
        }

        messages
    }

    /// Handles a Version PDU from the server.
    fn handle_version(&mut self, pdu: &VersionPdu, _channel_id: u32) -> PduResult<Vec<DvcMessage>> {
        debug!("Received AUDIN Version PDU: version {}", pdu.version);

        // Respond with our version
        let response = VersionPdu::new(AUDIN_VERSION);
        let msg = AudinDvcMessage::from_version(&response)?;

        self.state = AudinState::VersionExchanged;
        Ok(vec![Box::new(msg)])
    }

    /// Handles a Sound Formats PDU from the server.
    fn handle_sound_formats(
        &mut self,
        server_formats: &[AudioFormat],
        _channel_id: u32,
    ) -> PduResult<Vec<DvcMessage>> {
        debug!(
            "Received {} audio formats from server",
            server_formats.len()
        );

        // Find formats we support that the server also supports
        let mut client_formats = Vec::new();
        for supported in SUPPORTED_FORMATS {
            for server_format in server_formats {
                if supported.sample_rate == server_format.sample_rate
                    && supported.channels == server_format.channels
                    && supported.bits_per_sample == server_format.bits_per_sample
                {
                    client_formats.push(*supported);
                    break;
                }
            }
        }

        // If no common formats, use our preferred formats anyway
        // (server should handle format conversion)
        if client_formats.is_empty() {
            debug!("No common formats found, advertising our supported formats");
            client_formats.extend_from_slice(SUPPORTED_FORMATS);
        }

        debug!("Responding with {} client formats", client_formats.len());

        let response = ClientSoundFormats::new(&client_formats);
        let msg = AudinDvcMessage::from_sound_formats(&response)?;

        self.state = AudinState::FormatsNegotiated;
        Ok(vec![Box::new(msg)])
    }

    /// Handles an Open PDU from the server (start recording).
    fn handle_open(
        &mut self,
        format: &AudioFormat,
        frames_per_packet: u32,
        _channel_id: u32,
    ) -> PduResult<Vec<DvcMessage>> {
        debug!(
            "Server requested audio capture: {}Hz {}ch {}bit, {} frames/packet",
            format.sample_rate, format.channels, format.bits_per_sample, frames_per_packet
        );

        self.current_format = Some(*format);
        self.frames_per_packet = frames_per_packet;

        // Start audio capture in audio thread via ToAudio message
        if let Some(ref tx) = self.audio_tx {
            let msg = ToAudio::StartCapture {
                format: *format,
                frames_per_packet,
            };
            if let Err(e) = tx.send(msg) {
                warn!("Failed to start audio capture: {}", e);
            }
        }

        // Send Open Reply PDU
        let response = OpenReply::success();
        let msg = AudinDvcMessage::from_open_reply(&response)?;

        self.state = AudinState::Capturing;
        Ok(vec![Box::new(msg)])
    }

    /// Handles a Format Change PDU from the server.
    fn handle_format_change(
        &mut self,
        new_format_index: u32,
        _channel_id: u32,
    ) -> PduResult<Vec<DvcMessage>> {
        debug!(
            "Server requested format change to index {}",
            new_format_index
        );

        // For now, acknowledge the format change
        // The actual format will be determined by the next Open PDU
        let response = pdu::FormatChange::new(new_format_index);
        let msg = AudinDvcMessage::from_format_change(&response)?;

        Ok(vec![Box::new(msg)])
    }
}

impl DvcProcessor for YardAudinHandler {
    fn channel_name(&self) -> &str {
        AUDIN_CHANNEL_NAME
    }

    fn start(&mut self, channel_id: u32) -> PduResult<Vec<DvcMessage>> {
        debug!("AUDIN channel started with ID {}", channel_id);
        self.channel_id = Some(channel_id);
        self.state = AudinState::Initial;
        Ok(Vec::new())
    }

    fn process(&mut self, channel_id: u32, payload: &[u8]) -> PduResult<Vec<DvcMessage>> {
        if payload.is_empty() {
            return Ok(Vec::new());
        }

        // Parse the PDU
        let pdu = AudinPdu::decode(payload)?;

        match pdu {
            AudinPdu::Version(version_pdu) => self.handle_version(&version_pdu, channel_id),
            AudinPdu::SoundFormats(formats) => self.handle_sound_formats(&formats, channel_id),
            AudinPdu::Open {
                format,
                frames_per_packet,
            } => self.handle_open(&format, frames_per_packet, channel_id),
            AudinPdu::FormatChange { new_format } => {
                self.handle_format_change(new_format, channel_id)
            }
            AudinPdu::Data(_) => {
                // Server shouldn't send Data PDUs to client
                warn!("Received unexpected Data PDU from server");
                Ok(Vec::new())
            }
        }
    }

    fn close(&mut self, _channel_id: u32) {
        debug!("AUDIN channel closed");

        // Stop audio capture via ToAudio message
        if let Some(ref tx) = self.audio_tx {
            let _ = tx.send(ToAudio::StopCapture);
        }

        self.state = AudinState::Closed;
        self.channel_id = None;
    }
}

impl DvcClientProcessor for YardAudinHandler {}

impl AsAny for YardAudinHandler {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Creates an AUDIN client for the RDP connection.
///
/// If capture channels are provided, audio data will be captured from microphone.
/// If `None`, the AUDIN channel is not registered.
///
/// # Arguments
///
/// * `audio_tx` - Optional sender to control capture in audio thread (via ToAudio).
/// * `capture_rx` - Optional receiver for captured audio from audio thread.
///
/// # Returns
///
/// A `YardAudinHandler` instance ready to be attached to the DVC manager.
pub fn create_audin_client(
    audio_tx: Option<Sender<ToAudio>>,
    capture_rx: Option<Receiver<FromAudio>>,
) -> YardAudinHandler {
    debug!("Creating AUDIN client for microphone redirection");
    YardAudinHandler::new(audio_tx, capture_rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supported_formats_valid() {
        for format in SUPPORTED_FORMATS {
            assert!(format.sample_rate > 0);
            assert!(format.channels > 0);
            assert!(format.bits_per_sample > 0);
        }
    }

    #[test]
    fn test_handler_creation() {
        let handler = YardAudinHandler::new(None, None);
        assert_eq!(handler.state, AudinState::Initial);
        assert!(handler.channel_id.is_none());
    }

    #[test]
    fn test_handler_channel_name() {
        let handler = YardAudinHandler::new(None, None);
        assert_eq!(handler.channel_name(), AUDIN_CHANNEL_NAME);
    }

    #[test]
    fn test_handler_start() {
        let mut handler = YardAudinHandler::new(None, None);
        let result = handler.start(42);
        assert!(result.is_ok());
        assert_eq!(handler.channel_id, Some(42));
    }

    #[test]
    fn test_from_audio_captured_data() {
        let msg = FromAudio::CapturedData {
            data: vec![0u8; 1024],
            format: AudioFormat::default(),
        };
        if let FromAudio::CapturedData { data, format } = msg {
            assert_eq!(data.len(), 1024);
            assert_eq!(format.sample_rate, 48000);
        } else {
            panic!("Expected CapturedData variant");
        }
    }

    #[test]
    fn test_to_audio_start_capture() {
        let msg = ToAudio::StartCapture {
            format: AudioFormat::new(48000, 1, 16),
            frames_per_packet: 480,
        };
        if let ToAudio::StartCapture {
            format,
            frames_per_packet,
        } = msg
        {
            assert_eq!(format.sample_rate, 48000);
            assert_eq!(format.channels, 1);
            assert_eq!(frames_per_packet, 480);
        } else {
            panic!("Expected StartCapture variant");
        }
    }

    #[test]
    fn test_poll_empty_when_not_capturing() {
        let mut handler = YardAudinHandler::new(None, None);
        let messages = handler.poll();
        assert!(messages.is_empty());
    }

    #[test]
    fn test_handler_process_version_pdu() {
        let mut handler = YardAudinHandler::new(None, None);
        handler.start(1).unwrap();

        // Simulate receiving Version PDU from server
        let version_data = vec![0x01, 0x01, 0x00, 0x00, 0x00]; // MessageType::Version, version=1
        let result = handler.process(1, &version_data);
        assert!(result.is_ok());

        // Handler should respond with version PDU
        let messages = result.unwrap();
        assert_eq!(messages.len(), 1);

        // State should advance to VersionExchanged
        assert_eq!(handler.state, AudinState::VersionExchanged);
    }

    #[test]
    fn test_handler_close() {
        let mut handler = YardAudinHandler::new(None, None);
        handler.start(1).unwrap();

        handler.close(1);
        assert_eq!(handler.state, AudinState::Closed);
        assert!(handler.channel_id.is_none());
    }

    #[test]
    fn test_create_audin_client() {
        let handler = create_audin_client(None, None);
        assert_eq!(handler.channel_name(), AUDIN_CHANNEL_NAME);
    }

    #[test]
    fn test_to_audio_stop_capture() {
        let msg = ToAudio::StopCapture;
        assert!(matches!(msg, ToAudio::StopCapture));
    }

    #[test]
    fn test_from_audio_capture_error() {
        let msg = FromAudio::CaptureError("Permission denied".to_string());
        if let FromAudio::CaptureError(err) = msg {
            assert_eq!(err, "Permission denied");
        } else {
            panic!("Expected CaptureError variant");
        }
    }

    #[test]
    fn test_from_audio_capture_stopped() {
        let msg = FromAudio::CaptureStopped;
        assert!(matches!(msg, FromAudio::CaptureStopped));
    }

    #[test]
    fn test_handler_poll_not_capturing_with_channel() {
        let mut handler = YardAudinHandler::new(None, None);
        handler.start(1).unwrap();

        // State is Initial, not Capturing, so poll should return empty
        let messages = handler.poll();
        assert!(messages.is_empty());
    }
}
