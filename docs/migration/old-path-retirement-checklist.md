# Old-Path Retirement Checklist

Items to address when gradually retiring SPICE-native interfaces in favor of
the Cirq IR pipeline. Each item represents a piece of the old path that can
eventually be replaced or removed once the Cirq IR path fully subsumes it.

## Primary Interface

- [x] **`thevenin_types::Netlist` as the primary input interface**
  The Stage 4 surface is `thevenin::circuit::simulate(&cirq_ir::Circuit)`.
  Every `IrElementKind` variant assembles into an `MnaSystem` directly via
  `thevenin::mna_ir::assemble_mna_from_circuit`, and all eight analyses
  (op / dc / tran / ac / noise / sens / pz / tf) have Circuit-input
  entry points in `thevenin::circuit::*` that are Netlist-free on the
  happy path. `thevenin::circuit::simulate(&Circuit)` is the top-level
  dispatcher and includes multi-temperature sweep + `.meas` evaluation.

  The Netlist-shaped `thevenin::simulate(&Netlist)` and per-analysis
  `simulate_*(&Netlist)` / `simulate_*_with_mna(MnaSystem, &Netlist)`
  helpers are `pub(crate)` post Stage 4. The harness, all `thevenin/tests`
  integration suites, `cirq-frontend/tests/integration.rs`, the
  `examples/`, and `src/main.rs` were migrated to use
  `thevenin::circuit::simulate*(&Circuit)` (with a shared
  `thevenin/tests/common/mod.rs` helper that wraps
  `cirq_spice_import::import_netlist` + the IR-shape dispatcher for the
  `Netlist::parse_single(spice) → simulate` flow that fixtures still
  use). `thevenin::circuit::simulate_op_with_xspice(&Circuit, registry)`
  is the public XSPICE entry; the legacy Netlist-shape XSPICE wrapper
  and its `mna::assemble_mna_with_xspice` plumbing are deleted.
  *Resolved in:* Stage 4 + the demotion + xspice-circuit sweeps.

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

- [x] **`.control` block interpreter dependency on SPICE Netlist shape**
  The `thevenin-control` crate interprets `.control` blocks that reference
  SPICE-shaped Items and Analysis variants. A Cirq-native control flow
  mechanism (or an adapter that presents IR-based analysis to the interpreter)
  is needed before the Netlist-dependent interpreter can retire.
  *Depends on:* Stage 4. *Resolved across Stage 4 Phases A–E.*
  - Phase A: `execute_control_block_ir(&Circuit)` / `has_control_block_ir(&Circuit)`
    are the canonical entry points; CLI and harness route `.control` through them.
  - Phase B: `SimContext` optionally owns the driving `cirq_ir::Circuit`;
    `SimContext::from_circuit` is the Stage 4 constructor.
  - Phase C: `alter` mutates `Circuit.elements` / `Circuit.models` when the
    context owns a Circuit; the cached netlist is re-derived so the next
    analysis sees the new state. Plain-form `alter v1=-5` accepted.
  - Phase D: `execute_control_block(&Netlist)` / `has_control_block(&Netlist)`
    deleted. `execute_control_block_ir(&Circuit)` / `has_control_block_ir(&Circuit)`
    are the only public surface for `.control` interpretation.
    `SimContext::netlist` and `SimContext::circuit` demoted to `pub(crate)`;
    public access through `SimContext::circuit()`. The cached Netlist
    remained as an internal SPICE-Expr-shape adapter consumed by TEMPER
    evaluation.
  - Stage 4.5 (incremental): `@device[param]` lookup (`resolve_device_param`,
    `resolve_device_param_vec` in `thevenin-control/src/vecexpr.rs`) walks
    `Circuit.models` / `Circuit.elements` directly.
  - Phase E (this stage, complete): `netlist_analysis_to_ir` lands in
    `cirq-frontend::from_netlist`; `.control` analysis dispatch
    (`run_analysis`, `run_tran_with_pause`, `execute_resume`,
    `run_temp_sweep`) now goes through `thevenin::circuit::simulate_*` and
    `thevenin::mna_ir::tran_params_from_circuit`. `evaluate_temper_exprs`
    is lifted to the IR shape as `evaluate_temper_exprs_circuit` operating
    on `Value::String("{...}")` brace params and resistor TC1/TC2/TCE.
    `SimContext::netlist`, `refresh_netlist_cache`, and the
    `circuit_to_netlists` lowering in `SimContext::from_circuit` are
    **deleted**. `resolved_models` is now keyed against IR `Value`s. The
    cached Netlist is gone.

## Simulation Path

- [x] **Direct SPICE parser -> simulate path**
  The CLI routes SPICE files through IR exclusively:
  `SPICE source -> import_spice() -> Circuit -> thevenin::circuit::simulate()`.
  The `--legacy` flag was deleted; the direct Netlist-shaped path has no
  CLI exit. 11 round-trip tests validate bit-identical results across the
  IR adapter.
  *Resolved in:* Stage 3; `--legacy` flag removed in Stage 4.

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
