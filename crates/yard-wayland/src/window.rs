//! Wayland window implementation for YARD.
//!
//! This module provides the WaylandWindow struct which handles:
//! - Window creation using xdg_shell
//! - Surface management and buffer handling
//! - Frame rendering via shared memory

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::Arc;

    use calloop::channel::{Channel, Sender};
    use calloop::{EventLoop, LoopHandle};
    use calloop_wayland_source::WaylandSource;
    use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
    use smithay_client_toolkit::output::{OutputHandler, OutputState};
    use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
    use smithay_client_toolkit::reexports::client::protocol::wl_output::WlOutput;
    use smithay_client_toolkit::reexports::client::protocol::wl_surface::WlSurface;
    use smithay_client_toolkit::reexports::client::{Connection, QueueHandle};
    use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
    use smithay_client_toolkit::shell::xdg::window::{
        Window, WindowConfigure, WindowDecorations, WindowHandler,
    };
    use smithay_client_toolkit::shell::xdg::XdgShell;
    use smithay_client_toolkit::shell::WaylandSurface;
    use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
    use smithay_client_toolkit::shm::{Shm, ShmHandler};
    use smithay_client_toolkit::{
        delegate_compositor, delegate_output, delegate_registry, delegate_shm, delegate_xdg_shell,
        delegate_xdg_window, registry_handlers,
    };

    /// Messages sent from the Wayland window to the main application.
    #[derive(Debug)]
    pub enum WindowEvent {
        /// Window was closed by the user.
        CloseRequested,
        /// Window was resized.
        Resized { width: u32, height: u32 },
        /// Window needs to be redrawn.
        RedrawRequested,
    }

    /// Configuration for creating a Wayland window.
    #[derive(Debug, Clone)]
    pub struct WindowConfig {
        /// Window title.
        pub title: String,
        /// Initial width in pixels.
        pub width: u32,
        /// Initial height in pixels.
        pub height: u32,
    }

    impl Default for WindowConfig {
        fn default() -> Self {
            Self {
                title: "YARD".to_string(),
                width: 1280,
                height: 720,
            }
        }
    }

    impl WindowConfig {
        /// Creates a window config with connection info in the title.
        pub fn with_connection_info(host: &str, port: u16, username: Option<&str>) -> Self {
            let title = match username {
                Some(user) => format!("YARD - {}:{} [{}]", host, port, user),
                None => format!("YARD - {}:{}", host, port),
            };
            Self {
                title,
                ..Default::default()
            }
        }
    }

    /// The main Wayland window state.
    pub struct WaylandWindow {
        registry_state: RegistryState,
        output_state: OutputState,
        shm: Shm,
        pool: SlotPool,
        window: Window,
        width: u32,
        height: u32,
        buffer: Option<Buffer>,
        event_tx: Sender<WindowEvent>,
        close_requested: bool,
        dirty: bool,
    }

    impl WaylandWindow {
        /// Creates a new Wayland window.
        ///
        /// Returns the window state, event loop, and a channel receiver for window events.
        pub fn new(
            config: WindowConfig,
        ) -> Result<
            (
                EventLoop<'static, Self>,
                Self,
                calloop::channel::Channel<WindowEvent>,
            ),
            Box<dyn std::error::Error>,
        > {
            // Connect to Wayland
            let conn = Connection::connect_to_env()?;
            let (globals, event_queue) = registry_queue_init(&conn)?;
            let qh = event_queue.handle();

            // Create event loop
            let event_loop: EventLoop<Self> = EventLoop::try_new()?;
            let loop_handle = event_loop.handle();

            // Insert Wayland source
            WaylandSource::new(conn.clone(), event_queue).insert(loop_handle.clone())?;

            // Create channel for window events
            let (event_tx, event_rx) = calloop::channel::channel();

            // Initialize states
            let compositor = CompositorState::bind(&globals, &qh)?;
            let xdg_shell = XdgShell::bind(&globals, &qh)?;
            let shm = Shm::bind(&globals, &qh)?;

            // Create surface and window
            let surface = compositor.create_surface(&qh);
            let window = xdg_shell.create_window(
                surface,
                WindowDecorations::ServerDefault,
                &qh,
            );

            window.set_title(config.title);
            window.set_app_id("yard");
            window.set_min_size(Some((320, 240)));
            window.commit();

            // Create shared memory pool for buffers
            let pool = SlotPool::new(
                (config.width * config.height * 4) as usize,
                &shm,
            )?;

            let state = Self {
                registry_state: RegistryState::new(&globals),
                output_state: OutputState::new(&globals, &qh),
                shm,
                pool,
                window,
                width: config.width,
                height: config.height,
                buffer: None,
                event_tx,
                close_requested: false,
                dirty: true,
            };

            Ok((event_loop, state, event_rx))
        }

        /// Returns true if the window close was requested.
        pub fn close_requested(&self) -> bool {
            self.close_requested
        }

        /// Returns the current window dimensions.
        pub fn dimensions(&self) -> (u32, u32) {
            (self.width, self.height)
        }

        /// Draws a solid color frame (placeholder until real frames arrive).
        ///
        /// Errors are logged but do not propagate - this allows graceful degradation
        /// if buffer allocation fails temporarily.
        pub fn draw_solid(&mut self, r: u8, g: u8, b: u8) {
            if !self.dirty {
                return;
            }

            let width = self.width;
            let height = self.height;
            let stride = width * 4;

            // Get or create buffer
            let (buffer, canvas) = match self.pool.create_buffer(
                width as i32,
                height as i32,
                stride as i32,
                smithay_client_toolkit::shm::wl_shm::Format::Argb8888,
            ) {
                Ok(result) => result,
                Err(e) => {
                    tracing::error!("Failed to create buffer for solid frame: {}", e);
                    return;
                }
            };

            // Fill with solid color (ARGB format)
            let color = [b, g, r, 255u8]; // BGRA order for ARGB8888
            for chunk in canvas.chunks_exact_mut(4) {
                chunk.copy_from_slice(&color);
            }

            // Attach and commit
            self.window
                .wl_surface()
                .attach(Some(buffer.wl_buffer()), 0, 0);
            self.window
                .wl_surface()
                .damage_buffer(0, 0, width as i32, height as i32);
            self.window.wl_surface().commit();

            self.buffer = Some(buffer);
            self.dirty = false;
        }

        /// Draws a frame from raw BGRA pixel data.
        pub fn draw_frame(&mut self, data: &[u8], width: u32, height: u32) {
            let stride = width * 4;
            let expected_len = (stride * height) as usize;

            if data.len() != expected_len {
                tracing::warn!(
                    "Frame data size mismatch: expected {}, got {}",
                    expected_len,
                    data.len()
                );
                return;
            }

            // Resize pool if needed
            if width != self.width || height != self.height {
                self.width = width;
                self.height = height;
                if let Err(e) = self.pool.resize(expected_len) {
                    tracing::error!("Failed to resize buffer pool: {}", e);
                    return;
                }
            }

            // Create buffer and copy data
            let (buffer, canvas) = match self.pool.create_buffer(
                width as i32,
                height as i32,
                stride as i32,
                smithay_client_toolkit::shm::wl_shm::Format::Argb8888,
            ) {
                Ok(result) => result,
                Err(e) => {
                    tracing::error!("Failed to create buffer for frame: {}", e);
                    return;
                }
            };

            canvas.copy_from_slice(data);

            // Attach and commit
            self.window
                .wl_surface()
                .attach(Some(buffer.wl_buffer()), 0, 0);
            self.window
                .wl_surface()
                .damage_buffer(0, 0, width as i32, height as i32);
            self.window.wl_surface().commit();

            self.buffer = Some(buffer);
        }
    }

    // Implement required traits for smithay-client-toolkit

    impl CompositorHandler for WaylandWindow {
        fn scale_factor_changed(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _surface: &WlSurface,
            _new_factor: i32,
        ) {
            // Handle scale factor changes if needed
        }

        fn transform_changed(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _surface: &WlSurface,
            _new_transform: smithay_client_toolkit::reexports::client::protocol::wl_output::Transform,
        ) {
            // Handle transform changes if needed
        }

        fn frame(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _surface: &WlSurface,
            _time: u32,
        ) {
            self.dirty = true;
            let _ = self.event_tx.send(WindowEvent::RedrawRequested);
        }

        fn surface_enter(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _surface: &WlSurface,
            _output: &WlOutput,
        ) {
        }

        fn surface_leave(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _surface: &WlSurface,
            _output: &WlOutput,
        ) {
        }
    }

    impl OutputHandler for WaylandWindow {
        fn output_state(&mut self) -> &mut OutputState {
            &mut self.output_state
        }

        fn new_output(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _output: WlOutput,
        ) {
        }

        fn update_output(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _output: WlOutput,
        ) {
        }

        fn output_destroyed(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _output: WlOutput,
        ) {
        }
    }

    impl WindowHandler for WaylandWindow {
        fn request_close(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _window: &Window) {
            self.close_requested = true;
            let _ = self.event_tx.send(WindowEvent::CloseRequested);
        }

        fn configure(
            &mut self,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
            _window: &Window,
            configure: WindowConfigure,
            _serial: u32,
        ) {
            let (new_width, new_height) = configure.new_size;

            // Use suggested size or keep current
            let width = new_width.map(|w| w.get()).unwrap_or(self.width);
            let height = new_height.map(|h| h.get()).unwrap_or(self.height);

            if width != self.width || height != self.height {
                self.width = width;
                self.height = height;
                self.dirty = true;
                let _ = self.event_tx.send(WindowEvent::Resized { width, height });
            }
        }
    }

    impl ShmHandler for WaylandWindow {
        fn shm_state(&mut self) -> &mut Shm {
            &mut self.shm
        }
    }

    impl ProvidesRegistryState for WaylandWindow {
        fn registry(&mut self) -> &mut RegistryState {
            &mut self.registry_state
        }

        registry_handlers![OutputState];
    }

    delegate_compositor!(WaylandWindow);
    delegate_output!(WaylandWindow);
    delegate_shm!(WaylandWindow);
    delegate_xdg_shell!(WaylandWindow);
    delegate_xdg_window!(WaylandWindow);
    delegate_registry!(WaylandWindow);
}

#[cfg(target_os = "linux")]
pub use linux::*;

// Stub implementation for non-Linux platforms (for compilation only)
#[cfg(not(target_os = "linux"))]
mod stub {
    /// Stub WindowEvent for non-Linux platforms.
    #[derive(Debug)]
    pub enum WindowEvent {
        CloseRequested,
        Resized { width: u32, height: u32 },
        RedrawRequested,
    }

    /// Stub WindowConfig for non-Linux platforms.
    #[derive(Debug, Clone)]
    pub struct WindowConfig {
        pub title: String,
        pub width: u32,
        pub height: u32,
    }

    impl Default for WindowConfig {
        fn default() -> Self {
            Self {
                title: "YARD".to_string(),
                width: 1280,
                height: 720,
            }
        }
    }

    impl WindowConfig {
        pub fn with_connection_info(host: &str, port: u16, username: Option<&str>) -> Self {
            let title = match username {
                Some(user) => format!("YARD - {}:{} [{}]", host, port, user),
                None => format!("YARD - {}:{}", host, port),
            };
            Self {
                title,
                width: 1280,
                height: 720,
            }
        }
    }

    /// Stub WaylandWindow for non-Linux platforms.
    pub struct WaylandWindow;

    impl WaylandWindow {
        pub fn new(
            _config: WindowConfig,
        ) -> Result<
            ((), Self, ()),
            Box<dyn std::error::Error>,
        > {
            Err("Wayland is only supported on Linux".into())
        }

        pub fn close_requested(&self) -> bool {
            false
        }

        pub fn dimensions(&self) -> (u32, u32) {
            (0, 0)
        }

        pub fn draw_solid(&mut self, _r: u8, _g: u8, _b: u8) {}

        pub fn draw_frame(&mut self, _data: &[u8], _width: u32, _height: u32) {}
    }
}

#[cfg(not(target_os = "linux"))]
pub use stub::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_window_config_default() {
        let config = WindowConfig::default();
        assert_eq!(config.title, "YARD");
        assert_eq!(config.width, 1280);
        assert_eq!(config.height, 720);
    }

    #[test]
    fn test_window_config_with_connection_info_no_username() {
        let config = WindowConfig::with_connection_info("server.example.com", 3389, None);
        assert_eq!(config.title, "YARD - server.example.com:3389");
        assert_eq!(config.width, 1280);
        assert_eq!(config.height, 720);
    }

    #[test]
    fn test_window_config_with_connection_info_with_username() {
        let config = WindowConfig::with_connection_info("server.example.com", 3389, Some("john"));
        assert_eq!(config.title, "YARD - server.example.com:3389 [john]");
    }

    #[test]
    fn test_window_config_with_custom_port() {
        let config = WindowConfig::with_connection_info("10.0.0.1", 13389, Some("admin"));
        assert_eq!(config.title, "YARD - 10.0.0.1:13389 [admin]");
    }

    #[test]
    fn test_window_config_with_unicode_username() {
        let config = WindowConfig::with_connection_info("server", 3389, Some("用户"));
        assert_eq!(config.title, "YARD - server:3389 [用户]");
    }
}
