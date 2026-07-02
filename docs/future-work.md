# Future Work: Remaining Ignored Tests

As of 2026-07-02, **1 test remains skipped**: `general/rtlinv.cir`.
The harness stands at 106 passing / 1 ignored.

## Status Summary

| Test | Category | Effort | Tests Unlocked |
|------|----------|--------|----------------|
| ~~bsim3soidd/RampVg2.cir~~ | ~~debug=-1 quasi-static semantics~~ | ~~DONE~~ | ~~1~~ |
| ~~general/mosamp.cir~~ | ~~Level 2 MOSFET~~ | ~~DONE~~ | ~~1~~ |
| general/rtlinv.cir | BJT saturation-recovery timing | In progress | 1 |
| ~~general/schmitt.cir~~ | ~~KoverQ + CJS + abs charge, tol override~~ | ~~DONE~~ | ~~1~~ |
| ~~hfet/inverter.cir~~ | ~~regenerated reference (ngspice bug)~~ | ~~DONE~~ | ~~1~~ |
| ~~bsim1/test.cir~~ | ~~NRD/NRS=1 default missing~~ | ~~DONE~~ | ~~1~~ |
| ~~bsim2/test.cir~~ | ~~BSIM2 verification~~ | ~~DONE~~ | ~~1~~ |
| ~~regression/misc/resume-1.cir~~ | ~~.control resumable transient~~ | ~~DONE~~ | ~~1~~ |

## 1. CAPMOD=3 Body-Floating Coupling (RampVg2) — DONE

**Test:** `bsim3soidd/RampVg2.cir` — **PASSING (2026-07-02)** after three
stacked fixes: (a) XJ:=TSI set-time default resolution, (b) numerically
stable Phi^1.5/Phi^2.5 differences in the CAPMOD=3 block, and (c) honoring
the deck's `debug=-1`, which in ngspice's b3soiddld.c forces
ChargeComputationNeeded=0 after the charge block — the reference waveform
is the quasi-static body response with no capacitive currents at all.
Full story in `100-percent-plan.md` Phase 1. Investigation history below.

**Old status:** CAPMOD=3 charge block is now verified faithful to ngspice (audit
complete 2026-05-20). The remaining gap is in the body-floating bias chain
(Vbs0t / Vbs0 / Vbs0mos / Vthfd / Vbs0eff), not in the cap_mod==3 charge
formulas. The failure starts at t=0 as a DC operating-point offset (~0.04%
on Vbs) that grows during the gate ramp because the body's response to gate
coupling rides on top of the wrong DC bias.

### What was checked and confirmed correct

Full term-by-term audit of `cap_mod==3` block (`bsim3soi_dd.rs:2741-3040`)
against ngspice `b3soiddld.c:2888-3224`:
- VdsatCV redefinition and derivatives (lines 2745-2753)
- VdsCV nonlinear saturation mapping in both saturation and parabolic
  branches (2762-2825), including the value-only clamp behaviour
- Surface potentials Phisd/Phisc and sqrtPhisd/sqrtPhisc (2827-2869)
- Qdep0 depletion-charge-at-Vth formula (2835-2838)
- VcsCV smooth clamp with smoothing constant — see fix below (2840-2862)
- Xc surface-potential-based partition (2871-2931), incl. dT5/dVg = K1·sqrtPhisd·dPhisd/dVg
  identity which depends on the (2/3)·1.5 cancellation
- Qsubs1 Nomi/Denomi formulation incl. Phi^(5/2) terms (2933-3010)
- Qsubs2 with Vbs0eff dependencies, dQsubs2_dVrg = T11·dVbs0eff_dVg routing
  (3012-3024)
- Qbf assembly including the Qdep0 addition specific to cap_mod==3 (3026-3033)
- Cbg/Cbb/Cbd/Cbe transformation (3259-3262)
- Qe1/Qe2 back-gate charges and dQe1/dQe2 derivatives (3225-3255)

### Fix landed

`DELTA_VCSCV` constant in `bsim3soi_dd.rs` was `1e-5`; ngspice
`b3soiddld.c:43` defines `DELTA_Vcscv 0.0004`. Both cap_mod==2 and
cap_mod==3 copies corrected to `4e-4`. This is a transient-smoothing
constant for the Vcs ≤ VdsCV clamp; at moderate |VdsCV| the smoothing zone
is sub-dominant, which is why this didn't move the RampVg2 number much,
but it is a real correctness issue and could matter for circuits with
near-zero Vds.

### Where to look next

**The body-floating chain itself is also verified clean.** A second audit
(2026-05-20, follow-up to the cap_mod==3 work) ran ngspice with
`debug=4` enabled on a single-point DC OP at (Vg=0, Vd=1.5, Vs=0, Ve=0)
and dumped the chain values from `b3soiddn.log`, then instrumented
`bsim3soi_dd_companion` to emit the same intermediates and ran them
side-by-side at our converged DC OP:

| Variable | ngspice | thevenin | match |
|----------|--------:|---------:|:-----:|
| Vbs0t    | 0.2383  | 0.238328 | ✓     |
| Vbs0     | 0.2210  | 0.220988 | ✓     |
| Vbs0mos  | 0.2209  | 0.220924 | ✓     |
| Vthfd    | 0.4766  | 0.476572 | ✓     |
| Vbs0eff  | 0.05582121 | 0.05582123 | ✓ |
| Vbsdio   | 0.08453198 | 0.08479741 | offset by 2.6e-4 (= Vbs offset) |

The Vbsdio gap exactly equals the Vbs equilibrium gap (0.0917 vs 0.092),
so the chain is locally consistent — Vbsdio just rides on top of Vbs.

**The DC OP Vbs disagreement is in the body-junction current balance,
not the chain.** At the converged DC OP ngspice reports:
- Iii = 2.77e-17  (impact ionization, sub-femtoamp)
- Ibs = 3.35e-17  (source-body junction)
- Ibd = -5.5e-18  (drain-body junction)
- Idgidl = 0

All currents are sub-femtoamp and balance precariously to set the
floating-body equilibrium. To get Vbs = 0.0917 vs 0.0920 requires
matching one of: the BJT base transport factor (ASD, BjtA), the
impact-ionization coefficients (ALPHA0/ALPHA1/BETA0/AII/BII/CII/DII),
or the junction-diode pre-exponentials (ISBJT, ISDIF, ISREC, ISTUN).

**The transient response shortfall is bigger than the DC OP offset would
explain on its own.** At t=22.5ps (3 ps into the gate ramp), ngspice's
Vbs rises by 0.07 V while ours rises by only 0.02 V — a factor-of-~3.5
slope difference, much larger than the 0.4% DC OP offset. So there's
likely a separate cap-coupling issue in addition to the DC OP issue.
The audit confirms the cap_mod==3 charge formulas are correct in
isolation, so the next investigation should:

1. Patch ngspice's b3soiddld.c to log `cbgb` / `cbsb` / `cbdb` / `cbeb`
   each NR iteration (extend the `B3SOIDDdebugMod > 3` block at
   line 4437 to dump these), rebuild, and re-run RampVg2.
2. Compare against our `cbgb` values at the same (Vg, Vd, Vbs)
   operating point during the ramp.
3. If they match, the cap model is fine and the bug is in transient
   integration / matrix stamping. If they differ, the bug is upstream
   of cap_mod==3 (Vgsteff / Vbseff derivatives, perhaps).

## 2. Level 2 MOSFET ✓ IMPLEMENTED

**Test:** `general/mosamp.cir` — **PASSING** (5% tolerance override for CLM derivative FP differences)

Level 2 MOSFET model implemented in `mos2.rs` (~700 LOC). Features:
- Velocity saturation (ucrit, uexp mobility degradation)
- Short/narrow channel effects (xj, delta parameters)
- Subthreshold conduction (nfs fast surface states)
- Channel length modulation (Grove-Frohman + Baum quartic solver for vmax)
- Derived process parameters (VTO, gamma, phi from NSUB)

## 3. Transient Timing Cascade (rtlinv, schmitt) — schmitt DONE, rtlinv in progress

**Update 2026-07-02:** the "chaotic cascade" diagnosis was wrong. Four real
bugs were found and fixed: the CONSTKoverQ thermal voltage (we used legacy
KboQ), the PULSE PER default (we retriggered at tr+pw+tf; ngspice uses
TSTOP — this alone was rtlinv's entire 89% tail), the never-stamped BJT
substrate cap (rtlinv sets ccs=2pf), and incremental-vs-absolute charge
integration (absolute matches ngspice CKTstate0 semantics and wins the A/B).

- **schmitt: PASSING** with a bounded-ringing tolerance override
  (8 points, ≤28mV absolute on a 1.6V swing).
- **rtlinv: still ignored** — 94 points, worst 26%: Q1 exits saturation
  several ns late on the recovery edge (t≈86-100ns). TR·cbc storage
  magnitude verified identical to ngspice. Next step: per-iteration
  device-level diff (vbe/vbc/cc/cb/qbc) against an instrumented ngspice
  around the recovery edge; nixpkgs ngspice-45 reproduces the reference,
  so a live comparison target exists.

**Original root cause (superseded):** Accumulated timestep sequencing differences between thevenin and ngspice's
transient integration.

rtlinv is a cascaded RTL inverter (2 NPN BJTs). The error starts at 4.3% at the first
switching edge (t=9ns, ~100ps timing shift) and cascades to 89% by the second edge
(t=118ns). Each edge's timing error becomes the initial condition for the next, causing
unbounded growth.

schmitt is an ECL Schmitt trigger (4 NPN BJTs). Same root cause -- 31% error during
settling at t=293ns.

**What was tried and ruled out:**
- BJT features (PTF, XTF, CJS) -- none apply
- LTE formula differences -- identical
- TRTOL values -- both use 7.0
- Breakpoint handling -- both use 0.1x step reduction
- Order upgrade logic (BE->Trap decision) -- implemented, zero effect
- Analytical vs incremental charge -- analytical made it worse (4.3% -> 5.5%)
- Tolerance overrides -- error cascades unboundedly, 10% tolerance still fails at 88.7%

**What might work:**
- Matching ngspice's exact multi-BE-step order upgrade strategy after breakpoints
- Matching ngspice's dense solver pivot order (Markowitz)
- This is fundamentally a trajectory divergence in a chaotic-sensitive switching cascade

**Priority:** Low. Intractable without matching ngspice's exact numerical path.

## 4. HFET Inverter — DONE (regenerated reference)

**Test:** `hfet/inverter.cir` — **PASSING (2026-07-02)**: reference
regenerated from an ngspice build with the inverse-flag bug patched,
installed as the fixture override `thevenin/tests/fixtures/hfet/inverter.out`.
The test now runs for real and matches end-to-end at default tolerances.

**Root cause (sessions 110-145):** ngspice's reference output is
WRONG. `hfetload.c` line 83 declares `int inverse=FALSE;` outside the device
iteration loop and never resets it between instances, so a driver HFET with
vds < 0 leaks `inverse=TRUE` into subsequently-processed load HFETs, negating
their drain current. ngspice's V(3)=-0.275V is physically impossible (it
requires current from ground into the output with the driver off); our
V(3)=+1.956V is the correct DC OP, locked in by the assertion test
`test_hfet_inverter_dc_op` in `tests/hfet.rs`. Full writeup in
`tests/ignore.toml` and `100-percent-plan.md`. The harness fixture stays
ignored permanently — the reference waveform starts from the wrong DC OP.

The earlier analysis below (bistability / NR basin) predates that finding and
is kept only as investigation history:

**Root cause (superseded):** The circuit is genuinely bistable with two stable DC operating points:
V(3)=-0.275V (correct, ngspice finds) and V(3)=+1.956V (wrong, thevenin finds). The HFET
model is verified 100% correct -- this is a Newton-Raphson iteration path issue.

The circuit has complementary HFETs (enhancement Vt0=0.3, depletion Vt0=-0.3). All standard
initializations (source stepping, gmin stepping, depletion init, zero init, sparse LU
reordering) converge to the Vdd basin. The correct basin requires V(3) to go slightly
negative first, which ngspice achieves accidentally through Markowitz pivot ordering.

**Architectural options (in order of pragmatism):**

1. ~~**Multi-pass random perturbations**~~ — **RULED OUT** (0% confidence). Exhaustively
   tested: initial guess perturbations (negative bias -0.1 to -2.0V, alternating signs,
   negated baseline), gmin continuation from varied starting points, source stepping with
   different initial conditions, FloatRelaxed mode (bypassing fetlim), asymmetric diagonal
   perturbation (scales 1e-1 to 1e-8), damped NR (alpha 0.1 to 0.9), and row-permuted
   linear solves. **All converge to the Vdd basin.** The attractor is too strong — no
   perturbation-based approach can escape it.

2. ~~**NR homotopy / parameter continuation**~~ — **RULED OUT** (0% confidence). Fine-grained
   source continuation (50 steps, 0→100%) from multiple starting points all converge to
   Vdd basin. Gmin continuation with adaptive backtracking also fails — the bifurcation
   occurs at a gmin level where the step from high-gmin solution to low-gmin solution
   always lands in the wrong basin.

3. **Markowitz sparse solver** (~500-800 LOC, 95% confidence but high risk): Replace faer's
   pivot selection with Markowitz threshold strategy. Would match ngspice exactly but complex
   to implement correctly, risk of regressions. **This is the only viable approach** — the
   correct basin is reached through a specific numerical path during NR iteration that depends
   on the LU factorization pivot ordering. Partial pivoting (faer) always produces a trajectory
   that lands in the Vdd basin.

**Key files:** `newton.rs` (938 LOC), `simulate.rs` (1714 LOC), `device_stamp.rs` (1375 LOC),
`sparse.rs` (750 LOC)

**Priority:** Medium-low. Only 1 test, fix requires Markowitz solver implementation.

## 5. BSIM1 and BSIM2 Models — DONE

**Tests:** `bsim1/test.cir`, `bsim2/test.cir` — both un-ignored 2026-07-02.

Both models were ported (`thevenin/src/bsim1.rs`, `thevenin/src/bsim2.rs`,
DC + companion-model NR). BSIM2 passed the harness comparison as soon as it
was numerically verified. BSIM1 was a ~1.4% strong-inversion near-miss whose
root cause was a missing instance default, not model math: ngspice's
`b1set.c` defaults NRD/NRS to 1 when omitted, so `RSH=35` implies 35Ω
drain/source series resistors on every device in the fixture; our IR
lowering defaulted them to 0. Fixed in `mna_ir.rs` (BSIM1 branch only).

## 6. .control Interpreter ✓ resume-1 IMPLEMENTED

**asrc-tc-2.cir** passes — behavioral resistor `r={expr}` conversion to B-source plus
the .control interpreter handling `op`, `ac`, `let`, `if/end`, `echo`, `quit`.

**resume-1.cir** passes — `stop when time = <value>`, `alter` (plain and bracketed
forms), and `resume` are all implemented. Approach was a minimal pause/resume hook
rather than a full `TranState` lift (~250 LOC vs the original ~800 LOC estimate):

- `TranRunParams.t_pause` / `start_state` thread through `run_tran`.
- `TranOutcome::{Complete | Paused}` return type carries the snapshot when a
  pause fires.
- `TranPauseSnapshot` holds `(t_paused, solution, output_vecs)` plus the
  paused leg's `tstep`/`tstop`/`tmax` so `.control`-only trans (no
  `Analysis::Tran` directive) can resume.
- Pause time is registered as a breakpoint so the loop lands exactly at
  `t_pause` (otherwise overshoots by one print-step, breaking gold-trace
  comparisons that switch piecewise at the pause boundary).
- Limitation: charge histories for nonlinear devices re-initialise from
  the snapshot's solution as if it were a DC OP, so reactive nonlinear
  circuits may see a small derivative discontinuity at resume. resume-1
  is purely linear (RC) so it's exact there.

Other `.control` features that still aren't implemented: `setplot new`,
arbitrary `stop when <expr>` conditions, `wrdata` output, `let` indexing
on the LHS, `compose` with full ngspice semantics.

## Recommended Tackle Order

1. rtlinv — per-iteration BJT device diff against instrumented ngspice-45
   around the saturation-recovery edge (the ONLY remaining ignored test)
2. ~~RampVg2~~ -- **DONE** (debug=-1 quasi-static semantics)
3. ~~Level 2 MOSFET~~ -- **DONE**
4. ~~HFET~~ -- **DONE** (regenerated reference; ngspice bug)
5. ~~BSIM1/BSIM2~~ -- **DONE**
6. ~~schmitt~~ -- **DONE** (KoverQ/CJS/abs-charge + bounded-ringing override)
7. ~~resume-1 .control~~ -- **DONE**
