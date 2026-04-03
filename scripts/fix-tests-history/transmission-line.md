# Transmission Line Test History

## Current status (4 tests)

| Test | Error |
|---|---|
| cpl3_4_line | 0.8% V(2) at t=20.3ns |
| cpl_ibm2 | ~6.4% at t=9.65ns |
| ltra2_2_line | ~5.8% V(3) at t=29.3ns |
| txl2_3_line | ~2.4% V(2) at t=16.2ns |

Eigendecomposition FP order differences + accumulated convolution rounding +
CMOS inverter switching error accumulation through cascaded stages. Extensively
investigated across sessions.

## Slope window radius (session 84)

Increasing the comparison slope window from 5 to 7 or 10 does NOT help cpl3_4_line
(new failure points appear at adjacent timesteps) or any other ignored test. The errors
are genuine simulation discrepancies, not tolerance calibration issues.

## Session 102 findings (2026-04-03)

### CPL setup code: exhaustive comparison with ngspice

Performed line-by-line comparison of ALL CPL setup functions between thevenin cpl.rs
and ngspice cplsetup.c:
- `pade_apx` / `Pade_apx`: identical matrix setup, Gaussian elimination, root finding
- `matrix_p_mult_fn` / `matrix_p_mult`: identical polynomial convolution and normalization
- `poly_match` / `match`: identical polint calls and point removal (0-based vs 1-based correctly mapped)
- `eval_frequency`: identical eigenvalue scaling (Scaling_F, Scaling_F2)
- `loop_zy`: identical two-stage eigendecomposition
- `eval_si_si_1`: identical Gauss-Jordan inversion
- `approx_mode`: identical Taylor expansion and exponential convolution recurrence

**Conclusion:** The CPL setup code is a faithful port of ngspice. The "compensating
setup bug" mentioned in commit 22f3962 (exposed by polint Neville fix) is NOT in the
setup code. It is likely in the transient convolution update functions:
- `update_cnv_cpl()` or `update_delayed_cnv_cpl()` — convolution coefficient computation
- `get_pvs_vi()` — delayed value interpolation from history
- `prepare_cpl_transient()` h2t/h3t contribution assembly

**What NOT to retry:** CPL setup code comparison (verified identical).
