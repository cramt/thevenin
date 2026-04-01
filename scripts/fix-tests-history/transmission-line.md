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
