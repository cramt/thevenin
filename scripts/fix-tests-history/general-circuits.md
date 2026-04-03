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
