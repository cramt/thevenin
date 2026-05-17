## MNA-on-IR Pivot Plan

Stage 4 work item: replace the `&Netlist` input of `thevenin::mna::assemble_mna_flat`
(and its public callers) with `&cirq_ir::Circuit`. Once complete, the simulator
core no longer depends on `thevenin_types::Netlist` shape — only the SPICE
import shim does, satisfying the "Netlist becomes internal-only" exit
criterion in `docs/migration/cirq-adoption-plan.md`.

Working branch: `feat/mna-circuit-input`.

### Why this exists

`thevenin::circuit::lower()` today does `circuit_to_netlists(&Circuit)` →
`flatten_netlist` → `assemble_mna_flat(&Netlist)`. When the original input was a
SPICE file, the path is `SPICE → import_netlist → Circuit → circuit_to_netlists
→ Netlist → flatten → assemble`. Two structural conversions per simulation.
Both conversions also strip information (e.g. `Value::Real(0.0)` round-trips
through `Expr::Num(0.0)` and back). Pivoting MNA input to `&Circuit` removes
the IR → Netlist → IR-shaped-Vec<Param> roundtrip and lets the simulator
consume the same canonical IR that the rest of the toolchain operates on.

### Impedance map (the gaps to bridge)

| Netlist concept                      | IR equivalent                                  | Bridging strategy                                                                                  |
|--------------------------------------|------------------------------------------------|----------------------------------------------------------------------------------------------------|
| `netlist.items` filter `Item::Model` | `circuit.models: Vec<Model>`                   | Use `convert_model` from `cirq_frontend::to_netlist` to produce a per-call `ModelDef` for existing `*Model::from_model_def` loaders. Avoids rewriting all 15+ device model loaders. |
| `ElementKind::Foo { pos, neg, … }`   | `Element { kind, connections, params, … }`     | Add `terminal(elem, name) -> Option<Id>` + `terminal_idx(elem, name, node_map) -> Option<usize>` helpers. |
| `params: Vec<Param>` with `Expr`     | `params: Vec<(String, Value)>`                 | Add `params_to_netlist(&[(String, Value)]) -> Vec<Param>` shim using `value_to_expr`. Lets all existing `apply_multipliers`, `extract_resistor_noise_params`, `get_bjt_level`, `get_mosfet_lw`, `get_nrd_nrs`, etc. keep working unchanged. |
| `source.dc: Option<Expr>` and `source.waveform: Option<Waveform>` | `Element.source_spec: Option<SourceSpec>` | Translate in-place: `expr_value` on a `Value` becomes a one-liner. IR `Waveform` is structurally similar to `thevenin_types::Waveform` but needs a small enum-to-enum converter (already mostly in `to_netlist`). |
| `netlist.source: String` (used by `.print @device[param]` text scan) | `circuit.raw_directives: Vec<String>` | The text scanner currently parses `netlist.source` line-by-line. Either (a) build a synthetic source string from `raw_directives` for legacy compatibility, or (b) lift the scanner to consume `Vec<String>` directly. (a) is cheaper for the pivot. |
| `netlist_temp(&Netlist)` reads `Item::Temp` / `Item::Options` | `circuit.temps`, `circuit.options` | Direct: `circuit.temps.first().copied().unwrap_or_else(|| read_tnom(&circuit.options))`. |
| `crate::expr::resolve_netlist_exprs` | (already done at IR construction time)         | Skip — IR params are pre-resolved.                                                                 |
| `flatten_netlist`                    | (IR is flat per `cirq_spice_import`)           | Skip — already flat.                                                                               |
| `vsource_offset_map` keyed by lowercase name | Same                                       | Keyed by `Element.name.to_lowercase()`; identical semantics.                                       |

### Per-session work breakdown

Each session migrates one concern. Harness must remain 100/0/7 at the end of
every session. Use `direct_path_equivalence.rs` as the equivalence pin —
extend it as device classes are migrated.

#### Session A — Enablers (this session)

- Promote `convert_model`, `value_to_expr`, `convert_waveform`, and
  `convert_element` helpers in `cirq_frontend::to_netlist` from private to
  `pub` (or `pub(crate)` with a re-export) so `thevenin::mna_ir` can use
  them.
- Document the plan (this file).
- No functional code change.

#### Session B — Skeleton + linear devices

- New file `thevenin/src/mna_ir.rs`:
  - `pub fn assemble_mna_from_circuit(&Circuit, modedc, xspice_registry)`
  - Helpers: `terminal_id`, `terminal_idx`, `numeric_param`, `string_param`,
    `bool_param`, `convert_model_for_loaders` (wraps `to_netlist::convert_model`),
    `convert_instance_params`, `model_lookup` (build the BTreeMap from `circuit.models`).
- First-pass loop (node sizing, vsource counting) handling **only** the linear
  subset: `Resistor`, `VoltageSource`, `CurrentSource`, `Capacitor`,
  `Inductor`, `Vcvs`, `Vccs`, `Ccvs`, `Cccs`. All other `ElementKind`
  variants short-circuit to `assemble_mna_flat(&Netlist)` via
  `circuit_to_netlists`.
- Second-pass stamping for the same linear subset, producing identical
  `MnaSystem` shape (matrix entries, `voltage_sources`, `current_sources`,
  `resistors`, `capacitors`, `inductors` vecs).
- Wire `thevenin::circuit::lower()` (and the per-analysis entry points) to
  call `assemble_mna_from_circuit` first; fall back to the existing
  `lower()` path on the all-or-nothing linear gate.
- Extend `direct_path_equivalence.rs` with the same 10 SPICE fixtures it
  already has, asserting `simulate_op` and any other analysis goes through
  the direct path.
- Delete `thevenin::circuit::simulate_op_direct` — superseded by
  `assemble_mna_from_circuit` for all analyses.

Estimated diff: +600 LOC (mna_ir.rs), -150 LOC (simulate_op_direct removal).

#### Session C — Diode + BJT

- Add `Diode` first-pass + stamp branch to `mna_ir.rs`. Internal-node
  bookkeeping mirrors `mna.rs:1146-1163`. Reuse `DiodeModel::from_model_def`
  via the `convert_model_for_loaders` shim.
- Same for `Bjt` (level 1 default + level 4 VBIC). VBIC has 7 conditional
  internal nodes — mirror the existing logic exactly.
- Extend `direct_path_equivalence.rs` with diode and BJT fixtures (port
  `ngspice-upstream/tests/diode/*.cir` and `bjt/*.cir`).
- Run full harness: must stay 100/0/7. Any regression points at a stamp
  detail that drifted between the two paths.

Estimated diff: +400 LOC.

#### Session D — MOSFET family

- `Mosfet` with level dispatch: 2 (MOS2), 6 (MOS6), 8/49 (BSIM3),
  14/54 (BSIM4), 55 (BSIM3SOI-FD), 56 (BSIM3SOI-DD), 57 (BSIM3SOI-PD),
  default (MOS1).
- Each level uses its own `*Model::from_model_def` loader via the same shim
  — no model code changes.
- Internal-node count differs per level (some depend on `nrd`/`nrs`
  instance params, some on `has_body_contact`). Mirror `mna.rs:1198-1287`
  exactly.
- Extend `direct_path_equivalence.rs` with one fixture per supported level.

Estimated diff: +600 LOC.

#### Session E — JFET / MESA / MESFET / HFET

- `Jfet`, `Mesa` (which dispatches to MESFET or HFET via model kind string),
  using the same shim pattern.
- Extend equivalence tests.

Estimated diff: +300 LOC.

#### Session F — Distributed + behavioral + XSPICE

- `Ltra`, `Txl`, `Cpl` — transmission line variants. Each adds 2+ branch
  equations; the existing code is the reference.
- `BehavioralSource` (V= and I= modes) — parses spec string already at the
  stamp site; same logic.
- `Xspice` — `circuit.xspice_registry` lookup unchanged.
- `Coupling` (`MutualCoupling`) — handled in a separate post-pass in `mna.rs`
  via `mutual_couplings_raw`; the equivalent in IR is `ElementKind::Coupling`
  with `params: [("k", _), ("l1", _), ("l2", _)]`. Port that post-pass.

Estimated diff: +400 LOC.

#### Session G — Netlist callers + final cutover

- Update remaining Netlist callers (`ac.rs`, `tf.rs`, `pz.rs`, `noise.rs`,
  `sens.rs`, `simulate.rs`) to either:
  - Accept `&Circuit` directly (preferred — pivots the whole simulator API),
    or
  - Wrap their `&Netlist` input via `cirq_spice_import::import_netlist`
    before calling the new `assemble_mna_from_circuit`.
- Delete the old `assemble_mna_flat(&Netlist)` and surrounding helpers.
- `mna_ir.rs` becomes `mna.rs`; the old `mna.rs` is removed.
- The `assemble_mna(&Netlist)` public API is gone. SPICE input goes
  through `cirq_spice_import::import_netlist` (already the harness's path).
- The `.control` interpreter's TEMPER / `@device[param]` paths still build
  a `Netlist` view from the current `Circuit`; that's the next Stage 4 work
  item, tracked separately.

Estimated diff: +200 LOC, -2,500 LOC (deleting the old assemble_mna_flat).

### Risk register

| Risk                                                       | Mitigation                                                                                                          |
|------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------|
| Drift between direct-stamp and lowering paths on edge cases | `direct_path_equivalence.rs` is the contract — every device class gets at least one fixture per supported variant. |
| Model `from_model_def` loaders silently depend on `ModelDef.kind` casing | `convert_model` already upper-cases kind strings — preserve that.                                                |
| `Expr::Param` references that survived through Cirq IR (`Value::String("{...}")`) | `value_to_expr` already handles this — keep the test in `direct_path_equivalence` that exercises a `temper`-referencing circuit. |
| `circuit.raw_directives` text scanners (`.print @device[param]`) | The text scanner is read-only over a string; build the string on demand from `raw_directives` joined with `\n`. Cheap. |
| Harness regression mid-migration                            | Each session's fallback gate (unsupported device → old path) keeps the harness green throughout.                   |
| Stage-4 `.control` migration entangled                      | `.control` interpreter currently calls `thevenin::simulate_op(&Netlist)`. Migrate at Session G or earlier by giving the interpreter an `&Circuit`-shaped dispatch. |

### Exit criteria

- `thevenin_types::Netlist` is no longer reachable from any public API in
  the `thevenin` crate.
- `cirq_spice_import::import_netlist` is the only path SPICE inputs take
  into the simulator.
- `docs/migration/cirq-adoption-plan.md` Stage 4 actions move from
  `[~]` to `[x]`.
- `docs/migration/old-path-retirement-checklist.md` updates the
  `thevenin_types::Netlist as the primary input interface` item to
  resolved.
