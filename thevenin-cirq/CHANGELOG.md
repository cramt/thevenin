# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0](https://github.com/cramt/thevenin/compare/thevenin-cirq-v0.1.0...thevenin-cirq-v0.5.0) - 2026-07-12

### Added

- *(cirq)* native four / fft analysis blocks
- *(cirq)* native measure expression syntax (= expr form)
- *(control)* retire SimContext::netlist cache; dispatch .control via Circuit
- *(mna)* sens Netlist-free; multi-temp on Circuit; SimContext Netlist private
- *(mna)* add Circuit-input top-level simulate dispatcher; CLI uses it
- *(mna)* route DC/AC/TRAN through mna_ir on Circuit-input path
- *(mna)* direct IR -> MNA path supports LTRA/TXL/CPL/XSPICE (full coverage)
- *(mna)* direct IR -> MNA path supports BehavioralSource + Coupling
- *(mna)* direct IR -> MNA path supports JFET / MESA family
- *(mna)* direct IR -> MNA path supports MOSFET family
- *(mna)* direct IR -> MNA path supports BJT (level 1 + VBIC)
- *(mna)* direct IR -> MNA path supports diodes
- *(mna)* assemble MnaSystem directly from Cirq IR (linear subset)
- *(circuit)* extend direct IR -> MNA path to dependent sources
- graduate Circuit-input simulation API into thevenin
- *(thevenin-cirq)* add SPICE-source convenience entry points
- add thevenin-cirq crate as Stage 4 simulation surface
- scaffold Cirq language, grammar, IR crates, and spec
- *(cli)* add SPICE-to-CirQ converter command
- *(control)* implement .control block interpreter

### Fixed

- *(cirq)* harden CirQ parser, fix spec divergence, add digital domain inference

### Other

- *(release)* unify workspace to 0.5.0 for first crates.io release
- polish docs.rs landing pages, examples, and metadata
- demote Netlist-shaped simulator APIs to pub(crate)
- tighten tolerance overrides for 5 harness tests
- fork netlists at analysis boundaries — one Netlist per analysis
- replace serde with facet for deserialization
