//! Audio message types for inter-thread communication.
//!
//! Defines the `ToAudio` enum used to send commands from the main/network
//! threads to the dedicated PipeWire audio thread.

/// Audio sample format information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    /// Sample rate in Hz (e.g., 44100, 48000).
    pub sample_rate: u32,
    /// Number of audio channels (e.g., 1 for mono, 2 for stereo).
    pub channels: u8,
    /// Bits per sample (e.g., 16, 24, 32).
    pub bits_per_sample: u8,
}

impl AudioFormat {
    /// Creates a new audio format with the specified parameters.
    pub fn new(sample_rate: u32, channels: u8, bits_per_sample: u8) -> Self {
        Self {
            sample_rate,
            channels,
            bits_per_sample,
        }
    }

    /// Standard CD quality: 44.1kHz, stereo, 16-bit.
    pub fn cd_quality() -> Self {
        Self::new(44100, 2, 16)
    }

    /// Standard DVD quality: 48kHz, stereo, 16-bit.
    pub fn dvd_quality() -> Self {
        Self::new(48000, 2, 16)
    }
}

impl Default for AudioFormat {
    fn default() -> Self {
        Self::dvd_quality()
    }
}

/// Messages sent to the audio thread.
///
/// These messages are used to control the PipeWire audio thread from
/// the main thread or network thread.
#[derive(Debug, Clone)]
pub enum ToAudio {
    /// Gracefully shutdown the audio thread.
    ///
    /// The thread will stop the PipeWire main loop and exit cleanly.
    Shutdown,

    /// Play audio data through the output device.
    ///
    /// This variant is a stub for Story 4.2 (Implement Audio Output via RDPSND).
    /// The actual audio playback implementation will be added in that story.
    PlayAudio {
        /// Raw audio sample data.
        data: Vec<u8>,
        /// Format of the audio data.
        format: AudioFormat,
    },

    /// Set the output volume.
    ///
    /// Volume is specified as a value from 0.0 (muted) to 1.0 (full volume).
    SetVolume(f32),

    /// Start capturing audio from the microphone.
    ///
    /// The audio thread will create a PipeWire capture stream and begin
    /// sending captured audio data back via the FromAudio channel.
    StartCapture {
        /// Required audio format for capture.
        format: AudioFormat,
        /// Number of frames per packet to capture.
        frames_per_packet: u32,
    },

    /// Stop capturing audio from the microphone.
    ///
    /// The audio thread will close the PipeWire capture stream.
    StopCapture,

    /// Request current ring buffer statistics.
    ///
    /// The audio thread will respond with `FromAudio::BufferStats`.
    GetBufferStats,

    /// Clear the audio ring buffer.
    ///
    /// Useful when audio format changes or to recover from desync.
    ClearBuffer,
}

/// Messages sent from the audio thread back to caller.
///
/// These messages are used to send captured audio data from the audio
/// thread back to the network thread for transmission.
#[derive(Debug, Clone)]
pub enum FromAudio {
    /// Captured audio data ready to be sent to server.
    CapturedData {
        /// Raw audio sample data.
        data: Vec<u8>,
        /// Format of the captured audio.
        format: AudioFormat,
    },
    /// Error during capture (e.g., permission denied, no device).
    CaptureError(String),
    /// Capture stopped (device disconnected, stream closed).
    CaptureStopped,
    /// Ring buffer statistics response.
    BufferStats(crate::ring_buffer::RingBufferStats),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_format_new() {
        let format = AudioFormat::new(48000, 2, 16);
        assert_eq!(format.sample_rate, 48000);
        assert_eq!(format.channels, 2);
        assert_eq!(format.bits_per_sample, 16);
    }

    #[test]
    fn test_audio_format_cd_quality() {
        let format = AudioFormat::cd_quality();
        assert_eq!(format.sample_rate, 44100);
        assert_eq!(format.channels, 2);
        assert_eq!(format.bits_per_sample, 16);
    }

    #[test]
    fn test_audio_format_dvd_quality() {
        let format = AudioFormat::dvd_quality();
        assert_eq!(format.sample_rate, 48000);
        assert_eq!(format.channels, 2);
        assert_eq!(format.bits_per_sample, 16);
    }

    #[test]
    fn test_audio_format_default() {
        let format = AudioFormat::default();
        assert_eq!(format, AudioFormat::dvd_quality());
    }

    #[test]
    fn test_to_audio_shutdown() {
        let msg = ToAudio::Shutdown;
        assert!(matches!(msg, ToAudio::Shutdown));
    }

    #[test]
    fn test_to_audio_play_audio() {
        let msg = ToAudio::PlayAudio {
            data: vec![0u8; 1024],
            format: AudioFormat::default(),
        };
        if let ToAudio::PlayAudio { data, format } = msg {
            assert_eq!(data.len(), 1024);
            assert_eq!(format.sample_rate, 48000);
        } else {
            panic!("Expected PlayAudio variant");
        }
    }

    #[test]
    fn test_to_audio_set_volume() {
        let msg = ToAudio::SetVolume(0.5);
        if let ToAudio::SetVolume(vol) = msg {
            assert!((vol - 0.5).abs() < f32::EPSILON);
        } else {
            panic!("Expected SetVolume variant");
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
    fn test_to_audio_stop_capture() {
        let msg = ToAudio::StopCapture;
        assert!(matches!(msg, ToAudio::StopCapture));
    }

    #[test]
    fn test_from_audio_captured_data() {
        let msg = FromAudio::CapturedData {
            data: vec![0u8; 1024],
            format: AudioFormat::new(48000, 1, 16),
        };
        if let FromAudio::CapturedData { data, format } = msg {
            assert_eq!(data.len(), 1024);
            assert_eq!(format.sample_rate, 48000);
            assert_eq!(format.channels, 1);
        } else {
            panic!("Expected CapturedData variant");
        }
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
    fn test_to_audio_get_buffer_stats() {
        let msg = ToAudio::GetBufferStats;
        assert!(matches!(msg, ToAudio::GetBufferStats));
    }

    #[test]
    fn test_to_audio_clear_buffer() {
        let msg = ToAudio::ClearBuffer;
        assert!(matches!(msg, ToAudio::ClearBuffer));
    }

    #[test]
    fn test_from_audio_buffer_stats() {
        use crate::ring_buffer::RingBufferStats;
        let stats = RingBufferStats::default();
        let msg = FromAudio::BufferStats(stats);
        assert!(matches!(msg, FromAudio::BufferStats(_)));
    }
}
