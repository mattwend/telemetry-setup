// SPDX-License-Identifier: MIT

use thiserror::Error;

/// Background task failure encountered during telemetry shutdown.
#[derive(Debug, Error)]
#[error("telemetry background task failed: {0}")]
pub struct BackgroundTaskError(#[source] pub tokio::task::JoinError);

/// Errors returned while configuring or installing telemetry.
#[derive(Debug, Error)]
pub enum TelemetryError {
    /// Trace provider construction failed.
    #[error("failed to initialize trace provider: {0}")]
    TraceProvider(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// Log provider construction failed.
    #[error("failed to initialize log provider: {0}")]
    LogProvider(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// Meter provider construction failed.
    #[error("failed to initialize meter provider: {0}")]
    MeterProvider(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// Tracing subscriber initialization failed.
    #[error("failed to initialize tracing subscriber: {0}")]
    Subscriber(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// Log-control server startup failed.
    #[error("failed to start log-control server: {0}")]
    LogControl(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// Journald output was requested without compiling the `journald` feature.
    #[error("the telemetry crate was built without the `journald` feature")]
    JournaldFeatureDisabled,
    /// Tokio metrics were requested without compiling the `tokio-metrics` feature.
    #[error("the telemetry crate was built without the `tokio-metrics` feature")]
    TokioMetricsFeatureDisabled,
    /// Tokio metrics were enabled without a runtime OTLP configuration.
    #[error("Tokio metrics require OTLP export to be configured")]
    TokioMetricsRequiresOtlp,
    /// An OTLP endpoint URL was invalid.
    #[error("invalid OTLP endpoint URL: {0}")]
    OtlpEndpoint(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// A tracked telemetry background task failed during shutdown.
    #[error("telemetry background task failed: {0}")]
    BackgroundTask(#[from] BackgroundTaskError),
}

impl TelemetryError {
    pub(crate) fn background_task(error: tokio::task::JoinError) -> Self {
        Self::BackgroundTask(BackgroundTaskError(error))
    }

    pub(crate) fn subscriber<E>(error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Subscriber(Box::new(error))
    }

    #[cfg(feature = "otlp")]
    pub(crate) fn trace_provider<E>(error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::TraceProvider(Box::new(error))
    }

    #[cfg(feature = "otlp")]
    pub(crate) fn log_provider<E>(error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::LogProvider(Box::new(error))
    }

    #[cfg(feature = "otlp")]
    pub(crate) fn meter_provider<E>(error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::MeterProvider(Box::new(error))
    }

    #[cfg(feature = "log-control")]
    pub(crate) fn log_control<E>(error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::LogControl(Box::new(error))
    }

    #[cfg(feature = "otlp")]
    pub(crate) fn otlp_endpoint<E>(error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::OtlpEndpoint(Box::new(error))
    }
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use super::{BackgroundTaskError, TelemetryError};

    #[derive(Debug)]
    struct TestError(&'static str);

    impl fmt::Display for TestError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl std::error::Error for TestError {}

    #[test]
    fn subscriber_error_formats_display_message() {
        let error = TelemetryError::subscriber(TestError("subscriber boom"));

        assert_eq!(
            error.to_string(),
            "failed to initialize tracing subscriber: subscriber boom"
        );
        assert!(matches!(error, TelemetryError::Subscriber(_)));
    }

    #[cfg(feature = "otlp")]
    #[test]
    fn provider_error_helpers_wrap_and_format_messages() {
        let trace = TelemetryError::trace_provider(TestError("trace boom"));
        let log = TelemetryError::log_provider(TestError("log boom"));
        let meter = TelemetryError::meter_provider(TestError("meter boom"));
        let endpoint = TelemetryError::otlp_endpoint(TestError("endpoint boom"));

        assert_eq!(
            trace.to_string(),
            "failed to initialize trace provider: trace boom"
        );
        assert_eq!(
            log.to_string(),
            "failed to initialize log provider: log boom"
        );
        assert_eq!(
            meter.to_string(),
            "failed to initialize meter provider: meter boom"
        );
        assert_eq!(
            endpoint.to_string(),
            "invalid OTLP endpoint URL: endpoint boom"
        );
        assert!(matches!(trace, TelemetryError::TraceProvider(_)));
        assert!(matches!(log, TelemetryError::LogProvider(_)));
        assert!(matches!(meter, TelemetryError::MeterProvider(_)));
        assert!(matches!(endpoint, TelemetryError::OtlpEndpoint(_)));
    }

    #[cfg(feature = "log-control")]
    #[test]
    fn log_control_error_helper_wraps_and_formats_message() {
        let error = TelemetryError::log_control(TestError("bind boom"));

        assert_eq!(
            error.to_string(),
            "failed to start log-control server: bind boom"
        );
        assert!(matches!(error, TelemetryError::LogControl(_)));
    }

    #[tokio::test]
    async fn background_task_error_formats_display_message() {
        let handle = tokio::spawn(async {
            panic!("background task failed");
        });
        let join_error = handle.await.unwrap_err();

        let error = TelemetryError::from(BackgroundTaskError(join_error));
        let message = error.to_string();

        assert!(message.contains("telemetry background task failed:"));
        assert!(matches!(error, TelemetryError::BackgroundTask(_)));
    }
}
