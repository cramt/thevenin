# Run 9 — Feature Parity Gaps: Cirq vs SPICE

## Objective

Close every gap between what `thevenin-types::Netlist` can represent and what
the Cirq IR can represent, so that the Cirq path never silently drops
information that the simulator needs.

This document is the authoritative list.  Each gap is tiered by severity and
includes the concrete IR/AST/spec/grammar changes needed.

---

## Severity tiers

| Tier | Meaning |
|------|---------|
| **1 — Silent data loss** | The Cirq path compiles without error but the simulator gets wrong input.  Must fix before any real circuit can trust the Cirq path. |
| **2 — Hard error on valid input** | The Cirq path rejects or skips a construct that thevenin can simulate.  Blocks feature coverage. |
| **3 — Missing convenience** | Directives and utilities the simulator reads from the Netlist that have no IR representation yet. |

---

## Tier 1 — Silent data loss

### 1.1  Waveforms (PULSE, SIN, EXP, PWL, SFFM, AM)

**Current state.**
The Cirq grammar and spec define waveform blocks (`pulse: { v1: 0, v2: 3.3, ... }`).
The AST represents them as `Expr::Block`.  The IR lowerer (`ir_lower.rs`)
evaluates `Block` to `Value::String("<block>")` and **discards the entries**.
The `to_netlist` adapter emits `Source { dc, ac: None, waveform: None }` for
every voltage/current source — waveform is always `None`.

**Impact.**
Any transient simulation through the Cirq path sees all sources pinned at their
DC value (or zero).  Results are silently wrong.

**Required changes.**

| Layer | Change |
|-------|--------|
| `cirq-ir` | Add a `Waveform` enum with variants: `Pulse`, `Sin`, `Exp`, `Pwl`, `Sffm`, `Am`.  Add an `AcSpec { mag: f64, phase: f64 }` struct.  Add a `SourceSpec { dc: Option<f64>, ac: Option<AcSpec>, waveform: Option<Waveform> }` struct.  Store `SourceSpec` on voltage/current source `Element`s (either as a dedicated field or as a typed param). |
| `cirq-frontend/ir_lower` | Lower `Expr::Block` entries into the new `Waveform` variants by matching the block's label (`pulse`, `sin`, `exp`, `pwl`, `sffm`, `am`).  Lower `ac` / `phase` named params into `AcSpec`. |
| `cirq-frontend/to_netlist` | Map `cirq_ir::Waveform` → `thevenin_types::Waveform`, `cirq_ir::AcSpec` → `thevenin_types::AcSpec`, and populate `Source.waveform` / `Source.ac`. |
| `cirq-spice-import` | Map `thevenin_types::Waveform` → `cirq_ir::Waveform` and `thevenin_types::AcSpec` → `cirq_ir::AcSpec`. |
| Tests | Round-trip: SPICE transient circuit with PULSE source → IR → Netlist → simulate.  Compare against direct SPICE path. |

### 1.2  AC source specification (magnitude and phase)

**Current state.**
`to_netlist` always emits `ac: None`.  `ir_lower` has no concept of AC source
parameters.  The IR stores voltage/current source params as flat `(String, Value)`
pairs — there is no typed `AcSpec`.

**Impact.**
AC analysis through the Cirq path sees all sources with zero AC excitation.
Results are silently wrong.

**Required changes.**
Covered by the same IR extension as 1.1 (`SourceSpec` struct).

### 1.3  Noise analysis lowering

**Current state.**
The Cirq grammar, spec, and AST support `analysis noise { ... }`.
`ir_lower.rs` emits a warning and returns `None` — the analysis is silently
dropped.  The IR types for `NoiseAnalysis` already exist.

**Impact.**
A Cirq file with `analysis noise` compiles to an IR with no analysis commands,
which defaults to `.op`.  User gets an operating point instead of noise data.

**Required changes.**

| Layer | Change |
|-------|--------|
| `cirq-frontend/ir_lower` | Implement `lower_noise_analysis` that extracts `output`, `reference`, `source`, `start`, `stop`, `points`, `scale` from analysis settings and sweep specs.  Resolve `output`/`reference`/`source` identifiers to net/element `Id`s. |
| Tests | Cirq noise circuit → IR → Netlist → verify `Analysis::Noise` fields. |

### 1.4  Pole-zero analysis lowering

**Current state.**
Same as noise — `ir_lower.rs` emits a warning and returns `None`.  IR types
for `PzAnalysis` already exist.

**Impact.**
Same as noise — silently replaced by `.op`.

**Required changes.**

| Layer | Change |
|-------|--------|
| `cirq-frontend/ir_lower` | Implement `lower_pz_analysis` that extracts `input_pos`, `input_neg`, `output_pos`, `output_neg`, `transfer` type, and `analysis_type` from settings.  Resolve to `Id`s. |
| Tests | Cirq PZ circuit → IR → Netlist → verify `Analysis::Pz` fields. |

### 1.5  Coupling element (`K`) not wired in Cirq source path

**Current state.**
The spec (04-elements.md) and the AST define `coupling` as an element type.
The IR has `ElementKind::Coupling`.  The `to_netlist` adapter handles it.
**But** `ir_lower.rs::element_kind_from_str` has no entry for `"coupling"`,
so a Cirq source file using `k1: coupling(...)` fails with "unknown element
type".

**Impact.**
Mutual inductance is unusable from Cirq source.  (SPICE import path works
fine because the importer maps `MutualCoupling` directly.)

**Required changes.**

| Layer | Change |
|-------|--------|
| `cirq-frontend/ir_lower` | Add `"coupling"` → `ElementKind::Coupling` in `element_kind_from_str`.  Define standard terminal names: `l1`, `l2`.  Handle the coupling coefficient param. |
| Tests | Cirq coupled inductors → IR → Netlist → verify `MutualCoupling`. |

---

## Tier 2 — Hard error on valid input

### 2.1  Subcircuit / module hierarchy and flattening

**Current state.**
The Cirq grammar and AST fully support `module` definitions and `ModuleInst`
instantiations.  The IR has **no** concept of hierarchy — no module
instantiation, no flattening.  `ir_lower.rs` silently skips `ModuleDef` and
`ModuleInst` nodes.  The SPICE importer skips `SubcktCall` elements (returns
`None`).

**Impact.**
Virtually all real SPICE designs use subcircuits.  Any circuit using `module`
or `X` instances produces an incomplete IR.

**Required changes.**

| Layer | Change |
|-------|--------|
| `cirq-ir` | **Option A (flatten in IR lowering):** No IR changes — the lowerer inlines module bodies, renaming nets and elements with hierarchical prefixes.  **Option B (hierarchical IR):** Add `ModuleInst { name, module_id, connections, params }` and `ModuleDef` to the IR, and defer flattening to a later pass or the adapter.  **Recommendation:** Option A for v0.1 — matches what `thevenin::subckt::flatten_netlist()` does today. |
| `cirq-frontend/ir_lower` | Implement recursive module inlining: resolve `ModuleInst` by substituting the module body, binding ports to actual nets, applying param overrides, and prefixing element/net names with the instance path. |
| `cirq-spice-import` | Implement `SubcktCall` import: look up `SubcktDef` by name, inline or store the hierarchy. |
| `to_netlist` | If using Option A, no changes needed — the flattened IR already maps directly. |
| Tests | Hierarchical Cirq design → IR (flattened) → Netlist → simulate.  Compare against flat equivalent. |

### 2.2  Behavioral sources (B element)

**Current state.**
`thevenin-types` has `BehavioralSource { pos, neg, spec }` where `spec` is
`"V={expr}"` or `"I={expr}"`.  Neither the Cirq spec, grammar, IR, nor any
adapter handles behavioral sources.

**Impact.**
SPICE files with `B` elements error out in the importer.  No Cirq syntax
exists.

**Required changes.**

| Layer | Change |
|-------|--------|
| Cirq spec | Define behavioral source syntax, e.g. `b1: behavioral(pos -> neg, v: { expr })` or a dedicated `vsource`/`isource` variant with an expression body. |
| Grammar | Add production rule for behavioral source. |
| `cirq-ast` | Add `BehavioralSpec` or extend `ElementInst` with expression body. |
| `cirq-ir` | Add `ElementKind::BehavioralVoltageSource` and `BehavioralCurrentSource` (or a single `Behavioral` variant with a mode flag) carrying the expression string/AST. |
| `ir_lower` | Lower the new AST node. |
| `to_netlist` | Map to `thevenin_types::ElementKind::BehavioralSource`. |
| `cirq-spice-import` | Import `BehavioralSource` into the new IR variant. |

### 2.3  MESFET / MESA element kind

**Current state.**
`thevenin-types` has `Mesa { d, g, s, model, params }`.  The IR has
`DeviceType::NMesfet` / `PMesfet` for models but **no** `ElementKind` for the
element.  The SPICE importer maps `Mesa` → `ElementKind::NJfet`, losing identity.

**Impact.**
MESFET circuits import lossily.  The simulator dispatches to `mesa.rs` or
`mesfet.rs` based on the model type string, so the wrong element kind means
the wrong device stamp.

**Required changes.**

| Layer | Change |
|-------|--------|
| `cirq-ir` | Add `ElementKind::NMesfet` and `ElementKind::PMesfet`. |
| Spec / grammar | Add `nmesfet` / `pmesfet` as element type keywords (or a single `mesfet` with model-based polarity). |
| `ir_lower` | Wire up in `element_kind_from_str`. |
| `to_netlist` | Map to `thevenin_types::ElementKind::Mesa`. |
| `cirq-spice-import` | Map `Mesa` to the correct new `ElementKind`. |

### 2.4  CPL coupled multiconductor transmission lines

**Current state.**
`thevenin-types` has `Cpl { in_nodes, out_nodes, gnd, model, params }`.
No Cirq representation exists anywhere.  Importer returns `UnsupportedElement`.

**Required changes.**

| Layer | Change |
|-------|--------|
| `cirq-ir` | Add `ElementKind::CoupledTransmissionLine` with variable-width connection lists. |
| Spec / grammar | Define syntax for multi-port elements or a CPL-specific form. |
| `ir_lower`, `to_netlist`, `cirq-spice-import` | Wire up. |

### 2.5  XSPICE code models (A element)

**Current state.**
`thevenin-types` has `Xspice { connections, model }`.  The `thevenin-xspice`
crate already provides a full code-model framework.  No Cirq representation
exists.  Importer returns `UnsupportedElement`.

**Required changes.**

| Layer | Change |
|-------|--------|
| `cirq-ir` | Add `ElementKind::Xspice` with a list of typed connections (scalar vs array). |
| Spec / grammar | Define syntax for XSPICE instantiation (connection arrays, model binding). |
| `ir_lower`, `to_netlist`, `cirq-spice-import` | Wire up. |

### 2.6  Model inheritance parameter merging

**Current state.**
The Cirq spec and AST support `model nch_fast: nch_base { ... }`.  `ir_lower`
resolves the base model's `DeviceType` but does **not** merge the base model's
parameter list into the child.

**Impact.**
A derived model that overrides only one parameter loses all parent defaults.

**Required changes.**

| Layer | Change |
|-------|--------|
| `cirq-frontend/ir_lower` | When lowering a model with a `base`, copy the base model's params first, then overlay the child's params. |
| Tests | Model with base → IR → verify all base params present plus overrides. |

---

## Tier 3 — Missing directives and utilities

### 3.1  Simulation options (`.options`)

**Current state.**
`thevenin-types` has `Item::Options(Vec<Param>)`.  The simulator reads GMIN,
ABSTOL, RELTOL, VNTOL, ITL1, ITL2, ITL4, TNOM, etc. from `.options` via
`nr_options_from_netlist()`.  No Cirq representation exists.

**Required changes.**

| Layer | Change |
|-------|--------|
| Cirq spec | Define `options { gmin: 1e-12, abstol: 1e-12, ... }` block syntax. |
| Grammar / AST | Add `options_decl` production, `OptionsDecl` AST node (or reuse analysis-style key-value block). |
| `cirq-ir` | Add `Circuit::options: Vec<(String, Value)>` (or a typed struct). |
| `ir_lower` | Lower options block. |
| `to_netlist` | Emit `Item::Options(...)`. |
| `cirq-spice-import` | Import `Item::Options(...)` into IR. |

### 3.2  Output selection (`.save`)

**Current state.**
`thevenin-types` has `Item::Save(Vec<String>)`.  No Cirq representation.

**Required changes.**

| Layer | Change |
|-------|--------|
| Cirq spec | Define `save v(out), i(R1)` or similar syntax within `circuit` or `analysis` blocks. |
| `cirq-ir` | Add `Circuit::save: Vec<String>` or analysis-scoped save targets. |
| All adapters | Wire up. |

### 3.3  Temperature (`.temp`)

**Current state.**
`thevenin-types` has `Item::Temp(f64)`.  The simulator reads it via
`netlist_temp()`.  No Cirq representation.

**Required changes.**

| Layer | Change |
|-------|--------|
| Cirq spec | Define `temp 27` or `options { temp: 27 }`. |
| `cirq-ir` | Add `Circuit::temp: Option<f64>` or include in options. |
| All adapters | Wire up. |

### 3.4  User-defined functions (`.func`)

**Current state.**
`thevenin-types` has `Item::Func { name, args, body }`.  No Cirq
representation.

**Required changes.**

| Layer | Change |
|-------|--------|
| Cirq spec | Define function syntax, e.g. `fn limit(x, lo, hi) = min(max(x, lo), hi)`. |
| Grammar / AST | Add function definition node. |
| `cirq-ir` | Add `FuncDef` and evaluate calls during constant folding, or defer to runtime. |

### 3.5  Include / library file resolution

**Current state.**
The Cirq grammar and AST support `import "file.cirq"`.  `ir_lower` never
resolves file references — imports are silently ignored.

**Required changes.**

| Layer | Change |
|-------|--------|
| `cirq-frontend` | Add a file resolver that reads imported files, parses them, and merges their top-level declarations into the importing file's scope before IR lowering.  Needs a search path mechanism. |
| `cirq-spice-import` | Map `Item::Include` / `Item::Lib` to `Import` in AST or resolve inline. |

### 3.6  Initial conditions and node presets (`.ic`, `.nodeset`)

**Current state.**
`.ic` values are carried as element params (`IC=val` on capacitors/inductors)
and pass through.  `.nodeset` has no thevenin-types representation and no Cirq
equivalent.

**Required changes.**

| Layer | Change |
|-------|--------|
| Cirq spec | Define `ic` block or element-level `ic:` param (element-level already works). |
| `cirq-ir` | Optionally add `Circuit::initial_conditions: Vec<(Id, f64)>` for node-level `.ic`. |

### 3.7  Transient analysis `tmax` parameter

**Current state.**
`thevenin_types::Analysis::Tran` has `tmax: Option<Expr>`.
`cirq_ir::TranAnalysis` has `step`, `stop`, `start`, `uic` but **no `tmax`**.
The transient solver uses `tmax` to limit the internal timestep.

**Required changes.**

| Layer | Change |
|-------|--------|
| `cirq-ir` | Add `tmax: Option<f64>` to `TranAnalysis`. |
| `ir_lower` | Extract `tmax` from analysis settings. |
| `to_netlist` | Map to `Analysis::Tran { tmax }`. |
| `cirq-spice-import` | Import `tmax` from SPICE `Tran`. |

---

## Implementation order

Recommended sequence based on dependency and impact:

1. **1.1 + 1.2 — Waveforms and AC specs** (largest silent-failure surface)
2. **1.5 — Coupling element wiring** (one-line fix in `element_kind_from_str`)
3. **1.3 + 1.4 — Noise and PZ analysis** (IR types exist; just need lowering)
4. **2.1 — Subcircuit flattening** (biggest feature gap; blocks real designs)
5. **2.6 — Model inheritance merging** (small fix, correctness)
6. **3.7 — Tran tmax** (small fix, correctness)
7. **3.1 — Options** (simulator needs GMIN/ABSTOL for convergence)
8. **3.3 — Temperature** (affects device models)
9. **2.3 — MESFET element kind** (small IR addition)
10. **3.5 — Include/lib resolution** (needed for real multi-file designs)
11. **2.2 — Behavioral sources** (larger design effort)
12. **3.4 — User-defined functions** (larger design effort)
13. **2.4 — CPL** (uncommon)
14. **2.5 — XSPICE** (larger design effort, separate framework)
15. **3.2 — Save** (convenience)
16. **3.6 — IC/nodeset** (partially works already)

---

## Acceptance criteria

This document is the tracking list.  Each item above is complete when:

1. the Cirq IR can represent the construct,
2. `ir_lower` produces it from Cirq AST,
3. `to_netlist` emits the correct `thevenin_types` form,
4. `cirq-spice-import` can import the SPICE equivalent,
5. a round-trip test passes (SPICE → IR → Netlist → simulate matches direct SPICE → simulate).
