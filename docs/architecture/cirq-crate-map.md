# Cirq Crate Map

```text
                    ┌──────────────────┐
                    │  Cirq source     │
                    │  (.cirq file)    │
                    └────────┬─────────┘
                             │
                    ┌────────▼─────────┐
                    │  cirq-grammar    │   Tree-sitter grammar (JS project, not Rust)
                    │  (CST)           │   generates C parser consumed by Rust via tree-sitter crate
                    └────────┬─────────┘
                             │
                    ┌────────▼─────────┐
                    │  cirq-ast        │   Rust crate
                    │  CST → AST       │   Source-faithful AST with spans
                    └────────┬─────────┘
                             │
                    ┌────────▼─────────┐
                    │  cirq-ir         │   Rust crate — THE SEMANTIC CENTER
                    │  AST → IR        │   Resolved, validated, canonical
                    └────────┬─────────┘
                             │
               ┌─────────────┼─────────────┐
               │             │             │
    ┌──────────▼───┐  ┌──────▼──────┐  ┌───▼──────────────┐
    │ cirq-frontend│  │ future      │  │ cirq-spice-import│
    │ (pipeline)   │  │ tooling     │  │ Netlist → IR     │
    └──────────┬───┘  │ (lint, fmt) │  └───┬──────────────┘
               │      └─────────────┘      │
               │                           │
               └─────────┬─────────────────┘
                         │
                         │  IR → Netlist adapter
                         │
                ┌────────▼─────────┐
                │  thevenin-types  │   existing — Netlist, SimResult
                └────────┬─────────┘
                         │
                ┌────────▼─────────┐
                │  thevenin        │   existing — solver, device models
                └────────┬─────────┘
                         │
                    ┌────▼────┐
                    │ SimResult│
                    └─────────┘
```

## Dependency Graph (Rust crates)

```text
cirq-ast           → tree-sitter (C parser binding)
cirq-ir            → cirq-ast (AST types)
cirq-frontend      → cirq-ast, cirq-ir, thevenin-types
cirq-spice-import  → cirq-ir, thevenin-types
thevenin-cli       → cirq-frontend (eventually)
```
