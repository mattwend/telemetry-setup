// SPDX-License-Identifier: MIT

use std::collections::HashMap;

use url::Url;

use super::config::OtlpConfig;
use crate::error::TelemetryError;

/// Returns the per-signal OTLP endpoint derived from `base_url` and `suffix`.
pub(super) fn signal_endpoint(base_url: &str, suffix: &str) -> Result<String, TelemetryError> {
    let parsed = Url::parse(base_url).map_err(TelemetryError::otlp_endpoint)?;
    if parsed.path().ends_with(suffix) {
        return Ok(base_url.to_string());
    }

    let trimmed = base_url.trim_end_matches('/');
    Ok(format!("{trimmed}/{suffix}"))
}

/// Builds export headers by merging common and signal-specific header configuration.
///
/// # Arguments
///
/// * `config` - OTLP configuration containing headers common to every signal.
/// * `signal_specific` - Headers that apply only to the signal being exported.
///
/// # Returns
///
/// A header map where signal-specific entries override common entries with the
/// same name.
pub(super) fn signal_headers(
    config: &OtlpConfig,
    signal_specific: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut headers = config.headers.common.clone();
    headers.extend(
        signal_specific
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    );
    headers
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{signal_endpoint, signal_headers};
    use crate::error::TelemetryError;
    use crate::otlp::{OtlpConfig, OtlpHeadersConfig};

    #[test]
    fn signal_endpoint_appends_suffix_once() {
        assert_eq!(
            signal_endpoint("http://localhost:4318", "v1/traces").unwrap(),
            "http://localhost:4318/v1/traces"
        );
        assert_eq!(
            signal_endpoint("http://localhost:4318/v1/traces", "v1/traces").unwrap(),
            "http://localhost:4318/v1/traces"
        );
    }

    #[test]
    fn signal_endpoint_reports_invalid_urls_as_endpoint_errors() {
        let error = signal_endpoint("://not-a-url", "v1/traces").unwrap_err();
        assert!(matches!(error, TelemetryError::OtlpEndpoint(_)));
    }

    #[test]
    fn signal_headers_merge_common_and_signal_specific_headers() {
        let headers = signal_headers(
            &OtlpConfig {
                headers: OtlpHeadersConfig {
                    common: HashMap::from([
                        ("X-Greptime-DB-Name".to_string(), "edge".to_string()),
                        ("x-shared".to_string(), "common".to_string()),
                    ]),
                    ..OtlpHeadersConfig::default()
                },
                ..OtlpConfig::default()
            },
            &HashMap::from([
                (
                    "x-greptime-pipeline-name".to_string(),
                    "greptime_trace_v1".to_string(),
                ),
                ("x-shared".to_string(), "signal".to_string()),
            ]),
        );

        assert_eq!(headers.get("X-Greptime-DB-Name"), Some(&"edge".to_string()));
        assert_eq!(
            headers.get("x-greptime-pipeline-name"),
            Some(&"greptime_trace_v1".to_string())
        );
        assert_eq!(headers.get("x-shared"), Some(&"signal".to_string()));
    }
}
