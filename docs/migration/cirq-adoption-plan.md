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
- [ ] Add a CI job that runs Cirq-path simulation for the ngspice regression
      suite (ported circuits) and diffs results against the SPICE-path baseline.
- [x] Fix the param naming gap: SPICE import stores passive values as
      `"value"` (normalized in the importer). Round-trip tests confirm the
      Netlist adapter reads them correctly.
- [x] Add AC source parameter support to the Netlist adapter. The `SourceSpec`
      struct carries `dc`, `ac` (`AcSpec { mag, phase }`), and `waveform`.
      Both the SPICE importer and Cirq ir_lower populate these fields. Verified
      by `spice_ac_source_round_trip` integration test.

**Exit criteria:** 100% of existing SPICE regression tests also pass through
the Cirq IR path.

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

- [ ] Wire `thevenin::simulate()` or a wrapper to accept SPICE source and
      internally route through IR.
- [ ] Ensure subcircuit flattening works at the IR level (the SPICE importer
      currently skips subcircuit calls; Cirq module inlining works for
      single-level hierarchies).
- [x] Handle behavioral sources, CPL, and XSPICE elements in the importer.
      All three are now supported: `BehavioralSource` with V=/I= parsing,
      `CoupledLine` with variable-width connections, and `Xspice` with
      scalar/array connections. Verified by unit tests and integration tests.
- [ ] Provide a `--legacy` flag or config to bypass IR for debugging.

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

- [ ] Implement a direct IR -> simulation path that bypasses Netlist entirely.
- [ ] Migrate the `.control` block interpreter to operate on Cirq IR or a
      control-flow IR rather than on the SPICE Netlist shape.
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
