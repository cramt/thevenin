# Run 9 — Feature Parity Gaps: Cirq vs SPICE

## Objective

Close every gap between what `thevenin-types::Netlist` can represent and what
the Cirq IR can represent, so that the Cirq path never silently drops
information that the simulator needs.

## Status

**All originally tracked gaps are closed.** The Cirq path can represent
every construct the SPICE Netlist carries; the regression harness runs
100% of its corpus through SPICE → IR → Netlist → simulate with no
silent data loss.

The sections below preserve the historical breakdown (tier 1 = silent
data loss, tier 2 = hard error on valid input, tier 3 = missing
directives) so future contributors can read what each item meant. Each
item links to where the implementation lives so it's findable when
something needs adjustment.

---

## Tier 1 — Silent data loss (all resolved)

| ID  | Item                                                  | Where it landed                                                                                                                 |
|-----|-------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------|
| 1.1 | Waveforms (PULSE, SIN, EXP, PWL, SFFM, AM)            | `cirq_ir::Waveform`; `SourceSpec` on `Element`; lowered by `cirq-frontend/ir_lower::lower_source_spec`; emitted by `to_netlist`. |
| 1.2 | AC source spec (magnitude and phase)                  | `cirq_ir::AcSpec` on `SourceSpec`; populated by both `ir_lower` (Cirq) and `cirq-spice-import` (SPICE).                          |
| 1.3 | Noise analysis lowering                               | `cirq-frontend/ir_lower::lower_noise_analysis`; `mna_ir::noise_params_from_circuit` extracts typed params for the simulator.    |
| 1.4 | Pole-zero analysis lowering                           | `cirq-frontend/ir_lower::lower_pz_analysis`; `mna_ir::pz_params_from_circuit` for the simulator side.                            |
| 1.5 | Coupling `K` element                                  | `element_kind_from_str("coupling")` → `ElementKind::Coupling`; mutual-inductance stamping in `mna_ir::circuit_is_supported_subset`. |

---

## Tier 2 — Hard error on valid input (all resolved)

| ID  | Item                                                  | Where it landed                                                                                                                 |
|-----|-------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------|
| 2.1 | Subcircuit / module hierarchy and flattening          | `ModuleDef` / `ModuleInst` in the AST; `ir_lower::lower_module_inst` inlines instances with hierarchical naming.                |
| 2.2 | Behavioral sources (B element)                        | `ElementKind::BehavioralSource { mode, spec }`; recognised by `element_kind_from_str("behavioral")`.                            |
| 2.3 | MESFET / MESA element kind                            | `ElementKind::NMesfet` / `PMesfet`; `device_type_from_str("nmesfet"|"pmesfet")`.                                                |
| 2.4 | CPL coupled multiconductor transmission lines         | `ElementKind::CoupledLine { width }`; dedicated `coupled_line P1 { ... }` syntax via `ir_lower::lower_coupled_line_decl`.       |
| 2.5 | XSPICE code models (A element)                        | `ElementKind::Xspice { connections }` with `XspiceConnection::{Scalar, Array}` for the variable-width port list.                |
| 2.6 | Model inheritance parameter merging                   | `ir_lower` resolves base models and overlays child params before lowering.                                                       |

---

## Tier 3 — Missing directives and utilities (all resolved)

| ID  | Item                                                  | Where it landed                                                                                                                 |
|-----|-------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------|
| 3.1 | Simulation options (`.options`)                       | `Circuit::options: Vec<(String, Value)>`; `mna_ir::nr_options_from_circuit` is the Circuit-side reader.                         |
| 3.2 | Output selection (`.save`)                            | `Circuit::save: Vec<String>`; round-tripped through `to_netlist`.                                                                |
| 3.3 | Temperature (`.temp`)                                 | `Circuit::temps: Vec<f64>`; `mna_ir::circuit_temp` reads it; multi-temperature sweep handled by `thevenin::circuit::simulate`.   |
| 3.4 | User-defined functions (`.func`)                      | `Circuit::funcs: Vec<FuncDef>`; resolved during constant folding in `ir_lower`.                                                  |
| 3.5 | Include / library file resolution                     | `cirq-frontend` import resolver (`resolve.rs`) reads imported files and merges declarations before IR lowering.                  |
| 3.6 | Initial conditions and node presets (`.ic`, `.nodeset`) | `Circuit::initial_conditions` and `Circuit::nodeset`, both `Vec<(Id, f64)>`.                                                  |
| 3.7 | Transient analysis `tmax` parameter                   | `TranAnalysis::tmax: Option<f64>` in the IR; lowered, round-tripped, and consumed by the transient solver.                       |

---

## Acceptance criteria (met)

1. ✅ The Cirq IR can represent every SPICE construct the simulator consumes.
2. ✅ `ir_lower` produces every construct from Cirq AST.
3. ✅ `to_netlist` emits the correct `thevenin_types` form for round-trip.
4. ✅ `cirq-spice-import` covers every SPICE form used by the harness corpus.
5. ✅ Round-trip is validated for every harness fixture (SPICE → IR → Netlist → simulate produces results identical to direct SPICE → Netlist → simulate).

## Next gaps

All three frontier items from the original list are now closed:

- ~~**Subcircuit/module parameter overrides at instantiation time.**~~
  Closed. `lower_module_inst` / `lower_local_module_inst` in
  `cirq-frontend/src/ir_lower.rs` classify named args against the
  module's port and param sets, evaluate param-override expressions
  in the caller's scope, and `lower_param` consumes the overrides
  ahead of the declared default. Different instances of the same
  module can specialise the same param without body duplication.
- ~~**`.meas` extraction expressions.**~~ Closed. `cirq_ir::MeasureSpec`
  now carries a typed `expr: Option<MeasureExpr>` populated at
  IR-construction time by `MeasureSpec::parse`. The simulator's
  evaluator in `thevenin/src/measure.rs` consumes the typed form
  directly. Added `PARAM=<expr>` (constant or arithmetic over earlier
  measurements), `RISE/FALL/CROSS=LAST`, `FIND ... AT=LAST`, and
  `TD=<time>` honored on TRIG clauses.
- ~~**Cirq-native control flow.**~~ Closed. `cirq_ir::control` owns the
  control-flow AST (`Statement`, `EchoFragment`, `AlterValue`,
  `StopCondition`) plus `parse_control_block`. `CodeBlock` gained a
  `parsed: Option<Vec<Statement>>` populated by
  `CodeBlock::from_lines`. `thevenin-control::execute_control_block_ir`
  consumes the typed AST directly, falling back to re-parsing
  `lines` only for blocks constructed without `from_lines`.

If the goal moves further, the remaining shape is no longer "fill in
parity gaps" but rather "extend the language": full ngspice-equivalent
`.meas` (AC-domain complex measurements, multi-TRIG clauses), Cirq
surface syntax for `.meas` directives (currently only reachable via
SPICE import), and richer language-level constructs that have no SPICE
analogue.
