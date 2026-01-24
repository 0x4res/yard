//! YARD Video - Video decode using FFmpeg.
//!
//! This crate provides the video decoding pipeline using FFmpeg.
//! It handles H.264/AVC and H.265/HEVC codec decoding for RDP graphics.
//!
//! **Note:** This crate requires FFmpeg libraries installed on the system.

#![cfg_attr(not(target_os = "linux"), allow(unused_imports))]

pub use yard_core::prelude::*;
