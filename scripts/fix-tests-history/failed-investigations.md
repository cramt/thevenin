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
| Tolerance retightening after sparse LU change (session 126) | Tested all 7 tolerance overrides at previously-failing thresholds after PartialPivLu change (commit a570cd6). All 7 still fail at same boundaries (CEamp 1.8e-2, FG 1.8e-2, temp 1.7e-2, txl2_3 2e-2, ltra2_2 7.5e-3, t3 3.2e-2, inv2 2.5e-3). Sparse LU pivot change did not improve or worsen accuracy. |
| HFET inverter subcircuit flattening audit (session 126) | Verified Z-device terminal mapping in subckt.rs (remap_node for d,g,s individually), parser terminal order (d=0,g=1,s=2), and MNA node assignment. All correct — x1.z1 gets drain=1,gate=3,source=3; x1.z2 gets drain=3,gate=2,source=0. Not the cause of wrong basin. |
| HFET inverter model equation audit (session 126) | Agent-assisted line-by-line comparison of hfet.rs vs hfetload.c: leak(), diode_fn(), hfeta_full(), stamp function, all defaults (GGR=40, DEL=0.04, etc.). 100% match for HFET1 level=5 gatemod=0 DC case. Confirmed: wrong basin is NR iteration path issue, not model bug. |
| General triage sweep (session 126) | Ran all 12 ignored tests — all still fail. Ran triage-listed tests not in ignore.toml (rc.cir, bsim3soidd/t5, bsim3soifd/t5, bsim3soifd/t4) — all already pass at default tolerance. general/rc.cir (listed as "vacuous pass") is actually passing correctly. |
| DD RampVg2 csbox/cdbox investigation (session 127) | csbox/cdbox completely missing from bsim3soi_dd.rs, but AS/AD not specified in RampVg2 model card → default to 0 → csbox=cdbox=0. Dead end for this specific test. |
| DD RampVg2 CAPMOD=3 assessment (session 127) | Model card specifies CAPMOD=3, code only implements CAPMOD=2. But charge-up already works with CAPMOD=2 (549mV vs 553mV). Decay issue is discharge mechanism, not charge values. 300+ LOC to implement, low probability of fixing decay. |
| Full 12-test re-triage (session 127) | All 12 remaining ignored tests independently confirmed as intractable. Each maps to an explicit intractable category. Error magnitudes unchanged from previous sessions: FO 0.4%→15%+, RampVg2 50%+ decay, rtlinv 4.3%→89%, schmitt 31%, mosamp 35%, HFET wrong basin (1.96V vs -0.275V), cpl_ibm2 6.4%+sign reversal, cpl3_4_line 13.8%, bsim1/bsim2 not implemented, asrc-tc-2/resume-1 .control scripting. |
| Full re-triage + DD body voltage chain audit (session 128) | Ran all 12 ignored tests — all still fail with unchanged error magnitudes. Ran full suite: 640 pass, 12 skipped, 0 failures. Tested inv2 tolerance tightening (2.5e-3) — still fails. Agent-assisted deep audit of BSIM3SOI-DD bsim3soi_dd.rs analytical body voltage chain (Vbs0t→Vbseff, 8 stages) + temp preprocessing + Nfb feedback factor + all derivative chains: 100% match with ngspice b3soiddld.c/b3soiddtemp.c. No FP eval order differences in expression structure — all intermediate variables match C's let-binding pattern. No `#[ignore]` unit tests remain. Clippy clean, fmt clean. Triage-listed tests (rc.cir, bsim3soidd/t5, bsim3soifd/t4/t5, CEamp, FG, temp, txl2_3) all already pass. All 12 remaining ignored tests confirmed in explicit intractable categories. |
| HFET inverter deep investigation (session 129) | Exhaustive model comparison: leak(), GGR, gmg/gmd, matrix stamps, Norton currents all verified identical to ngspice. gmg/gmd are correctly zero for gatemod==0 (default). Circuit is genuinely bistable (two stable OPs: -0.275V and +1.96V). Tried: force source stepping (wrong basin at every ramp step), depletion Vgs=0 init, Vgs=Vgd=0 init — all converge to wrong basin. V(3)<0 is unreachable from any standard initialization because z1 drain current dominates z2 Schottky until V(3) is already negative (chicken-and-egg). Needs MODEINITFIX or multi-pass NR — major architectural change. |
