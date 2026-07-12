# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0](https://github.com/cramt/thevenin/compare/cirq-spice-import-v0.1.0...cirq-spice-import-v0.5.0) - 2026-07-12

### Added

- complete Goal C — SPICE importer accepts arbitrary netlists
- *(cirq)* complete Goal B — language registry, native URC, port arrays
- *(expr)* short-circuit ternary `cond ? then : else`
- *(urc)* SPICE U element + URC model with importer-side expansion
- *(fourier)* .four (DFT harmonics + THD) and .fft (windowed FFT) post-processing of .tran
- *(vdmos)* port vertical-DMOS power MOSFET from ngspice
- *(tline)* add T element (ideal lossless transmission line)
- *(importer)* resolve .include and .lib file I/O with search path, cycle detection, and Latin-1 fallback
- *(importer)* MOS3 warning, line continuation, .step diagnostic, TEMPER + ternary in brace eval
- *(switches)* add S/W voltage- and current-controlled switches
- *(importer)* R/L/C tc=, option scale, .width, graceful unknown directives
- *(csparam)* support .csparam directive with control-scope seeding
- *(control)* typed control AST in IR; executor consumes parsed form
- *(meas)* typed MeasureExpr in IR, add PARAM=/LAST/TD support
- *(mna)* sens Netlist-free; multi-temp on Circuit; SimContext Netlist private
- support BSIM4 model binning through Cirq IR round-trip
- route ngspice harness through Cirq IR via THEVENIN_VIA_CIRQ
- close remaining SPICE import gaps — nodeset, measure, multi-temp, arithmetic expressions
- resolve SPICE parametric expressions and sanitize numeric node names

### Fixed

- *(urc)* use __urc__ sigil for synthetic nodes to avoid user collision

### Other

- *(release)* unify workspace to 0.5.0 for first crates.io release
- polish docs.rs landing pages, examples, and metadata
- *(release)* README pass, CHANGELOG, api-stability + #[non_exhaustive] on public enums
- Merge csparam support
- demote Netlist-shaped simulator APIs to pub(crate)

## [0.1.0](https://github.com/cramt/thevenin/releases/tag/cirq-spice-import-v0.1.0) - 2026-04-26

### Added

- add module hierarchy, control blocks, and code "lang" syntax
- add coupled_line block syntax and expand test coverage
- add user-defined functions and initial conditions (gaps 3.4, 3.6)
- add behavioral source support (gap 2.2)
- add save targets support (gap 3.2)
- add simulation options and temperature support (gaps 3.1, 3.3)
- close all Tier 1 gaps and key Tier 2/3 gaps (run 09 batch 1)
- implement full Cirq frontend pipeline (runs 04-09)
- scaffold Cirq language, grammar, IR crates, and spec

### Fixed

- add version fields to all path deps and enable publishing
- close medium pipeline gaps (M1-M5, M9)
- close three critical pipeline gaps (ic, uic, tline terminals)

### Other

- cargo fmt
