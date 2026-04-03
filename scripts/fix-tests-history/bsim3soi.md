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

## Session 97 findings (2026-04-02)

### DD RampVg2 investigation
Investigated the RampVg2 floating body voltage issue (Vbs=3.28e-5 vs expected 91.7mV).

**Finding 1: Transient .OPTIONS not propagated (FIXED)**
Discovered that `simulate_tran` used `NrOptions::default()` instead of parsing circuit
`.OPTIONS` — GMIN, ABSTOL, RELTOL, VNTOL, ITL1, ITL2 were all ignored for transient
analysis. Fixed by calling `nr_options_from_netlist(netlist)` and using those options for
both the initial DC OP (`solve_op_raw_with_opts`) and transient NR iterations. Also fixed
`simulate_op` for consistency. No regression (603 tests pass).

However, this fix alone does NOT resolve the RampVg2 body voltage issue. With the correct
gmin=1e-20, the body stability conductance (gmin*1e-6 = 1e-26) is negligible compared to
junction currents (~1e-17 S conductance). The NR solver still converges to Vbs≈0 instead
of ~92mV.

**Finding 2: DD t3 bodyMod=0 rules out Ibp**
Confirmed that the DD model card has `RBODY = 0.0` and no explicit `BODYMOD` parameter,
so bodyMod defaults to 0. With bodyMod=0, `Ibp = 0` (body punch-through current is zero).
The ignore.toml reason claiming "missing Ibp" is incorrect for this operating point — the
0.6% error must have a different root cause.

**Finding 3: DD t3 error margin is only 1.4% above tolerance**
At x=0.24, expected=2.3687e-5, got=2.3837e-5, diff=1.498e-7. Combined tolerance =
rel_tol(4.77e-8) + abs_tol(1.0e-7) = 1.477e-7. Exceeds by only 1.4%.

**What was NOT tried:** Adding debug prints during NR iterations for RampVg2 to trace the
body node equation residual at each step. Worth trying in a future session to understand
why NR converges to Vbs≈0 despite correct junction currents.

## Session 99 findings (2026-04-02)

**Goal:** Fix DD t3 (0.6% error, 1.4% over tolerance) by matching ngspice's combined body
current stamping structure.

**Attempted:**
1. Added missing gii_e (impact ionization back-gate derivative, ngspice Giie) to companion
   and matrix stamps. Correctness improvement, no numerical effect at Vd=0.24V (Iii≈0).
2. Restructured body stamping from separate component stamps (stamp_conductance + cross-coupling
   + Iii + GIDL) to combined per-node derivative blocks (gddp*/gssp*/gbb*) matching ngspice
   b3soiddld.c lines 2596-2630 and 3908-3928. Removed duplicate stamp_conductance calls.
3. Added gate-drain CKTgmin conductance (ngspice lines 4103-4110).

**Result:** All three changes produce EXACTLY the same converged Ids value (2.383699e-5).
The restructured stamping and combined body CEQ computation (cbody) generate the same FP
result as the old separate approach. The Rust compiler likely optimizes both to equivalent
FP operations.

**Conclusion:** The 0.6% error is NOT from stamp structure or FP accumulation order in the
RHS assembly. It's inherent in the model computation — the floating body voltage equilibrium
converges to a Vbs ~1.5mV different from ngspice, regardless of how the NR equations are
structured. The combined stamps are a valid correctness improvement (committed) but don't
reduce the error. The remaining fix options are:
- Full body-current chain linearization matching ngspice's combined computation (~300 LOC)
- Or accepting this as a precision limitation for floating-body DD circuits

**What NOT to retry:** Combined body stamping restructure — confirmed no effect.

## Session 100 findings (2026-04-03)

### DD t3: minIsub investigation

**Discovered:** ngspice adds `minIsub` to body current CEQs (b3soiddld.c lines 2602, 2614,
2627) and b3soiddtemp.c line 744. This is a convergence aid:
- `ceq_jd -= minIsub/2`
- `ceq_js -= minIsub/2`
- `ceq_body += minIsub`

Where `minIsub = 5e-2 * weff * tsi * max(isdif, isrec)`.

For the t3 model card: `minIsub = 5e-2 * 10e-6 * 5e-8 * 1e-5 = 2.5e-19 A`.

**Implemented:** Added `min_isub` field to `Bsim3SoiDdSizeParam` and included it in the
three CEQ terms matching ngspice exactly. No regressions (603 tests pass).

**Result:** Virtually no effect on DD t3 error (diff changed from 1.497600e-7 to 1.497900e-7,
a 3e-11 change = 1.3 ppm). The minIsub is correct but too small to affect the converged
solution at this operating point.

**Key insight discovered:** The DD model computes body voltage ANALYTICALLY through the
Vbs0t→Vbs0eff→Vbsdio→Vbsmos→Vbseff chain (ngspice lines 940-1185). The NR body node
voltage `vbs_i` feeds into `Vbsdio` via `smooth_max(vbs_i, Vbs0eff + 0.02)`, but when
`vbs_i` is near or below the analytical floor, `Vbsdio ≈ Vbs0eff + 0.02` regardless of NR
body voltage. This means:

1. The NR body node equation is SECONDARY to the analytical computation
2. All previous body node fixes (combined stamps, minIsub, junction restructure) cannot
   affect Ids because the Ids computation depends on the analytical Vbseff, not NR Vbs
3. The 0.6% error is in the analytical body voltage chain itself (Vbs0t computation,
   Nfb feedback factor, or some intermediate) — not in the NR body node balance

**What NOT to retry:** ANY body node current/conductance modification (minIsub, stamps, gmin,
junction paths). The body node doesn't control Ids in DD model — the analytical chain does.

**Next steps (for future sessions):** Compare Vbs0t, Vbs0eff, Nfb, Vbsdio, Vbsmos, Vbseff
intermediate values between Rust and ngspice at the failing operating point (Vg=0.5V,
Vd=0.24V). The 1.5mV discrepancy in the analytical body voltage chain is the root cause.

## Session 101 findings (2026-04-03)

### DD t3: Exhaustive formula verification

**Investigated:** Line-by-line comparison of the FULL analytical body voltage chain
(Vbs0t→Vbs0→Vbs0mos→Vthfd→Vbs0teff→Vbs0eff→Vbsdio→Vbsmos→Vbseff) between Rust
(bsim3soi_dd.rs lines 1243-1391) and ngspice (b3soiddld.c lines 920-1193).

**Result:** ALL 8 stages of the chain match EXACTLY term-by-term:
- Part 1 (Vbs0t): v0 = vbi - phi ✓, dvbd0/dvbd1/litl parameters ✓
- Part 2 (Vbs0): kb1 coupling, phi-delp limiter with DELT_Vbseff=0.005 ✓
- Part 3 (Vbs0mos): csieff/qsieff correction ✓
- Part 4 (Vbs0teff): Vthfd gate-threshold coupling ✓
- Part 5 (Nfb): k1, kb3*Cbox/Cox feedback factor ✓
- Part 6 (Vbsdio): smooth_max with OFF_Vbsdio=0.02 ✓
- Part 7 (Vbsmos): second capacitive coupling ✓
- Part 8 (Vbseff): final phi-delp limiter ✓

Also verified:
- Physical constants: EPSOX=3.453133e-11, EPSSI=1.03594e-10, KBOQ=8.617087e-5, CHARGE_Q=1.60219e-19 — all match
- Cbox = EPSOX/tbox (both model-level, matching)
- Vthfd formula matches (K1, K2, DVT0/1/2, ETAB, NLX, KT1/2)
- Abulk formula matches (k1eff = k1, A0, AGS, KETA)
- Temperature-dependent phi: both use precomputed phi at TNOM (test runs at default 27°C = TNOM)

**Conclusion:** The 0.6% error is NOT from any formula difference. The analytical chain,
Vthfd, Abulk, and Ids computations all match exactly. The error is from the NR-converged
body voltage being 1.5mV different (likely from FP evaluation order or NR convergence
tolerance), propagating through the analytical chain to shift Ids.

**What NOT to retry:** Formula comparison of the analytical chain (confirmed identical).
Temperature-dependent phi (irrelevant at TEMP=TNOM).
