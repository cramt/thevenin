# Plan: 100% ngspice Test Coverage

Current state: 100 harness tests passing, 7 skipped (488 total tests passing across all
test binaries). Goal: eliminate as many skips as possible.

## Phase 1: RampVg2 Transient Convergence (bsim3soidd/RampVg2.cir)

**Effort:** ~20 LOC | **Confidence:** 85% | **Tests unlocked:** 1

SOI-DD CAPMOD=3 transient NR oscillates within ITL4=10 iterations per timestep. The
dev_gmin and fallback chain have been fixed, but the core issue is likely SOI-DD charge
Jacobian accuracy — all other DD tests pass.

### Optional follow-up: SOI-DD LTE estimation

SOI-DD CAPMOD=3 charges are tracked in `soidd_charge_histories` but never fed into
`estimate_new_timestep`. Without LTE feedback from the charge derivatives, the adaptive
timestep has no visibility into how fast the SOI body charges are changing. This causes
blind timestep shrinking at gate edges.

**Fix:** Add SOI-DD charges to `estimate_new_timestep`, following the existing BJT
charge LTE pattern. (~40-80 LOC, medium risk)

---

## Phase 2: HFET Inverter — RESOLVED (ngspice bug)

~~**Effort:** ~50-200 LOC | **Confidence:** 60% | **Tests unlocked:** 1~~

**Status: CLOSED — ngspice reference output is wrong.**

After exhaustive investigation across sessions 110-145, the root cause was identified as
a confirmed bug in ngspice's `hfetload.c`:

**The bug:** On line 83, `int inverse=FALSE;` is declared outside the device iteration
loop and is never reset between device instances. When a driver HFET with `vds < 0` sets
`inverse = TRUE` (line 298), this flag leaks to all subsequently processed load HFETs.
The leaked flag causes the load devices' `cdrain` to be wrongly negated (line 305:
`cdrain = -cdrain`), flipping the sign of their drain current RHS contributions.

**Circuit analysis proving ngspice is wrong:**

The DCFL inverter subcircuit has a depletion load (VT0=-0.3, always ON at VGS=0) and an
enhancement driver (VT0=0.3, OFF when VGS < 0.3V). With VIN=0V:

- Driver VGS = 0V < VT0 = 0.3V → driver is OFF, no current path to ground
- Load VGS = 0V > VT0 = -0.3V → load is ON, pulls output toward VDD
- Therefore V(3) must be close to VDD = 2V

ngspice reports V(3) = -0.275V, which is physically impossible: it would require current
flowing from ground to the output node with the driver off. The only explanation is the
leaked inverse flag flipping the load's drain current sign.

**Our result:** V(3) ≈ 1.956V, V(4) ≈ 0.206V — the physically correct DC operating
point, verified by the assertion test `test_hfet_inverter_dc_op` in `tests/hfet.rs`.

**What was tried before root-causing:**
- Verified HFET1 model (hfeta, leak, stamp) matches ngspice 100% (sessions 110-130)
- Verified fetlim, pnjlim voltage limiting matches exactly
- Verified InitJct bias (vgs=vgd=-1) matches ngspice
- Compared NR trajectories iteration by iteration with debug-instrumented ngspice builds
- Ruled out: InitFix mode, node damping, convergence check timing, matrix assembly,
  gate leakage, sparse vs dense solver, Markowitz ordering
- Built ngspice from source 3 times with increasing debug instrumentation to trace the
  inverse flag leak

**Fixes landed in the process:**
- FullPivLu for dense solver (better numerical stability for ill-conditioned HFET matrices)
- Depletion-mode InitJct bias to break symmetry in DCFL circuits
- Transient NR simplified to match ngspice dctran (no gmin stepping fallback)
- Gmin stepping fixed to only elevate diagonal, not device-model gmin
- ITL4 option parsing added
- Transient output column ordering fixed (descending node index)
- All debug instrumentation removed

The harness test is ignored with full documentation in `ignore.toml`.

---

## Phase 3: resume-1 Control Interpreter (regression/misc/resume-1.cir)

**Effort:** ~500-600 LOC | **Confidence:** 90% | **Tests unlocked:** 1

The test needs `stop when time = <val>`, plain-form `alter v1=-5`, and `resume`.

### 3a: Refactor `simulate_tran` into resumable `TranState` (~200-300 LOC)

Lift the ~30 local variables in the transient loop into a `TranState` struct. Expose
`step_until(t_pause)` and `resume_until(t_stop)` methods. Purely mechanical — no
algorithm changes.

### 3b: `stop when` parser + executor (~50 LOC)

Add `Statement::Stop` AST variant, parse `stop when time = <val>`, store
`stop_time: Option<f64>` in `SimContext`.

### 3c: Plain-form `alter` + netlist mutation (~100 LOC)

Parse `alter v1=-5` and `alter r1=100` (no `@` prefix). Mutate the actual netlist
elements so the resumed simulation sees new values.

### 3d: `resume` executor (~40 LOC)

Look up `ctx.paused_tran`, call `TranState::resume_until`, merge time vectors.

---

## Phase 4: Accept as Intractable

### rtlinv + schmitt (general/rtlinv.cir, general/schmitt.cir)

Chaotic switching cascades where ~100ps timing shift at first edge grows to 89% error.
Every reasonable approach exhaustively tried (sessions 97-103). Would require matching
ngspice's exact numerical integration path. **Accept as-is.**

### BSIM1 + BSIM2 (bsim1/test.cir, bsim2/test.cir)

~6,000 LOC combined for obsolete models superseded by BSIM3/BSIM4 (already
implemented). **Skip unless users need legacy PDK compatibility.**

### HFET inverter (hfet/inverter.cir)

ngspice's reference output is wrong due to the inverse flag bug documented above. Our
code produces the correct result. **Not a thevenin bug — ngspice bug.**

---

## Projected Outcome

| Phase | Tests Fixed | Running Total | Pass Rate |
|-------|------------|---------------|-----------|
| Current | — | 100/107 | 93.5% |
| Phase 1 | +1 | 101/107 | 94.4% |
| Phase 2 | — (ngspice bug) | 101/107 | 94.4% |
| Phase 3 | +1 | 102/107 | 95.3% |
| Intractable | — | 102/107 | 95.3% |

Best realistic outcome: **102/107 harness tests (95.3%)**. The remaining 5 tests are:
- 2 obsolete models (BSIM1, BSIM2) — not worth implementing
- 2 chaotic timing cascades (rtlinv, schmitt) — would require exact numerical path matching
- 1 ngspice bug (HFET inverter) — our code is correct, ngspice is wrong
