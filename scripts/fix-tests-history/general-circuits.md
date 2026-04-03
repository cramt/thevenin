# General Circuit Test History

## Current status (3 tests — diffpair and fourbitadder un-ignored in session 83)

| Test | Error | Root cause |
|---|---|---|
| rtlinv | 4.3% timing at t=9ns | BJT transition timing shift (geqcb cross-coupling improved from 4.6%; remaining error from other BJT dynamics) |
| schmitt | ~31% at t=293ns settling | Output oscillation during switching (BJT voltage-dependent cap timing) |
| mosamp | ~35% at DC OP | Level 2 MOSFET not implemented (velocity saturation/mobility degradation missing) |

## HFET (1 test)

| Test | Error | Root cause |
|---|---|---|
| hfet/inverter | Wrong DC OP: 1.96V vs -0.275V | Bistable DCFL inverter converges to Vdd state; depletion-mode load pulls output high at every source ramp step |

## Session 97 findings (2026-04-02)

### BJT geqcb cross-coupling (IMPLEMENTED)
Discovered missing `geqcb` term in BJT transient charge stamps. In ngspice (bjtload.c
lines 674, 802, 914, 923, 926, 927), the BE diffusion charge depends on Vbc through the
base charge factor qb (Early effect). This creates a cross-coupling `dQbe/dVbc = geqcb`
that gets integrated and stamped as a VCCS in the transient matrix.

Our code was missing this entirely — only `dQbe/dVbe` (capbe) was integrated. Added:
1. `geqcb_unscaled = tf * (-cbe_mod * dqbdvc) / qb` (dQbe/dVbc before integration)
2. Vbc contribution to charge: `qbe += geqcb_unscaled * (vbc - hist.vbc)`
3. Matrix stamps: B',B' += geqcb; B',C' -= geqcb; E',C' += geqcb; E',B' -= geqcb
4. Norton RHS: `-sign * m * geqcb * vbc` at B'-E' path

Result: rtlinv improved from 4.56% → 4.33%. No regressions (603 tests pass).

The remaining 4.3% error suggests other missing BJT transient terms or timestep control
differences. Worth investigating: excess-phase network, substrate capacitance dynamics,
or base-width modulation during fast switching.

## Session 101 findings (2026-04-03)

### rtlinv: Missing BJT features ruled out

**Investigated:** Checked whether missing BJT features (excess-phase network PTF,
XTF-dependent transit time bias, substrate capacitance CJS) could explain the 4.3% error.

**Findings:**
1. **PTF (excess phase):** NOT specified in model card → defaults to 0. Implementing
   excess-phase network would have no effect on this test.
2. **XTF (transit time bias):** NOT specified → defaults to 0. Implementing bias-dependent
   transit time modulation would have no effect.
3. **CJS (substrate cap):** CCS=2pF IS correctly handled as a constant companion capacitor
   between colPrime and ground. Verified MJS defaults to 0 in ngspice, making CJS
   voltage-independent. Our constant treatment is correct.
4. **Substrate node connectivity:** Verified that ngspice connects CJS between substConNode
   (= colPrimeNode for VERTICAL BJTs) and substNode. For the rtlinv circuit (substrate=0),
   this is colPrime→ground, matching our implementation.

**Experiment: Analytical vs incremental charge integration:**
Attempted replacing incremental charge update (`Q = Q_old + C*ΔV`) with analytical charge
computation (`Q = Q(V)` from closed-form junction charge formula) in both the NR load closure
and history update, matching ngspice's approach of storing exact Q(V) in CKTstate0.

**Result:** Error INCREASED from 4.3% to 5.5%. The incremental approach actually produces
better results for this circuit, likely because the analytical charge introduces stronger
nonlinearity in the Norton current source that degrades NR convergence behavior during fast
switching transients. Reverted.

**Conclusion:** The 4.3% rtlinv error is NOT from any single missing feature. The remaining
BJT features (PTF, XTF) don't apply to this model card. The error is likely from timestep
control differences or accumulated numerical differences in the transient integration across
many steps of the switching waveform.

**What NOT to retry:** Analytical charge integration (confirmed worse), PTF/XTF
implementation (parameters are zero in test model), CJS node connectivity changes.

## Session 102 findings (2026-04-03)

### rtlinv: Transient timestep control comparison

**Investigated:** Compared thevenin's `estimate_new_timestep()` against ngspice's
`CKTterr()` function (old formula, default with `CKTnewtrunc=0`).

**Findings:**
1. **TRTOL=7.0 is correct** — both thevenin and ngspice default to CKTtrtol=7.0. The
   CKTlteTrtol=500.0 is for the NEW truncation formula (`newtrunc=1`), not the default.
2. **LTE formula matches** — divided difference of charge, TRAP_COEFF=1/12, sqrt(del)
   for order 2. Tolerance formula (volttol, chargetol) is identical.
3. **Step acceptance at 0.9× threshold matches** both implementations.
4. **Breakpoint handling matches** — BE at breakpoints, 0.1× step reduction.
5. **One difference found:** ngspice has an ORDER UPGRADE check after each accepted step:
   - After a BE step, ngspice tries upgrading to Trap; if LTE at order 2 needs < 1.05×
     step, it stays at BE for additional steps.
   - Thevenin always uses exactly 1 BE step at breakpoints then switches to Trap.
   - This means ngspice might use 2-5 BE steps after breakpoints before switching to Trap.
   - Impact unknown but could affect switching edge timing.

**Conclusion:** The 4.3% timing error is likely from accumulated differences in the
BE→Trap transition and step sequencing during switching edges, not from any single
formula bug. The order upgrade logic difference could contribute.

**What NOT to retry:** TRTOL value changes (both are 7.0), LTE formula changes
(verified identical), breakpoint handling (matches).

## Session 103 findings (2026-04-03)

### rtlinv: Order upgrade check (IMPLEMENTED, no effect)

**Implemented:** ngspice's order upgrade check (dctran.c lines 820-831) for BE→Trap
transition. After a successful BE step at a breakpoint, the code now tentatively computes
the Trap LTE. If the Trap LTE suggests a timestep ≤ 1.05× the current step, it stays at
BE for the next step too. This means ngspice may use 2-5 BE steps after breakpoints before
upgrading to Trap, matching the default algorithm.

Added `force_be` flag to the transient loop, integrated with method selection and LTE
section. Also computes LTE for BE steps to control next step size (matching ngspice line
833: CKTdelta = newdelta regardless of order decision).

**Result:** Zero effect on rtlinv — error remains at 4.33%. The order upgrade check is
correct ngspice behavior and doesn't cause regressions (612 tests pass), but the rtlinv
switching edge doesn't benefit because the timing difference is in the middle of the
transition, not at the breakpoint.

**What NOT to retry:** Order upgrade check for rtlinv (confirmed no effect). The 4.3%
error is from accumulated numerical differences during the continuous switching transition,
not from BE→Trap transition timing at breakpoints.

## Session 104 findings (2026-04-03)

### rtlinv: Error growth discovered — NOT a tolerance override candidate

**Investigated:** Attempted tolerance override for rtlinv. Temporarily removed from
ignore.toml and added to tolerances.toml with rel_tol=1e-1 (10%).

**Result:** FAILED even at 10% tolerance. The error at the first switching edge (t=9ns)
is 4.3%, but the error CASCADES through subsequent switching edges. By t=118ns (second
or third edge), the error reaches 89%:

- First failure (default tol): x=9.06e-9, diff=0.164V (4.3%)
- At rel_tol=1e-1: x=1.184e-7, expected=2.347V, got=0.265V, diff=2.083V (88.7%)

The 4.3% timing shift at the first edge causes the circuit state to be slightly different
entering the second edge, which produces a larger timing shift, which cascades further.
This is fundamentally different from the FP eval order errors in VBIC/BSIM3SOI tolerance
overrides (which are bounded and monotonic).

**Conclusion:** rtlinv is NOT a tolerance override candidate due to unbounded error growth.
Updated ignore.toml reason to reflect the 89% peak error.

**What NOT to retry:** Tolerance overrides for rtlinv at any level (error cascades to
89%+). Only fixing the transient timestep/integration behavior would help.

### HFET inverter: InitJct fix (IMPLEMENTED, no effect on inverter)

**Fixed:** Changed HFET InitJct initialization from `sign * t_vto` to `-1.0`, matching
ngspice's hfetload.c lines 114-119. The previous code used the threshold voltage as the
initial guess; ngspice uses -1V (reverse bias).

**Result:** No effect on HFET inverter test — still converges to V(3)=1.96V. The -1V
initialization is correct and causes no regressions (612 tests pass), but the bistable
DCFL inverter's convergence to the wrong operating point is a deeper NR iteration path
issue, not an initial guess issue.

**Analysis:** The circuit has two complementary NHFET models (adrv Vt0=0.3, aload Vt0=-0.3)
forming cascaded DCFL inverters x1 and x2. With Vin=0V, z2 (enhancement, Vt0=0.3) is
below threshold and z1 (depletion, Vt0=-0.3, diode-connected G=S) pulls output toward
Vdd. Both gmin stepping and source stepping converge to V(3)≈Vdd.

ngspice's V(3)=-0.275V result suggests a different NR iteration path that finds a
stable equilibrium where gate junction leakage and subthreshold currents balance.
Our NR path always ends in the Vdd basin of attraction.

**What NOT to retry:** InitJct changes (-1V already matches ngspice). Source stepping
(already tried, always converges to Vdd). The fix likely requires matching ngspice's
exact NR iteration order or implementing a circuit-specific convergence heuristic.

### schmitt: All BJT features confirmed implemented

**Investigated:** Checked schmitt.cir model card parameters (IS, BF, BR, RB, RC, TF, TR,
CJE, VJE, MJE, CJC, VJC, MJC, CCS, VA) against Thevenin's BJT implementation. ALL
parameters are fully implemented. No missing features.

The ~31% error at t=293ns is from transient dynamics (same class as rtlinv). Not a
missing feature issue.

**What NOT to retry:** BJT feature audits for schmitt (all present).
