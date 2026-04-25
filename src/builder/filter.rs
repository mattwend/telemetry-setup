// SPDX-License-Identifier: MIT

/// Resolves the local filter from the configured environment variable or fallback string.
///
/// # Arguments
///
/// * `env_var_name` - Optional environment variable name to query first.
/// * `stdout_filter` - Optional explicit fallback filter.
///
/// # Returns
///
/// The environment-provided filter, explicit fallback filter, or `info` default.
pub(crate) fn resolve_stdout_filter(
    env_var_name: &Option<String>,
    stdout_filter: &Option<String>,
) -> String {
    if let Some(env_var_name) = env_var_name
        && let Ok(filter) = std::env::var(env_var_name)
    {
        return filter;
    }

    stdout_filter.clone().unwrap_or_else(|| "info".to_string())
}

/// Resolves the local filter using a caller-provided lookup function.
///
/// # Arguments
///
/// * `env_var_name` - Optional environment variable name to query first.
/// * `lookup` - Function used to resolve environment values for tests.
/// * `stdout_filter` - Optional explicit fallback filter.
///
/// # Returns
///
/// The lookup-provided filter, explicit fallback filter, or `info` default.
#[cfg(test)]
pub(crate) fn resolve_stdout_filter_with_lookup(
    env_var_name: &Option<String>,
    lookup: impl Fn(&str) -> Option<String>,
    stdout_filter: &Option<String>,
) -> String {
    if let Some(env_var_name) = env_var_name
        && let Some(filter) = lookup(env_var_name)
    {
        return filter;
    }

    stdout_filter.clone().unwrap_or_else(|| "info".to_string())
}
