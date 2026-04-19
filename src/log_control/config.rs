// SPDX-License-Identifier: MIT

use serde::{Deserialize, Serialize};

/// Configuration for the localhost-only runtime log-control server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct LogControlConfig {
    /// TCP port bound on `127.0.0.1`.
    pub port: u16,
}

impl Default for LogControlConfig {
    fn default() -> Self {
        Self { port: 6669 }
    }
}

#[cfg(test)]
mod tests {
    use super::LogControlConfig;

    #[test]
    fn log_control_config_deserializes_with_default_port() {
        let config: LogControlConfig = toml::from_str("").unwrap();

        assert_eq!(config, LogControlConfig::default());
    }

    #[test]
    fn log_control_config_deserializes_port_override() {
        let config: LogControlConfig = toml::from_str("port = 7777").unwrap();

        assert_eq!(config.port, 7777);
    }
}
