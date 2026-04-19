// SPDX-License-Identifier: MIT

use telemetry_setup::TelemetryBuilder;

fn main() -> Result<(), telemetry_setup::TelemetryError> {
    let _telemetry = TelemetryBuilder::new("controller").init()?;

    tracing::info!("started");
    Ok(())
}
