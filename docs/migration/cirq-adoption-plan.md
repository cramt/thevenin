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
      **Session B landed:** new `thevenin::mna_ir` module provides
      `assemble_mna_from_circuit(&Circuit, modedc, xspice_registry)`
      that builds a full `MnaSystem` directly from IR for the linear
      subset (R/V/I/C/L/E/G/H/F), returning `Ok(None)` for circuits
      containing any other element kind so the caller falls back to
      `assemble_mna(&Netlist)`. `thevenin::circuit::simulate_op_direct`
      is now a 40-line wrapper around the new module instead of a
      ~300-line bespoke linear stamper. All 13 direct-path equivalence
      tests and 100/0/7 harness state preserved.
      **Session B follow-up:** `simulate_op` extracted a public
      `simulate_op_with_mna(&MnaSystem, &NrOptions, &[(usize, f64)])`
      helper so Netlist and IR paths share the post-assembly solve +
      formatter. `simulate_op_with_xspice` routes through the same
      helper. `mna_ir` gained `nr_options_from_circuit` and
      `resolve_nodeset_from_circuit` Circuit-side equivalents.
      **Session C landed (diode):** `mna_ir` now also accepts diode
      elements — model lookup via `cirq_frontend::to_netlist::convert_model`
      + `extra_params` shims, internal-node allocation when RS > 0,
      synthetic CJO capacitor, and `DiodeInstance` push. The NR loop
      downstream picks up diodes automatically via `has_nonlinear()`,
      so no further wiring was needed. Three new diode fixtures land
      bit-for-bit equivalence; harness 100/0/7 unchanged.
      **Session C landed (BJT):** Npn / Pnp now route through the
      direct IR path. Level 1 Gummel-Poon uses BjtModel + push_bjt_caps
      (made pub(crate)) for CJE / CJC / CJS junction caps; level 4 VBIC
      allocates the full 3-always + 4-conditional + thermal internal-
      node set with `temperature_adjust(circuit_temp(circuit))`. New
      `circuit_temp` helper mirrors `crate::netlist_temp` reading
      `.temp` first then `Options.TEMP`. Four new equivalence
      fixtures (NPN CE, NPN with RB/RC/RE, PNP, full VBIC); harness
      stays 100/0/7. 998/998 workspace tests pass.
      **Session D landed (MOSFET):** Nmos / Pmos with full level
      dispatch (MOS1/2/6, BSIM3 8/49, BSIM4 14/54, BSIM3SOI-FD/DD/PD
      55/56/57). New `ModelTables` struct owns Netlist-shaped
      `ModelDef` copies and exposes the `models_by_name` +
      `bins_by_base` maps that `resolve_model_with_bins` operates on
      (synthetic-alias filtering matches `circuit_to_netlists`).
      `crate::mna::{get_mosfet_level, get_mosfet_lw, get_nrd_nrs,
      resolve_model_with_bins, push_mosfet_caps}` promoted to
      pub(crate) for reuse. Four new equivalence fixtures (MOS1 NMOS,
      MOS1 PMOS, MOS2 with RD/RS, BSIM3 with full nmosParameters
      ported from ngspice-upstream). 1006/1006 workspace tests pass;
      harness still 100/0/7.
      **Session E landed (JFET + MESA family):** NJfet / PJfet /
      NMesfet / PMesfet now route through the direct IR path. JFET
      uses JfetModel + JfetInstance with RD/RS internal nodes. The
      Mesa kind dispatches by resolved model kind: NMF/PMF level=1 →
      MesfetModel; NHFET/PHFET → HfetModel with up to 5 conditional
      internal nodes + HfetPrecomp; anything else → generic MesaModel
      with TS/TD/DTEMP instance params + MesaPrecomp. New
      `circuit_tnom` helper mirrors crate::netlist_tnom. Two new
      equivalence fixtures (JFET, MESFET); HFET/MESA generic paths
      validated once the harness routes through mna_ir.
      **Session F landed (behavioural + mutual coupling):**
      BehavioralSource (V= and I= modes with full parse_bsrc_params
      tc1/tc2/reciproctc semantics) and Coupling (K-element mutual
      inductance via post-pass over already-allocated inductor
      branches) now route through the direct path. Three new
      equivalence fixtures; distributed elements (LTRA/TXL/CPL) and
      XSPICE deferred to a follow-up session. 1008/1008 workspace
      tests pass; harness still 100/0/7.
      **Session F-distributed landed (LTRA/TXL/CPL/XSPICE):** the
      final four element kinds — TransmissionLine (O), Txl (Y),
      CoupledLine (P, variable width), and Xspice (A, registry-
      driven) — now route through mna_ir. With these,
      `mna_ir::circuit_is_supported_subset` returns true for every
      existing IrElementKind variant; every Cirq IR circuit can be
      assembled directly. Both stamping passes are exhaustive (no
      wildcard arms) so a future IrElementKind addition would
      compile-error rather than silently misroute. One new LTRA
      equivalence fixture (TXL/CPL/XSPICE land in code, rely on
      harness fixtures for broader coverage once Session G routes
      the harness through mna_ir). 1009/1009 workspace tests pass;
      harness still 100/0/7.
      **Session G landed (DC/AC/TRAN routing):** simulate_dc /
      simulate_ac / simulate_tran extracted to _with_mna helpers
      that accept a pre-assembled MnaSystem. thevenin::circuit's
      per-analysis Circuit-input entry points now build the
      MnaSystem via mna_ir and dispatch to those helpers, skipping
      the assemble_mna(&Netlist) re-assembly step that previously
      ran on every Circuit-input simulation. Four new equivalence
      fixtures (linear DC sweep, nonlinear DC sweep with diode, AC
      complex sweep, transient pulse through RC) pin the end-to-end
      bit-for-bit equivalence; the assert_results_equal helper
      iterates all plots and compares both Real and Complex vector
      data element-wise. 1014/1014 workspace tests pass; harness
      still 100/0/7.
      **Session H landed (_with_mna surface complete; harness via
      mna_ir):** every analysis the simulator supports now has a
      _with_mna variant (op_dc, noise, sens, tf, pz on top of the
      Session G additions). The regression harness was reshaped to
      track each emitted Netlist's source `cirq_ir::Circuit` and
      route every fixture's MnaSystem through
      `mna_ir::assemble_mna_from_circuit` + the appropriate
      _with_mna dispatcher — so the entire ngspice regression
      corpus (100 fixtures × ~8 analysis kinds) now exercises mna_ir
      end-to-end. Two real mna_ir bugs surfaced and were fixed
      along the way: model-based resistors (R3 4 0 rmodel1 L=... W=...)
      and CPL/XSPICE model lookups (which store the model name in a
      string param rather than `Element.model: Option<Id>`).
      mna_ir reuses resolve_resistor_value /
      extract_resistor_noise_params (promoted to pub(crate)) and
      adds lookup_model_by_string_param for the CPL/XSPICE case.
      `crate::mna` and `crate::mna_ir` are now pub so the harness
      can call them directly. 1014/1014 workspace tests pass;
      harness still 100/0/7.
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
