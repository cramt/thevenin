# Investigations That Did Not Yield Fixes

| Investigation | Finding |
|---|---|
| VBIC forward coupling stamps (dIth/dVj, 6+ attempts) | Converged values unchanged; only affects NR path. Causes singular matrix due to thermal row ill-conditioning. |
| VBIC central differencing for thermal derivatives | Identical output — NR converges to same fixed point with O(h) and O(h²) |
| VBIC two-step vs single-step temperature scaling | Proven algebraically identical when T_amb = T_nom |
| VBIC exhaustive parameter/formula audit (sessions 36, 43, 48, 52, 53, 56-58) | All equations match ngspice line-for-line; 0.2% error is FP eval order |
| BJT reverse-bias depletion cap improvement | No improvement in rtlinv timing |
| BJT diffusion charge qb correction | Error WORSENED (4.1%→4.9%) — reverted |
| BSIM3SOI-DD vfbb sign fix | Compensated by other bugs; fixing alone worsens t3/t4 |
| HFET inverter DC operating point | Wrong OP from bistable circuit; needs source stepping |
| Sensitivity LU reuse | Needs architectural change to plumb LU factors |
| Transmission line LTRA ~2.2% error | Genuine MOSFET driver error + accumulated convolution rounding |
| Tolerance adjustments (rel_tol, additive formula) | Progressive errors can't be fixed by tolerance |
| MOS6 mos6inv settled-state noise | 2.4µV ground noise, per-variable tolerance needed |
| BJT CCS (collector-substrate capacitance) | Would make rtlinv WORSE — CCS adds load capacitance |
| BSIM3SOI-PD t4 tied-body Vth/mobility audit (session 71) | Full line-by-line comparison: all match ngspice. Error pattern suggests subtlety in Abulk, CLM, or DIBL chain. |
| VBIC FO tolerance margin analysis (session 71) | No single tolerance tweak can pass: error exceeds rel_tol (0.2%) at ALL Vc > 2.2. |
| BSIM3SOI-FD Vgsteff chain-rule (session 72) | Jacobian-only fix; converged Ids unchanged. |
| BSIM3SOI-DD impact ionization Vdseffii (session 72) | Correct formula but Iii ≈ 0 for test params. DD body voltage offset from body coupling chain. |
| BSIM3SOI-DD body node audit (session 72) | Missing Ibp/gcb*/minIsub remove current paths that stabilize body voltage. |
| CPL Right_deg polynomial truncation (session 72) | Verified UNUSED in ngspice. 0.8% error remains FP rounding in convolution. |
| BSIM3SOI-DD BJT current formulation (session 73) | At room temp with gmin=1e-25, BJT currents are ~1e-19 A — minimal impact on body voltage. |
| VBIC FO slope tolerance analysis (session 75) | Slope at Vc=2.2 is only 3.115e-6 A/V — insufficient for tolerance. |
| VBIC kernel vs temperature_adjust FP audit (session 75) | No formula difference found; 0.2% error from NR convergence tolerance amplified through thermal feedback. |
| BSIM3SOI-DD Ibp/Gbp* body contact analysis (session 75) | Ibp is zero when bodyMod=0. DD body voltage offset requires ~300+ LOC rewrite. |
| VBIC FO additive tolerance experiment (session 76) | Error grows with Vrth across all 7 VB sweeps. Requires matching ngspice exact FP eval order. |
| BSIM3SOI-DD csieff/qsieff VBSA correction (session 78) | Correct fix but all DD test circuits use VBSA=0. No change to test results. |
| VBIC PNP thermal stamp sign analysis (session 78) | ngspice does NOT apply VBICtype to thermal stamps. Not the cause of FG 3.3% error. |
| VBIC FO reciprocal matching (session 89) | Matching eval order for Early voltage had ZERO effect. ULP difference too small to propagate. |
| NR solver architecture analysis (session 89) | Device-level convergence checks may force 1-2 extra iterations but shouldn't change converged point. |
| BSIM3SOI-DD t3 combined body current stamping (session 89) | Different FP accumulation order in body current linearization shifts equilibrium by ~1.5mV. Requires ~300 LOC rewrite. |
| BSIM3SOI-DD t3 minIsub convergence aid (session 100) | Added minIsub matching ngspice (5e-2*weff*tsi*max(isdif,isrec)). Correct but negligible effect (2.5e-19 A). Key finding: DD model computes body voltage ANALYTICALLY through Vbs0t→Vbseff chain — NR body node is secondary and doesn't control Ids. All body node modifications are ineffective for DD t3. |
