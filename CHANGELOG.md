# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project is preparing for semantic version tags.

## [Unreleased]

### Added

- `TelemetryBuilder::with_late_configuration` and
  `TelemetryGuard::apply_late_configuration`: apply a stdout filter and start
  OTLP export once after `init()`, without a second subscriber.

## 0.1.0 - 2026-04-26

Initial release.
