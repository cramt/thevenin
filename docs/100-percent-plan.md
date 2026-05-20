# Plan: 100% ngspice Test Coverage

Current state: **101 harness tests passing, 6 skipped** (1020 total tests passing across
all test binaries). Goal: eliminate as many skips as possible.

## Phase 1: RampVg2 Charge Coupling (bsim3soidd/RampVg2.cir)

**Effort:** large (300+ LOC, model-internal) | **Confidence:** low | **Tests unlocked:** 1

NR convergence is **no longer the issue** — bypass and no-floor fixes have landed and the
solve converges cleanly through ITL4=10. The actual remaining gap is **device physics**:
during the 100ps gate ramp (Vg: 0 → 2 V over t=20-120ps), our Vbs only climbs to ~0.25 V
where ngspice reaches ~0.55 V. The body-to-gate charge coupling `dqbf/dvg` is roughly
half what ngspice computes.

A smaller bounded drift (~1% over 3ps) also appears in the pre-ramp holding region,
suggesting our CAPMOD=3 dQ/dt isn't quite zero at the steady state.

### Where to look

Our `thevenin/src/bsim3soi_dd.rs` `cap_mod == 3` branch (around lines 2741-3260) maps to
ngspice `sys/b3soiddld.c` lines 2888-3224. The divergence is almost certainly in one of
the qbf-contributing terms (`qac0`, `qsub0`, `qsubs1_3`, `qsubs2_3`, `qdep0`) or their
chain-rule derivatives feeding `cbg/cbb/cbd/cbe`.

### Optional follow-up: SOI-DD LTE estimation

SOI-DD CAPMOD=3 charges are tracked in `soidd_charge_histories` but never fed into
`estimate_new_timestep`. Without LTE feedback from the charge derivatives, the adaptive
timestep has no visibility into how fast the SOI body charges are changing. This is a
separate concern from the charge-coupling magnitude bug above and likely won't fix it
alone.

**Fix shape:** Add SOI-DD charges to `estimate_new_timestep`, following the existing BJT
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

## Phase 3: resume-1 Control Interpreter — DONE

**Status: PASSING.** resume-1.cir was unignored when `stop when`/`resume` landed; the
harness is now 101/0/6.

The original estimate was ~500-600 LOC for a full `TranState` lift. The actual
implementation came in much smaller (~250 LOC) by leaning on existing
machinery:

- `Statement::StopWhen(StopCondition::TimeEq)` and `Statement::Resume` AST
  variants with parser support (`stop when time = <value>`).
- `TranRunParams` gained two optional fields: `t_pause` (consumed in the
  main step loop) and `start_state` (gates whether the loop initialises
  from DC OP or from a snapshot). No state-machine lift required.
- A `TranPauseSnapshot` carries the `(t_paused, solution, output_vecs)`
  triple plus the original `tstep`/`tstop`/`tmax` so a resumed leg can
  drive a `.control`-only tran (where the netlist's `Analysis::Tran` is
  reconstructed from the snapshot rather than parsed from a directive).
- `run_tran` returns `TranOutcome::{Complete | Paused}` instead of a bare
  `SimResult`. Non-pause callers (`simulate_tran_with_mna`,
  `thevenin::circuit::simulate_tran`) extract via `.into_result()`.
- `execute_resume` in `thevenin-control` re-assembles the MNA from the
  altered netlist, applies TEMPER so changed tc1/tc2 take effect, then
  invokes `run_tran` with `start_state = Some(snapshot)`.
- Plain-form `alter` (no `@` prefix) was already implemented in Stage 4
  Phase C, so no work was needed there.
- The pause time is registered as an extra breakpoint so the loop clamps
  to land exactly at `t_pause` (otherwise the paused leg's last sample
  overshoots by one print-step, breaking comparisons against piecewise
  golden traces).
- Two tokenizer fixes: `vecexpr` now recognises ngspice word-form
  comparison operators (`le`, `gt`, `lt`, `ge`), and SPICE time literals
  with an `s` suffix (`1ms`, `200us`) strip the `s` so the SI prefix
  resolves.

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
| Baseline (pre-resume-1) | — | 100/107 | 93.5% |
| Phase 3 (resume-1) | +1 (DONE) | 101/107 | 94.4% |
| Phase 1 (RampVg2) | +1 | 102/107 | 95.3% |
| Phase 2 | — (ngspice bug) | 102/107 | 95.3% |
| Intractable | — | 102/107 | 95.3% |

Best realistic outcome: **102/107 harness tests (95.3%)**. The remaining 5 tests are:
- 2 obsolete models (BSIM1, BSIM2) — not worth implementing
- 2 chaotic timing cascades (rtlinv, schmitt) — would require exact numerical path matching
- 1 ngspice bug (HFET inverter) — our code is correct, ngspice is wrong
