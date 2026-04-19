// SPDX-License-Identifier: MIT

use telemetry_setup::{OtlpConfig, TelemetryBuilder};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let otlp_config: OtlpConfig = toml::from_str(include_str!("greptime_otlp.toml"))?;

    let telemetry = TelemetryBuilder::new("controller")
        .with_otlp_config(otlp_config)
        .init()?;

    tracing::info!("started");
    telemetry.shutdown().await?;
    Ok(())
}
