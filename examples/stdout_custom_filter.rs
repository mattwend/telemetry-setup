// SPDX-License-Identifier: MIT

use telemetry_setup::TelemetryBuilder;

fn main() -> Result<(), telemetry_setup::TelemetryError> {
    let _telemetry = TelemetryBuilder::new("controller")
        .with_stdout_filter("controller=debug,telemetry=info")
        .init()?;

    tracing::debug!("debug logging enabled for controller");
    // Keep `_telemetry` alive until process exit. In a real async service,
    // call `shutdown().await` during teardown for graceful shutdown.
    Ok(())
}
