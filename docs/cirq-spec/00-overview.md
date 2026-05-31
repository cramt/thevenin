# Cirq Language Specification — Overview

**Version:** 1.0
**Status:** Stable — targeted for the 1.0 release

## What is Cirq?

Cirq is a circuit description language designed to replace SPICE netlists as the primary input format for the Thevenin simulator. It provides:

- A clean, modern syntax free of SPICE's historical quirks
- Hierarchical circuit composition with explicit module boundaries
- Strong typing for parameters and values
- SI unit literals as first-class syntax
- Named nets instead of positional node numbers
- Explicit analysis configuration

## Design Principles

1. **Readable over terse.** SPICE optimized for 80-column punch cards. Cirq optimizes for humans reading code in 2025.
2. **Explicit over implicit.** No magic first-character element identification. No implicit ground node rules.
3. **Composable.** Subcircuits are proper modules with typed ports, not textual includes.
4. **Compatible.** Every valid SPICE netlist can be mechanically translated to Cirq (via import tooling). Cirq can express everything SPICE can.
5. **Incremental.** You can mix SPICE and Cirq in the same project during migration.

## File Extension

`.cirq`

## Encoding

UTF-8. No BOM.

## Spec Structure

| File | Content |
|------|---------|
| `01-lexical.md` | Lexical structure, tokens, comments, whitespace |
| `02-types-and-values.md` | Type system, SI units, numeric literals |
| `03-circuits-and-modules.md` | Circuit definitions, modules, ports, instantiation |
| `04-elements.md` | Built-in element types (resistor, capacitor, source, etc.) |
| `05-parameters.md` | Parameter declarations, expressions, defaults |
| `06-models.md` | Device model definitions and references |
| `07-analysis.md` | Analysis commands (DC, AC, transient, etc.) |
| `08-expressions.md` | Expression syntax and evaluation rules |
| `09-attributes.md` | Metadata annotations |
| `10-spice-compat.md` | SPICE compatibility notes and mapping rules |
