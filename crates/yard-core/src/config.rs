//! Configuration file support for YARD.
//!
//! This module handles loading and parsing the YARD configuration file
//! from `~/.config/yard/config.toml` (or `$XDG_CONFIG_HOME/yard/config.toml`).

use std::path::PathBuf;

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
}

impl Default for ConnectionDefaults {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            domain: None,
            username: None,
        }
    }
}

/// Error details for configuration parsing failures.
#[derive(Debug)]
pub struct ConfigParseError {
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
    /// Returns an error if the file exists but cannot be parsed.
    pub fn load() -> Result<Self> {
        let path = Self::config_path();
        Self::load_from_path(&path)
    }

    /// Loads configuration from a specific path.
    ///
    /// Returns `Ok(Config::default())` if the file doesn't exist.
    /// Returns an error if the file exists but cannot be parsed.
    pub fn load_from_path(path: &PathBuf) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = std::fs::read_to_string(path).map_err(|e| {
            Error::Config(format!("Failed to read config file {}: {}", path.display(), e))
        })?;

        Self::parse(&contents)
    }

    /// Parses configuration from a TOML string.
    pub fn parse(contents: &str) -> Result<Self> {
        toml::from_str(contents).map_err(|e| {
            let parse_error = ConfigParseError {
                message: e.message().to_string(),
                line: e.span().map(|s| {
                    // Count newlines before the error span to get line number
                    contents[..s.start].chars().filter(|&c| c == '\n').count() + 1
                }),
                column: e.span().map(|s| {
                    // Find column within the line
                    let before = &contents[..s.start];
                    before.rfind('\n').map(|pos| s.start - pos).unwrap_or(s.start + 1)
                }),
            };
            Error::Config(parse_error.to_string())
        })
    }

    /// Returns the path to the configuration file.
    ///
    /// Uses `$XDG_CONFIG_HOME/yard/config.toml` if set,
    /// otherwise falls back to `~/.config/yard/config.toml`.
    pub fn config_path() -> PathBuf {
        Self::config_dir().join(CONFIG_FILE_NAME)
    }

    /// Returns the configuration directory path.
    pub fn config_dir() -> PathBuf {
        if let Ok(xdg_config) = std::env::var("XDG_CONFIG_HOME") {
            PathBuf::from(xdg_config).join(APP_DIR_NAME)
        } else if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home).join(".config").join(APP_DIR_NAME)
        } else if let Ok(home) = std::env::var("USERPROFILE") {
            // Windows fallback
            PathBuf::from(home).join(".config").join(APP_DIR_NAME)
        } else {
            // Last resort fallback
            PathBuf::from(".").join(".config").join(APP_DIR_NAME)
        }
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
    }
}
