# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0](https://github.com/cramt/thevenin/compare/cirq-grammar-v0.1.0...cirq-grammar-v0.5.0) - 2026-07-12

### Added

- *(cirq)* complete Goal B — language registry, native URC, port arrays
- *(cirq)* compile-time if/elseif/else conditionals
- *(cirq)* native measure expression syntax (= expr form)
- *(grammar)* tree-sitter-control + scanner finishes bash/js/control 1.0
- *(cirq)* ternary operator + sim-context constants (temper, time, freq, hertz)
- *(cirq-grammar)* inject embedded languages in code blocks
- *(cirq)* native measure block syntax
- add export blocks and named imports to Cirq language

### Other

- *(release)* unify workspace to 0.5.0 for first crates.io release
- polish docs.rs landing pages, examples, and metadata
- *(cirq-grammar)* scaffold multi-language tree-sitter bindings

## [0.1.0](https://github.com/cramt/thevenin/releases/tag/cirq-grammar-v0.1.0) - 2026-04-26

### Added

- add module hierarchy, control blocks, and code "lang" syntax
- add coupled_line block syntax and expand test coverage
- add user-defined functions and initial conditions (gaps 3.4, 3.6)
- add save targets support (gap 3.2)
- add simulation options and temperature support (gaps 3.1, 3.3)
- complete tree-sitter Cirq grammar with queries and tests
- scaffold Cirq language, grammar, IR crates, and spec

### Other

- make cirq-grammar a publishable workspace crate
