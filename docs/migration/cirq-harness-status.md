# Cirq IR Harness Routing — Status

The ngspice regression harness can be routed through the Cirq IR pipeline by
setting `THEVENIN_VIA_CIRQ=1`. With the env var set, each parsed `Netlist` is
passed through `cirq_spice_import::import_netlist` → `circuit_to_netlists`
before flattening and simulation, exercising both adapter directions against
the full regression corpus on every run.

The default mode is still legacy (`Netlist::parse` → simulate). The Cirq route
will become the default after the remaining drift items below are closed.

## How to run

```bash
# Legacy path (default)
nix develop --command cargo nextest run -p thevenin --test harness

# Cirq IR round-trip path
THEVENIN_VIA_CIRQ=1 nix develop --command cargo nextest run -p thevenin --test harness
```

The `cirq_import` / `cirq_emit` phases are visible in `TRIAGE_JSON:` lines on
failure so machine triage can distinguish import gaps from simulation drift.

## Current results

| Mode | Pass | Fail | Skip | Notes |
|------|-----:|-----:|-----:|-------|
| Legacy | 100 | 0 | 7 | Same 7 historical ignores |
| Via Cirq | 95 | 5 | 7 | Same 7 historical ignores + 5 Cirq-only failures |

## Cirq-only failures (5)

Each of these passes on the legacy path; the failure is introduced by the
`Netlist → cirq_ir::Circuit → Netlist` round-trip.

### `bsim3soifd/RampVg2.cir` — numerical near-miss

Phase: `compare`. Output values drift from the legacy baseline in the gate
ramp region. The IR round-trip apparently changes ordering or representation
in a way that nudges the BSIM3SOIFD CAPMOD=3 transient through a slightly
different NR trajectory.

Likely culprit: an element/model parameter losing precision via the
`Value::Real` round-trip, or a `.options` token being reordered.

### `regression/model/binning-1.cir` — MOSFET model binning

Phase: `cirq_import`. Error: `model not found: nmos_tst`.

The fixture defines several `.model nmos_tst.<n>` variants and references
them as `nmos_tst` (SPICE picks the right bin by W/L). The importer's model
table is keyed on the literal model name, so the suffix variants aren't
findable by the base name. Needs binning-aware lookup in the importer
(~50–200 LOC).

### `regression/temper/temper-1.cir` — `@dplain[is]` device-parameter access

Phase: `simulate`. Error: `cannot resolve device parameter: @dplain[is]`.

The test uses `.control` to read a device parameter via the `@elem[param]`
syntax. Likely the device name (or its prefix) is being mangled through the
IR adapter so the `.control` lookup misses.

### `regression/temper/temper-2.cir` — `.control` computed assertion fails

Phase: `simulate`. Error: `.control quit with exit code 1\nNote: err = 0.99`.

The test runs a DC temperature sweep and computes `err = vecmax(abs(val/gold - 1))`,
asserting `err < 1e-12`. Through the Cirq route, `err` evaluates to ~0.99 —
the simulation produces materially different values. Probable cause: the
`temper` runtime variable in a behavioural expression is constant-folded
or lost during import.

### `regression/temper/temper-3.cir` — parameter expressions in element values

Phase: `simulate`. Error:
`non-numeric value in element r_test: parameter expressions not yet supported`.

The fixture has `.model rtest r r='1000 + temper'` (resistor model with a
parametric resistance). The legacy simulator handles `'1000 + temper'`
internally; via Cirq, the value reaches the simulator in a form it can't
parse. Same root cause as temper-2.

## Resolved on the way to 95/100

These import/emit gaps were uncovered by routing the harness through Cirq
and are closed in this branch:

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

The remaining 5 items break into independent slices:

1. **`temper-3`** — wire parametric model-param expressions through the IR.
   Probable fix is in `convert_params` / `convert_model`: SPICE `Expr::Brace`
   values currently lower to `Value::String("{1000 + temper}")` which the
   simulator can't read back. Likely fix: emit them back as `Expr::Brace` in
   `value_to_expr`. ~30 LOC.
2. **`temper-2`** — probably falls out of #1, since `temper-3` has the
   simpler form of the same expression-loss bug.
3. **`temper-1`** — investigate device-parameter name resolution after the
   IR round-trip; element/model name preservation across `to_netlist`.
4. **`bsim3soifd/RampVg2`** — bisect on parameter precision / option order.
5. **`model/binning-1`** — implement model-binning lookup in the importer.
   Lowest priority — niche SPICE feature, single test.
