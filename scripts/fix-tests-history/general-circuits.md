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
