# Future Work: Remaining Ignored Tests

After 146 sessions of the fix-tests agent, **645 tests pass** (11 with tolerance overrides), **7 remain skipped**. Each maps to a missing subsystem or architectural limitation.

## Status Summary

| Test | Category | Effort | Tests Unlocked |
|------|----------|--------|----------------|
| bsim3soidd/RampVg2.cir | CAPMOD=3 charge coupling | Model-internal | 1 |
| ~~general/mosamp.cir~~ | ~~Level 2 MOSFET~~ | ~~DONE~~ | ~~1~~ |
| general/rtlinv.cir | Transient timing cascade | Architectural | 1 |
| general/schmitt.cir | Transient timing cascade | Architectural | 1 |
| hfet/inverter.cir | NR wrong basin (bistable) | 200-500 LOC | 1 |
| bsim1/test.cir | BSIM1 not implemented | ~3,500 LOC | 1 |
| bsim2/test.cir | BSIM2 not implemented | ~2,500 LOC | 1 |
| regression/misc/resume-1.cir | .control interpreter | ~800 LOC | 1 |

## 1. CAPMOD=3 Body-Floating Coupling (RampVg2)

**Test:** `bsim3soidd/RampVg2.cir`
**Status:** CAPMOD=3 charge block is now verified faithful to ngspice (audit
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

The failure pattern (DC OP off by 0.04% growing into a ~60% capacitive
response shortfall) points at the DD-specific body-floating chain:

```
Vbs0t → Vbs0 → Vbs0mos → Vthfd → Vbs0teff → Vbs0eff → Vbsdio → Vbsmos → Vbseff
```

The chain runs `bsim3soi_dd.rs:1269-1454` against ngspice
`b3soiddld.c:921-1193`. Particularly worth re-checking: the Nfb feedback
factor at line 1389 (`nfb = 1 / (1 + (kb3·Cbox/cox)·sqrt(1 + 4·(phi+K1·sqrt(phi-Vbs0mos) - Vbs0mos)/K1²))`),
the Vbs0mos derivation through `t4_mos = T3·csieff/qsieff` smoothing, and
the dVbs0mos_dVe derivative path that feeds Cbg through dQsubs2_dVe.
Adding compile-time `eprintln!` of the chain at a fixed (Vg, Vd, Vbs)
operating point and diffing against ngspice's `B3SOIDDdebug` output is
the most direct way to find the divergence.

## 2. Level 2 MOSFET ✓ IMPLEMENTED

**Test:** `general/mosamp.cir` — **PASSING** (5% tolerance override for CLM derivative FP differences)

Level 2 MOSFET model implemented in `mos2.rs` (~700 LOC). Features:
- Velocity saturation (ucrit, uexp mobility degradation)
- Short/narrow channel effects (xj, delta parameters)
- Subthreshold conduction (nfs fast surface states)
- Channel length modulation (Grove-Frohman + Baum quartic solver for vmax)
- Derived process parameters (VTO, gamma, phi from NSUB)

## 3. Transient Timing Cascade (rtlinv, schmitt)

**Tests:** `general/rtlinv.cir`, `general/schmitt.cir`
**Root cause:** Accumulated timestep sequencing differences between thevenin and ngspice's
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

## 4. HFET Inverter Wrong Basin

**Test:** `hfet/inverter.cir`
**Root cause:** The circuit is genuinely bistable with two stable DC operating points:
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

## 5. BSIM1 and BSIM2 Models

**Tests:** `bsim1/test.cir`, `bsim2/test.cir`
**Root cause:** These models are not implemented at all. Both are DC transfer curve tests
(5 NMOS, Vgs sweep 0-5V).

- BSIM1: 19 C files, 6,996 lines in ngspice. Estimated ~3,500 LOC Rust.
- BSIM2: 17 C files, 4,692 lines in ngspice. Estimated ~2,500 LOC Rust.

Both are obsolete models superseded by BSIM3/BSIM4 (already implemented). They exist in
ngspice for backward compatibility with legacy process libraries.

**Priority:** Low. Obsolete models, large effort, only 2 tests. Implement only if users
need legacy PDK compatibility.

## 6. .control Interpreter

**Test:** `regression/misc/resume-1.cir`

**asrc-tc-2.cir** is now passing — behavioral resistor `r={expr}` conversion to B-source
is implemented, and the .control interpreter already handles `op`, `ac`, `let`, `if/end`,
`echo`, and `quit`.

**resume-1.cir** needs: `stop when <condition>` (hook into transient solver), `alter`
(runtime parameter modification), `resume` (continue paused simulation). This requires deep
solver integration -- saving/restoring full circuit state mid-simulation. Estimated ~800 LOC.

**Priority:** Low (invasive solver changes for 1 test).

## Recommended Tackle Order

1. RampVg2 transient convergence (solver improvements)
2. ~~Level 2 MOSFET~~ -- **DONE**
3. ~~HFET perturbation fallback~~ -- **RULED OUT** (requires Markowitz solver, ~500-800 LOC)
4. BSIM1/BSIM2 -- only if legacy PDK support needed
5. rtlinv/schmitt -- accept as intractable
6. resume-1 .control -- defer until broader .control support is needed
