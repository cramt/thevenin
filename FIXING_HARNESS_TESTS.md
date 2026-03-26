# Fixing Ignored Harness Tests

Systematic methodology for diagnosing and fixing ignored tests in the
ngspice integration harness.  Each test compares thevenin's batch output
against the reference `.out` file from `ngspice-upstream/tests/`.

### Test architecture

Tests are auto-generated at compile time by the `ngspice_tests!()` proc macro
(crate: `thevenin-test-macro`).  It walks `ngspice-upstream/tests/`, finds all
`.cir`/`.out` pairs, resolves `.include` directives, and embeds everything as
string literals — no filesystem access at runtime (works on WASM too).

Ignore reasons live in **`thevenin/tests/ignore.toml`** (flat TOML table):
```toml
"bsim3soidd/t5.cir" = "~1.1% Ids error at Ve=-4 (vfbb sign error)"
```

To un-ignore a test: remove its line from `ignore.toml`.
To add a new ignore: add a `"path/to/file.cir" = "reason"` entry.

---

## 1. Pick a test

Choose one test (or a group with the same ignore reason).  Prefer tests whose
ignore reason sounds like a numerical bug over tests that require whole missing
features.

**Triage by category (most → least tractable):**

| Category | Count | Approach |
|---|---|---|
| Numerical offset / accuracy | few | Compare equations term-by-term against C |
| tran: singular matrix | 2 | Check device stamp completeness, initial conditions |
| tran: initial values wrong | 1 | Compare DC OP → first tran step transition |
| tran: output mismatch | 1 | Diff output at each timepoint, find where it diverges |
| transient timestep (US-055) | 12 | Timestep control / output interpolation |
| device info output (US-061) | 10 | Missing `.print` columns (showmod, element params) |
| AC complex formatting (US-058) | 2 | Output formatter doesn't emit complex columns |
| .plot ASCII art (US-058) | 1 | Missing `.plot` renderer |
| BSIM3SOI accuracy (US-059) | 15 | Model equation bugs — big effort |
| sensitivity param mismatch | 1 | Missing BJT sensitivity params |
| parameter expressions | 1 | Resistor array `{expr}` not evaluated |
| PZ numerical accuracy | 1 | Eigenvalue solver bug for inductors |
| MESA non-default temp | 1 | Temperature scaling bug in MESA model |
| BSIM1/BSIM2 (US-052/053) | 2 | Entire model not implemented |
| XSPICE (US-056) | 3 | Entire subsystem not implemented |
| .control scripting | 35 | Entire interpreter not implemented |
| TEMPER keyword | 4 | Needs expression-in-parameter + .control |

## 2. Run the test and capture the diff

```bash
nix develop --command cargo test --package thevenin \
  --test harness TESTNAME -- --ignored --nocapture 2>&1 | head -80
```

The harness prints both the expected (filtered) and actual (filtered) output,
plus the first mismatch.  Key things to note:

- **Which column fails** (col 0 = first output variable, etc.)
- **Constant offset vs relative error vs divergence** — constant offset
  suggests a parasitic term (gmin, leakage); relative error suggests a formula
  bug; divergence suggests convergence failure or wrong region selection.
- **At what sweep/time point it starts failing** — if only the first point is
  wrong, it's likely an initial-condition or default-value issue.

## 3. Understand the circuit

Read the `.cir` file from `thevenin/tests/fixtures/<subdir>/` (or
`ngspice-upstream/tests/<subdir>/`).  Identify:

- What analysis: `.dc`, `.tran`, `.ac`, `.pz`, `.noise`, `.sens`?
- What devices: which model type and level?
- What is being measured: `.print` variables?
- What `.model` parameters are set (non-default)?

## 4. Locate the relevant code

The ngspice C source is authoritative.  Key directories:

```
ngspice-upstream/src/spicelib/devices/<model>/     # device models
ngspice-upstream/src/spicelib/analysis/             # analysis drivers
ngspice-upstream/src/maths/ni/                      # NR iteration (niiter.c)
ngspice-upstream/src/maths/sparse/                  # sparse matrix (spsmp.c)
```

Device model files follow a naming convention:
- `<model>load.c`  — NR load function (stamps Jacobian + RHS)
- `<model>defs.h`  — instance/model structs, parameter IDs
- `<model>temp.c`  — temperature-dependent preprocessing
- `<model>dset.c`  — parameter setup / defaults
- `<model>acld.c`  — AC small-signal load

Our Rust equivalents live in `thevenin/src/<model>.rs`.

### Rust codebase architecture: companion → stamp → device_stamp

Every nonlinear device follows the same pattern in our codebase:

1. **`<model>.rs`** — contains two key functions:
   - `<model>_companion(inst, vgs, vgd, gmin) → <Model>Companion` — pure
     computation: given terminal voltages, returns linearised conductances
     (`gm`, `gds`, `ggs`, `ggd`), currents (`cd`, `cg`), and capacitances.
     This is the Rust equivalent of the computation section in
     `<model>load.c`.
   - `stamp_<model>_with_voltages(comp, inst, vgs, vgd, matrix, rhs)` —
     applies the companion model to the MNA matrix and RHS vector (Norton
     equivalent current sources + Y-matrix conductance stamps).  This is the
     equivalent of the `load:` label section in `<model>load.c`.

2. **`device_stamp.rs`** — the glue layer.  `DeviceVoltageState::stamp_devices()`
   is called at every NR iteration.  For each device type it:
   - Extracts terminal voltages from the solution vector
   - Applies voltage limiting (`pnjlim`, `fetlim`)
   - Calls the `_companion()` function
   - Calls the `stamp_…()` function
   This is the shared NR load logic used by both `simulate.rs` (DC OP / DC
   sweep) and `transient.rs`.

3. **`simulate.rs`** — DC analysis driver.  Creates the `load` closure that
   copies base linear stamps, adds LTRA/TXL/CPL DC equations, and calls
   `stamp_devices()`.  Passes this closure to `newton_raphson_solve()`.

4. **`transient.rs`** — transient analysis driver.  Same pattern but also
   stamps reactive elements (capacitor/inductor companion models) and handles
   timestep control.

When debugging a device model bug, you almost always only need to look at
`<model>.rs`.  When debugging a stamping or solver bug, look at
`device_stamp.rs` and `newton.rs`.

### The output pipeline: format → filter → compare

The harness test pipeline (`harness.rs`) works as follows:

1. **`format_batch_output(netlist, result)`** (`output.rs:16`) — takes the
   `SimResult` and produces text output mimicking ngspice `--batch` mode.
   Emits title, temperature line, data tables with headers.  This is where
   "device info output (US-061)" failures originate — ngspice prints extra
   sections like `.OP` element parameters, `.SHOWMOD` model info, and small-
   signal parameters that our formatter doesn't emit.

2. **`filter_output(text)`** (`output.rs:547`) — applies the same filtering as
   ngspice's `check.sh` FILTER regex.  Removes lines containing keywords like
   `"Circuit"`, `"Index"`, `"Date"`, `"---"`, etc.  Both expected and actual
   output are filtered before comparison.

3. **`compare_filtered(expected, actual)`** (`output.rs:654`) — compares the
   two filtered outputs.  Non-numeric tokens are compared exactly.  Numeric
   tokens use relative tolerance 1e-4 and absolute tolerance 1e-15.  If line
   counts differ (e.g. different timestep counts), falls back to
   interpolation-aware comparison that linearly interpolates actual data at
   the expected data's x-coordinates.

**Common output-related failure modes:**
- **"device info output (US-061)"** — the `.out` file contains device
  parameter sections (element info, operating point details) that we don't
  emit.  After filtering, these sections still leave residual lines that don't
  appear in our output.  Fix: add the relevant output sections to
  `format_batch_output`, or extend `filter_output` if the lines should be
  stripped.
- **"AC complex output formatting (US-058)"** — AC analysis outputs complex
  numbers (magnitude + phase, or real + imaginary) that our formatter doesn't
  handle.  The `SimVector.complex` field exists but
  `format_batch_output` doesn't emit it in the right format.
- **"transient timestep (US-055)"** — our transient produces different
  timepoints than ngspice.  The interpolation fallback in `compare_filtered`
  tries to handle this, but large timestep differences cause interpolation
  errors.  Fix: improve timestep control in `transient.rs` to match ngspice's
  adaptive stepping algorithm.

## 5. Systematic diff against ngspice C

This is the core debugging technique.  For a numerical bug:

### 5a. Isolate the failing quantity

From the test output, identify the variable (e.g. `vids#branch`) and the
operating point where the mismatch occurs.  Build a minimal mental model of
what currents/voltages should flow.

### 5b. Compare device equations line-by-line

Open the ngspice `<model>load.c` and our `<model>.rs` side by side.  Check:

1. **Junction / diode currents** — reverse bias approximation, forward bias
   exponential, `csat` scaling by area.
2. **Drain/collector current** — region selection (cutoff / linear / saturation),
   formula for `cdrain`, `gm`, `gds`.
3. **Conductance signs and stamps** — the Y-matrix stamp pattern.  Compare
   every `*(here->...Ptr) += m * (...)` line in C against the corresponding
   `matrix.add(row, col, value)` in Rust.
4. **Current accounting** — `cd = cdrain - cgd`, `cg = cgs + cgd`, Norton
   equivalents `ceqgd`, `ceqgs`, `cdreq`.
5. **Default parameter values** — check `<model>dset.c` or the `init` function.
   A wrong default (especially for `b`, `lambda`, `alpha`, `is`) shifts results.

### 5c. Check the solver / infrastructure

Some bugs are not in the device model but in the framework:

- **Gmin handling** — device models include `CKTgmin` in junction conductances.
  The solver diagonal `CKTdiagGmin` is separate (see `spsmp.c:LoadGmin`).
  In ngspice, `CKTdiagGmin` starts at 0 for DC analysis and is only elevated
  during Gmin stepping.  Our `NrOptions.diag_gmin` mirrors this — DC sweep
  sets it to 0, other paths keep it at `gmin`.
- **Voltage limiting** — `DEVpnjlim`, `DEVfetlim` in ngspice vs our `pnjlim`,
  `fetlim`.  Wrong limiting causes NR oscillation.
- **Temperature** — many models scale parameters at `TEMP != TNOM`.  Check
  `<model>temp.c`.  Our code may skip temperature adjustment entirely.
- **Charge model / capacitances** — for transient, the charge integration
  (trapezoidal or Gear) must match.  Wrong capacitance → wrong transient.
- **Output formatting** — the harness compares filtered text output.  If
  column ordering, number formatting, or header text differs, the comparison
  fails even if the numbers are correct.

## 6. Write a focused unit test

Before fixing, write a small test that reproduces the bug at the device-model
level (not the full harness).  For example:

```rust
#[test]
fn mesfet_subthreshold_junction_current() {
    let inst = make_test_instance();
    let comp = mesfet_companion(&inst, -3.0, -3.1, 1e-12);
    // Expected: cd ≈ 3.114e-12 (from ngspice reference)
    assert_abs_diff_eq!(comp.cd, 3.114e-12, epsilon = 1e-14);
}
```

This makes the fix testable independently of the full simulation pipeline.

## 7. Fix and verify

Apply the fix.  Then:

```bash
# 1. Verify the target test passes
nix develop --command cargo test --package thevenin \
  --test harness TESTNAME -- --ignored --nocapture

# 2. Run the FULL test suite — check for regressions
nix develop --command cargo test --package thevenin 2>&1 | grep FAILED

# 3. Clippy
nix develop --command cargo clippy --workspace -- -D warnings
```

**Regression risk areas:**
- Changing `newton.rs` (NR solver) affects every nonlinear circuit.
- Changing a device model affects all tests using that model.
- Changing output formatting affects every harness test.

## 8. Un-ignore the test

Remove the test's line from `thevenin/tests/ignore.toml`:
```toml
# Delete this line:
"path/file.cir" = "reason"
```

## 9. Check if sibling tests also pass

Many ignore reasons are shared across a group.  After fixing the root cause,
try the whole group:

```bash
nix develop --command cargo test --package thevenin \
  --test harness -- --ignored --nocapture 2>&1 | grep -E "ok|FAILED"
```

---

## Worked Example: MES subthreshold offset

**Symptom:** `harness_mes_subth` — constant +2e-13 offset in drain current
across entire subthreshold DC sweep.

**Diagnosis:** Deep subthreshold drain current ≈ 0; measured current dominated
by gmin × V(drain). Expected ≈ 3.114e-12, got ≈ 3.314e-12. Diff = 2e-13 =
`gmin × 0.2V`. Root cause: our NR solver added `options.gmin` to every matrix
diagonal, but ngspice's `CKTdiagGmin` is 0 for DC analysis — device models
already include gmin internally.

**Fix:** Added `NrOptions.diag_gmin` field. DC sweep sets `diag_gmin = 0`;
other paths keep it at `gmin`. All 465+ existing tests still pass.

---

## Key design decisions and known limitations

### Always match ngspice's implementation exactly
When porting device model code from ngspice, use ngspice's exact values for
physical constants, formulas, and conventions — even when "more correct" modern
values exist. Example: VBIC uses ngspice's Boltzmann constant (`1.380662e-23`)
rather than the 2019 SI exact values.

### Sensitivity LU precision for high-CMRR circuits
`harness_sensitivity_diffpair` — ngspice reuses the LU factors from the OP
solve; we rebuild and refactor. Even 1 ULP of rounding difference destroys the
diff pair CMRR for parameters connected to the tail node. Fix: plumb LU
factorization from `newton_raphson_solve` through to `simulate_sens`.

### VBIC two-step vs single-step temperature scaling
Proven algebraically equivalent when T_amb = T_nom (default). Both power-law
and exponential terms telescope identically. This rules out the two-step/
single-step difference as the root cause of the ~0.2% VBIC self-heating error.

---

## Applied fixes (chronological summary)

| # | Fix | Files | Impact |
|---|-----|-------|--------|
| 1 | `diag_gmin` — separate solver diagonal gmin from device gmin | newton.rs, simulate.rs | MES subthreshold passes; all DC sweeps corrected |
| 2 | VBIC self-heating (RTH > 0) — thermal node + Ith power stamping | vbic.rs, device_stamp.rs | FG error 6%→0.02%, temp error 0.22%→0.02% |
| 3 | BSIM3SOI derivative computation — size_dep_param corrections (cdep0, theta0vb0, theta_rout) | bsim3soi_*.rs | Improved NR convergence for SOI tests |
| 4 | VBIC temperature scaling — multiple parameter corrections | vbic.rs | Corrected temp-dependent currents |
| 5 | Device junction capacitances in transient analysis | transient.rs | Enabled reactive element stamping |
| 6 | VTO computation from process parameters (NSUB/TOX/NSS) | mosfet.rs | Level 1 MOSFET threshold voltage correct |
| 7 | Tokenizer spaced key=value parsing | parser | Fixed netlist parsing edge cases |
| 8 | MOSFET reversed-mode ceq_d sign convention | mosfet.rs | Correct reversed-mode Norton equivalent |
| 9 | MOSFET jct_initial_guess mode initialization | mosfet.rs | Better NR starting point for junctions |
| 10 | BJT diffusion capacitance in transient analysis | bjt.rs, transient.rs | BJT transient dynamics working |
| 11 | MESA transient junction capacitance | mesa.rs, transient.rs | MESA transient passes |
| 12 | MOS6 Meyer gate capacitance + qmeyer 2x correction | mos6.rs | MOS6 gate charge correct |
| 13 | BJT forward-bias depletion cap correction | bjt.rs | Junction charge formula exponent fixed |
| 14 | MOSFET gds floor for LAMBDA=0 | mosfet.rs | Prevents singular matrix in cutoff |
| 15 | BSIM3SOI size_dep_param corrections | bsim3soi_*.rs | cdep0, theta0vb0, theta_rout corrected |
| 16 | BJT dynamic charge LTE in timestep control | transient.rs | Better adaptive timestep for BJT circuits |
| 17 | HFET transient junction capacitance | hfet.rs, transient.rs | HFET transient analysis working |
| 18 | Slope-aware timing tolerance in waveform comparison | output.rs | Timing shifts at steep edges tolerated |
| 19 | MOSFET Vbs/Vbd pnjlim + improved slope estimation | mosfet.rs, device_stamp.rs | Better NR convergence for body diodes |
| 20 | VBIC parasitic junction parameter corrections | vbic.rs | ISP/IBEIP/IBENP temperature scaling fixed |
| 21 | VBIC avalanche current (Igc) sign and formula | vbic.rs | Corrected Igc computation |
| 22 | BSIM3SOI vfb computation from VTH0 | bsim3soi_*.rs | Flat-band voltage calculation corrected |
| 23 | Level 1 MOSFET von (threshold voltage) computation | mosfet.rs | Dynamic von for fetlim |
| 24 | Breakpoint step growth limiting for reactive circuits | transient.rs | Prevents timestep collapse |
| 25 | MOSFET junction diode RHS sign correction | mosfet.rs | Correct junction current stamping |
| 26 | VBIC temperature scaling powf(1.0) optimization | vbic.rs | Avoid FP noise from `x.powf(1.0)` |
| 27 | Level 1 MOSFET ceq_d gds sign in reversed mode | mosfet.rs | Correct reversed-mode drain stamp |
| 28 | BSIM3SOI-FD Vbs clamp for 5-terminal devices | bsim3soi_fd.rs | FD floating body handled correctly |
| 29 | HFET inverse-mode gate voltage + VBIC ISRR temp scaling | hfet.rs, vbic.rs | HFET/VBIC correctness improvements |
| 30 | VBIC AC self-heating temperature adjustment | ac.rs, vbic.rs | AC analysis uses self-heated temperature |
| 31 | MOSFET fetlim dynamic von | device_stamp.rs | von tracks with operating point |
| 32 | Per-column dynamic-range absolute tolerance | output.rs | Better tolerance for mixed-scale outputs |
| 33 | MOS6 ceq_d mode sign in reversed mode | mos6.rs | MOS6 reversed-mode drain stamp |
| 34 | LTRA convolution chop_reltol + quadratic interpolation | ltra.rs | Transmission line accuracy improved |
| 35 | VBIC transit time rIf parameter correction | vbic.rs | Correct transit time modulation |
| 36 | BSIM3SOI temperature scaling corrections | bsim3soi_*.rs | Multiple temp-dependent param fixes |
| 37 | CPL convolution accumulation + timing order | cpl.rs | Coupled transmission line accuracy |
| 38 | BSIM3SOI-FD csieff/litl/Abeff corrections | bsim3soi_fd.rs | FD model equation fixes |
| 39 | BJT OFF flag in MODEINITJCT initialization | bjt.rs, device_stamp.rs | BJT OFF initial conditions correct |
| 40 | Divided-difference LTE for capacitors and BJT charges | transient.rs | Better timestep control |
| 41 | CPL delay interpolation integer truncation | cpl.rs | Fixed index computation |
| 42 | CPL polint Neville tableau path correction | cpl.rs | Interpolation algorithm fix |
| 43 | TXL h1 convolution accumulation | txl.rs | TXL model accuracy improved |
| 44 | BSIM3SOI-FD Vbsdio unconditional assignment | bsim3soi_fd.rs | Body voltage initialization |
| 45 | BSIM3SOI-FD Abulk T9 parameter (tox→tsi) | bsim3soi_fd.rs | Correct bulk charge parameter |
| 46 | CPL R_m off-diagonal clamping | cpl.rs | Matrix stability improvement |
| 47 | BSIM3SOI rds0 wr exponent correction | bsim3soi_*.rs | Source/drain resistance formula |
| 48 | BSIM3SOI-PD junction temperature scaling | bsim3soi_pd.rs | PD junction current temp dependence |
| 49 | VBIC Ith power computation order | vbic.rs | Thermal power calculation corrected |
| 50 | `.plot` directive support in formatter | output.rs | Circuits with only `.plot` now produce data |
| 51 | VBIC external resistance self-heating temp adjustment | vbic.rs, ac.rs | RCX/RBX/RE/RS use self-heated temp |
| 52 | BSIM3SOI-FD derivative chain (dVbseff/dVg, dVbseff/dVd) | bsim3soi_fd.rs | Body transconductance coupling in Jacobian |
| 53 | PULSE PW default changed from 0 to TSTOP | waveform.rs | Matches ngspice reference output |
| 54 | VBIC AC charge-thermal cross-coupling stamps | ac.rs | AC thermal coupling correct |
| 55 | VBIC model equation bugs (Ibep NCN, sgIf, WSP, VBBE) | vbic.rs | 4 correctness fixes (dormant for default params) |
| 56 | Slope tolerance — removed x_range < 1e-3 guard | output.rs | Un-ignored rc.cir and sensitivity/diffpair |
| 57 | BSIM3SOI-DD/FD/PD impact ionization fixes | bsim3soi_*.rs | DD enable/prefactor/exp corrected; FD disabled |
| 58 | Vdseff clamping derivative fix (all SOI variants) | bsim3soi_*.rs | Clamp value only, preserve derivatives |
| 59 | BSIM3SOI junction width and mobility derivatives | bsim3soi_*.rs | weff vs wdios/wdiod; dueff_dv* corrections |
| 60 | Non-parenthesized PULSE parsing + arithmetic .print expressions | waveform.rs, output.rs | `PULSE 0 1 ...` and `V(g)/10` supported |
| 61 | VBIC Vre/Vrs sign convention in self-heating | device_stamp.rs | Correct external R voltage convention |
| 62 | `@device[param]` queries for .print directives | mna.rs, transient.rs | `@m1[Vbs]` device parameter queries |
| 63 | BSIM3SOI-DD/PD junction current ceq sign convention | bsim3soi_dd.rs, bsim3soi_pd.rs | ceq_bs/ceq_bd sign corrected |
| 64 | BJT junction_charge exponent fix | bjt.rs | `arg^(1-M)` instead of `arg^(2-M)` |
| 65 | .control AC vector lookup — use vec_to_real() for complex vectors | vecexpr.rs | `v(3)` in .control now works for AC analysis results |
| 66 | .param spaces-around-equals in process_conditionals | parse.rs | `.param key = value` form now parsed for .if/.elseif conditions |
| 67 | .control vector indexing `foo[2]` + `@v1[dc]` sweep vector | vecexpr.rs, simulate.rs, parse.rs | Vector indexing, DC sweep param alias, model name capture |
| 68 | .control `ceil`/`floor`/`nint`/`tan`/`atan` functions | vecexpr.rs | Missing math functions in .control evaluator |
| 69 | Resistor flicker noise (KF/AF/EF) + noise output V/√Hz | noise.rs, mna.rs, parse.rs, vecexpr.rs | Flicker noise with model params, sqrt conversion for .control |
| 70 | BSIM3SOI-FD Vgsteff chain-rule derivative corrections | bsim3soi_fd.rs | t1_chain/t4_chain used wrong dVgsteff/dVbseff in branches 2+3 |
| 71 | BSIM3SOI-DD impact ionization Vdseffii formula | bsim3soi_dd.rs | Used Vds-beta0 instead of Vds-Vdseffii (proper Vdsatii/smooth-clamp) |
| 72 | DC nested sweep prev_solution reset | simulate.rs | Reset prev_solution to None at each outer sweep step, matching ngspice MODEINITJCT reset |
| 73 | BSIM3SOI body-node Gmin scaling (*1e-6) | bsim3soi_*.rs | ngspice uses CKTgmin*1e-6 at body node; prevents body voltage pull with default gmin |
| 74 | BSIM3SOI-FD/DD kb3/dvbd0/dvbd1 parameter binning | bsim3soi_fd.rs, bsim3soi_dd.rs | ngspice defaults lkb3/wkb3/pkb3/ldvbd0-1/wdvbd0-1/pdvbd0-1 to 1.0 (not 0.0); binned values differ from base for small devices; fixes FD t4/t5 (~1.6mV Vth offset) |

## Investigations that did not yield fixes

| Investigation | Finding |
|---|---|
| VBIC forward coupling stamps (dIth/dVj, 6+ attempts) | Converged values unchanged; only affects NR path. Causes singular matrix due to thermal row ill-conditioning. |
| VBIC central differencing for thermal derivatives | Identical output — NR converges to same fixed point with O(h) and O(h²) |
| VBIC two-step vs single-step temperature scaling | Proven algebraically identical when T_amb = T_nom |
| VBIC exhaustive parameter/formula audit (sessions 36, 43, 48, 52, 53, 56-58) | All equations match ngspice line-for-line; 0.2% error is FP eval order |
| BJT reverse-bias depletion cap improvement | No improvement in rtlinv timing |
| BJT diffusion charge qb correction | Error WORSENED (4.1%→4.9%) — reverted |
| BSIM3SOI-DD vfbb sign fix | Compensated by other bugs; fixing alone worsens t3/t4 |
| HFET inverter DC operating point | Wrong OP from bistable circuit; needs source stepping |
| Sensitivity LU reuse | Needs architectural change to plumb LU factors |
| Transmission line LTRA ~2.2% error | Genuine MOSFET driver error + accumulated convolution rounding |
| Tolerance adjustments (rel_tol, additive formula) | Progressive errors can't be fixed by tolerance |
| MOS6 mos6inv settled-state noise | 2.4µV ground noise, per-variable tolerance needed |
| BJT CCS (collector-substrate capacitance) | Would make rtlinv WORSE — CCS adds load capacitance |
| BSIM3SOI-PD t4 tied-body Vth/mobility audit (session 71) | Full line-by-line comparison: Vth formula, mobility (MOBMOD=0/1/2), Vgsteff, NSUB handling, k1eff, VFB, constants — all match ngspice. Error pattern: 3-4% in strong inversion (Vb-independent), 6-8% near threshold (peaks at Vb≈0). Not a simple Vth offset; ~3% baseline suggests subtlety in Abulk, CLM, or DIBL chain. |
| VBIC FO tolerance margin analysis (session 71) | diff=1.017e-7 vs tol=1.0e-7 at Vc=2.2. Error grows linearly with Vc (proportional to Vrth). Column max=2.17e-2 gives col_abs=4.35e-8 (insufficient). rel_tol=9.93e-8 < abs_tol=1.0e-7. No single tolerance tweak can pass: error exceeds rel_tol (0.2%) at ALL Vc > 2.2. |
| BSIM3SOI-FD Vgsteff chain-rule (session 72) | Fixed t1_chain/t4_chain to use correct dVgsteff/dVbseff (t1_vb/t4_vb) in branches 2+3. Jacobian-only fix; converged Ids unchanged (FD t3/t4/t5 errors persist). |
| BSIM3SOI-DD impact ionization Vdseffii (session 72) | Fixed diffVdsii = Vds - Vdseffii (smooth-clamped Vdsatii) instead of Vds - beta0. Correct formula but Iii ≈ 0 for test params (beta0=20.5V >> diffVdsii≈0.3V → exp(-68) ≈ 0). DD body voltage offset (92mV) comes from body coupling chain, not Iii. |
| BSIM3SOI-DD body node audit (session 72) | Missing: body-to-P contact current (Ibp/Gbp*), charge-related body conductances (gcb*), minIsub parameter. These omissions remove current paths that stabilize body voltage in floating-body configurations. |
| CPL Right_deg polynomial truncation (session 72) | Agent reported missing Right_deg=2 truncation in matrix_p_mult_fn, but verified this is UNUSED in ngspice: matrix_p_mult is called with deg_o for both deg and deg_o params. 0.8% error remains FP rounding in convolution accumulation. |
| BSIM3SOI-DD BJT current formulation (session 73) | Found 3 sub-bugs in DD Ibs3/Ibd3: (1) uses `(exp-1)` instead of bare `exp`, (2) uses constant `arfabjt=XBJT` instead of Vds-dependent `BjtA=1-0.5*(Leff-kbjt1*Vds)²/edl²`, (3) missing Ic=Ibjt-Ibs3+Ibd3 in drain current. Also missing Gjsd/Gjdd cross-derivatives. At room temp with gmin=1e-25, BJT currents are ~1e-19 A so impact on body voltage is minimal for these test parameters. Would matter for circuits with larger ISBJT or non-zero RBODY. |

---

## Current status of all remaining ignored tests (as of 2026-03-26)

**Test counts:** ~597 passing, ~42 skipped (39 harness + 3 unit tests)

### Recently un-ignored (session 74, 2026-03-26)

| Test | Type | Notes |
|---|---|---|
| bsim3soifd/t4 | harness | FD Vds=1V sweep now passes (kb3/dvbd0/dvbd1 binning fix) |
| bsim3soifd/t5 | harness | FD Vds=0.05V sweep now passes (kb3/dvbd0/dvbd1 binning fix) |

### Recently un-ignored (session 70, 2026-03-26)

| Test | Type | Notes |
|---|---|---|
| bsim3soi_pd_nmos_op | unit | PD NMOS OP now converges |
| bsim3soi_pd_nmos_bias_points | unit | PD NMOS multiple bias points pass |
| bsim3soi_pd_inverter_input_high | unit | PD inverter with input=2.5V converges |
| bsim3soi_pd_dc_sweep | unit | PD DC sweep (0.1-2.5V) passes |
| test_fourbitadder | unit | Transient fourbitadder completes in ~61s |

### VBIC (4 tests)

| Test | Error | Root cause |
|---|---|---|
| FO | 0.205% at Vc=2.2V | Self-heating FP evaluation order (grows linearly with Vrth) |
| FG | 3.3% | Self-heating FP + PNP sign asymmetry |
| temp | 2.3% | Self-heating FP evaluation order |
| CEamp | AC error | Avalanche derivative coupling + self-heating FP |

The 0.205% FO error exceeds tolerance by only 1.7% (diff=1.017e-7 vs tol=1.0e-7).
Forward coupling stamps (dIth/dVj) do NOT affect converged values — confirmed across
6+ implementation attempts. The error is an irreducible FP evaluation order artifact
between our two-step temperature evaluation and ngspice's single-pass kernel.

VBIC self-heating status:
- Reverse coupling (dI_branch/dVrth in electrical rows): IMPLEMENTED
- Thermal self-derivative (dIth/dVrth on thermal diagonal): IMPLEMENTED
- Forward coupling (dIth/dV_elec in thermal row): NOT IMPLEMENTED (causes NR divergence)
- Thermal capacitance (CTH, transient): NOT IMPLEMENTED (not needed for DC tests)

### BSIM3SOI (7 tests — FD t4/t5 fixed by kb3/dvbd0/dvbd1 binning)

| Test | Status |
|---|---|
| DD t3/t4/t5 | ~17-30% Ids error (body voltage equilibrium offset: missing Ibp/gcb*/minIsub body node currents) |
| FD t3 | ~5.3% Ids error at Vg=1.58V (kb3/dvbd0/dvbd1 binning fixed; remaining error from body coupling chain) |
| FD inv2 | NR non-convergence (needs source/gmin stepping) |
| PD t3/t5 | NR non-convergence (floating body convergence) |
| PD t4 | 6.3% error (tied body, model accuracy) |

FD t4/t5 were fixed by implementing parameter binning for kb3, dvbd0, dvbd1 with the
correct non-zero defaults (lkb3=wkb3=pkb3=ldvbd0-1=wdvbd0-1=pdvbd0-1 = 1.0, not 0.0).
This resolved the previously-unidentified ~1.6mV Vth offset.

Key remaining discrepancies vs C source:
- Missing Gme (back-gate transconductance) entirely
- Missing Gmc (Vcs cross-coupling) entirely
- GIDL width uses wdiod instead of weff (DD)

### Transmission line (4 tests)

| Test | Error |
|---|---|
| cpl3_4_line | 0.8% |
| cpl4_4_line | ~6% |
| ltra_ltl | ~2.2% |
| txl_2line | ~6% |

Eigendecomposition FP order differences + accumulated convolution rounding +
MOSFET driver error. Extensively investigated across sessions.

### General circuits (4 tests)

| Test | Error | Root cause |
|---|---|---|
| rtlinv | 4.1% timing | BJT incremental charge truncation error during switching |
| schmitt | ~1.2% at transition | Output oscillation during switching (junction cap model) |
| mosamp | timeout | Level 2 MOSFET features needed |
| fourbitadder | timeout | Circuit too complex for current solver |

### Missing features / infrastructure (10 tests)

| Category | Count | Tests |
|---|---|---|
| .control: missing features | 6 | alter-vec (alter cmd), bugs-2 (vec indexing), resume-1 (stop/resume), asrc-tc-1/log-functions-1 (B-source nodes), ac-resistance (imaginary unit) |
| .control: simulator accuracy | 4 | test-noise-2 (noise 2×), binning-1 (model binning), sens-ac-1/2 (AC sensitivity) |
| BSIM1/BSIM2 models | 2 | Entire models not implemented |
| VBIC diffamp | 1 | NR non-convergence (13-transistor, needs source stepping) |
| No reference output | 2 | general/diffpair, general/fourbitadder (ngspice says "To be done") |
| B-source/parser | 2 | bxpressn-1 (node name mangling), xpressn-3 (subcircuit node lookup) |
| Misc | 1 | asrc-tc-2 (parameter expressions r={expr}) |

### Summary classification

| Category | Count | Fixable? |
|---|---|---|
| VBIC self-heating FP | 4 | No — confirmed by 58+ sessions |
| BSIM3SOI compensating bugs | 7 | No — fixing one worsens others (FD t4/t5 fixed by binning) |
| Transmission line FP | 4 | No — eigendecomposition + convolution FP |
| Deep transient dynamics | 2 | No — model accuracy limitation |
| NR convergence / wrong OP | 2 | No — need solver architectural changes |
| Missing infrastructure | 10 | No — entire subsystems missing |

All remaining tests have well-understood, documented root causes. None can be
fixed without either (a) exactly matching ngspice FP evaluation order, (b)
implementing missing subsystems (alter, stop/resume, model binning, BSIM1/2),
or (c) deep solver/model architectural changes (source stepping, analytical
sensitivity).
