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

#### Session F-distributed — LTRA / TXL / CPL / XSPICE — landed

The last four element kinds are now in `mna_ir`:

- **TransmissionLine** (LTRA, O element): 4 terminals (in_pos/in_neg/
  out_pos/out_neg), 2 branch equations, LtraModel::from_model_def.
  DC stamps stay deferred to `MnaSystem::stamp_ltra_dc_all`.
- **Txl** (Y element): same 4 terminals + 2 branches. TxlModel +
  `setup_txline(model, length)` with optional `LEN`/`LENGTH` instance
  override via `expr_val_or`.
- **CoupledLine** (CPL, P element): variable width — `in0..inN`,
  `out0..outN`, optional `gnd` terminal. Allocates `2 * width` branch
  equations (interleaved as `ibr1[0..N]` then `ibr2[0..N]`). CplModel
  + `setup_cpline` with length override.
- **Xspice** (A element): registry-driven. Iterates `cm_def.ports`,
  resolves per-port (scalar / array) connections to node indices,
  allocates a vsource branch for voltage-out / current-in ports,
  collects `ParamValue` defaults from `cm_def.params` overlaid with
  the model's params. Skips the element when no registry is supplied
  (matches the Netlist path's backward-compat behaviour).

To enable reuse without duplication:
- `crate::ltra::LtraModel::from_model_def`, `crate::txl::TxlModel::from_model_def`,
  and `crate::cpl::CplModel::from_model_def` are already `pub`.
- `crate::txl::setup_txline` and `crate::cpl::setup_cpline` are already
  `pub`. No further visibility changes needed.

Both element-kind `match` sites in `stamp_circuit` are now exhaustive
(no `_ => unreachable!()` arm). The compiler enforces full coverage if
a new `IrElementKind` variant is added to cirq_ir — the gate
`circuit_is_supported_subset` still uses `matches!()` so unknown
variants safely fall back to the lowering path at runtime.

Coverage complete: `mna_ir::circuit_is_supported_subset` now returns
`true` for every existing `IrElementKind` variant. Every circuit
shaped from Cirq IR can be assembled directly without going through
`circuit_to_netlists`. The remaining Stage 4 work (`Session G`) is
wiring DC / AC / transient analyses through the same path so the
direct route is exercised end-to-end, not just for `.op`.

One new equivalence fixture (`ltra_lossy_line_op`); TXL/CPL/XSPICE
paths land in code but rely on the existing harness fixtures for
broader coverage once Session G routes the harness through mna_ir.

#### Session G — DC / AC / TRAN routing through mna_ir (landed)

The three primary analysis functions (`simulate_dc`, `simulate_ac`,
`simulate_tran`) now have `_with_mna` siblings that accept a pre-
assembled `MnaSystem` + the Netlist (still needed for analysis params /
source resolution / nodeset / `.OPTIONS`):

  pub fn simulate_dc_with_mna  (mut mna: MnaSystem, &Netlist) -> Result<...>
  pub fn simulate_ac_with_mna  (    mna: MnaSystem, &Netlist) -> Result<...>
  pub fn simulate_tran_with_mna(mut mna: MnaSystem, &Netlist) -> Result<...>

`thevenin::circuit::simulate_dc/ac/tran` (the Circuit-input entry points)
build the MnaSystem via `mna_ir::assemble_mna_from_circuit` whenever it
accepts the circuit and dispatch to the corresponding `_with_mna`
helper. The IR → Netlist conversion still happens once per call (we
need the lowered netlist for `.dc` source resolution, `.ac` AC source
excitation, `.tran` `.ic` overrides), but the simulator no longer
re-runs `assemble_mna(&Netlist)` on it — a meaningful chunk of work
saved on every Circuit-input simulation.

Four new equivalence fixtures in `direct_path_equivalence`:
  - dc_sweep_voltage_divider:   linear DC sweep over V1
  - dc_sweep_diode_iv:          nonlinear DC sweep (NR + diode model)
  - ac_sweep_rc_lowpass:        complex AC small-signal sweep
  - tran_pulse_through_rc:      time-domain pulse + reactive elements

The equivalence helper grew an `assert_results_equal` core that
iterates all plots (transient prepends the OP plot) and compares
both Real and Complex vector data element-wise.

Verification: `cargo nextest run --workspace` → 1014/1014 pass;
harness 100/0/7 unchanged; `cargo clippy -p thevenin --lib` clean.

#### Session H landed — `_with_mna` surface complete + harness routes via mna_ir

The remaining analysis functions (`simulate_op_dc`, `simulate_noise`,
`simulate_sens`, `simulate_tf`, `simulate_pz`) gained `_with_mna`
siblings matching the pattern from Session G. After Session H, every
analysis the simulator supports has a Netlist-input wrapper that hands
off to a Circuit-friendly `_with_mna` helper for the actual work:

  simulate_op_dc_with_mna   (op_dc-style, diag_gmin=0)
  simulate_noise_with_mna
  simulate_sens_with_mna
  simulate_tf_with_mna
  simulate_pz_with_mna

**The regression harness now routes every fixture through `mna_ir`.**
`thevenin/tests/harness.rs::run_all_analyses` was reshaped:

- Tracks each emitted Netlist's source `cirq_ir::Circuit` via a
  `Vec<(usize, Netlist)>` pairing (one emitted netlist per analysis
  declared on the circuit).
- For every (circuit, netlist) pair, assembles the MnaSystem via
  `mna_ir::assemble_mna_from_circuit(&circuit, false, None)` and
  dispatches to the appropriate `_with_mna` helper. The Netlist-shaped
  `assemble_mna` is no longer called from the harness — the full
  ngspice regression corpus (100 fixtures across OP, DC sweep,
  transient, AC, noise, TF, sens, PZ) exercises mna_ir end-to-end.

Two real mna_ir bugs surfaced through this routing and were fixed:

1. **Model-based resistors** (`R3 4 0 rmodel1 L=11u W=2u`): the IR
   resistor stores the model name in `params: [("value", Value::String)]`,
   not in `Element.model`. mna_ir was insisting on a numeric `value`
   and erroring. Fixed by reusing the existing
   `resolve_resistor_value` (made `pub(crate)`) which understands
   model-based RSH * L / W computation; same for
   `extract_resistor_noise_params` to populate the
   `ResistorInstance` noise fields correctly.
2. **CPL and XSPICE model lookups**: both store their model name as a
   `params: [("model", Value::String)]` string param (per
   cirq_spice_import's encoding), not in `Element.model: Option<Id>`.
   Added `lookup_model_by_string_param(&Circuit, &Element)` and use
   it (with the typed-Id lookup as a fallback) for those two device
   classes.

Visibility changes for harness reach:
- `crate::mna` and `crate::mna_ir` modules promoted to `pub` so the
  test harness can call `mna_ir::assemble_mna_from_circuit` and
  reference `mna::MnaSystem` directly.
- `crate::mna::{resolve_resistor_value, extract_resistor_noise_params}`
  promoted to `pub(crate)` for mna_ir reuse.
- `NodeMap` gained `is_empty()` to satisfy clippy now that it's `pub`.

Verification: `cargo nextest run --workspace` → 1014/1014 pass;
harness 100/0/7 unchanged (still 100 pass / 0 fail / 7 ignored);
`cargo clippy -p thevenin --lib` clean.

#### Session I — Netlist-free analyses (DC + AC landed)

Two analyses pivoted to be Netlist-free on the Circuit-input path:

**DC sweep.** The internal helpers were already mostly Netlist-free —
`resolve_sweep_source` and `get_source_dc_value` only ever needed
`MnaSystem` data (`vsource_names` for V-source lookup,
`current_sources` for I-source lookup + original DC value). Refactored
to drop the Netlist arg; updated `simulate_dc_with_mna` accordingly.
Extracted the sweep core into `pub fn run_dc_sweep(mna, NrOptions,
DcSweepRunParams)` taking pre-resolved typed params. New
`mna_ir::dc_sweep_params_from_circuit` builds the params from
`circuit.analyses[*Analysis::Dc(DcAnalysis)]` (resolving source `Id`s
to element names via `element_name_by_id`).
`thevenin::circuit::simulate_dc(&Circuit)` now skips lowering to
Netlist entirely on the happy path.

**AC sweep.** Decoupled `apply_ac_excitation` from the Netlist by
introducing a typed `AcExcitation { target: AcTarget, real: f64,
imag: f64 }` with `AcTarget::VoltageBranch(usize)` /
`CurrentInjection { ni, nj }`. Two collectors:
`collect_ac_excitations_from_netlist` (existing call-path preserved)
and `mna_ir::collect_ac_excitations_from_circuit` (new). Extracted
`pub fn run_ac_sweep(mna, AcSweepRunParams)` with the sweep core, and
`mna_ir::ac_sweep_params_from_circuit` to build the params from
`Analysis::Ac(AcAnalysis)`. `solve_ac_point` / `solve_ac_frequencies`
/ `build_ac_system` now take `&[AcExcitation]` instead of `&Netlist`
— updated `noise.rs` and `sens.rs` callers accordingly.
`thevenin::circuit::simulate_ac(&Circuit)` is also Netlist-free on
the happy path.

Pattern that emerged for both:
1. Extract analysis core into `run_X_sweep(mna, params)` taking a
   `pub struct XSweepRunParams` with pre-resolved typed fields.
2. Netlist wrapper extracts params from `&Netlist` and calls
   `run_X_sweep`.
3. Add Circuit-side params extractor in `mna_ir` (reads from
   `circuit.analyses`, `circuit.options`, `circuit.elements`).
4. `thevenin::circuit::simulate_X` calls the Circuit extractor +
   `run_X_sweep` directly, skipping the lower-to-Netlist step.

**Transient also pivoted.** Extract `TranRunParams` and `run_tran`;
add `mna_ir::tran_params_from_circuit` that resolves `.ic` overrides
(IR `(Id, voltage)` pairs) to MnaSystem matrix indices and collects
`.print @device[param]` queries from `circuit.raw_directives`.
`collect_device_param_queries` made generic over `IntoIterator<Item =
&str>` so both `netlist.source.lines()` and
`circuit.raw_directives.iter().map(String::as_str)` plug in.
`thevenin::circuit::simulate_tran(&Circuit)` is now Netlist-free on
the happy path.

**TF + PZ + Noise + Sens Circuit entry points.** The rare analyses
get Circuit-input entry points (`thevenin::circuit::simulate_tf`,
`simulate_pz`, `simulate_noise`, `simulate_sens`) that build the
MnaSystem via mna_ir + dispatch through the existing `_with_mna`
helpers. The Netlist is still constructed for analysis-param lookups
(typed `_with_circuit` variants for these would mechanically follow
the DC / AC / TRAN pattern; deferred to follow-up given they're
rarely exercised). `find_input_source` in `tf.rs` was made
Netlist-free by reading `mna.current_sources` directly; `run_tf`
extracted to take pre-resolved `(output, input)` strings.

Final state after Session I:
- OP / DC / AC / TRAN: **fully Netlist-free** on Circuit-input path.
- **TF / PZ / Noise: also fully Netlist-free.** Each extracted to a
  `run_X(mna, params)` core taking pre-resolved typed
  `XRunParams`; new `mna_ir::{tf_spec_from_circuit,
  pz_params_from_circuit, noise_params_from_circuit}` build the
  params from IR `Analysis::{Tf, Pz, Noise}` (resolving net + element
  Ids to names + reusing `collect_ac_excitations_from_circuit` for
  noise's AC excitation list). `find_input_vsource_branch` (pz)
  decoupled from `&Netlist` after extending
  [`VoltageSourceInstance`] with `pos_idx` / `neg_idx` / `name`
  fields. `find_input_source` (tf) was already Netlist-free in the
  previous slice.
- Sens: Circuit-input entry exists but still lowers to Netlist for
  analysis-param lookups. The IR's `SensAnalysis { output: String }`
  loses the tokenized `Vec<String>` shape the Netlist carries (which
  encodes the optional AC variant), so a clean Circuit-only path
  requires extending the IR — a separate work item, not just
  mechanical refactoring.

Verification: 1014/1014 workspace tests pass; harness still 100/0/7.

#### Session J (open) — Final Netlist-API retirement

The remaining Stage 4 work is removing `thevenin_types::Netlist` from
the public API surface entirely. Concretely:

- Pivot `simulate_dc/ac/tran` (Netlist-shaped wrappers) to take
  `&Circuit` directly. The current `_with_mna` helpers still need
  a Netlist for source resolution / analysis params; those reads
  would lift onto `Circuit` fields (`circuit.analyses`,
  `circuit.options`, instance lookups via `Element.name`).
- Same for the other analyses: `noise`, `sens`, `pz`, `tf`.
- `.control` interpreter's TEMPER / `@device[param]` paths still
  build a `Netlist` view from the current `Circuit`; that's a
  parallel work item that can run in any order against this one.
- After all simulator entry points accept `&Circuit`:
  - `crate::mna::assemble_mna(&Netlist)` becomes pub(crate) (used
    only by the Netlist wrapper that calls `cirq_spice_import::import_netlist`
    first).
  - The Netlist-shaped public API can be deprecated / removed.

Estimated diff: +400 LOC of Circuit-side analysis helpers, -200 LOC of
Netlist-shaped wrappers, possibly minus another chunk if the
`.control` work overlaps.

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
