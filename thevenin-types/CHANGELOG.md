# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0](https://github.com/cramt/thevenin/compare/thevenin-types-v0.3.0...thevenin-types-v0.5.0) - 2026-07-12

### Added

- update ngspice submodule (1a621eb -> 037b657), pass its new convergence test
- complete Goal C — SPICE importer accepts arbitrary netlists
- *(urc)* SPICE U element + URC model with importer-side expansion
- *(fourier)* .four (DFT harmonics + THD) and .fft (windowed FFT) post-processing of .tran
- *(tline)* add T element (ideal lossless transmission line)
- *(importer)* MOS3 warning, line continuation, .step diagnostic, TEMPER + ternary in brace eval
- *(switches)* add S/W voltage- and current-controlled switches
- *(importer)* R/L/C tc=, option scale, .width, graceful unknown directives
- *(csparam)* support .csparam directive with control-scope seeding
- close remaining SPICE import gaps — nodeset, measure, multi-temp, arithmetic expressions

### Fixed

- un-ignore hfet/inverter with corrected reference from patched ngspice

### Other

- *(release)* unify workspace to 0.5.0 for first crates.io release
- *(thevenin)* neutral ModelParams boundary, Expr-free device layer
- polish docs.rs landing pages, examples, and metadata

## [0.3.0](https://github.com/cramt/thevenin/compare/thevenin-types-v0.2.0...thevenin-types-v0.3.0) - 2026-04-26

### Added

- behavioral resistor R n+ n- r={expr} to B-source conversion
- *(control)* implement .control block interpreter
- *(parser,output)* support non-parenthesized PULSE and arithmetic print expressions

### Fixed

- close three critical pipeline gaps (ic, uic, tline terminals)
- add new_gmin stepping, source stepping InitJct, and SINE parser
- un-ignore bugs-2 and test-noise-2 via vector indexing and flicker noise
- un-ignore ac-zero and if-elseif tests via two targeted fixes

### Other

- fix 24 broken intra-doc links causing cargo doc warnings
- fork netlists at analysis boundaries — one Netlist per analysis
- ergonomic public API with VectorData enum, Index impls, and simulate() dispatcher
- format

## [0.2.0](https://github.com/cramt/thevenin/compare/thevenin-types-v0.1.0...thevenin-types-v0.2.0) - 2026-03-22

### Added

- *(bjt)* implement OFF keyword and MODEINITJCT initial guess; improve output filter
- *(output,mna)* netlist echo, RSH resistor model, un-ignore 2 tests
- US-065 - XSPICE A-element parser with bracketed port groups
- US-038 - Regression tests for models, subcircuit processing, and misc
- US-037 - Regression tests for parser, func, and lib-processing
- US-036 - TXL and CPL transmission line models
- US-035 - Lossy transmission line model (LTRA)
- US-032 MESA FET model (Ytterdal/Lee/Shur/Fjeldly GaAs MESFET)

### Fixed

- add missing version specs and changelogs for release-plz
- *(parser,mosfet)* tokenizer spaced key=value and MOSFET reversed-mode ceq_d sign
- *(simulate)* correct LTRA+MOSFET DC OP convergence
- *(parser,ac)* fix continuation lines after star-comments + AC fixes

### Other

- *(wasm)* switch wasm tests from browser to Node runner
