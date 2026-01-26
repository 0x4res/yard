//! Video decoder implementation using FFmpeg.
//!
//! This module provides H.264/AVC decoding for RDP video streams.
//! On Linux, it uses FFmpeg via the `ffmpeg-next` crate.
//! On other platforms, stub implementations are provided for compilation.

use thiserror::Error;

/// Bytes per pixel for BGRA format.
const BYTES_PER_PIXEL_BGRA: u32 = 4;

/// Errors that can occur during video decoding.
#[derive(Error, Debug)]
pub enum DecoderError {
    /// FFmpeg initialization failed.
    #[error("Failed to initialize FFmpeg: {0}")]
    InitFailed(String),

    /// Unsupported codec requested.
    #[error("Unsupported codec: {0}")]
    UnsupportedCodec(String),

    /// Decoding failed.
    #[error("Decode error: {0}")]
    DecodeFailed(String),

    /// Invalid frame data.
    #[error("Invalid frame data: {0}")]
    InvalidData(String),
}

/// Result type for decoder operations.
pub type Result<T> = std::result::Result<T, DecoderError>;

/// Supported video codecs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    /// H.264/AVC - most common for RDP.
    H264,
    /// H.265/HEVC - less common.
    H265,
}

impl std::fmt::Display for VideoCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VideoCodec::H264 => write!(f, "H.264/AVC"),
            VideoCodec::H265 => write!(f, "H.265/HEVC"),
        }
    }
}

/// A decoded video frame with raw pixel data.
///
/// Represents either a full frame or a partial update region.
/// For partial updates, `x` and `y` indicate where on the desktop
/// this region should be placed.
#[derive(Debug, Clone)]
pub struct DecodedFrame {
    /// Raw pixel data in BGRA format.
    pub data: Vec<u8>,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// X position on desktop (for partial updates).
    pub x: u32,
    /// Y position on desktop (for partial updates).
    pub y: u32,
    /// Stride (bytes per row).
    pub stride: u32,
    /// Optional timestamp in microseconds for frame pacing.
    pub timestamp: Option<u64>,
}

impl DecodedFrame {
    /// Creates a new decoded frame at position (0, 0).
    ///
    /// Use this for full-screen frames or when the position is not relevant.
    #[must_use]
    pub fn new(data: Vec<u8>, width: u32, height: u32) -> Self {
        Self::with_position(data, width, height, 0, 0)
    }

    /// Creates a new decoded frame with a specific position.
    ///
    /// Use this for partial screen updates where the frame represents
    /// a region that should be placed at (x, y) on the desktop.
    #[must_use]
    pub fn with_position(data: Vec<u8>, width: u32, height: u32, x: u32, y: u32) -> Self {
        let stride = width * BYTES_PER_PIXEL_BGRA;
        Self {
            data,
            width,
            height,
            x,
            y,
            stride,
            timestamp: None,
        }
    }

    /// Creates a new decoded frame with a timestamp.
    #[must_use]
    pub fn with_timestamp(mut self, timestamp: u64) -> Self {
        self.timestamp = Some(timestamp);
        self
    }

    /// Returns the expected data length for the frame dimensions.
    #[must_use]
    pub fn expected_len(&self) -> usize {
        (self.stride * self.height) as usize
    }

    /// Returns true if the data buffer matches the expected dimensions.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.data.len() == self.expected_len()
    }
}

// Linux implementation using FFmpeg
#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use tracing::debug;

    /// Video decoder using FFmpeg.
    pub struct VideoDecoder {
        decoder: ffmpeg_next::decoder::Video,
        scaler: Option<ffmpeg_next::software::scaling::Context>,
        codec: VideoCodec,
        width: u32,
        height: u32,
    }

    impl VideoDecoder {
        /// Creates a new video decoder for the specified codec.
        ///
        /// # Errors
        ///
        /// Returns an error if FFmpeg initialization fails or the codec is unsupported.
        pub fn new(codec: VideoCodec) -> Result<Self> {
            ffmpeg_next::init().map_err(|e| DecoderError::InitFailed(e.to_string()))?;

            let ffmpeg_codec = match codec {
                VideoCodec::H264 => ffmpeg_next::decoder::find(ffmpeg_next::codec::Id::H264),
                VideoCodec::H265 => ffmpeg_next::decoder::find(ffmpeg_next::codec::Id::HEVC),
            };

            let ffmpeg_codec =
                ffmpeg_codec.ok_or_else(|| DecoderError::UnsupportedCodec(codec.to_string()))?;

            let context = ffmpeg_next::codec::Context::new_with_codec(ffmpeg_codec);
            let decoder = context
                .decoder()
                .video()
                .map_err(|e| DecoderError::InitFailed(e.to_string()))?;

            debug!(?codec, "Video decoder initialized");

            Ok(Self {
                decoder,
                scaler: None,
                codec,
                width: 0,
                height: 0,
            })
        }

        /// Returns the configured codec.
        #[must_use]
        pub fn codec(&self) -> VideoCodec {
            self.codec
        }

        /// Decodes an encoded frame and returns the raw BGRA pixel data.
        ///
        /// # Errors
        ///
        /// Returns an error if decoding fails.
        pub fn decode(&mut self, encoded_data: &[u8]) -> Result<Option<DecodedFrame>> {
            // Create packet from encoded data
            let packet = ffmpeg_next::Packet::copy(encoded_data);

            // Send packet to decoder
            self.decoder
                .send_packet(&packet)
                .map_err(|e| DecoderError::DecodeFailed(e.to_string()))?;

            // Try to receive and convert frame
            self.receive_and_convert_frame()
        }

        /// Flushes the decoder, returning any remaining frames.
        ///
        /// # Errors
        ///
        /// Returns an error if flushing fails.
        pub fn flush(&mut self) -> Result<Vec<DecodedFrame>> {
            self.decoder
                .send_eof()
                .map_err(|e| DecoderError::DecodeFailed(e.to_string()))?;

            let mut frames = Vec::new();
            loop {
                match self.receive_and_convert_frame()? {
                    Some(frame) => frames.push(frame),
                    None => break,
                }
            }

            Ok(frames)
        }

        /// Receives a frame from the decoder and converts it to BGRA format.
        fn receive_and_convert_frame(&mut self) -> Result<Option<DecodedFrame>> {
            use ffmpeg_next::format::Pixel;
            use ffmpeg_next::software::scaling::{Context, Flags};

            // Try to receive decoded frame
            let mut frame = ffmpeg_next::frame::Video::empty();
            match self.decoder.receive_frame(&mut frame) {
                Ok(()) => {}
                Err(ffmpeg_next::Error::Other {
                    errno: ffmpeg_next::error::EAGAIN,
                }) => {
                    // Need more data
                    return Ok(None);
                }
                Err(e) => {
                    return Err(DecoderError::DecodeFailed(e.to_string()));
                }
            }

            let width = frame.width();
            let height = frame.height();

            // Update scaler if dimensions changed
            if self.width != width || self.height != height {
                self.width = width;
                self.height = height;

                self.scaler = Some(
                    Context::get(
                        frame.format(),
                        width,
                        height,
                        Pixel::BGRA,
                        width,
                        height,
                        Flags::BILINEAR,
                    )
                    .map_err(|e| DecoderError::DecodeFailed(e.to_string()))?,
                );

                debug!(width, height, "Video dimensions updated");
            }

            // Convert to BGRA
            let scaler = self
                .scaler
                .as_mut()
                .ok_or_else(|| DecoderError::DecodeFailed("Scaler not initialized".to_string()))?;

            let mut bgra_frame = ffmpeg_next::frame::Video::empty();
            scaler
                .run(&frame, &mut bgra_frame)
                .map_err(|e| DecoderError::DecodeFailed(e.to_string()))?;

            // Extract BGRA data
            let data = bgra_frame.data(0).to_vec();

            Ok(Some(DecodedFrame::new(data, width, height)))
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::VideoDecoder;

// Stub implementation for non-Linux platforms (for compilation only)
#[cfg(not(target_os = "linux"))]
mod stub {
    use super::*;

    /// Stub video decoder for non-Linux platforms.
    pub struct VideoDecoder {
        codec: VideoCodec,
    }

    impl VideoDecoder {
        /// Creates a new video decoder (stub).
        ///
        /// # Errors
        ///
        /// Always returns an error on non-Linux platforms.
        pub fn new(_codec: VideoCodec) -> Result<Self> {
            Err(DecoderError::InitFailed(
                "Video decoding requires Linux with FFmpeg".to_string(),
            ))
        }

        /// Returns the configured codec.
        #[must_use]
        pub fn codec(&self) -> VideoCodec {
            self.codec
        }

        /// Decodes an encoded frame (stub).
        ///
        /// # Errors
        ///
        /// Always returns an error on non-Linux platforms.
        pub fn decode(&mut self, _encoded_data: &[u8]) -> Result<Option<DecodedFrame>> {
            Err(DecoderError::InitFailed(
                "Video decoding requires Linux with FFmpeg".to_string(),
            ))
        }

        /// Flushes the decoder (stub).
        ///
        /// # Errors
        ///
        /// Always returns an error on non-Linux platforms.
        pub fn flush(&mut self) -> Result<Vec<DecodedFrame>> {
            Err(DecoderError::InitFailed(
                "Video decoding requires Linux with FFmpeg".to_string(),
            ))
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub use stub::VideoDecoder;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_video_codec_display_h264() {
        assert_eq!(VideoCodec::H264.to_string(), "H.264/AVC");
    }

    #[test]
    fn test_video_codec_display_h265() {
        assert_eq!(VideoCodec::H265.to_string(), "H.265/HEVC");
    }

    #[test]
    fn test_decoded_frame_new() {
        let data = vec![0u8; 1920 * 1080 * 4];
        let frame = DecodedFrame::new(data.clone(), 1920, 1080);
        assert_eq!(frame.width, 1920);
        assert_eq!(frame.height, 1080);
        assert_eq!(frame.x, 0);
        assert_eq!(frame.y, 0);
        assert_eq!(frame.stride, 1920 * 4);
        assert_eq!(frame.data.len(), 1920 * 1080 * 4);
        assert!(frame.timestamp.is_none());
    }

    #[test]
    fn test_decoded_frame_with_position() {
        let data = vec![0u8; 100 * 100 * 4];
        let frame = DecodedFrame::with_position(data, 100, 100, 50, 75);
        assert_eq!(frame.width, 100);
        assert_eq!(frame.height, 100);
        assert_eq!(frame.x, 50);
        assert_eq!(frame.y, 75);
    }

    #[test]
    fn test_decoded_frame_with_timestamp() {
        let data = vec![0u8; 100 * 100 * 4];
        let frame = DecodedFrame::new(data, 100, 100).with_timestamp(12345);
        assert_eq!(frame.timestamp, Some(12345));
    }

    #[test]
    fn test_decoded_frame_expected_len() {
        let frame = DecodedFrame::new(vec![], 1920, 1080);
        assert_eq!(frame.expected_len(), 1920 * 1080 * 4);
    }

    #[test]
    fn test_decoded_frame_is_valid() {
        let valid_data = vec![0u8; 100 * 100 * 4];
        let valid_frame = DecodedFrame::new(valid_data, 100, 100);
        assert!(valid_frame.is_valid());

        let invalid_frame = DecodedFrame::new(vec![0u8; 100], 100, 100);
        assert!(!invalid_frame.is_valid());
    }

    #[test]
    fn test_decoder_error_display() {
        let err = DecoderError::UnsupportedCodec("VP9".to_string());
        assert!(err.to_string().contains("VP9"));
    }

    #[test]
    fn test_decoder_error_variants() {
        let init_err = DecoderError::InitFailed("test".to_string());
        assert!(init_err.to_string().contains("initialize"));

        let decode_err = DecoderError::DecodeFailed("test".to_string());
        assert!(decode_err.to_string().contains("Decode"));

        let invalid_err = DecoderError::InvalidData("test".to_string());
        assert!(invalid_err.to_string().contains("Invalid"));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn test_decoder_stub_h264_returns_error() {
        let result = VideoDecoder::new(VideoCodec::H264);
        assert!(result.is_err());
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn test_decoder_stub_h265_returns_error() {
        let result = VideoDecoder::new(VideoCodec::H265);
        assert!(result.is_err());
    }
}
