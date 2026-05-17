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

#### Session B — Skeleton + linear devices — landed

Landed on `feat/mna-circuit-input` as follow-up commits after the Session A
plan doc.

- New file `thevenin/src/mna_ir.rs` (≈500 LOC) exporting
  `assemble_mna_from_circuit(&Circuit, modedc, xspice_registry) -> Result<Option<MnaSystem>, MnaError>`.
  Internal helpers: `circuit_is_linear_subset`, `build_net_name_map`
  (with `gnd` → `0` rewrite to match `circuit_to_netlists`),
  `terminal_name`, `numeric_param`, `string_param`, `resistor_multipliers`,
  `evaluate_source_dc` (mirrors the MODEDC / MODEDCOP convention in
  `mna::stamp_element`).
- First-pass loop handles the linear subset: `Resistor`, `VoltageSource`,
  `CurrentSource`, `Capacitor`, `Inductor`, `Vcvs`, `Vccs`, `Ccvs`, `Cccs`.
  Any other `ElementKind` returns `Ok(None)` so the caller falls back to
  `assemble_mna(&Netlist)`.
- Second-pass stamping produces an `MnaSystem` (via the new
  `MnaSystem::empty(dim, xspice_registry)` constructor that initialises
  every device-instance vec to empty) with matrix entries, `vsource_names`,
  `voltage_sources`, `current_sources`, `resistors`, `capacitors`, and
  `inductors` populated. Bit-for-bit equivalent with the lowered path on
  the existing 10-fixture `direct_path_equivalence.rs` suite.
- `thevenin::circuit::simulate_op_direct` deleted — replaced by a 40-line
  thin wrapper that calls `mna_ir::assemble_mna_from_circuit` + solves +
  formats the `SimResult` exactly like `simulate::simulate_op` does
  (descending matrix-index node order, element-insertion vsource branch
  order). Net delete of ~300 LOC of bespoke linear stamping that
  `mna_ir` now subsumes.
- `cirq_frontend::to_netlist::convert_source_spec` and `convert_waveform`
  promoted to `pub` so `mna_ir` can read IR sources via the existing
  conversion path (avoids duplicating waveform / AC spec handling).
- `NodeMap::new` and `NodeMap::index` exposed `pub(crate)` so the new
  module can build the same string-keyed map as the Netlist path.

Verification:

- `cargo nextest run -p thevenin-cirq` → 13/13 pass (direct-path equivalence).
- `cargo nextest run -p thevenin --test harness` → 100/0/7 (unchanged).
- `cargo nextest run --workspace` → 994/994 pass.
- `cargo clippy -p thevenin --lib` → clean.

#### Session C — Diode (landed) + BJT

**Diode landed.** `mna_ir.rs` now accepts circuits containing `IrElementKind::Diode`:

- First pass: indexes anode/cathode nets and increments `internal_node_count`
  when the resolved `DiodeModel::has_series_resistance()`.
- Second pass: builds the `DiodeModel` via the
  `cirq_frontend::to_netlist::convert_model` shim (preserving the existing
  `DiodeModel::from_model_def` loader unchanged), layers instance params
  with `with_instance_params(&extra_params(elem, &["value"]))`, allocates an
  internal node when RS > 0, and pushes both a `DiodeInstance` and the
  synthetic CJO junction capacitor. The NR loop downstream picks up the
  diodes automatically via `MnaSystem::has_nonlinear()` →
  `solve_op_raw_with_opts` → `solve_nonlinear_op`.
- New helpers in `mna_ir`: `lookup_model(&Circuit, &Element)` resolves
  `Element.model: Option<Id>` against `circuit.models`;
  `load_diode_model(&Circuit, &Element)` returns a fully-resolved model.
- `cirq_frontend::to_netlist::extra_params` promoted to `pub` so the
  instance-param projection lives in one place.
- Three new equivalence fixtures in `direct_path_equivalence.rs`:
  `diode_voltage_drop`, `diode_with_series_resistance`, `diode_clamp_pair`.

Verification: `cargo nextest run --workspace` → 995/995 pass; harness
100/0/7 unchanged.

**BJT landed (level 1 + VBIC).** `mna_ir.rs` now also accepts Npn / Pnp
elements:

- Level 1 (Gummel-Poon) via `BjtModel::from_model_def` + `with_instance_params`
  on `extra_params(elem, &["value"])`. Allocates `base_prime`,
  `col_prime`, `emit_prime` internal nodes when RB / RC / RE > 0. Pushes
  `BjtInstance` plus the synthetic CJE / CJC / CJS junction capacitors via
  `push_bjt_caps` (re-exported as `pub(crate)`).
- Level 4 (VBIC) via `VbicModel::from_model_def` + `temperature_adjust` —
  no `with_instance_params` (matches the Netlist path's behaviour, which
  doesn't apply VBIC instance overrides even though the method exists).
  Always-internal: `collCI`, `baseBI`, `baseBP` (3 nodes). Conditional:
  `collCX` (RCX > 0), `baseBX` (RBX > 0), `emitEI` (RE > 0), `subsSI`
  (RS > 0), `rth` (RTH > 0). The first-pass count uses
  `internal_node_count()` unconditionally — mirrors `assemble_mna_flat`'s
  bookkeeping where the second pass allocates an SI internal node when
  `vm.rs > 0` regardless of whether the substrate terminal is wired.
- New helpers: `circuit_temp(&Circuit) -> f64` (mirrors `crate::netlist_temp`
  reading first `.temp` then `Options.TEMP`); `bjt_level(model, &elem.params)
  -> i32`; `load_bjt_model` / `load_vbic_model`; `numeric_value(&Value)
  -> Option<f64>`.
- Four new equivalence fixtures in `direct_path_equivalence.rs`:
  `bjt_common_emitter_npn`, `bjt_with_series_resistances` (RB / RC / RE
  internal-node exercise + push_bjt_caps), `bjt_pnp_high_side` (PNP
  device kind), `bjt_vbic_level4` (full VBIC conditional internal-node
  set including thermal node).

Verification: `cargo nextest run --workspace` → 998/998 pass; harness
100/0/7 unchanged.

#### Session D — MOSFET family — landed

`mna_ir.rs` now accepts Nmos / Pmos with the full level dispatch:
1 (MOS1 default), 2 (MOS2), 6 (MOS6), 8/49 (BSIM3), 14/54 (BSIM4),
55 (BSIM3SOI-FD), 56 (BSIM3SOI-DD), 57 (BSIM3SOI-PD).

- New `ModelTables` struct holds owning `Vec<(name, ModelDef)>` plus the
  Netlist-shaped `models_by_name` and `bins_by_base` lookups that
  `resolve_model_with_bins` expects. Synthetic-alias filtering mirrors
  `cirq_frontend::to_netlist::circuit_to_netlists`: empty-param models
  whose name is the base of `<name>.<digits>` siblings are skipped so
  the bin resolver picks the right `.N` variant.
- Each level uses its own `from_model_def` loader unchanged; the level-1
  / 2 / 6 paths also push junction caps via `push_mosfet_caps`
  (re-exported as `pub(crate)`). The BSIM3SOI-FD path mirrors the
  conditional body-internal-node allocation: only created when the
  element has a body contact (matches `b3soifdset.c`'s
  `bNode = pNode = 0 when bNodeExt == -1`).
- `crate::mna::{get_mosfet_level, get_mosfet_lw, get_nrd_nrs,
  resolve_model_with_bins, push_mosfet_caps}` promoted to `pub(crate)`
  so the new module can reuse them with the `extra_params` shim
  producing `Vec<Param>` from IR.
- Six new equivalence fixtures in `direct_path_equivalence.rs`:
  `mosfet_level1_nmos`, `mosfet_level1_pmos`, `mosfet_level2_with_series_resistances`,
  `mosfet_bsim3_level8` (uses the full nmosParameters set ported from
  `ngspice-upstream/tests/bsim3/nmos/parameters` so the NR solve
  converges).

Verification: `cargo nextest run --workspace` → 1006/1006 pass; harness
100/0/7 unchanged.

#### Session E — JFET / MESA / MESFET / HFET — landed

`mna_ir.rs` now also accepts NJfet / PJfet / NMesfet / PMesfet:

- **JFET** (`JfetModel` + `JfetInstance`): drain/gate/source terminals
  with RD/RS internal-node allocation; AREA / M instance params.
- **MESA elements** (`NMesfet`/`PMesfet`) dispatch the same way
  `assemble_mna_flat` does — by the resolved model's `kind` string:
  - `NMF`/`PMF` with level=1 → `MesfetModel` + `MesfetInstance`
    (RD/RS internal nodes only).
  - `NHFET`/`PHFET` (any level) → `HfetModel::from_model_def_with_level`
    + `HfetInstance` with up to 5 conditional internal nodes
    (drain'/source'/gate' for RD/RS/RG, drain''/source'' for RF/RI)
    plus a `HfetPrecomp::compute(model, 300.15, 300.15, w, l)` call.
  - Anything else → generic `MesaModel` + `MesaInstance` with TS / TD /
    DTEMP instance params, `MesaPrecomp::compute(model, ts, td, tnom, w, l)`,
    and up to 5 conditional internal nodes.
- New `circuit_tnom(&Circuit)` helper mirrors `crate::netlist_tnom`
  (reads `TNOM` from `circuit.options`, returns Kelvin).
- Two new equivalence fixtures (`jfet_njf_op`, `mesfet_nmf_op`); the
  HFET and generic-MESA paths are exercised via the existing harness
  fixtures once the harness routes through `mna_ir`.

Verification: `cargo nextest run --workspace` → 1006/1006 pass; harness
100/0/7 unchanged; `cargo clippy -p thevenin --lib` clean.

#### Session F — Behavioral + MutualCoupling (landed); distributed/XSPICE deferred

- **BehavioralSource** (V= and I= modes): added to `mna_ir` with full
  `parse_bsrc_params` semantics (tc1/tc2/reciproctc trailing-param
  parsing). V= mode stamps a vsource branch + KCL topology and pushes
  `BehavioralVoltageSourceInstance`; I= mode pushes
  `BehavioralSourceInstance` (no branch). Temperature coefficient
  computed via `circuit_temp(circuit) - 27.0` matching the Netlist
  path's `netlist_temp - 27.0` convention.
- **Coupling** (K-element / mutual inductance): added as a post-pass
  that runs after the main stamping loop, once every inductor has its
  `branch_idx` and inductance allocated. Reads `l1` / `l2` string
  params, finds them via the same `vsource_offset_map` the inductors
  populated, and pushes `MutualCouplingInstance` with
  `factor = k * sqrt(|L1 * L2|)`.
- Bumped both element-kind `match` sites to `match &elem.kind` so the
  new `BehavioralSource { mode, spec }` arm can pattern-match the
  String spec by reference. The pre-existing arms use unit variants and
  are unaffected.
- Promoted `crate::mna::{parse_bsrc_params, BsrcParams}` to `pub(crate)`
  so `mna_ir` can reuse the parser unchanged.
- Three new equivalence fixtures (`behavioural_voltage_source`,
  `behavioural_current_source`, `mutual_coupling_two_inductors`).

Verification: `cargo nextest run --workspace` → 1008/1008 pass; harness
100/0/7 unchanged; `cargo clippy -p thevenin --lib` clean.

**Deferred to a follow-up session:** LTRA, TXL, CPL, and XSPICE. These
distributed-element kinds need multi-branch allocation (LTRA/TXL add 2,
CPL adds `2 * width`) plus the XSPICE registry lookup with per-port
voltage/current branch allocation. Coverage gap remains tractable —
mutual coupling and behavioural sources cover the most-used cases for
analog circuits.

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
