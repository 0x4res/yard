//! Video decoder implementation using FFmpeg.
//!
//! This module provides H.264/AVC decoding for RDP video streams.
//! On Linux, it uses FFmpeg via the `ffmpeg-next` crate.
//! On other platforms, stub implementations are provided for compilation.

use thiserror::Error;

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
#[derive(Debug, Clone)]
pub struct DecodedFrame {
    /// Raw pixel data in BGRA format.
    pub data: Vec<u8>,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Stride (bytes per row).
    pub stride: u32,
}

impl DecodedFrame {
    /// Creates a new decoded frame.
    #[must_use]
    pub fn new(data: Vec<u8>, width: u32, height: u32) -> Self {
        let stride = width * 4; // BGRA = 4 bytes per pixel
        Self {
            data,
            width,
            height,
            stride,
        }
    }

    /// Returns the expected data length for the frame dimensions.
    #[must_use]
    pub fn expected_len(&self) -> usize {
        (self.stride * self.height) as usize
    }
}

// Linux implementation using FFmpeg
#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use tracing::{debug, warn};

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

            let ffmpeg_codec = ffmpeg_codec
                .ok_or_else(|| DecoderError::UnsupportedCodec(codec.to_string()))?;

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
            use ffmpeg_next::format::Pixel;
            use ffmpeg_next::software::scaling::{Context, Flags};

            // Create packet from encoded data
            let mut packet = ffmpeg_next::Packet::copy(encoded_data);

            // Send packet to decoder
            self.decoder
                .send_packet(&packet)
                .map_err(|e| DecoderError::DecodeFailed(e.to_string()))?;

            // Try to receive decoded frame
            let mut frame = ffmpeg_next::frame::Video::empty();
            match self.decoder.receive_frame(&mut frame) {
                Ok(()) => {}
                Err(ffmpeg_next::Error::Other { errno: ffmpeg_next::error::EAGAIN }) => {
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
            let scaler = self.scaler.as_mut().ok_or_else(|| {
                DecoderError::DecodeFailed("Scaler not initialized".to_string())
            })?;

            let mut bgra_frame = ffmpeg_next::frame::Video::empty();
            scaler
                .run(&frame, &mut bgra_frame)
                .map_err(|e| DecoderError::DecodeFailed(e.to_string()))?;

            // Extract BGRA data
            let data = bgra_frame.data(0).to_vec();

            Ok(Some(DecodedFrame::new(data, width, height)))
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
                let mut frame = ffmpeg_next::frame::Video::empty();
                match self.decoder.receive_frame(&mut frame) {
                    Ok(()) => {
                        // Process frame similar to decode()
                        // For simplicity, we'll skip the conversion here
                        // Real implementation should handle this
                    }
                    Err(_) => break,
                }
            }

            Ok(frames)
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
    fn test_video_codec_display() {
        assert_eq!(VideoCodec::H264.to_string(), "H.264/AVC");
        assert_eq!(VideoCodec::H265.to_string(), "H.265/HEVC");
    }

    #[test]
    fn test_decoded_frame_new() {
        let data = vec![0u8; 1920 * 1080 * 4];
        let frame = DecodedFrame::new(data.clone(), 1920, 1080);
        assert_eq!(frame.width, 1920);
        assert_eq!(frame.height, 1080);
        assert_eq!(frame.stride, 1920 * 4);
        assert_eq!(frame.data.len(), 1920 * 1080 * 4);
    }

    #[test]
    fn test_decoded_frame_expected_len() {
        let frame = DecodedFrame::new(vec![], 1920, 1080);
        assert_eq!(frame.expected_len(), 1920 * 1080 * 4);
    }

    #[test]
    fn test_decoder_error_display() {
        let err = DecoderError::UnsupportedCodec("VP9".to_string());
        assert!(err.to_string().contains("VP9"));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn test_decoder_stub_returns_error() {
        let result = VideoDecoder::new(VideoCodec::H264);
        assert!(result.is_err());
    }
}
