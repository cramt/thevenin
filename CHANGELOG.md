# Changelog

All notable changes to the thevenin / Cirq workspace are documented here.
Per-crate detail lives in each crate's own `CHANGELOG.md` (managed by
release-plz); this file is the workspace-level summary.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versioning follows [SemVer](https://semver.org/spec/v2.0.0.html).

## [Unreleased] — 1.0 preparation

The 0.3.x line is feature-frozen relative to the
[`docs/1.0-checklist.md`](docs/1.0-checklist.md) targets; remaining work is
release polish (versioning, API stability statement, publish dry-run) plus
the URC element and HiSIMHV high-voltage extensions.

### Added — device models

- **MOS Level 3** (semi-empirical short-channel) — full Liu/Kwok port with
  DIBL (ETA), mobility degradation (THETA), velocity saturation (VMAX),
  channel-length modulation (KAPPA), junction-depth effect (XJ), and
  subthreshold conduction (NFS). Previously degraded silently to Level 1.
- **BSIM1** (`LEVEL=4`) — Berkeley short-channel IGFET. Vds-dependent
  threshold, mobility degradation, velocity saturation, N0/NB/ND
  subthreshold, `_L`/`_W` size binning on every process parameter.
- **BSIM2** (`LEVEL=5`) — BSIM1's successor with eta/Ua/Ub/U1 mobility,
  impact ionisation, cubic-spline blend between strong and weak inversion.
- **HiSIM2** (`LEVEL=68`) — surface-potential-based bulk MOSFET. Inner
  Newton solves ψs(Vgs, Vbs) from the Pao-Sah equation with residual-based
  convergence; outer NR uses the resulting gm/gds/gmbs.
- **HiSIMHV** (`LEVEL=73`) — partial; currently dispatches into the
  simplified HiSIM core. The HV-specific extensions (RDRIFT region, body
  resistance, breakdown) are not modelled yet.
- **VDMOS** power MOSFET — vertical-DMOS with built-in body diode and
  Vgd-dependent Miller plateau capacitance. Dispatched off the model kind
  (`.model NAME VDMOS / VDMOSN / VDMOSP`), not via LEVEL.
- **T element** — ideal lossless transmission line. DC = wire (V1=V2,
  I1=-I2), transient = method of characteristics with linear-interpolated
  history, AC = closed-form ABCD matrix.
- **Switches S, W** — voltage- and current-controlled switches with
  hysteretic conductance. Latched state persists across NR iterations
  and transient timesteps.

### Added — analyses

- **`.four`** — DFT of the final fundamental period of the preceding
  `.tran` (DC + 9 harmonics + THD). Note: the linear-interpolation
  resampler leaks roughly 5% into adjacent harmonic bins; a cubic/sinc
  resampler is post-1.0 follow-up.
- **`.fft`** — windowed radix-2 Cooley-Tukey FFT over a user-selectable
  transient interval. Rectangular, Hann, Hamming, Blackman, Bartlett
  windows; `npoints` is rounded up to the next power of two.
- **AC `.sens`** — `run_ac_sens` wraps the direct-method sensitivity
  algorithm around the complex AC solve; per-frequency Y(ω) factor +
  parameter perturbation for every R/C/L/V/I. Verified against the
  closed-form RC and RL transfer-function sensitivities at multiple sweep
  points (`thevenin/tests/sensitivity_ac.rs`).

### Added — `.options`

- `ITL1` / `ITL2` / `ITL4` / `ITL5` / `ITL6` (with `SRCSTEPS` alias) —
  iteration limits, sentinel `ITL5=0` meaning unlimited.
- `CHGTOL` — capacitive LTE charge tolerance.
- `GMINSTEPS` — Gmin-stepping fallback iteration count (default 10,
  sentinel `0` disables stepping entirely).
- `NOOPITER` — skip the initial direct NR attempt and go straight to Gmin
  stepping.
- `RSHUNT` — node-to-ground shunt resistance for ill-conditioned matrices
  (default 0 = disabled).
- `scale` — geometry scale factor (stored on `Circuit::options`).

### Added — output formats

- **ngspice raw file** (binary, little-endian IEEE 754) and ASCII variant —
  the canonical interchange format for KiCad, matplotlib, gnuplot, and
  regression frameworks.
- **CSV** — header row + comma-separated values per sweep point.
- **`write` control command** — `write filename.raw` / `write filename.csv`
  honours filename extension and the `set filetype = ascii|binary`
  ngspice convention.
- Format spec at [`docs/architecture/raw-file-format.md`](docs/architecture/raw-file-format.md).

### Added — Cirq language

- **Native `measure` expression syntax** — `measure <kind> <name> = <expr>`
  where the body is a first-class Cirq expression. Probe functions
  (`max`/`min`/`avg`/`rms`/`pp`, `integ`, `find`, `when`, `deriv`, `delay` with
  `cross(...)` events) take named windowing/edge arguments; any other
  expression is derived/conditional arithmetic over earlier measurements. The
  legacy `measure <kind> "name" { spec: "..." }` block form is retained as an
  escape hatch. Both lower to the same typed `MeasureExpr`, and the expression
  form synthesizes a canonical clause string so it round-trips with SPICE
  `.meas` imports.
- **Comparisons, logical operators, and the ternary `cond ? t : e`** in
  measurement expressions — the SPICE pass/fail idiom
  `(vout_diff < 100k) ? 1 : 0` now parses and evaluates, in both the SPICE
  clause parser and native Cirq. Named call arguments (`f(x, key: value)`) were
  added to the grammar for the probe functions.
- **Compile-time `if / elseif / else` conditionals** — native brace blocks at
  circuit and module scope (`if vdd > 1.5 { ... } elseif ... { ... } else { ... }`)
  that select which declarations reach the IR. Conditions are constant
  expressions over params/literals (numeric, boolean, and string `==`/`!=`);
  resolved during lowering, so the non-taken branch is dropped. The native
  counterpart of SPICE `.if/.elseif/.else/.endif`.
- Built-in functions: `asin`, `acos`, `atan`, `atan2`, `sinh`, `cosh`,
  `tanh`, `sgn`, `floor`, `ceil`, `int`, `db`, `db20`, `limit(x, lo, hi)`.
- `.control` commands: `while`, `repeat N`, `save`.

### Added — SPICE importer

- **`.include` / `.lib` actual file I/O** — preprocessor resolves nested
  includes, two-arg `.lib <path> <name>` block extraction, one-arg
  `.lib name` / `.endl name` conditional sections, search-path control
  (`IncludeOptions::lib_paths`), circular-include detection, Latin-1
  encoding fallback for PDK kit decks that aren't UTF-8.
- HSPICE/PSPICE `\` line continuation in addition to ngspice `+`.
- `.csparam` parsed and seeded into the `.control` variable scope
  (`.param` deliberately is not — matches ngspice behaviour).
- R/L/C element `tc=tc1,tc2` temperature-coefficient parsing.
- `.step` recognised with a stderr warning (parsed-but-deferred for 1.0).
- `TEMPER` and ternary `?:` in brace-expression evaluator.
- Graceful unknown-directive policy (stderr warning + skip; matches
  ngspice).

### Fixed

- `parse_sens_output` differential output `v(a, b)` no longer drops the
  negative terminal due to leading-whitespace mismatch in the split.
- MOS Level 3 (and any other unhandled MOSFET LEVEL) no longer silently
  degrades to Level 1 without notice — emits a one-time, deduplicated
  stderr warning naming the model and unhandled level.
- HiSIM `solve_surface_potential` inner NR now uses residual-based
  convergence (`|f| < 1e-12 V`) instead of a step-based check that step
  damping could mask; returns a convergence flag so callers can react.

## [0.3.0] — 2026-03-22

### Changed

- **Stage 4 IR pivot** — the public simulator surface is now
  `thevenin::circuit::simulate(&Circuit)` operating on `cirq_ir::Circuit`.
  The Netlist-shaped `simulate_*` and `simulate_*_with_mna` helpers were
  demoted to `pub(crate)`. The `--legacy` CLI flag is gone.

### Added

- Cirq native control syntax via the embedded-language `code "control"
  { ... }` block.
- Module hierarchy with typed ports and parameter overrides.
- Coupled-line block syntax.
- Behavioural sources (B element, `V=expr` / `I=expr`).
- Subcircuit / module flattening.
- Save targets, simulation options, temperature.
- `.include` file resolution at the importer level (initial cut).
- User-defined functions, initial conditions.
- `.cirq` file support in the CLI.

### Fixed

- LTRA convolution `chop_reltol` + quadratic interpolation.
- MOS6 mode factor in `ceq_d` RHS stamp for reversed mode.
- MOSFET dynamic `von` in `fetlim`, per-column tolerance, VBIC `q1`
  clamp.
- VBIC self-heating temperature for AC analysis.
- HFET inverse-mode gate voltage; VBIC `ISRR` temperature scaling.

## [0.2.0] — 2026-03-10

### Added

- WebAssembly target via Node runner.
- Initial regression-corpus diff against ngspice output for the
  `ngspice-upstream/tests/` fixtures.

### Fixed

- Missing version specs and changelogs for the initial release-plz pass.

## [0.1.0] — 2026-03-10

Initial release. Core MNA solver, NR iteration, basic device models
(R/L/C, V/I sources, diodes, BJT GP, MOSFET Level 1-2-6, BSIM3/4,
BSIM3SOI FD/PD/DD, JFET, MESFET, HFET, MESA, LTRA/TXL/CPL), and the SPICE
parser + Cirq grammar.
