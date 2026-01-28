//! RDPSND audio channel handler for RDP audio output.
//!
//! This module implements the RDPSND static virtual channel for receiving
//! audio data from the RDP server and routing it to the PipeWire audio thread.
//!
//! Audio data flow:
//! 1. Server sends audio in `SndWavePdu` / `Wave2Pdu`
//! 2. `YardRdpsndHandler::wave()` receives the data
//! 3. Data is sent to audio thread via `ToAudio::PlayAudio`
//! 4. Audio thread plays through PipeWire (implemented in yard-audio)

use std::borrow::Cow;
use std::sync::mpsc::Sender;

use ironrdp::rdpsnd::client::{NoopRdpsndBackend, Rdpsnd, RdpsndClientHandler};
use ironrdp::rdpsnd::pdu::{
    AudioFormat as RdpAudioFormat, AudioFormatFlags, PitchPdu, VolumePdu, WaveFormat,
};
use tracing::{debug, trace, warn};

use yard_audio::{AudioFormat, ToAudio};

/// Default supported audio formats for YARD.
///
/// We support PCM audio in common configurations for low-latency playback.
/// Prefer 48kHz stereo 16-bit as it's widely supported and optimal for voice.
const SUPPORTED_FORMATS: &[RdpAudioFormat] = &[
    // PCM 48kHz stereo 16-bit (preferred - DVD quality)
    RdpAudioFormat {
        format: WaveFormat::PCM,
        n_channels: 2,
        n_samples_per_sec: 48000,
        n_avg_bytes_per_sec: 192000, // 48000 * 2 * 2
        n_block_align: 4,            // 2 channels * 2 bytes
        bits_per_sample: 16,
        data: None,
    },
    // PCM 44.1kHz stereo 16-bit (CD quality)
    RdpAudioFormat {
        format: WaveFormat::PCM,
        n_channels: 2,
        n_samples_per_sec: 44100,
        n_avg_bytes_per_sec: 176400, // 44100 * 2 * 2
        n_block_align: 4,
        bits_per_sample: 16,
        data: None,
    },
    // PCM 48kHz mono 16-bit (for voice)
    RdpAudioFormat {
        format: WaveFormat::PCM,
        n_channels: 1,
        n_samples_per_sec: 48000,
        n_avg_bytes_per_sec: 96000, // 48000 * 1 * 2
        n_block_align: 2,
        bits_per_sample: 16,
        data: None,
    },
];

/// RDPSND client handler that routes audio to the PipeWire thread.
///
/// This handler receives audio data from the RDP server and sends it
/// to the audio thread via the `ToAudio` channel.
#[derive(Debug)]
pub struct YardRdpsndHandler {
    /// Sender to the audio thread.
    audio_tx: Sender<ToAudio>,
    /// Current audio format being used.
    current_format: Option<AudioFormat>,
}

impl YardRdpsndHandler {
    /// Creates a new RDPSND handler with the given audio sender.
    pub fn new(audio_tx: Sender<ToAudio>) -> Self {
        Self {
            audio_tx,
            current_format: None,
        }
    }

    /// Converts an RDP audio format to our internal AudioFormat.
    fn convert_format(rdp_format: &RdpAudioFormat) -> AudioFormat {
        AudioFormat::new(
            rdp_format.n_samples_per_sec,
            rdp_format.n_channels as u8,
            rdp_format.bits_per_sample as u8,
        )
    }
}

impl RdpsndClientHandler for YardRdpsndHandler {
    fn get_flags(&self) -> AudioFormatFlags {
        // No special flags needed
        AudioFormatFlags::empty()
    }

    fn get_formats(&self) -> &[RdpAudioFormat] {
        SUPPORTED_FORMATS
    }

    fn wave(&mut self, format_no: usize, _ts: u32, data: Cow<'_, [u8]>) {
        // Get the format for this wave data
        // format_no indexes into SUPPORTED_FORMATS (the formats we advertised to the server)
        let format = if format_no < SUPPORTED_FORMATS.len() {
            Self::convert_format(&SUPPORTED_FORMATS[format_no])
        } else {
            warn!(
                "Invalid format index {} (have {} supported formats), using default",
                format_no,
                SUPPORTED_FORMATS.len()
            );
            AudioFormat::default()
        };

        // Check for format change
        if self.current_format.as_ref() != Some(&format) {
            debug!(
                "Audio format changed to {}Hz {}ch {}bit",
                format.sample_rate, format.channels, format.bits_per_sample
            );
            self.current_format = Some(format);
        }

        let current_format = self.current_format.unwrap_or_default();

        trace!(
            "Received {} bytes of audio data (format {})",
            data.len(),
            format_no
        );

        // Send audio data to the audio thread
        if let Err(e) = self.audio_tx.send(ToAudio::PlayAudio {
            data: data.into_owned(),
            format: current_format,
        }) {
            warn!("Failed to send audio to audio thread: {e}");
        }
    }

    fn set_volume(&mut self, volume: VolumePdu) {
        // Convert RDP volume (0-65535) to 0.0-1.0 range
        // VolumePdu has left and right channel volumes
        let left = f32::from(volume.volume_left) / 65535.0;
        let right = f32::from(volume.volume_right) / 65535.0;
        let avg_volume = (left + right) / 2.0;

        debug!("Volume set to {:.1}%", avg_volume * 100.0);

        if let Err(e) = self.audio_tx.send(ToAudio::SetVolume(avg_volume)) {
            warn!("Failed to send volume to audio thread: {e}");
        }
    }

    fn set_pitch(&mut self, pitch: PitchPdu) {
        // Pitch adjustment is not commonly used, log for now
        debug!("Pitch adjustment received: {:?}", pitch);
    }

    fn close(&mut self) {
        debug!("RDPSND channel closed");
        // Don't send Shutdown here - the audio thread lifecycle is managed elsewhere
    }
}

/// Creates an RDPSND client for the RDP connection.
///
/// If `audio_tx` is provided, audio data will be routed to the audio thread.
/// If `None`, a no-op backend is used (audio is discarded).
///
/// # Arguments
///
/// * `audio_tx` - Optional sender to the audio thread.
///
/// # Returns
///
/// An `Rdpsnd` instance ready to be attached to the RDP connector.
pub fn create_rdpsnd_client(audio_tx: Option<Sender<ToAudio>>) -> Rdpsnd {
    match audio_tx {
        Some(tx) => {
            debug!("Creating RDPSND client with audio routing");
            Rdpsnd::new(Box::new(YardRdpsndHandler::new(tx)))
        }
        None => {
            debug!("Creating RDPSND client with no-op backend (audio disabled)");
            Rdpsnd::new(Box::new(NoopRdpsndBackend))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn test_supported_formats_valid() {
        // Verify our supported formats are correctly configured
        for format in SUPPORTED_FORMATS {
            assert_eq!(format.format, WaveFormat::PCM);
            assert!(format.n_channels > 0);
            assert!(format.n_samples_per_sec > 0);
            assert!(format.bits_per_sample > 0);

            // Verify calculated values
            let expected_bytes_per_sec = format.n_samples_per_sec
                * u32::from(format.n_channels)
                * u32::from(format.bits_per_sample / 8);
            assert_eq!(format.n_avg_bytes_per_sec, expected_bytes_per_sec);

            let expected_block_align = format.n_channels * (format.bits_per_sample / 8);
            assert_eq!(format.n_block_align, expected_block_align);
        }
    }

    #[test]
    fn test_convert_format() {
        let rdp_format = RdpAudioFormat {
            format: WaveFormat::PCM,
            n_channels: 2,
            n_samples_per_sec: 48000,
            n_avg_bytes_per_sec: 192000,
            n_block_align: 4,
            bits_per_sample: 16,
            data: None,
        };

        let audio_format = YardRdpsndHandler::convert_format(&rdp_format);
        assert_eq!(audio_format.sample_rate, 48000);
        assert_eq!(audio_format.channels, 2);
        assert_eq!(audio_format.bits_per_sample, 16);
    }

    #[test]
    fn test_handler_creation() {
        let (tx, _rx) = mpsc::channel();
        let handler = YardRdpsndHandler::new(tx);
        assert!(handler.current_format.is_none());
    }

    #[test]
    fn test_handler_get_formats() {
        let (tx, _rx) = mpsc::channel();
        let handler = YardRdpsndHandler::new(tx);
        let formats = handler.get_formats();
        assert!(!formats.is_empty());
        assert!(formats.iter().all(|f| f.format == WaveFormat::PCM));
    }

    #[test]
    fn test_handler_get_flags() {
        let (tx, _rx) = mpsc::channel();
        let handler = YardRdpsndHandler::new(tx);
        let flags = handler.get_flags();
        assert!(flags.is_empty());
    }

    #[test]
    fn test_create_rdpsnd_with_audio() {
        let (tx, _rx) = mpsc::channel();
        let _rdpsnd = create_rdpsnd_client(Some(tx));
        // Just verify it creates without panic
    }

    #[test]
    fn test_create_rdpsnd_without_audio() {
        let _rdpsnd = create_rdpsnd_client(None);
        // Just verify it creates without panic
    }

    #[test]
    fn test_wave_sends_to_channel() {
        let (tx, rx) = mpsc::channel();
        let mut handler = YardRdpsndHandler::new(tx);

        // Simulate receiving wave data with format_no=0 (first SUPPORTED_FORMAT: 48kHz stereo)
        let data = vec![0u8; 1024];
        handler.wave(0, 0, Cow::Borrowed(&data));

        // Verify message was sent with correct data and format
        let msg = rx.try_recv().expect("Expected message");
        match msg {
            ToAudio::PlayAudio { data: d, format } => {
                assert_eq!(d.len(), 1024);
                // format_no=0 should be PCM 48kHz stereo 16-bit
                assert_eq!(format.sample_rate, 48000);
                assert_eq!(format.channels, 2);
                assert_eq!(format.bits_per_sample, 16);
            }
            _ => panic!("Expected PlayAudio message"),
        }
    }

    #[test]
    fn test_wave_with_different_format() {
        let (tx, rx) = mpsc::channel();
        let mut handler = YardRdpsndHandler::new(tx);

        // Simulate receiving wave data with format_no=1 (second SUPPORTED_FORMAT: 44.1kHz stereo)
        let data = vec![0u8; 512];
        handler.wave(1, 0, Cow::Borrowed(&data));

        let msg = rx.try_recv().expect("Expected message");
        match msg {
            ToAudio::PlayAudio { data: d, format } => {
                assert_eq!(d.len(), 512);
                // format_no=1 should be PCM 44.1kHz stereo 16-bit
                assert_eq!(format.sample_rate, 44100);
                assert_eq!(format.channels, 2);
                assert_eq!(format.bits_per_sample, 16);
            }
            _ => panic!("Expected PlayAudio message"),
        }
    }

    #[test]
    fn test_wave_with_invalid_format_uses_default() {
        let (tx, rx) = mpsc::channel();
        let mut handler = YardRdpsndHandler::new(tx);

        // Simulate receiving wave data with invalid format_no
        let data = vec![0u8; 256];
        handler.wave(99, 0, Cow::Borrowed(&data));

        let msg = rx.try_recv().expect("Expected message");
        match msg {
            ToAudio::PlayAudio { data: d, format } => {
                assert_eq!(d.len(), 256);
                // Invalid format should fall back to default (DVD quality: 48kHz stereo)
                let default_format = AudioFormat::default();
                assert_eq!(format.sample_rate, default_format.sample_rate);
                assert_eq!(format.channels, default_format.channels);
                assert_eq!(format.bits_per_sample, default_format.bits_per_sample);
            }
            _ => panic!("Expected PlayAudio message"),
        }
    }

    #[test]
    fn test_set_volume_sends_to_channel() {
        let (tx, rx) = mpsc::channel();
        let mut handler = YardRdpsndHandler::new(tx);

        // Simulate volume change (half volume)
        let volume = VolumePdu {
            volume_left: 32767,
            volume_right: 32767,
        };
        handler.set_volume(volume);

        // Verify message was sent
        let msg = rx.try_recv().expect("Expected message");
        match msg {
            ToAudio::SetVolume(vol) => {
                assert!((vol - 0.5).abs() < 0.01);
            }
            _ => panic!("Expected SetVolume message"),
        }
    }
}
