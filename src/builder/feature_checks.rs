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

#[cfg(test)]
mod tests {
    use super::{reject_journald_without_feature, reject_tokio_metrics_without_feature};

    #[test]
    fn disabled_optional_features_are_accepted() {
        assert!(reject_journald_without_feature(false).is_ok());
        assert!(reject_tokio_metrics_without_feature(false).is_ok());
    }
}
