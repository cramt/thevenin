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

- [ ] **Passive element param naming mismatch**
  The SPICE importer stores values as `"resistance"`, `"capacitance"`,
  `"inductance"`. The Netlist adapter expects `"value"`. This gap needs to
  be resolved (normalize in one direction) before the round-trip path is
  fully reliable.
  *Depends on:* Stage 2.

## Analysis and Control

- [ ] **`.control` block interpreter dependency on SPICE Netlist shape**
  The `thevenin-control` crate interprets `.control` blocks that reference
  SPICE-shaped Items and Analysis variants. A Cirq-native control flow
  mechanism (or an adapter that presents IR-based analysis to the interpreter)
  is needed before the Netlist-dependent interpreter can retire.
  *Depends on:* Stage 4.

## Simulation Path

- [ ] **Direct SPICE parser -> simulate path**
  Currently: `SPICE source -> Netlist::parse() -> simulate()`.
  Target: `SPICE source -> import_spice() -> Circuit -> simulate_ir()`.
  The direct path remains available as a fallback through Stage 3. It can be
  deprecated once the IR path passes the full regression suite.
  *Depends on:* Stage 3 exit criteria.

## Model Representation

- [ ] **SPICE-shaped model kind strings ("NPN", "NMOS", "D", etc.)**
  The `thevenin_types::ModelDef` stores the model kind as a plain `String`.
  The Cirq IR uses a typed `DeviceType` enum, which prevents typos and enables
  exhaustive matching. The string-based kind can be retired once the simulator
  reads `DeviceType` directly.
  *Depends on:* Stage 4.

## Hierarchy

- [ ] **Ad-hoc subcircuit flattening in thevenin**
  The `thevenin::subckt::flatten_netlist()` function flattens `.subckt`
  definitions at the Netlist level. The Cirq IR should handle hierarchy
  resolution at the IR level (module inlining, port binding). Once IR-level
  flattening is complete, the Netlist-level flattener can be retired.
  *Depends on:* module instantiation lowering in `cirq-frontend` + Stage 3.

## Unsupported Constructs

- [ ] **Behavioral sources (B elements)**
  The SPICE importer currently returns `UnsupportedElement` for behavioral
  sources. These need an IR representation before the import path is complete.
  *Depends on:* IR extension + importer update.

- [ ] **XSPICE code models (A elements)**
  Same as behavioral sources -- currently unsupported in the importer.
  *Depends on:* XSPICE IR representation.

- [ ] **CPL (coupled multiconductor transmission line)**
  Currently unsupported in the importer.
  *Depends on:* IR extension.

## How to Use This Checklist

Each item should be checked off only when:

1. The replacement path is implemented and tested.
2. No test regressions occur when the old path is disabled.
3. The change is merged and the deprecation is documented.

Items can be tackled in any order within their stage dependency, but items
marked as depending on a later stage should not be started until that stage's
entry criteria are met.
