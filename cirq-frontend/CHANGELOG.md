# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/cramt/thevenin/releases/tag/cirq-frontend-v0.1.0) - 2026-04-26

### Added

- add module hierarchy, control blocks, and code "lang" syntax
- add coupled_line block syntax and expand test coverage
- add user-defined functions and initial conditions (gaps 3.4, 3.6)
- add behavioral source support (gap 2.2)
- add import file resolution (gap 3.5)
- add save targets support (gap 3.2)
- add simulation options and temperature support (gaps 3.1, 3.3)
- implement subcircuit/module flattening (gap 2.1)
- close all Tier 1 gaps and key Tier 2/3 gaps (run 09 batch 1)
- implement full Cirq frontend pipeline (runs 04-09)
- scaffold Cirq language, grammar, IR crates, and spec

### Fixed

- add version fields to all path deps and enable publishing
- close medium pipeline gaps (M1-M5, M9)
- close three critical pipeline gaps (ic, uic, tline terminals)

### Other

- cargo fmt
- cargo fmt
- make cirq-grammar a publishable workspace crate
