// SPDX-License-Identifier: MIT

use crate::error::TelemetryError;

/// Rejects journald usage when the crate was built without journald support.
///
/// # Arguments
///
/// * `enabled` - Whether the builder requested journald output.
///
/// # Returns
///
/// `Ok(())` when the request is supported.
///
/// # Errors
///
/// Returns [`TelemetryError::JournaldFeatureDisabled`] when journald was requested
/// without compiling the `journald` feature.
pub(crate) fn reject_journald_without_feature(enabled: bool) -> Result<(), TelemetryError> {
    #[cfg(not(feature = "journald"))]
    if enabled {
        return Err(TelemetryError::JournaldFeatureDisabled);
    }

    let _ = enabled;
    Ok(())
}

/// Rejects Tokio metrics when the crate was built without Tokio metrics support.
///
/// # Arguments
///
/// * `enabled` - Whether the builder requested Tokio runtime metric collection.
///
/// # Returns
///
/// `Ok(())` when the request is supported.
///
/// # Errors
///
/// Returns [`TelemetryError::TokioMetricsFeatureDisabled`] when metrics were requested
/// without compiling the `tokio-metrics` feature.
pub(crate) fn reject_tokio_metrics_without_feature(enabled: bool) -> Result<(), TelemetryError> {
    #[cfg(not(feature = "tokio-metrics"))]
    if enabled {
        return Err(TelemetryError::TokioMetricsFeatureDisabled);
    }

    let _ = enabled;
    Ok(())
}

/// Rejects Tokio metrics when OTLP export cannot be configured.
///
/// # Arguments
///
/// * `enabled` - Whether the builder requested Tokio runtime metric collection.
/// * `otlp_configured` - Whether the builder has an OTLP configuration.
///
/// # Returns
///
/// `Ok(())` when Tokio metrics are disabled or OTLP is configured.
///
/// # Errors
///
/// Returns [`TelemetryError::TokioMetricsRequiresOtlp`] when Tokio metrics are enabled
/// without a runtime OTLP configuration.
#[cfg(feature = "tokio-metrics")]
pub(crate) fn reject_tokio_metrics_without_otlp_config(
    enabled: bool,
    otlp_configured: bool,
) -> Result<(), TelemetryError> {
    if enabled && !otlp_configured {
        return Err(TelemetryError::TokioMetricsRequiresOtlp);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{reject_journald_without_feature, reject_tokio_metrics_without_feature};

    #[test]
    fn disabled_optional_features_are_accepted() {
        assert!(reject_journald_without_feature(false).is_ok());
        assert!(reject_tokio_metrics_without_feature(false).is_ok());
    }

    #[cfg(feature = "tokio-metrics")]
    #[test]
    fn tokio_metrics_with_otlp_config_is_accepted() {
        assert!(super::reject_tokio_metrics_without_otlp_config(true, true).is_ok());
    }
}
