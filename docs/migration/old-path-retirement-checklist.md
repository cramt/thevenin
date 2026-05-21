# Old-Path Retirement Checklist

Items to address when gradually retiring SPICE-native interfaces in favor of
the Cirq IR pipeline. Each item represents a piece of the old path that can
eventually be replaced or removed once the Cirq IR path fully subsumes it.

## Primary Interface

- [~] **`thevenin_types::Netlist` as the primary input interface**
  The simulator currently requires a `Netlist` to run. As the Cirq IR matures,
  the simulator should accept `cirq_ir::Circuit` directly. The Netlist type
  would become an internal intermediate or compatibility layer.
  *Depends on:* Stage 4 of the adoption plan.
  *Mostly landed on `feat/mna-circuit-input`* — session-by-session migration
  plan in [`docs/migration/mna-ir-pivot-plan.md`](mna-ir-pivot-plan.md).
  As of the last commit on that branch:
  - Every `IrElementKind` variant assembles into an `MnaSystem` directly
    via `thevenin::mna_ir::assemble_mna_from_circuit` (no Netlist
    conversion needed). The full ngspice regression corpus
    (100 fixtures) runs through `mna_ir` end-to-end on every commit.
  - All eight analyses (op / dc / tran / ac / noise / sens / pz / tf)
    have Circuit-input entry points in `thevenin::circuit::*`. Seven
    of them (everything but sens) are Netlist-free on the happy path;
    sens requires an IR-shape change to preserve the tokenized
    `Vec<String>` it currently joins.
  - Top-level `thevenin::circuit::simulate(&Circuit)` dispatcher mirrors
    `thevenin::simulate(&Netlist)`. The CLI uses it for non-`.control`
    circuits.
  - Netlist-shaped public APIs still exist for the `.control`
    interpreter, which mutates a Netlist in-place for TEMPER
    expression evaluation (see the `.control` item below).

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
  *Depends on:* Stage 4. *Resolved across Stage 4 Phases A–D.*
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
    remains as an internal SPICE-Expr-shape adapter consumed by TEMPER
    evaluation, but is no longer reachable from the public API.
  - Stage 4.5 (incremental): `@device[param]` lookup (`resolve_device_param`,
    `resolve_device_param_vec` in `thevenin-control/src/vecexpr.rs`) walks
    `Circuit.models` / `Circuit.elements` directly — one of the two cache
    consumers is gone. The other (TEMPER) is blocked on the
    Analysis-converter described below.

### Blocker: thevenin_types::Analysis → cirq_ir::Analysis converter

Killing the cached Netlist entirely requires moving the `.control`
analysis dispatch from `simulate_*(&Netlist)` to `circuit::simulate_*(&Circuit)`
(or to a Circuit-input transient pause/resume path built on
`assemble_mna_from_circuit` + `tran_params_from_circuit`). That's
blocked on a missing converter: when `parse_analysis_command` returns
a parsed `thevenin_types::Analysis` (Netlist shape) at runtime, there
is currently no way to set the equivalent `cirq_ir::Analysis` on a
Circuit clone for dispatch.

The two shapes are not 1:1 — IR variants carry node `Id`s where
Netlist variants carry source/node names:

- `cirq_ir::DcSweep { source: Id, ... }` vs Netlist `DcSweep { src: String, ... }`
- `cirq_ir::PzAnalysis { input_pos: Id, input_neg: Id, output_pos: Id, output_neg: Id, ... }`
  vs Netlist `Pz { in_pos: String, in_neg: String, out_pos: String, out_neg: String, ... }`
- `cirq_ir::NoiseAnalysis { output_net: Id, reference_net: Id, source: Id, ... }`
  vs Netlist counterparts holding net/source names

So the converter needs `Circuit` access to resolve names → IDs. It
belongs in `thevenin-control` (where the parsed Netlist Analysis
originates) or in `cirq-frontend` next to the existing `to_netlist`
adapter. Once it lands, TEMPER eval can be ported to Circuit (Brace
expressions round-trip through `Value::String("{...}")` per
`cirq-frontend/src/to_netlist.rs:268`), the analysis-dispatch sites
in `thevenin-control/src/exec.rs` (`run_analysis`, `run_tran_with_pause`,
`execute_resume`, `run_temp_sweep`) can move to Circuit-shape entry
points, and `SimContext::netlist` + `refresh_netlist_cache` can be
deleted along with the `circuit_to_netlists` calls in
`thevenin-control/src/context.rs`.

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
