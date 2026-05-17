# Cirq Adoption Plan

Staged migration from SPICE-native simulation to Cirq-native simulation.

## Stage 1 -- Current (complete)

The Cirq toolchain infrastructure is in place. All paths coexist and nothing
is deprecated.

**What exists:**

- `cirq-frontend`: full pipeline from Cirq source to AST, IR, and Netlist.
  - `parse()` -- source to AST
  - `compile()` -- source to IR
  - `compile_to_netlist()` -- source to Netlist
- `cirq-spice-import`: bridge from SPICE Netlist to Cirq IR.
  - `import_netlist()` -- Netlist to IR
  - `import_spice()` -- SPICE source to IR
- `cirq-ast`: source-faithful AST types with span information.
- `cirq-ir`: canonical IR (name-resolved, parameter-evaluated, model-linked).
- `cirq-grammar`: Tree-sitter grammar with highlights/folds/locals queries.
- Netlist adapter: `cirq_frontend::to_netlist::circuit_to_netlists()` converts
  IR back to `thevenin_types::Netlist` for the existing simulator.

**Primary simulation input:** SPICE (`thevenin_types::Netlist`).

**Cirq status:** fully parseable and compilable; reaches the simulator only
via the Netlist adapter path (Cirq source -> IR -> Netlist -> simulate).

## Stage 2 -- Validation

Run the Cirq pipeline alongside the direct SPICE path and verify equivalence.

**Goals:**

- Cirq IR lowers into Netlists for a growing set of test circuits.
- Round-trip tests confirm that SPICE -> IR -> Netlist produces simulation
  results equivalent to direct SPICE -> Netlist -> simulate.
- CI runs both paths on every PR; mismatches are treated as failures.

**Actions:**

- [x] Expand the integration test suite (`cirq-frontend/tests/integration.rs`)
      to cover all supported element types and analysis modes.
      19 integration tests now cover: OP/DC/AC/tran/noise/PZ analyses,
      resistors/capacitors/inductors/MOSFETs/coupling/dependent-sources/
      behavioral-sources, waveforms (PULSE/SIN), AC specs, SPICE round-trips,
      options/temp, and semantic equivalence at IR level.
- [x] Route the ngspice regression harness through the Cirq IR pipeline.
      `thevenin/tests/harness.rs` now passes every netlist through
      `cirq_spice_import::import_netlist` → `circuit_to_netlists` before
      flattening and simulation. 100 / 0 / 7 (pass / fail / skip); all
      Cirq-only round-trip failures are closed. The remaining 7 ignores are
      unrelated historical issues (BSIM1/BSIM2 not implemented, `.control`
      resume, BJT transient timing, BSIM3SOI-DD body discharge, HFET
      reference bug). See `docs/migration/cirq-harness-status.md`.
- [x] Fix the param naming gap: SPICE import stores passive values as
      `"value"` (normalized in the importer). Round-trip tests confirm the
      Netlist adapter reads them correctly.
- [x] Add AC source parameter support to the Netlist adapter. The `SourceSpec`
      struct carries `dc`, `ac` (`AcSpec { mag, phase }`), and `waveform`.
      Both the SPICE importer and Cirq ir_lower populate these fields. Verified
      by `spice_ac_source_round_trip` integration test.

**Exit criteria:** 100% of existing SPICE regression tests also pass through
the Cirq IR path. ✅ Met — every non-ignored harness test passes via the
Cirq round-trip; the remaining ignores are unrelated to the IR adapter.

## Stage 3 -- SPICE Import Convergence

All SPICE inputs route through Cirq IR before reaching the simulator.

```
SPICE source --> thevenin_types::Netlist --> cirq_ir::Circuit --> Netlist --> simulate()
                                                    ^
Cirq source ---------> cirq_ir::Circuit ------------+
```

**Goals:**

- The canonical path for SPICE files becomes SPICE -> IR -> Netlist -> simulate.
- The direct SPICE -> Netlist -> simulate path remains available as a fallback
  but is no longer the recommended entry point.
- All tooling (linting, formatting, analysis) operates on the Cirq IR, even
  for SPICE inputs.

**Actions:**

- [x] Wire `thevenin::simulate()` or a wrapper to accept SPICE source and
      internally route through IR. The CLI now routes all SPICE files through
      `cirq_spice_import::import_spice()` → `circuit_to_netlists()` → `simulate()`
      by default. 11 round-trip tests validate bit-identical results across
      OP, transient, AC, DC sweep, diode, BJT, MOSFET, subcircuit, mutual
      inductor, temperature/options, and VCVS circuits.
- [x] Ensure subcircuit flattening works at the IR level. The SPICE importer
      calls `thevenin::subckt::flatten_netlist()` on the parsed Netlist before
      importing to IR. Subcircuit round-trip test confirms correctness.
- [x] Handle behavioral sources, CPL, and XSPICE elements in the importer.
      All three are now supported: `BehavioralSource` with V=/I= parsing,
      `CoupledLine` with variable-width connections, and `Xspice` with
      scalar/array connections. Verified by unit tests and integration tests.
- [x] Provide a `--legacy` flag or config to bypass IR for debugging.
      `thevenin run --legacy <file>` uses the direct SPICE parser path.

**Exit criteria:** removing the direct SPICE -> Netlist path causes no test
regressions.

## Stage 4 -- Gradual Retirement

Old SPICE-shaped interfaces begin to retire as confidence grows.

**Goals:**

- The simulator may eventually consume Cirq IR directly, removing the Netlist
  adapter entirely.
- SPICE-specific naming conventions (R1, C2 prefixes), Expr representation,
  and .control block dependencies are replaced by IR-native equivalents.
- `thevenin_types::Netlist` becomes an internal compatibility type rather than
  the primary API surface.

**Actions:**

- [~] Implement a direct IR -> simulation path that bypasses Netlist entirely.
      `thevenin` itself now exposes `simulate_op`, `simulate_dc`,
      `simulate_tran`, and `simulate_ac` taking `&cirq_ir::Circuit` directly
      from a new `thevenin::circuit` module — thevenin has a regular
      dependency on `cirq-ir` + `cirq-frontend` so it owns the Circuit
      lowering internally. The `thevenin-cirq` crate is now a thin re-export
      with the SPICE-source convenience helpers on top. For now the
      implementation still lowers `Circuit -> Netlist` via
      `circuit_to_netlists` before reaching the MNA assembler; subsequent
      passes will replace that with direct IR -> MNA assembly device-class
      by device-class. Callers see no behavioural change during the
      migration.
- [~] Replace the internal `circuit_to_netlists` step in `thevenin::circuit`
      with direct IR -> MNA assembly. Start with linear devices (R/C/L/V/I,
      dependent sources) for `.op`, then nonlinear, then transient/AC.
      Linear DC OP slice landed: `thevenin::circuit::simulate_op` takes a
      direct path through `LinearSystem` (no Netlist intermediate) when the
      circuit contains only R/V/I/C/L/E/G/H/F. Otherwise it falls back to
      the lowering path. Bit-for-bit equivalence with the lowered path is
      pinned by `thevenin-cirq/tests/direct_path_equivalence.rs` (10 SPICE
      fixtures: voltage divider, current source, RC/RL OP, VCVS/VCCS/
      CCVS/CCCS, parallel R, ladder).
      **Next:** generalise the direct path from "linear-only OP" to a real
      `assemble_mna_from_circuit(&Circuit)` covering every device class,
      working on branch `feat/mna-circuit-input`. The session-by-session
      breakdown — including which device families migrate in what order
      and the shim strategy that reuses existing `*Model::from_model_def`
      loaders via `cirq_frontend::to_netlist::convert_model` /
      `value_to_expr` — is laid out in
      [`docs/migration/mna-ir-pivot-plan.md`](mna-ir-pivot-plan.md).
      Session A (enabler) landed: those two helpers are now `pub` so the
      future `thevenin::mna_ir` module can use them without copying.
- [x] Migrate the `.control` block interpreter to operate on Cirq IR or a
      control-flow IR rather than on the SPICE Netlist shape.
      **Phase A landed:** `thevenin_control::execute_control_block_ir(&Circuit)`
      and `has_control_block_ir(&Circuit)` are the canonical IR-shaped entry
      points. Both the CLI (`src/main.rs`) and the regression harness
      (`thevenin/tests/harness.rs`) route `.control` through them when a
      `cirq_ir::Circuit` is on hand; the legacy `execute_control_block(&Netlist)`
      stays available for `--legacy` SPICE input.
      **Phase B landed:** `SimContext` now optionally owns the driving
      `cirq_ir::Circuit` alongside the working netlist;
      `SimContext::from_circuit(c)` is the Stage-4 constructor and the IR
      entry point uses it. `.control` lines now come from
      `Circuit.code_blocks` rather than `Item::Control` on the lowered
      netlist. Analysis dispatch still routes through the cached
      `Netlist` because TEMPER evaluation is intrinsic to the SPICE
      `Expr` shape — lifting TEMPER onto the IR's typed `Value` belongs
      to a later phase. Harness still 100/0/7; 125 unit tests in
      `thevenin-control` (2 new) pin the new constructor.
      **Phase C landed:** `alter` now mutates `Circuit.elements`
      (source DC values, R/C/L `value` params, generic element params)
      and `Circuit.models` (named params) when the context owns a
      Circuit; the cached netlist is re-derived after mutation so the
      next analysis sees the new state. Plain-form `alter v1=-5` (no
      `@`, no `[param]`) is now accepted alongside the bracketed form.
      Vector alters still take the legacy stored-vector path because
      waveform parameters don't have a flat-coefficient IR shape. The
      legacy `SimContext::new(&Netlist)` path keeps the historical
      stash-as-named-vector behavior unchanged. This unblocks one of
      the three prerequisites for `regression/misc/resume-1.cir`;
      `stop when` and `resume` are the remaining two.
      **Phase D landed:** `execute_control_block(&Netlist)` and
      `has_control_block(&Netlist)` are now `#[deprecated]` pointing at
      their IR-shaped counterparts. The CLI's `--legacy` SPICE fallback
      (the only remaining caller in production code) carries
      `#[allow(deprecated)]`; the `cirq-frontend` integration test that
      deliberately exercises the legacy round-trip does the same.
      Removing the Netlist-shaped entry points entirely waits on the
      broader `thevenin_types::Netlist` public-API retirement listed
      below. Stage 4 `.control` work is otherwise complete.
- [ ] Deprecate `thevenin_types::Netlist` as a public API; expose
      `cirq_ir::Circuit` as the primary simulation input.
- [ ] Remove SPICE element-prefix naming requirements from the simulator core.

**Exit criteria:** the Netlist type is internal-only and all external APIs
accept either Cirq source or Cirq IR.

## Risk Mitigations

| Risk | Mitigation | Status |
|------|------------|--------|
| Param naming inconsistency between SPICE import and Netlist adapter | Normalize during Stage 2; add round-trip tests that catch drift | ✅ Resolved — importer uses `"value"` consistently; round-trip tests confirm |
| Subcircuit flattening divergence | Port ngspice subcircuit tests and diff IR-level flattening against Netlist-level | Open — importer still skips subcircuit calls |
| Performance regression from extra IR layer | Profile in Stage 3; the IR conversion is negligible compared to matrix solve time | Open |
| Unsupported SPICE constructs (behavioral sources, XSPICE) | Maintain fallback path through Stage 3; incrementally add importer coverage | ✅ Resolved — behavioral, CPL, and XSPICE all supported |
