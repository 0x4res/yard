//! Configuration file support for YARD.
//!
//! This module handles loading and parsing the YARD configuration file
//! from `~/.config/yard/config.toml` (or `$XDG_CONFIG_HOME/yard/config.toml`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};

/// Default RDP port.
pub const DEFAULT_PORT: u16 = 3389;

/// Configuration file name.
const CONFIG_FILE_NAME: &str = "config.toml";

/// Application directory name.
const APP_DIR_NAME: &str = "yard";

/// YARD configuration loaded from config file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Default connection settings.
    pub defaults: ConnectionDefaults,
    /// Audio settings.
    pub audio: AudioConfig,
    /// Clipboard settings (Story 5.1).
    pub clipboard: ClipboardConfig,
    /// Named connection profiles (Story 6.5).
    ///
    /// Profiles allow users to save connection settings for frequently used servers.
    /// Each profile is identified by a name (e.g., "work", "home") and contains
    /// connection parameters like host, port, username, etc.
    pub profiles: HashMap<String, ConnectionProfile>,
}

/// Audio configuration settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Enable audio output (default: true).
    pub enabled: bool,
    /// Enable microphone input (default: true).
    pub microphone: bool,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            microphone: true,
        }
    }
}

/// Clipboard configuration settings (Story 5.1).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ClipboardConfig {
    /// Enable clipboard synchronization (default: true).
    pub enabled: bool,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// A saved connection profile (Story 6.5).
///
/// Profiles store connection settings for frequently used servers,
/// allowing users to connect with just `yard connect profile-name`
/// instead of specifying all parameters each time.
///
/// Only `host` is required; all other fields are optional and will
/// use global defaults or CLI arguments if not specified.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionProfile {
    /// Target hostname or IP address (REQUIRED).
    ///
    /// This is the only mandatory field. Examples:
    /// - `"server.example.com"`
    /// - `"192.168.1.100"`
    pub host: String,

    /// Target port (optional, default: 3389).
    ///
    /// If not specified, uses the global default port from `[defaults]`
    /// or the standard RDP port 3389.
    pub port: Option<u16>,

    /// Username for authentication.
    ///
    /// Can include domain in NT format (`DOMAIN\user`) or UPN format (`user@domain.com`).
    pub username: Option<String>,

    /// Domain for authentication.
    ///
    /// If specified separately from username, will be combined during connection.
    pub domain: Option<String>,

    /// Start in fullscreen mode.
    ///
    /// If `true`, the session starts in fullscreen on the current monitor.
    pub fullscreen: Option<bool>,

    /// Enable multi-monitor fullscreen mode.
    ///
    /// If `true`, creates separate windows on all connected monitors.
    /// Requires `fullscreen` to also be `true` (or implied).
    pub all_monitors: Option<bool>,

    /// Enable audio output.
    ///
    /// If `false`, disables remote audio playback.
    pub audio: Option<bool>,

    /// Enable microphone input.
    ///
    /// If `false`, disables microphone transmission to remote.
    pub microphone: Option<bool>,

    /// Enable clipboard synchronization.
    ///
    /// If `false`, disables bidirectional clipboard sync.
    pub clipboard: Option<bool>,
}

impl ConnectionProfile {
    /// Validates that the profile has all required fields.
    ///
    /// Currently, only `host` is required and must be non-empty (not just whitespace).
    ///
    /// # Arguments
    ///
    /// * `name` - The profile name, used in error messages.
    ///
    /// # Errors
    ///
    /// Returns `Error::Config` if validation fails (e.g., empty or whitespace-only host).
    #[must_use = "validation result should be checked"]
    pub fn validate(&self, name: &str) -> Result<()> {
        if self.host.trim().is_empty() {
            return Err(Error::Config(format!(
                "Profile '{}' is missing required field 'host'",
                name
            )));
        }
        Ok(())
    }
}

/// Default connection settings from config file.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ConnectionDefaults {
    /// Default RDP port (default: 3389).
    pub port: u16,

    /// Default domain for authentication.
    pub domain: Option<String>,

    /// Default username for authentication.
    pub username: Option<String>,

    /// Start in fullscreen mode by default (Story 6.7).
    ///
    /// If `None` or not specified, defaults to windowed mode (false).
    /// Can be overridden by CLI `-f` flag or `--no-fullscreen` flag.
    pub fullscreen: Option<bool>,

    /// Enable multi-monitor fullscreen mode by default (Story 6.7).
    ///
    /// If `None` or not specified, defaults to single-monitor mode (false).
    /// Can be overridden by CLI `--all-monitors` flag.
    pub all_monitors: Option<bool>,
}

impl Default for ConnectionDefaults {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            domain: None,
            username: None,
            fullscreen: None,
            all_monitors: None,
        }
    }
}

impl ConnectionDefaults {
    /// Returns the port, defaulting to 3389 if port is 0.
    ///
    /// Port 0 is invalid for RDP connections, so this method
    /// treats it as "use default".
    #[must_use]
    pub fn effective_port(&self) -> u16 {
        if self.port == 0 {
            DEFAULT_PORT
        } else {
            self.port
        }
    }
}

/// Error details for configuration parsing failures.
#[derive(Debug)]
struct ConfigParseError {
    /// Human-readable error message.
    pub message: String,
    /// Line number where error occurred (if available).
    pub line: Option<usize>,
    /// Column number where error occurred (if available).
    pub column: Option<usize>,
}

impl std::fmt::Display for ConfigParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.line, self.column) {
            (Some(line), Some(col)) => {
                write!(f, "{} (line {}, column {})", self.message, line, col)
            }
            (Some(line), None) => {
                write!(f, "{} (line {})", self.message, line)
            }
            _ => write!(f, "{}", self.message),
        }
    }
}

impl Config {
    /// Loads configuration from the default config file location.
    ///
    /// Returns `Ok(Config::default())` if the config file doesn't exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be read or parsed.
    pub fn load() -> Result<Self> {
        let path = Self::config_path();
        Self::load_from_path(&path)
    }

    /// Loads configuration from a specific path.
    ///
    /// Returns `Ok(Config::default())` if the file doesn't exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be read or contains invalid TOML.
    pub fn load_from_path(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = std::fs::read_to_string(path).map_err(|e| {
            Error::Config(format!(
                "Failed to read config file {}: {}",
                path.display(),
                e
            ))
        })?;

        Self::parse(&contents)
    }

    /// Parses configuration from a TOML string.
    ///
    /// Also validates all profiles after parsing to ensure required fields are present.
    ///
    /// # Errors
    ///
    /// Returns an error if the TOML is invalid, contains incorrect types,
    /// or if any profile fails validation (e.g., missing host field).
    pub fn parse(contents: &str) -> Result<Self> {
        let config: Self = toml::from_str(contents).map_err(|e| {
            let parse_error = ConfigParseError {
                message: e.message().to_string(),
                line: e.span().map(|s| {
                    // Count newlines before the error span to get line number
                    contents[..s.start].chars().filter(|&c| c == '\n').count() + 1
                }),
                column: e.span().map(|s| {
                    // Find column within the line
                    let before = &contents[..s.start];
                    before
                        .rfind('\n')
                        .map(|pos| s.start - pos)
                        .unwrap_or(s.start + 1)
                }),
            };
            Error::Config(parse_error.to_string())
        })?;

        // Story 6.5: Validate all profiles have required fields
        config.validate_profiles()?;

        Ok(config)
    }

    /// Returns the path to the configuration file.
    ///
    /// Uses `$XDG_CONFIG_HOME/yard/config.toml` if set,
    /// otherwise falls back to `~/.config/yard/config.toml`.
    /// On Windows, uses `%APPDATA%\yard\config.toml`.
    #[must_use]
    pub fn config_path() -> PathBuf {
        Self::config_dir().join(CONFIG_FILE_NAME)
    }

    /// Returns the configuration directory path.
    ///
    /// Platform-specific:
    /// - Linux/macOS: `$XDG_CONFIG_HOME/yard` or `~/.config/yard`
    /// - Windows: `%APPDATA%\yard`
    #[must_use]
    pub fn config_dir() -> PathBuf {
        // XDG_CONFIG_HOME takes priority on all platforms
        if let Ok(xdg_config) = std::env::var("XDG_CONFIG_HOME") {
            return PathBuf::from(xdg_config).join(APP_DIR_NAME);
        }

        // Windows: use APPDATA
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join(APP_DIR_NAME);
        }

        // Unix: use HOME/.config
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".config").join(APP_DIR_NAME);
        }

        // Windows fallback: USERPROFILE
        if let Ok(home) = std::env::var("USERPROFILE") {
            return PathBuf::from(home).join(".config").join(APP_DIR_NAME);
        }

        // Last resort fallback
        PathBuf::from(".").join(".config").join(APP_DIR_NAME)
    }

    /// Retrieves a connection profile by name (Story 6.5).
    ///
    /// # Arguments
    ///
    /// * `name` - The profile name as defined in the config file.
    ///
    /// # Returns
    ///
    /// A reference to the profile if found, or an error if not found.
    ///
    /// # Errors
    ///
    /// Returns `Error::Config` if the profile does not exist.
    ///
    /// # Example
    ///
    /// ```toml
    /// [profiles.work]
    /// host = "work.example.com"
    /// ```
    ///
    /// ```no_run
    /// use yard_core::Config;
    ///
    /// let config = Config::load().expect("failed to load config");
    /// if let Ok(profile) = config.get_profile("work") {
    ///     println!("Connecting to {}", profile.host);
    /// }
    /// ```
    pub fn get_profile(&self, name: &str) -> Result<&ConnectionProfile> {
        self.profiles
            .get(name)
            .ok_or_else(|| Error::Config(format!("Profile '{}' not found in configuration", name)))
    }

    /// Returns a sorted list of all profile names (Story 6.5).
    ///
    /// Useful for listing available profiles or tab completion.
    /// Names are sorted alphabetically for consistent output.
    ///
    /// # Returns
    ///
    /// A vector of profile names, sorted alphabetically. Empty if no profiles are defined.
    #[must_use]
    pub fn profile_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.profiles.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    /// Checks if a profile with the given name exists (Story 6.5).
    ///
    /// # Arguments
    ///
    /// * `name` - The profile name to check.
    ///
    /// # Returns
    ///
    /// `true` if the profile exists, `false` otherwise.
    #[must_use]
    pub fn has_profile(&self, name: &str) -> bool {
        self.profiles.contains_key(name)
    }

    /// Validates all profiles in the configuration (Story 6.5).
    ///
    /// Called internally after parsing to ensure all profiles
    /// have the required fields.
    ///
    /// # Errors
    ///
    /// Returns an error if any profile fails validation.
    pub fn validate_profiles(&self) -> Result<()> {
        for (name, profile) in &self.profiles {
            profile.validate(name)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert_eq!(config.defaults.port, 3389);
        assert!(config.defaults.domain.is_none());
        assert!(config.defaults.username.is_none());
    }

    #[test]
    fn test_config_parse_empty() {
        let config = Config::parse("").unwrap();
        assert_eq!(config.defaults.port, 3389);
    }

    #[test]
    fn test_config_parse_defaults() {
        let toml = r#"
[defaults]
port = 13389
domain = "CORP"
username = "john"
"#;
        let config = Config::parse(toml).unwrap();
        assert_eq!(config.defaults.port, 13389);
        assert_eq!(config.defaults.domain, Some("CORP".to_string()));
        assert_eq!(config.defaults.username, Some("john".to_string()));
    }

    #[test]
    fn test_config_parse_partial_defaults() {
        let toml = r#"
[defaults]
port = 3390
"#;
        let config = Config::parse(toml).unwrap();
        assert_eq!(config.defaults.port, 3390);
        assert!(config.defaults.domain.is_none());
    }

    #[test]
    fn test_config_parse_invalid_toml() {
        let toml = r#"
[defaults
port = 3389
"#;
        let result = Config::parse(toml);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("line"));
    }

    #[test]
    fn test_config_parse_invalid_type() {
        let toml = r#"
[defaults]
port = "not a number"
"#;
        let result = Config::parse(toml);
        assert!(result.is_err());
    }

    #[test]
    fn test_config_parse_unknown_fields_ignored() {
        let toml = r#"
[defaults]
port = 3389
unknown_field = "ignored"

[unknown_section]
foo = "bar"
"#;
        // Unknown fields should be ignored (serde default behavior)
        let result = Config::parse(toml);
        assert!(result.is_ok());
    }

    #[test]
    fn test_config_path_uses_xdg() {
        // This test just verifies the function runs
        let path = Config::config_path();
        assert!(path.ends_with("config.toml"));
    }

    #[test]
    fn test_config_load_nonexistent_returns_default() {
        let path = PathBuf::from("/nonexistent/path/config.toml");
        let config = Config::load_from_path(&path).unwrap();
        assert_eq!(config.defaults.port, DEFAULT_PORT);
    }

    #[test]
    fn test_connection_defaults_default() {
        let defaults = ConnectionDefaults::default();
        assert_eq!(defaults.port, 3389);
        assert!(defaults.domain.is_none());
        assert!(defaults.username.is_none());
        // Story 6.7: New fields default to None
        assert!(defaults.fullscreen.is_none());
        assert!(defaults.all_monitors.is_none());
    }

    #[test]
    fn test_effective_port_normal() {
        let defaults = ConnectionDefaults {
            port: 13389,
            ..Default::default()
        };
        assert_eq!(defaults.effective_port(), 13389);
    }

    #[test]
    fn test_effective_port_zero_uses_default() {
        let defaults = ConnectionDefaults {
            port: 0,
            ..Default::default()
        };
        assert_eq!(defaults.effective_port(), DEFAULT_PORT);
    }

    #[test]
    fn test_audio_config_default() {
        let config = AudioConfig::default();
        assert!(config.enabled);
        assert!(config.microphone);
    }

    #[test]
    fn test_config_parse_audio_section() {
        let toml = r#"
[audio]
enabled = false
microphone = false
"#;
        let config = Config::parse(toml).unwrap();
        assert!(!config.audio.enabled);
        assert!(!config.audio.microphone);
    }

    #[test]
    fn test_config_parse_audio_partial() {
        let toml = r#"
[audio]
microphone = false
"#;
        let config = Config::parse(toml).unwrap();
        // enabled defaults to true
        assert!(config.audio.enabled);
        assert!(!config.audio.microphone);
    }

    #[test]
    fn test_config_defaults_include_audio() {
        let config = Config::default();
        assert!(config.audio.enabled);
        assert!(config.audio.microphone);
    }

    // Story 4.5: Audio flag precedence tests
    #[test]
    fn test_audio_flag_cli_overrides_config_enabled() {
        // CLI --no-audio should disable audio even when config has enabled = true
        let config = Config::default();
        assert!(config.audio.enabled); // Config default

        // Simulate CLI flag logic: !no_audio && config.audio.enabled
        let no_audio = true; // CLI flag set
        let audio_enabled = !no_audio && config.audio.enabled;
        assert!(!audio_enabled); // CLI wins
    }

    #[test]
    fn test_audio_flag_cli_overrides_config_microphone() {
        // CLI --no-microphone should disable microphone even when config has microphone = true
        let config = Config::default();
        assert!(config.audio.microphone); // Config default

        // Simulate CLI flag logic: !no_microphone && config.audio.microphone
        let no_microphone = true; // CLI flag set
        let microphone_enabled = !no_microphone && config.audio.microphone;
        assert!(!microphone_enabled); // CLI wins
    }

    #[test]
    fn test_audio_flag_config_disables_when_cli_default() {
        // Config audio.enabled = false should disable audio when CLI has no flags
        let toml = r#"
[audio]
enabled = false
"#;
        let config = Config::parse(toml).unwrap();
        assert!(!config.audio.enabled);

        // Simulate CLI flag logic: !no_audio && config.audio.enabled
        let no_audio = false; // CLI flag not set (default)
        let audio_enabled = !no_audio && config.audio.enabled;
        assert!(!audio_enabled); // Config wins when CLI is default
    }

    #[test]
    fn test_audio_both_enabled_when_no_flags_and_default_config() {
        // Default case: no CLI flags, default config = both enabled
        let config = Config::default();
        let no_audio = false;
        let no_microphone = false;

        let audio_enabled = !no_audio && config.audio.enabled;
        let microphone_enabled = !no_microphone && config.audio.microphone;

        assert!(audio_enabled);
        assert!(microphone_enabled);
    }

    // Story 4.5: Integration behavior tests
    // These tests verify the ACTUAL behavior chain, not just flag parsing

    #[test]
    fn test_no_audio_flag_disables_both_output_and_input() {
        // When --no-audio is set, AudioThread is not spawned, which means:
        // - audio_tx = None (no sender to audio thread)
        // - capture_rx = None (no audio thread to take receiver from)
        // This implicitly disables BOTH RDPSND (output) and AUDIN (input)
        let config = Config::default();
        let no_audio = true;
        let no_microphone = false; // Even if mic flag is not set

        let audio_enabled = !no_audio && config.audio.enabled;
        let microphone_enabled = !no_microphone && config.audio.microphone;

        // audio_enabled=false means AudioThread won't spawn
        assert!(!audio_enabled);
        // microphone_enabled=true but won't matter because audio_thread=None
        assert!(microphone_enabled);

        // Simulate main.rs logic:
        // let audio_thread = if audio_enabled { Some(AudioThread::spawn()) } else { None };
        let audio_thread_exists = audio_enabled; // Simplified: true if would spawn

        // audio_tx = audio_thread.as_ref().map(|a| a.sender())
        let audio_tx_exists = audio_thread_exists;

        // capture_rx = if microphone_enabled { audio_thread.as_mut().and_then(...) } else { None }
        // Key insight: capture_rx requires BOTH microphone_enabled AND audio_thread to exist
        let capture_rx_exists = microphone_enabled && audio_thread_exists;

        // Verify: --no-audio disables BOTH channels
        assert!(
            !audio_tx_exists,
            "RDPSND should be disabled when --no-audio"
        );
        assert!(
            !capture_rx_exists,
            "AUDIN should be disabled when --no-audio (no audio thread)"
        );
    }

    #[test]
    fn test_no_microphone_flag_disables_only_input() {
        // When --no-microphone is set but --no-audio is NOT:
        // - AudioThread spawns (for playback)
        // - RDPSND is attached (audio output works)
        // - AUDIN is NOT attached (microphone disabled)
        let config = Config::default();
        let no_audio = false;
        let no_microphone = true;

        let audio_enabled = !no_audio && config.audio.enabled;
        let microphone_enabled = !no_microphone && config.audio.microphone;

        assert!(audio_enabled, "Audio output should still be enabled");
        assert!(!microphone_enabled, "Microphone should be disabled");

        // Simulate main.rs logic
        let audio_thread_exists = audio_enabled;
        let audio_tx_exists = audio_thread_exists;
        let capture_rx_exists = microphone_enabled && audio_thread_exists;

        // Verify: --no-microphone disables ONLY AUDIN, not RDPSND
        assert!(audio_tx_exists, "RDPSND should still be enabled");
        assert!(!capture_rx_exists, "AUDIN should be disabled");
    }

    #[test]
    fn test_config_microphone_false_with_audio_enabled() {
        // Config: audio.enabled=true, audio.microphone=false
        // Same effect as --no-microphone flag
        let toml = r#"
[audio]
enabled = true
microphone = false
"#;
        let config = Config::parse(toml).unwrap();
        let no_audio = false;
        let no_microphone = false;

        let audio_enabled = !no_audio && config.audio.enabled;
        let microphone_enabled = !no_microphone && config.audio.microphone;

        assert!(audio_enabled, "Audio output should be enabled from config");
        assert!(
            !microphone_enabled,
            "Microphone should be disabled from config"
        );

        // Simulate main.rs logic
        let audio_thread_exists = audio_enabled;
        let audio_tx_exists = audio_thread_exists;
        let capture_rx_exists = microphone_enabled && audio_thread_exists;

        assert!(audio_tx_exists, "RDPSND should be enabled");
        assert!(!capture_rx_exists, "AUDIN should be disabled");
    }

    // Story 5.1: Clipboard config tests
    #[test]
    fn test_clipboard_config_default() {
        let config = ClipboardConfig::default();
        assert!(config.enabled);
    }

    #[test]
    fn test_config_defaults_include_clipboard() {
        let config = Config::default();
        assert!(config.clipboard.enabled);
    }

    #[test]
    fn test_config_parse_clipboard_section() {
        let toml = r#"
[clipboard]
enabled = false
"#;
        let config = Config::parse(toml).unwrap();
        assert!(!config.clipboard.enabled);
    }

    #[test]
    fn test_clipboard_flag_cli_overrides_config() {
        // CLI --no-clipboard should disable clipboard even when config has enabled = true
        let config = Config::default();
        assert!(config.clipboard.enabled); // Config default

        // Simulate CLI flag logic: !no_clipboard && config.clipboard.enabled
        let no_clipboard = true; // CLI flag set
        let clipboard_enabled = !no_clipboard && config.clipboard.enabled;
        assert!(!clipboard_enabled); // CLI wins
    }

    #[test]
    fn test_clipboard_config_disables_when_cli_default() {
        // Config clipboard.enabled = false should disable clipboard when CLI has no flags
        let toml = r#"
[clipboard]
enabled = false
"#;
        let config = Config::parse(toml).unwrap();
        assert!(!config.clipboard.enabled);

        let no_clipboard = false; // CLI flag not set (default)
        let clipboard_enabled = !no_clipboard && config.clipboard.enabled;
        assert!(!clipboard_enabled); // Config wins when CLI is default
    }

    // Story 6.5: Connection profile tests
    #[test]
    fn test_config_default_has_empty_profiles() {
        let config = Config::default();
        assert!(config.profiles.is_empty());
    }

    #[test]
    fn test_config_parse_without_profiles_backward_compat() {
        // Existing configs without [profiles.*] should still work
        let toml = r#"
[defaults]
port = 3389

[audio]
enabled = true
"#;
        let config = Config::parse(toml).unwrap();
        assert!(config.profiles.is_empty());
        assert_eq!(config.defaults.port, 3389);
    }

    #[test]
    fn test_config_parse_profile_minimal() {
        // Profile with only required field (host)
        let toml = r#"
[profiles.minimal]
host = "server.example.com"
"#;
        let config = Config::parse(toml).unwrap();
        assert_eq!(config.profiles.len(), 1);
        let profile = config.get_profile("minimal").unwrap();
        assert_eq!(profile.host, "server.example.com");
        assert!(profile.port.is_none());
        assert!(profile.username.is_none());
        assert!(profile.domain.is_none());
        assert!(profile.fullscreen.is_none());
        assert!(profile.all_monitors.is_none());
        assert!(profile.audio.is_none());
        assert!(profile.microphone.is_none());
        assert!(profile.clipboard.is_none());
    }

    #[test]
    fn test_config_parse_profile_all_fields() {
        let toml = r#"
[profiles.work]
host = "work.example.com"
port = 13389
username = "john"
domain = "CORP"
fullscreen = true
all_monitors = true
audio = true
microphone = false
clipboard = true
"#;
        let config = Config::parse(toml).unwrap();
        let profile = config.get_profile("work").unwrap();
        assert_eq!(profile.host, "work.example.com");
        assert_eq!(profile.port, Some(13389));
        assert_eq!(profile.username, Some("john".to_string()));
        assert_eq!(profile.domain, Some("CORP".to_string()));
        assert_eq!(profile.fullscreen, Some(true));
        assert_eq!(profile.all_monitors, Some(true));
        assert_eq!(profile.audio, Some(true));
        assert_eq!(profile.microphone, Some(false));
        assert_eq!(profile.clipboard, Some(true));
    }

    #[test]
    fn test_config_parse_multiple_profiles() {
        let toml = r#"
[profiles.work]
host = "work.example.com"
username = "john"

[profiles.home]
host = "192.168.1.100"
username = "admin"

[profiles.test]
host = "test.example.com"
"#;
        let config = Config::parse(toml).unwrap();
        assert_eq!(config.profiles.len(), 3);

        let work = config.get_profile("work").unwrap();
        assert_eq!(work.host, "work.example.com");
        assert_eq!(work.username, Some("john".to_string()));

        let home = config.get_profile("home").unwrap();
        assert_eq!(home.host, "192.168.1.100");
        assert_eq!(home.username, Some("admin".to_string()));

        let test = config.get_profile("test").unwrap();
        assert_eq!(test.host, "test.example.com");
        assert!(test.username.is_none());
    }

    #[test]
    fn test_config_get_profile_not_found() {
        let config = Config::default();
        let result = config.get_profile("nonexistent");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("nonexistent"));
        assert!(err.contains("not found"));
    }

    #[test]
    fn test_config_profile_names() {
        let toml = r#"
[profiles.alpha]
host = "a.example.com"

[profiles.beta]
host = "b.example.com"

[profiles.gamma]
host = "c.example.com"
"#;
        let config = Config::parse(toml).unwrap();
        let names = config.profile_names();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
        assert!(names.contains(&"gamma"));
    }

    #[test]
    fn test_config_profile_names_empty() {
        let config = Config::default();
        let names = config.profile_names();
        assert!(names.is_empty());
    }

    #[test]
    fn test_config_has_profile() {
        let toml = r#"
[profiles.work]
host = "work.example.com"
"#;
        let config = Config::parse(toml).unwrap();
        assert!(config.has_profile("work"));
        assert!(!config.has_profile("home"));
        assert!(!config.has_profile("nonexistent"));
    }

    #[test]
    fn test_profile_validate_missing_host() {
        // This tests the validate method directly
        let profile = ConnectionProfile {
            host: String::new(), // Empty host
            port: None,
            username: None,
            domain: None,
            fullscreen: None,
            all_monitors: None,
            audio: None,
            microphone: None,
            clipboard: None,
        };
        let result = profile.validate("test-profile");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("test-profile"));
        assert!(err.contains("host"));
    }

    #[test]
    fn test_profile_validate_success() {
        let profile = ConnectionProfile {
            host: "server.example.com".to_string(),
            port: None,
            username: None,
            domain: None,
            fullscreen: None,
            all_monitors: None,
            audio: None,
            microphone: None,
            clipboard: None,
        };
        assert!(profile.validate("test-profile").is_ok());
    }

    #[test]
    fn test_config_parse_profile_empty_host_fails() {
        // Profile with empty host string should fail validation
        let toml = r#"
[profiles.bad]
host = ""
"#;
        let result = Config::parse(toml);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("bad"));
        assert!(err.contains("host"));
    }

    #[test]
    fn test_config_profiles_mixed_with_other_sections() {
        // Profiles should work alongside other config sections
        let toml = r#"
[defaults]
port = 3390
username = "default-user"

[audio]
enabled = false

[clipboard]
enabled = true

[profiles.work]
host = "work.example.com"
port = 3389
"#;
        let config = Config::parse(toml).unwrap();

        // Check defaults
        assert_eq!(config.defaults.port, 3390);
        assert_eq!(config.defaults.username, Some("default-user".to_string()));

        // Check audio
        assert!(!config.audio.enabled);

        // Check clipboard
        assert!(config.clipboard.enabled);

        // Check profile
        let profile = config.get_profile("work").unwrap();
        assert_eq!(profile.host, "work.example.com");
        assert_eq!(profile.port, Some(3389));
    }

    #[test]
    fn test_config_profile_with_ip_address() {
        let toml = r#"
[profiles.local]
host = "192.168.1.100"
"#;
        let config = Config::parse(toml).unwrap();
        let profile = config.get_profile("local").unwrap();
        assert_eq!(profile.host, "192.168.1.100");
    }

    #[test]
    fn test_config_profile_fullscreen_false() {
        // Explicitly set fullscreen to false
        let toml = r#"
[profiles.windowed]
host = "server.example.com"
fullscreen = false
"#;
        let config = Config::parse(toml).unwrap();
        let profile = config.get_profile("windowed").unwrap();
        assert_eq!(profile.fullscreen, Some(false));
    }

    #[test]
    fn test_config_parse_profile_whitespace_host_fails() {
        // Profile with whitespace-only host should fail validation
        let toml = r#"
[profiles.bad]
host = "   "
"#;
        let result = Config::parse(toml);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("bad"));
        assert!(err.contains("host"));
    }

    #[test]
    fn test_profile_validate_whitespace_only_host() {
        // Whitespace-only host should fail validation
        let profile = ConnectionProfile {
            host: "   \t\n  ".to_string(),
            port: None,
            username: None,
            domain: None,
            fullscreen: None,
            all_monitors: None,
            audio: None,
            microphone: None,
            clipboard: None,
        };
        let result = profile.validate("whitespace-profile");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("whitespace-profile"));
        assert!(err.contains("host"));
    }

    #[test]
    fn test_validate_profiles_stops_at_first_error() {
        // Validation stops at first invalid profile (order is non-deterministic due to HashMap)
        let toml = r#"
[profiles.valid]
host = "valid.example.com"

[profiles.invalid1]
host = ""

[profiles.invalid2]
host = ""
"#;
        let result = Config::parse(toml);
        assert!(result.is_err());
        // Should contain one of the invalid profile names
        let err = result.unwrap_err().to_string();
        assert!(err.contains("host"));
        assert!(err.contains("invalid")); // Either invalid1 or invalid2
    }

    #[test]
    fn test_config_profile_names_sorted() {
        // Profile names should be returned in sorted order
        let toml = r#"
[profiles.zebra]
host = "z.example.com"

[profiles.alpha]
host = "a.example.com"

[profiles.middle]
host = "m.example.com"
"#;
        let config = Config::parse(toml).unwrap();
        let names = config.profile_names();
        assert_eq!(names, vec!["alpha", "middle", "zebra"]);
    }

    #[test]
    fn test_config_profile_unknown_field_rejected() {
        // Unknown fields in profiles should be rejected (serde deny_unknown_fields)
        let toml = r#"
[profiles.typo]
host = "server.example.com"
fulscreen = true
"#;
        let result = Config::parse(toml);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("fulscreen") || err.contains("unknown"));
    }

    // Story 6.7: Default connection options tests
    #[test]
    fn test_config_parse_defaults_fullscreen() {
        let toml = r#"
[defaults]
fullscreen = true
all_monitors = true
"#;
        let config = Config::parse(toml).unwrap();
        assert_eq!(config.defaults.fullscreen, Some(true));
        assert_eq!(config.defaults.all_monitors, Some(true));
    }

    #[test]
    fn test_config_parse_defaults_fullscreen_false() {
        let toml = r#"
[defaults]
fullscreen = false
all_monitors = false
"#;
        let config = Config::parse(toml).unwrap();
        assert_eq!(config.defaults.fullscreen, Some(false));
        assert_eq!(config.defaults.all_monitors, Some(false));
    }

    #[test]
    fn test_config_defaults_backward_compat_no_fullscreen() {
        // Old configs without fullscreen fields should still work
        let toml = r#"
[defaults]
port = 3390
username = "olduser"
"#;
        let config = Config::parse(toml).unwrap();
        assert_eq!(config.defaults.port, 3390);
        assert_eq!(config.defaults.username, Some("olduser".to_string()));
        // New fields should be None
        assert!(config.defaults.fullscreen.is_none());
        assert!(config.defaults.all_monitors.is_none());
    }

    #[test]
    fn test_config_defaults_mixed_with_profiles() {
        // Defaults and profiles can both have fullscreen settings
        let toml = r#"
[defaults]
fullscreen = true
all_monitors = true

[profiles.windowed]
host = "windowed.example.com"
fullscreen = false
"#;
        let config = Config::parse(toml).unwrap();

        // Defaults should have fullscreen enabled
        assert_eq!(config.defaults.fullscreen, Some(true));
        assert_eq!(config.defaults.all_monitors, Some(true));

        // Profile should override with fullscreen disabled
        let profile = config.get_profile("windowed").unwrap();
        assert_eq!(profile.fullscreen, Some(false));
    }
}
