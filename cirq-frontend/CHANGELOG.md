# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0](https://github.com/cramt/thevenin/compare/cirq-frontend-v0.1.0...cirq-frontend-v0.5.0) - 2026-07-12

### Added

- *(cirq)* complete Goal B — language registry, native URC, port arrays
- *(cirq)* native four / fft analysis blocks
- *(cirq)* compile-time if/elseif/else conditionals
- *(cirq)* native measure expression syntax (= expr form)
- *(cirq)* ternary operator + sim-context constants (temper, time, freq, hertz)
- *(fourier)* .four (DFT harmonics + THD) and .fft (windowed FFT) post-processing of .tran
- *(vdmos)* port vertical-DMOS power MOSFET from ngspice
- *(tline)* add T element (ideal lossless transmission line)
- *(switches)* add S/W voltage- and current-controlled switches
- *(cirq)* native measure block syntax
- *(control)* typed control AST in IR; executor consumes parsed form
- *(meas)* typed MeasureExpr in IR, add PARAM=/LAST/TD support
- *(cirq-frontend)* module parameter overrides at instantiation
- *(control)* retire SimContext::netlist cache; dispatch .control via Circuit
- *(cirq-frontend)* netlist_analysis_to_ir converter
- delete --legacy CLI flag and deprecated Netlist control entry points
- *(mna)* sens Netlist-free; multi-temp on Circuit; SimContext Netlist private
- *(mna)* direct IR -> MNA path supports diodes
- *(mna)* assemble MnaSystem directly from Cirq IR (linear subset)
- support BSIM4 model binning through Cirq IR round-trip
- route ngspice harness through Cirq IR via THEVENIN_VIA_CIRQ
- wire .ic/UIC, .nodeset, .meas, multi-temp, and mutual coupling
- close remaining SPICE import gaps — nodeset, measure, multi-temp, arithmetic expressions
- resolve SPICE parametric expressions and sanitize numeric node names
- add export blocks and named imports to Cirq language

### Fixed

- guard two stack-overflow DoS paths reachable from untrusted input
- populate netlist.source on Cirq IR -> Netlist round-trip
- restore Expr::Brace on Cirq IR -> Netlist round-trip

### Other

- *(release)* unify workspace to 0.5.0 for first crates.io release
- native ModelParams::from_ir, drop convert_model on Circuit device loads
- polish docs.rs landing pages, examples, and metadata
- *(control)* parse .control analyses straight to IR
- *(release)* README pass, CHANGELOG, api-stability + #[non_exhaustive] on public enums
- Merge doc archival
- archive cirq-plan and migration plans
- demote Netlist-shaped simulator APIs to pub(crate)
- *(mna)* add MNA-on-IR pivot plan; promote to_netlist shims to pub
- *(control)* deprecate Netlist-shaped .control entry points (Stage 4 / Phase D)

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
