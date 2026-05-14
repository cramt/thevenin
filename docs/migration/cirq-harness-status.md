# Cirq IR Harness Routing — Status

Every ngspice regression harness test runs through the Cirq IR pipeline.
Each parsed `Netlist` is passed through `cirq_spice_import::import_netlist`
→ `circuit_to_netlists` before flattening and simulation, so the import +
emit adapters are continuously validated against the full regression corpus.

## How to run

```bash
nix develop --command cargo nextest run -p thevenin --test harness
```

The `cirq_import` / `cirq_emit` phases are visible in `TRIAGE_JSON:` lines on
failure so machine triage can distinguish import gaps from simulation drift.

## Current results

99 / 0 / 8 (pass / fail / skip). 7 of the skips are historical ignores; the
single Cirq-only round-trip failure below is quarantined in `ignore.toml`.

## Cirq-only failures (1)

This test passes on the direct `Netlist::parse → simulate` path; the failure
is introduced by the `Netlist → cirq_ir::Circuit → Netlist` round-trip.

### `regression/model/binning-1.cir` — MOSFET model binning

Phase: `cirq_import`. Error: `model not found: nmos_tst`.

The fixture defines several `.model nmos_tst.<n>` variants and references
them as `nmos_tst` (SPICE picks the right bin by W/L). The importer's model
table is keyed on the literal model name, so the suffix variants aren't
findable by the base name. Needs binning-aware lookup in the importer
(~50–200 LOC).

## Resolved on the way to 99/100

These import/emit gaps were uncovered by routing the harness through Cirq
and are closed in this branch:

- **Brace expressions reaching the simulator as opaque param names.** SPICE
  `Expr::Brace("1000 + temper")` round-tripped via
  `Value::String("{1000 + temper}")` and `value_to_expr` blindly remapped it
  to `Expr::Param("{1000 + temper}")` — passing the literal `{` through to
  the simulator's parametric-resistor reader. `value_to_expr` now strips the
  `{...}` wrapping and emits `Expr::Brace` so `temper` and other runtime
  references survive the round-trip. Closed `temper-1`, `temper-2`,
  `temper-3`.
- **`.print @device[param]` queries silently dropped.** The simulator parses
  `.print` device-parameter queries by scanning `netlist.source` line-by-line
  (`transient::collect_device_param_queries`), but `circuit_to_netlists`
  emitted a Netlist with `source: String::new()`. The `.print` directive
  itself round-tripped through `Item::Raw`, but the simulator never read it,
  so the requested vector (e.g. `@m1[vbs]`) simply didn't appear in the
  output. `circuit_to_netlists` now sets `nl.source = format!("{nl}")` so
  raw directives are visible to legacy text-scanning paths. Same fix
  re-enables the `.options list` source echo for round-tripped netlists.
  Closed `bsim3soifd/RampVg2`.

- **`Item::Raw` directives** (`.print`, `.plot`) were silently dropped by the
  importer. Added `Circuit.raw_directives: Vec<String>` to preserve them
  verbatim through the round-trip.
- **Numeric SPICE node names** (`1`, `2`, …) were renamed to `n1`, `n2`, …
  by the importer, which broke `.print v(2)` references because the raw
  directive strings still pointed at the original names. Removed the
  rewrite; the IR now stores SPICE-original names. The valid-Cirq-identifier
  constraint belongs at the Cirq emitter, not in the IR.
- **Unknown model kinds** (TXL, LTRA, CPL, NHFET, D_RAM, D_SOURCE, D_STATE,
  D_XOR, R-as-model, …) were silently dropped by the importer because
  `map_device_type` returned an error and the caller `continue`d. Added
  `DeviceType::Other(String)` to preserve the original SPICE kind string;
  the simulator dispatches several model families on the kind string
  directly, so a lossy import gutted entire device classes (transmission
  lines, HFETs, XSPICE code models, behavioural resistors).
- **Unresolved trailing waveform args** (`SIN 0 1 1K 0 0 DISTOF1 0 DISTOF2 0`)
  caused a hard import error because the SPICE parser greedily filled SIN's
  positional args, slotting `DISTOF1` into the `phi` field. The legacy
  simulator tolerated unresolved expressions in optional waveform tail
  fields; the importer now mirrors that and falls back to `None` instead
  of failing the whole import.

## Implementation order suggestion

One remaining item:

1. **`model/binning-1`** — implement model-binning lookup in the importer.
   Lowest priority — niche SPICE feature, single test.
