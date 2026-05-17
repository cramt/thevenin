# Old-Path Retirement Checklist

Items to address when gradually retiring SPICE-native interfaces in favor of
the Cirq IR pipeline. Each item represents a piece of the old path that can
eventually be replaced or removed once the Cirq IR path fully subsumes it.

## Primary Interface

- [ ] **`thevenin_types::Netlist` as the primary input interface**
  The simulator currently requires a `Netlist` to run. As the Cirq IR matures,
  the simulator should accept `cirq_ir::Circuit` directly. The Netlist type
  would become an internal intermediate or compatibility layer.
  *Depends on:* Stage 4 of the adoption plan.

## Naming Conventions

- [ ] **SPICE-specific element prefix naming (R1, C2, V3, etc.)**
  The `to_netlist` adapter adds SPICE prefix letters to element names
  (`spice_name()` in `cirq-frontend/src/to_netlist.rs`). Once the simulator
  consumes IR directly, element names no longer need the prefix letter
  convention.
  *Depends on:* simulator accepting IR natively.

## Parameter Representation

- [ ] **`thevenin_types::Expr` for parameter values**
  The Netlist uses `Expr` (Num/Param/Brace variants) for all parameter values.
  The Cirq IR uses `Value` (Real/Integer/Bool/String), which is more precise
  and already constant-folded. The `Expr` type can be retired once the
  simulator reads `Value` directly.
  *Depends on:* Stage 4.

- [x] **Passive element param naming mismatch**
  The SPICE importer normalizes passive values to `"value"`. The Netlist
  adapter reads them correctly. Round-trip tests confirm this works.
  *Resolved in:* Stage 2.

## Analysis and Control

- [~] **`.control` block interpreter dependency on SPICE Netlist shape**
  The `thevenin-control` crate interprets `.control` blocks that reference
  SPICE-shaped Items and Analysis variants. A Cirq-native control flow
  mechanism (or an adapter that presents IR-based analysis to the interpreter)
  is needed before the Netlist-dependent interpreter can retire.
  *Depends on:* Stage 4.
  **Phase A done:** `execute_control_block_ir(&Circuit)` /
  `has_control_block_ir(&Circuit)` are the canonical entry points; CLI and
  harness route `.control` through them. The interpreter's internals still
  consume `Netlist` — Phases B and C of the adoption plan will lift
  `SimContext` and `alter` onto the IR shape.

## Simulation Path

- [x] **Direct SPICE parser -> simulate path**
  The CLI now routes SPICE files through IR by default:
  `SPICE source -> import_spice() -> Circuit -> circuit_to_netlists() -> simulate()`.
  The direct path remains available via `--legacy` flag.
  11 round-trip tests validate bit-identical results.
  *Resolved in:* Stage 3.

## Model Representation

- [ ] **SPICE-shaped model kind strings ("NPN", "NMOS", "D", etc.)**
  The `thevenin_types::ModelDef` stores the model kind as a plain `String`.
  The Cirq IR uses a typed `DeviceType` enum, which prevents typos and enables
  exhaustive matching. The string-based kind can be retired once the simulator
  reads `DeviceType` directly.
  *Depends on:* Stage 4.

## Hierarchy

- [x] **Ad-hoc subcircuit flattening in thevenin**
  The SPICE importer calls `thevenin::subckt::flatten_netlist()` before
  importing to IR. Subcircuit round-trip test confirms correctness.
  The Netlist-level flattener is still used but is invoked as part of the
  IR import pipeline rather than separately.
  *Resolved in:* Stage 3.

## Unsupported Constructs

- [x] **Behavioral sources (B elements)**
  Supported in the SPICE importer with V=/I= parsing. Verified by unit and
  integration tests.
  *Resolved in:* Stage 3.

- [x] **XSPICE code models (A elements)**
  Supported in the SPICE importer with scalar/array connections. Verified by
  unit tests.
  *Resolved in:* Stage 3.

- [x] **CPL (coupled multiconductor transmission line)**
  Supported in the SPICE importer with variable-width connections. Verified by
  unit tests.
  *Resolved in:* Stage 3.

## How to Use This Checklist

Each item should be checked off only when:

1. The replacement path is implemented and tested.
2. No test regressions occur when the old path is disabled.
3. The change is merged and the deprecation is documented.

Items can be tackled in any order within their stage dependency, but items
marked as depending on a later stage should not be started until that stage's
entry criteria are met.
