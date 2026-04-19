// SPDX-License-Identifier: MIT

use std::sync::Arc;

pub(crate) type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Resolves the local filter from the configured environment variable or fallback string.
///
/// # Arguments
///
/// * `env_var_name` - Optional environment variable name to query first.
/// * `env_lookup` - Lookup callback used to read environment values.
/// * `stdout_filter` - Optional explicit fallback filter.
///
/// # Returns
///
/// The environment-provided filter, explicit fallback filter, or `info` default.
pub(crate) fn resolve_stdout_filter(
    env_var_name: &Option<String>,
    env_lookup: &EnvLookup,
    stdout_filter: &Option<String>,
) -> String {
    if let Some(env_var_name) = env_var_name
        && let Some(filter) = env_lookup(env_var_name)
    {
        return filter;
    }

    stdout_filter.clone().unwrap_or_else(|| "info".to_string())
}
