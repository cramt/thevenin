# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/cramt/thevenin/releases/tag/thevenin-control-v0.1.0) - 2026-04-26

### Added

- add coupled_line block syntax and expand test coverage
- alter command, B-source tc1/tc2, complex vecexpr, pole() function
- *(control)* implement .control block interpreter

### Fixed

- add version fields to all path deps and enable publishing
- close three critical pipeline gaps (ic, uic, tline terminals)
- use total_cmp for NaN safety, error on unimplemented alter, fix stale comment
- un-ignore bugs-2 and test-noise-2 via vector indexing and flicker noise
- un-ignore ac-zero and if-elseif tests via two targeted fixes

### Other

- implement AC sensitivity analysis, un-ignore sens-ac-1/2
- fork netlists at analysis boundaries — one Netlist per analysis
- ergonomic public API with VectorData enum, Index impls, and simulate() dispatcher
- cargo fmt
- format
