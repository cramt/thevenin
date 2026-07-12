# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0](https://github.com/cramt/thevenin/compare/thevenin-control-v0.1.0...thevenin-control-v0.5.0) - 2026-07-12

### Added

- update ngspice submodule (1a621eb -> 037b657), pass its new convergence test
- complete Goal C — SPICE importer accepts arbitrary netlists
- *(cirq)* complete Goal B — language registry, native URC, port arrays
- *(control)* source command, measure command, vector arithmetic in print
- *(fourier)* .four (DFT harmonics + THD) and .fft (windowed FFT) post-processing of .tran
- *(output)* ngspice raw file format (binary + ASCII) + CSV + write command
- *(control)* add while, repeat, save commands
- *(csparam)* support .csparam directive with control-scope seeding
- *(control)* typed control AST in IR; executor consumes parsed form
- *(control)* retire SimContext::netlist cache; dispatch .control via Circuit
- *(control)* implement stop/resume + unignore resume-1.cir (101/0/6)
- delete --legacy CLI flag and deprecated Netlist control entry points
- *(mna)* sens Netlist-free; multi-temp on Circuit; SimContext Netlist private
- *(control)* alter mutates Circuit.elements / Circuit.models (Stage 4 / Phase C)
- *(control)* SimContext owns the driving Circuit (Stage 4 / Phase B)
- *(control)* add IR-shaped .control entry point (Stage 4 / Phase A)

### Fixed

- .control 'run' now executes netlist's declared analysis

### Other

- *(release)* unify workspace to 0.5.0 for first crates.io release
- rustdoc polish, README refresh, getting-started guide
- polish docs.rs landing pages, examples, and metadata
- *(control)* parse .control analyses straight to IR
- *(release)* README pass, CHANGELOG, api-stability + #[non_exhaustive] on public enums
- Merge .control while/repeat/save commands
- *(control)* @device[param] lookup walks Circuit instead of cached Netlist
- *(control)* deprecate Netlist-shaped .control entry points (Stage 4 / Phase D)

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
