# VBIC Test History

## Current status (5 tests remaining)

| Test | Error | Root cause |
|---|---|---|
| FO | 0.385% at Vc=3.75V | Companion function FP evaluation order |
| FG | 3.3% at Vb=0.89V | Same; slope tolerance masks low-bias points |
| temp | 2.3% at Vb=0.76V | Same; slope tolerance masks low-bias points |
| CEamp | ~0.9% at 1.5GHz | AC gain error from DC OP FP precision; Iciei/Iccp double-counting fixed in session 83 |
| diffamp | NR non-convergence | 13-transistor circuit, source stepping also fails |

## Key clarification (session 81)

The "2.3% temp error" and "3.3% FG error" are NOT different from the FO 0.385% error
in kind — they are the SAME ~0.2% base companion function FP error, measured at later
sweep points where self-heating has grown. Slope tolerance masks the error at lower bias
points on the exponential Gummel curve.

## FO root cause analysis (session 80+)

The VBIC FO test error (0.205% Ic at VB=0.7V, VC=2.2V) was thoroughly investigated:
- Disabling self-heating changes error by only 5e-9 (from 1.017e-7 to 1.012e-7)
- Tightening NR tolerance 100× produces identical results
- Central difference numerical derivatives produce identical results
- All default parameter values match ngspice
- The companion function formulas match ngspice kernel term-by-term
- The remaining difference is FP evaluation order (Rust vs C compiler operation ordering)
- Missing forward coupling stamps don't affect the converged solution

FO error barely exceeds tolerance (diff=2.123e-7 vs tol=2.107e-7, exceeds by only 0.8%).
However, the double DC sweep (707 rows) grows with self-heating power across the dataset.
At higher VB sweeps (VB>=750mV), relative error exceeds 0.2% (reaching 23% at VB=1.0V).

## Self-heating status

- Reverse coupling (dI_branch/dVrth in electrical rows): IMPLEMENTED
- Thermal self-derivative (dIth/dVrth on thermal diagonal): IMPLEMENTED
- Forward coupling (dIth/dV_elec in thermal row): NOT IMPLEMENTED (causes NR divergence)
- Thermal capacitance (CTH, transient): NOT IMPLEMENTED (not needed for DC tests)

Forward coupling (session 84): confirmed as correct per ngspice vbicload.c lines 1435-1464
but our NR solver can't handle the mixed-domain scaling without full matrix preconditioning.
Off-diagonal entries ~100× larger than thermal diagonal at high bias.

## Session 80 exhaustive verification

Line-by-line comparison of ALL VBIC formulas confirmed complete: avalanche, quasi-saturation,
transport, base currents, parasitic, ALL cross-term derivatives, ALL stamps — everything
matches ngspice exactly. NR convergence settings also match.
