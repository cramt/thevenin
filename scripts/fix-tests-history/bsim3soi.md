# BSIM3SOI Test History

## Current status (4 remaining — DD t4/t5 fixed by vfbb sign, FD t4/t5 by binning, PD t4 by poly depletion)

| Test | Status |
|---|---|
| DD t3 | ~0.6% Ids error at Vd=0.24V (floating body: Gmc implemented, Gme irrelevant for Ve=0; remaining error from missing Ibp body punch-through current and full body-current linearization) |
| FD t3 | ~5.3% Ids error at Vg=1.58V (kb3/dvbd0/dvbd1 binning fixed; remaining error from body coupling chain) |
| FD inv2 | NR non-convergence (needs source/gmin stepping) |
| PD t3/t5 | ~125%/~500% Ids error (floating body voltage offset: missing body current paths) |

## DD fixes

DD t4/t5 were fixed by correcting the vfbb sign (b3soiddtemp.c line 587: vfbb = -type *
Vtm * ln(npeak/nsub), was missing -type). Also added dVbseff/dVg and dVbseff/dVd
chain-rule derivatives and Gmb0 cross-coupling in Gm/Gds. DD t3 improved from 17% to
0.63% but remains above 0.2% tolerance. Session 83 confirmed Gmc IS implemented and Gme
is irrelevant (Ve=0 in t3). Remaining error from missing Ibp (body punch-through current)
and full body-current linearization (~300 LOC needed).

## FD fixes

FD t4/t5 were fixed by implementing parameter binning for kb3, dvbd0, dvbd1 with the
correct non-zero defaults (lkb3=wkb3=pkb3=ldvbd0-1=wdvbd0-1=pdvbd0-1 = 1.0, not 0.0).

## PD fixes

PD t4 was fixed by correcting the poly gate depletion coefficient from 1e18 to 1e6.
PD t3/t5 fixed by impact ionization Vdsatii model (fix 95) and recombination current
reverse bias T11 term (fix 80).

## Key remaining discrepancies vs C source

- Missing Gme (back-gate transconductance) entirely
- Missing Gmc (Vcs cross-coupling) entirely — primary blocker for DD t3
- GIDL width uses wdiod instead of weff (DD)
- PD model: no L/W/P binning support (180+ missing coefficients), missing SOI-specific
  params (kb1, k1w1/k1w2, fbody, ntox, delvt, ~30 more), several base defaults differ
  from ngspice (k3=80 vs 0, keta=-0.047 vs -0.6). Does NOT affect t4 test (model card
  sets all critical params), but limits model completeness for other circuits.

## Session 92 finding

Session 92 discovered a critical DC sweep infrastructure bug: `newton_raphson_solve` always
used `NrMode::InitJct` for the first NR iteration, even for DC sweep continuations. This
was the actual root cause of FD t3 (previously attributed to "body coupling chain" for 90+
sessions).
