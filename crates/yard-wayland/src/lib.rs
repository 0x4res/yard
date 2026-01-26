//! YARD Wayland - Wayland surfaces, input, and multi-monitor support.
//!
//! This crate provides the Wayland integration layer using smithay-client-toolkit.
//! It handles window creation, input events, and multi-monitor management.
//!
//! **Note:** This crate requires Linux with Wayland support.

#![cfg_attr(not(target_os = "linux"), allow(unused_imports, dead_code))]

pub mod input;
pub mod window;

pub use input::wayland_to_rdp_scancode;
pub use window::{KeyboardShortcut, WaylandWindow, WindowConfig, WindowEvent};
pub use yard_core::prelude::*;
