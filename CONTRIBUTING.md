# Contributing

Thanks for contributing to `telemetry-setup`.

## Before you start

- Keep changes focused and submit them in logical commits
- Follow conventional commits: `type(scope): description`
- Include a commit body explaining *why* the change is needed
- When behavior changes materially, update the durable documentation in the owning component
- If a change introduces new system-tool requirements, record them in `README.md` prerequisites

## Development workflow

Before opening a pull request, run:

1. Format the code:
   ```bash
   cargo fmt
   ```
2. Lint the code:
   ```bash
   cargo clippy --workspace --all-features --all-targets -- -D warnings
   ```
3. Run the test suite:
   ```bash
   cargo test --workspace --all-features
   ```
4. Build the documentation to catch rustdoc issues:
   ```bash
   cargo doc --workspace --all-features --no-deps
   ```

## Rust code guidelines

- Use `tracing`, not `println!`
- All fallible functions should return `Result<T, E>`
- Use crate-local `thiserror` enums for error types
- Propagate errors with `?`
- Do not use `unwrap()` or `expect()` in production code
  - Exception: `#[cfg(test)]`
- Do not silently swallow errors; log or propagate them
- Document functions, including their arguments and return values

## Project notes

- The crate is a shared telemetry setup library for Rust services
- Optional functionality is feature-gated (`otlp`, `journald`, `log-control`, `tokio-metrics`)
- Update examples or README usage snippets when public APIs or expected setup flows change
