// SPDX-License-Identifier: MIT

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Configuration for optional OTLP export.
///
/// This type is serializable so consuming services can include an OTLP section
/// in their own configuration file, deserialize it, and pass it to
/// [`TelemetryBuilder::with_otlp_config`](crate::TelemetryBuilder::with_otlp_config).
/// The telemetry crate does not load configuration files automatically.
///
/// # Examples
///
/// ```
/// use telemetry_setup::OtlpConfig;
///
/// let config: OtlpConfig = toml::from_str(r#"
/// url = "http://localhost:4318"
/// log_level = "info"
/// service_name = "controller"
/// log_rate_limit_per_sec = 100
/// "#)?;
///
/// assert_eq!(config.url, "http://localhost:4318");
/// # Ok::<(), toml::de::Error>(())
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct OtlpConfig {
    /// Base collector URL.
    ///
    /// When this URL does not already include a signal suffix, the crate
    /// appends `/v1/traces`, `/v1/logs`, and `/v1/metrics` for individual
    /// OTLP/HTTP requests. The default value targets a local OTLP/HTTP
    /// collector on port `4318`.
    pub url: String,
    /// Filter expression applied to OTLP trace and log export layers.
    ///
    /// This does not affect local stdout or journald output; configure those
    /// through [`TelemetryBuilder`](crate::TelemetryBuilder).
    pub log_level: String,
    /// Optional override for the emitted `service.name` resource attribute.
    pub service_name: Option<String>,
    /// Maximum OTLP log events exported per second.
    ///
    /// This limit applies only to OTLP log export. It does not rate-limit local
    /// stdout or journald output.
    pub log_rate_limit_per_sec: Option<u64>,
    /// HTTP headers attached to OTLP export requests.
    pub headers: OtlpHeadersConfig,
    /// Interval between OTLP metric export cycles.
    ///
    /// The default is five seconds. Values must be between one second and one
    /// day.
    #[serde(with = "duration_seconds")]
    pub metrics_interval: std::time::Duration,
}

/// HTTP headers attached to OTLP export requests.
///
/// Headers in signal-specific maps override [`OtlpHeadersConfig::common`]
/// entries with the same header name. Service-specific collector headers,
/// including GreptimeDB database and pipeline headers, must be configured
/// explicitly.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct OtlpHeadersConfig {
    /// Headers attached to every OTLP signal request.
    pub common: HashMap<String, String>,
    /// Headers attached only to trace export requests.
    pub traces: HashMap<String, String>,
    /// Headers attached only to log export requests.
    pub logs: HashMap<String, String>,
    /// Headers attached only to metric export requests.
    pub metrics: HashMap<String, String>,
}

impl Default for OtlpConfig {
    fn default() -> Self {
        Self {
            url: "http://localhost:4318".to_string(),
            log_level: "info".to_string(),
            service_name: None,
            log_rate_limit_per_sec: None,
            headers: OtlpHeadersConfig::default(),
            metrics_interval: std::time::Duration::from_secs(5),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{OtlpConfig, OtlpHeadersConfig};

    #[test]
    fn otlp_config_defaults_are_local_first() {
        let config = OtlpConfig::default();
        assert_eq!(config.url, "http://localhost:4318");
        assert_eq!(config.log_level, "info");
        assert_eq!(config.service_name, None);
        assert_eq!(config.log_rate_limit_per_sec, None);
        assert_eq!(config.headers, OtlpHeadersConfig::default());
        assert_eq!(config.metrics_interval, std::time::Duration::from_secs(5));
    }

    #[test]
    fn config_round_trips_via_toml() {
        let config = OtlpConfig {
            url: "http://collector:4318".to_string(),
            log_level: "controller=debug".to_string(),
            service_name: Some("controller".to_string()),
            log_rate_limit_per_sec: Some(25),
            headers: OtlpHeadersConfig {
                common: HashMap::from([("X-Greptime-DB-Name".to_string(), "edge".to_string())]),
                traces: HashMap::from([(
                    "x-greptime-pipeline-name".to_string(),
                    "greptime_trace_v1".to_string(),
                )]),
                ..OtlpHeadersConfig::default()
            },
            metrics_interval: std::time::Duration::from_secs(10),
        };

        let encoded = toml::to_string(&config).expect("config should serialize");
        let decoded: OtlpConfig = toml::from_str(&encoded).expect("config should deserialize");

        assert_eq!(decoded, config);
    }

    #[test]
    fn otlp_config_uses_defaults_for_partial_deserialize() {
        let decoded: OtlpConfig = toml::from_str(
            r#"
            [headers.common]
            X-Greptime-DB-Name = "edge"
            "#,
        )
        .expect("partial config should deserialize");

        assert_eq!(decoded.url, "http://localhost:4318");
        assert_eq!(
            decoded.headers.common.get("X-Greptime-DB-Name"),
            Some(&"edge".to_string())
        );
        assert_eq!(decoded.log_level, "info");
        assert_eq!(decoded.service_name, None);
        assert_eq!(decoded.log_rate_limit_per_sec, None);
        assert_eq!(decoded.metrics_interval, std::time::Duration::from_secs(5));
    }

    #[test]
    fn otlp_config_rejects_zero_metrics_interval() {
        let error = toml::from_str::<OtlpConfig>(
            r#"
            metrics_interval = 0
            "#,
        )
        .expect_err("zero metrics interval must be rejected");

        assert!(
            error
                .to_string()
                .contains("metrics_interval must be between 1 and 86400 seconds")
        );
    }

    #[test]
    fn otlp_config_rejects_excessive_metrics_interval() {
        let error = toml::from_str::<OtlpConfig>(
            r#"
            metrics_interval = 86401
            "#,
        )
        .expect_err("excessive metrics interval must be rejected");

        assert!(
            error
                .to_string()
                .contains("metrics_interval must be between 1 and 86400 seconds")
        );
    }
}

mod duration_seconds {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    const MAX_METRICS_INTERVAL_SECONDS: u64 = 86_400;

    pub fn serialize<S>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(value.as_secs())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let seconds = u64::deserialize(deserializer)?;
        if !(1..=MAX_METRICS_INTERVAL_SECONDS).contains(&seconds) {
            return Err(serde::de::Error::custom(format!(
                "metrics_interval must be between 1 and {MAX_METRICS_INTERVAL_SECONDS} seconds"
            )));
        }
        Ok(Duration::from_secs(seconds))
    }
}
