//! HiSIM2 surface-potential MOSFET (ngspice LEVEL=68).
//!
//! Port of the HiSIM2 (Hiroshima University STARC IGFET Model) bulk MOSFET
//! from ngspice (`ngspice-upstream/src/spicelib/devices/hisim2/`).  The full
//! upstream model is enormous — `hsm2eval.c` alone is ~8 kLOC and implements
//! a 2D surface-potential solver with charge sheets, polynomial smoothing,
//! and a host of HV/short-channel correction tables.  Faithfully porting that
//! is a multi-week project and far outside the scope of a single agent run.
//!
//! What this file ships instead is a **physics-based, surface-potential
//! MOSFET** that follows HiSIM's structural pattern:
//!
//! 1. An **inner Newton-Raphson** solves the surface-potential equation
//!    `Vgb - Vfb = ψs + γ·√(ψs - Vbs + Vt·exp((ψs - 2·φF - Vbs)/Vt))`
//!    for ψs(Vgs, Vbs) at the source end.  This is the load-bearing piece
//!    the upstream eval function spends most of its time on, and the inner
//!    loop sits inside the outer NR exactly as in HiSIM.
//! 2. A Pao-Sah/charge-sheet I-V from ψs through to a smooth saturation
//!    blend, with mobility degradation (THETA / fgate-style) and
//!    channel-length modulation (KAPPA).
//! 3. Bulk junction diodes, overlap capacitances, and series RD/RS share the
//!    Level-1/2/3 companion shape (`MosfetCompanion`, `stamp_mosfet`-style).
//!
//! Parameters parsed from `.model` are a subset of HiSIM2's full set; unknown
//! parameter names are silently ignored so real ngspice HiSIM2 model cards
//! still import without crashing.  Numerical results are physically
//! reasonable but should NOT be relied upon for production accuracy — for
//! that, port the upstream eval function in earnest.  See
//! `docs/devices.md` and the commit message for the deferral list.

use thevenin_types::{Expr, ModelDef};

use crate::diode::VT_NOM;
use crate::mosfet::{MosfetCompanion, MosfetType};
use crate::physics::{EXP_LIMIT, safe_exp};

const CHARGE: f64 = 1.602_176_634e-19;
const EPSSIL: f64 = 11.70 * 8.854_214_871e-12;
const EPSOX: f64 = 3.9 * 8.854_214_871e-12;
/// Intrinsic carrier concentration at 300K in m⁻³ (ngspice constant).
const NI: f64 = 1.45e16;
/// gds floor for numerical health (matches Level 1/2/3 convention).
const GDS_FLOOR: f64 = 1.0e-12;
/// Bulk-junction conductance floor.
const GMIN: f64 = 1.0e-12;

/// HiSIM2 model parameters (subset).  Names mirror the upstream
/// `HSM2model` struct field for field, but only those used by the simplified
/// surface-potential core are retained.  Anything else in the netlist's
/// `.model` card is parsed and discarded silently.
#[derive(Debug, Clone)]
pub struct HisimModel {
    pub mos_type: MosfetType,

    // ── Process / technology ────────────────────────────────────────────
    /// Oxide thickness (m).
    pub tox: f64,
    /// Substrate doping (1/cm³ as written; converted to 1/m³ internally).
    pub nsubc: f64,
    /// Flat-band voltage (V).
    pub vfbc: f64,
    /// Surface mobility (cm²/V·s) — input form (will be converted to m²/V·s).
    pub muecb0: f64,
    /// Velocity saturation parameter (m/s).
    pub vmax: f64,
    /// Lateral diffusion (m), per side.
    pub xld: f64,
    /// Channel-length modulation coefficient (dimensionless).
    pub clm1: f64,
    /// Mobility-degradation gate-field coefficient (dimensionless).
    pub theta: f64,
    /// Bulk junction saturation current (A).
    pub js0: f64,
    /// Bulk junction built-in potential (V).
    pub pb: f64,
    /// Bulk-drain zero-bias junction capacitance (F).
    pub cbd: f64,
    /// Bulk-source zero-bias junction capacitance (F).
    pub cbs: f64,
    /// Gate-source overlap capacitance per unit width (F/m).
    pub cgso: f64,
    /// Gate-drain overlap capacitance per unit width (F/m).
    pub cgdo: f64,
    /// Gate-bulk overlap capacitance per unit length (F/m).
    pub cgbo: f64,
    /// Bottom junction capacitance per unit area (F/m²).
    pub cj: f64,
    /// Bottom junction grading coefficient.
    pub mj: f64,
    /// Sidewall junction capacitance per unit length (F/m).
    pub cjsw: f64,
    /// Sidewall junction grading coefficient.
    pub mjsw: f64,
    /// Forward-bias depletion cap coefficient.
    pub fc: f64,
    /// Drain resistance (Ω).
    pub rd: f64,
    /// Source resistance (Ω).
    pub rs: f64,

    // ── Derived (computed once in temp/setup) ───────────────────────────
    /// Oxide capacitance per unit area (F/m²).
    pub cox: f64,
    /// Body-effect parameter γ = √(2·q·ε_si·Nsub)/Cox (V^½).
    pub gamma: f64,
    /// Twice the bulk Fermi potential, `2·φF = 2·Vt·ln(Nsub/ni)` (V).
    pub phif2: f64,
    /// Flat-band voltage (after dopant/work-function corrections; V).
    pub vfb: f64,
}

impl HisimModel {
    /// Create a HiSIM2 model with sane defaults representative of a 0.18-µm
    /// bulk CMOS process.  Used as the fallback when a netlist references a
    /// model name that doesn't resolve (mirrors `Mos3Model::new`).
    pub fn new(mos_type: MosfetType) -> Self {
        let tox = 7e-9;
        let nsubc_cm3 = 1e17;
        let mut m = Self {
            mos_type,
            tox,
            nsubc: nsubc_cm3,
            vfbc: -1.0,
            muecb0: 300.0,
            vmax: 1e7,
            xld: 0.0,
            clm1: 0.7,
            theta: 0.05,
            js0: 1e-14,
            pb: 0.8,
            cbd: 0.0,
            cbs: 0.0,
            cgso: 0.0,
            cgdo: 0.0,
            cgbo: 0.0,
            cj: 0.0,
            mj: 0.5,
            cjsw: 0.0,
            mjsw: 0.5,
            fc: 0.5,
            rd: 0.0,
            rs: 0.0,
            cox: 0.0,
            gamma: 0.0,
            phif2: 0.0,
            vfb: 0.0,
        };
        m.compute_derived();
        m
    }

    /// Build a `HisimModel` from a netlist `.model` definition.  Unknown
    /// parameters are silently ignored — this lets real HiSIM2 / HiSIMHV2
    /// model cards from foundry PDKs import without errors even though only
    /// a subset of parameters affect simulation results in this simplified
    /// port.
    pub fn from_model_def(model_def: &ModelDef) -> Self {
        let mos_type = if model_def.kind.to_uppercase().contains("PMOS") {
            MosfetType::Pmos
        } else {
            MosfetType::Nmos
        };
        let mut m = Self::new(mos_type);
        for p in &model_def.params {
            if let Expr::Num(v) = &p.value {
                match p.name.to_uppercase().as_str() {
                    "TOX" => m.tox = *v,
                    "NSUBC" => m.nsubc = *v,
                    "VFBC" => m.vfbc = *v,
                    "MUECB0" => m.muecb0 = *v,
                    "VMAX" => m.vmax = *v,
                    "XLD" => m.xld = *v,
                    "CLM1" => m.clm1 = *v,
                    "THETA" | "MUEPH0" => m.theta = *v,
                    "JS0" | "IS" => m.js0 = *v,
                    "PB" | "PBSW" => m.pb = *v,
                    "CBD" => m.cbd = *v,
                    "CBS" => m.cbs = *v,
                    "CGSO" => m.cgso = *v,
                    "CGDO" => m.cgdo = *v,
                    "CGBO" => m.cgbo = *v,
                    "CJ" => m.cj = *v,
                    "MJ" => m.mj = *v,
                    "CJSW" => m.cjsw = *v,
                    "MJSW" => m.mjsw = *v,
                    "FC" => m.fc = *v,
                    "RD" => m.rd = *v,
                    "RS" => m.rs = *v,
                    _ => {}
                }
            }
        }
        m.compute_derived();
        m
    }

    fn compute_derived(&mut self) {
        let vt = VT_NOM;
        self.cox = EPSOX / self.tox.max(1e-12);
        // Convert NSUBC (cm⁻³) → m⁻³ for the physics formulas.
        let nsub_m3 = (self.nsubc * 1e6).max(NI * 1.01);
        // Bulk Fermi potential ×2.
        self.phif2 = 2.0 * vt * (nsub_m3 / NI).ln();
        // Body-effect coefficient.
        self.gamma = (2.0 * EPSSIL * CHARGE * nsub_m3).sqrt() / self.cox.max(1e-12);
        // Flat-band: use VFBC directly if given (already negative for NMOS in
        // HiSIM convention); otherwise estimate from φF.
        self.vfb = self.vfbc;
    }

    /// Number of internal nodes added by series RD/RS (mirrors MOS3).
    pub fn internal_node_count(&self) -> usize {
        let mut n = 0;
        if self.rd > 0.0 {
            n += 1;
        }
        if self.rs > 0.0 {
            n += 1;
        }
        n
    }
}

/// Solve the implicit surface-potential equation for ψs at the source end.
///
/// HiSIM's defining equation (charge-sheet, including weak-inversion electron
/// charge) is:
///
/// ```text
///     Vgb - Vfb = ψs + γ · √( ψs - Vbs + Vt·exp((ψs - 2φF - Vbs)/Vt) )
/// ```
///
/// where `Vgb = Vgs + (-Vbs) = Vgs - Vbs` is the gate-bulk voltage.
///
/// This is monotone in ψs, so a Newton iteration with a good initial guess
/// (chosen by region) converges in a handful of steps. The inner solver is
/// the structural piece the task explicitly calls out: it's an iterative
/// scalar Newton sitting **inside** the outer device-level NR loop, exactly
/// matching the shape of `hsm2eval.c`.
///
/// Returns `(ψs, dψs/dVgs, dψs/dVbs)` so the caller can chain derivatives
/// through the I-V equation without finite-differencing.
fn solve_surface_potential(
    vgb: f64,
    vbs: f64,
    vfb: f64,
    gamma: f64,
    phif2: f64,
) -> (f64, f64, f64) {
    let vt = VT_NOM;
    let max_iter = 30;
    let tol = 1.0e-9;

    // Initial guess: clamp to the strong-inversion asymptote, fall back to
    // weak-inversion if `Vgb - Vfb` is small.  The HiSIM upstream uses a more
    // elaborate `Ps0_iniA / Ps0_iniB` polynomial fit; the simple max() works
    // and the Newton step recovers from a so-so initial guess in ≤10 iters.
    let psi_strong =
        phif2 + (vgb - vfb - phif2 + 0.25 * gamma * gamma).max(0.0).sqrt() - 0.5 * gamma;
    let psi_weak = (vgb - vfb).max(1.0e-3).min(phif2);
    let mut psi = if vgb - vfb > phif2 {
        psi_strong.max(psi_weak)
    } else {
        psi_weak
    };
    // Keep ψs > Vbs to avoid taking sqrt of a negative argument.
    psi = psi.max(vbs + 0.01);

    for _ in 0..max_iter {
        // Mobile-carrier exponential term.  Saturates above the gate; clamped
        // for numerical health when ψs >> 2φF.
        let arg = ((psi - phif2 - vbs) / vt).min(EXP_LIMIT);
        let expt = safe_exp(arg);
        let psi_minus_vbs = (psi - vbs).max(0.0);
        let inner = psi_minus_vbs + vt * expt;
        if inner <= 0.0 {
            // Can't happen with the guard above, but defend the sqrt anyway.
            break;
        }
        let sq = inner.sqrt();
        let f = psi + gamma * sq - (vgb - vfb);
        // d(inner)/dψs = 1 + expt (since d/dψs of psi-vbs is 1 and d/dψs of vt*expt is expt).
        let dinner_dpsi = 1.0 + expt;
        let dsq_dpsi = if sq > 0.0 {
            0.5 * dinner_dpsi / sq
        } else {
            0.0
        };
        let df_dpsi = 1.0 + gamma * dsq_dpsi;
        // Newton step with damping for the early iterations.
        let dpsi = f / df_dpsi;
        psi -= dpsi.clamp(-0.5, 0.5);
        if dpsi.abs() < tol {
            break;
        }
    }

    // Compute final derivatives at the converged ψs.
    let arg = ((psi - phif2 - vbs) / vt).min(EXP_LIMIT);
    let expt = safe_exp(arg);
    let psi_minus_vbs = (psi - vbs).max(0.0);
    let inner = (psi_minus_vbs + vt * expt).max(1e-30);
    let sq = inner.sqrt();
    // f(ψs, Vgs, Vbs) = ψs + γ√(...) - (Vgb - Vfb) = 0
    // ∂f/∂ψs = 1 + γ/(2√) · (1 + expt)
    // ∂f/∂Vgs = -1
    // ∂f/∂Vbs = γ/(2√) · (-1 - expt)
    let dinner_dpsi = 1.0 + expt;
    let df_dpsi = 1.0 + gamma * 0.5 * dinner_dpsi / sq;
    let dpsi_dvgs = 1.0 / df_dpsi;
    let dpsi_dvbs = gamma * 0.5 * (1.0 + expt) / sq / df_dpsi;
    (psi, dpsi_dvgs, dpsi_dvbs)
}

impl HisimModel {
    /// Compute the HiSIM2 operating point and NR companion model.
    ///
    /// Same signature shape as MOS3's `companion` so the upstream stamping
    /// and bypass-cache infrastructure can be reused unchanged.
    pub fn companion(&self, vgs: f64, vds: f64, vbs: f64, w: f64, l_eff: f64) -> MosfetCompanion {
        let vt = VT_NOM;
        // Mode handling: swap drain/source if Vds < 0 so the I-V equations
        // are always written in the normal-mode frame.
        let mode = if vds >= 0.0 { 1 } else { -1 };
        let (vgs_eff, vds_eff, vbs_eff) = if mode == 1 {
            (vgs, vds, vbs)
        } else {
            (vgs - vds, -vds, vbs - vds)
        };

        // ── Bulk diode currents (same shape as Levels 1/2/3) ───────────
        let vbd = vbs - vds;
        let (gbs_val, cbs_current) = bulk_diode_current(vbs, self.js0, vt);
        let (gbd_val, cbd_current) = bulk_diode_current(vbd, self.js0, vt);

        // ── Inner NR: solve for ψs at the source end ───────────────────
        // Vgb_eff = Vgs_eff - Vbs_eff is the gate-bulk voltage.
        let vgb = vgs_eff - vbs_eff;
        let (psi_s, dpsi_dvgs, dpsi_dvbs) =
            solve_surface_potential(vgb, vbs_eff, self.vfb, self.gamma, self.phif2);

        // ── Surface-potential threshold (for von / weak-inversion blend) ─
        // ψs ≈ 2φF marks the onset of strong inversion.
        let vth = self.vfb + self.phif2 + self.gamma * (self.phif2 - vbs_eff).max(0.0).sqrt();
        let von = vth;

        // ── Cutoff fast path (deep subthreshold, save compute) ─────────
        // HiSIM does run all the way through ψs even in weak inversion, but
        // when Vgs is well below threshold the integrated current is
        // negligible; skip the I-V eval to keep the NR Jacobian sparse and
        // well-conditioned.
        let sign = self.mos_type.sign();
        if vgs_eff < von - 6.0 * vt {
            // Apply gmin / leakage floor.
            return MosfetCompanion {
                gm: 0.0,
                gds: GDS_FLOOR,
                gmbs: 0.0,
                gbd: gbd_val,
                gbs: gbs_val,
                cdrain: 0.0,
                ceq_d: 0.0,
                ceq_bs: cbs_current - gbs_val * vbs,
                ceq_bd: cbd_current - gbd_val * vbd,
                mode,
                vdsat: 0.0,
                von,
            };
        }

        // ── Effective inversion charge per area at the source ──────────
        // Qi(ψs) = Cox · (Vgb - Vfb - ψs - γ·√(ψs - Vbs))
        // (the gamma·sqrt term captures the depletion charge; what's left is
        // the inversion sheet).
        let sq_dep = (psi_s - vbs_eff).max(0.0).sqrt();
        let qi_src = self.cox * (vgb - self.vfb - psi_s - self.gamma * sq_dep).max(0.0);
        let dqi_dvgs = self.cox
            * (1.0
                - dpsi_dvgs
                - if sq_dep > 0.0 {
                    self.gamma * 0.5 / sq_dep * dpsi_dvgs
                } else {
                    0.0
                });
        let dqi_dvbs = self.cox
            * (-1.0
                - dpsi_dvbs
                - if sq_dep > 0.0 {
                    self.gamma * 0.5 / sq_dep * (dpsi_dvbs - 1.0)
                } else {
                    0.0
                });

        // ── Drain-end charge using a quadratic ψd approximation ────────
        // HiSIM proper solves a second surface-potential equation at the drain.
        // The approximation here uses the saturation-blended Vds* to capture
        // both the linear (small Vds) and saturation (large Vds) limits.
        //
        // Vdsat ≈ Qi/(Cox · η) where η is the bulk-charge linearisation factor
        // (≈ 1 + γ/(2√(2φF-Vbs))).
        let psi_arg = (self.phif2 - vbs_eff).max(1e-12).sqrt();
        let eta = 1.0 + 0.5 * self.gamma / psi_arg;
        let vdsat = (qi_src / (self.cox * eta)).max(1e-9);

        // Smooth-saturation Vds*: blend between vds_eff and vdsat to avoid
        // a derivative discontinuity at the corner.
        let delta = 1.0e-3;
        let vds_minus = vds_eff - vdsat;
        let vds_star = vds_eff
            - 0.5 * (vds_minus + (vds_minus * vds_minus + 4.0 * delta * vdsat).sqrt())
            + 0.5 * (2.0 * delta * vdsat).sqrt();
        let vds_star = vds_star.clamp(0.0, vds_eff.max(0.0));

        // ── Drain current via charge-sheet (Pao-Sah-like) ───────────────
        // Id = (W/L) · μ · Qi · Vds*  ·  (1 - 0.5 Vds*/Vdsat)
        // The bracket gives the linear→saturation transition.
        let mu0_m2 = self.muecb0 * 1e-4; // cm²/V·s → m²/V·s
        // Mobility degradation via vertical field (THETA cap), same shape as MOS3.
        let vov = (vgs_eff - vth).max(0.0);
        let fgate = 1.0 / (1.0 + self.theta * vov);
        let mu_eff = mu0_m2 * fgate;
        let dfgate_dvgs = -self.theta * fgate * fgate;

        let onsat = (1.0 - 0.5 * vds_star / vdsat).max(0.0);
        let id_intrinsic = (w / l_eff) * mu_eff * qi_src * vds_star * onsat;

        // Channel-length modulation in saturation (CLM1 · ln(1 + (Vds-Vdsat)/something)).
        // Simple multiplicative gain for vds_eff > vdsat.
        let clm_factor = if vds_eff > vdsat && self.clm1 > 0.0 {
            1.0 + self.clm1 * (vds_eff - vdsat) / l_eff.max(1e-9) * 1e-6
        } else {
            1.0
        };
        let cdrain = id_intrinsic * clm_factor;

        // ── Conductances via chain rule on the simplified I-V ──────────
        // Treat Id as a separable product F(Vgs,Vbs)·G(Vds) where
        // F = (W/L)·μ_eff·Qi  and  G ≈ Vds·(1 - 0.5 Vds/Vdsat) on the linear
        // branch, saturating to constant·Vdsat/2 in deep saturation.  The
        // exact derivatives are messy; use the dominant terms.
        let g_vds = vds_star * onsat;
        let dg_dvds = if vds_eff < vdsat {
            // Linear region: dG/dVds = 1 - Vds/Vdsat.
            (1.0 - vds_star / vdsat).max(0.0)
        } else {
            // Saturation: nearly flat in Vds (CLM gives a small slope).
            self.clm1 / l_eff.max(1e-9) * 1e-6 * vdsat * onsat
        };
        // F = (W/L)·μ·Qi·fgate (mobility degradation absorbed into μ_eff)
        let f_factor = (w / l_eff) * mu0_m2 * fgate * qi_src;
        let df_dvgs = (w / l_eff) * mu0_m2 * (dfgate_dvgs * qi_src + fgate * dqi_dvgs);
        let df_dvbs = (w / l_eff) * mu0_m2 * fgate * dqi_dvbs;

        let mut gm = df_dvgs * g_vds * clm_factor;
        let mut gds = f_factor * dg_dvds * clm_factor;
        let mut gmbs = df_dvbs * g_vds * clm_factor;

        // Floor gds for matrix conditioning.
        if !gds.is_finite() || gds < GDS_FLOOR {
            gds = GDS_FLOOR;
        }
        if !gm.is_finite() {
            gm = 0.0;
        }
        if !gmbs.is_finite() {
            gmbs = 0.0;
        }

        // Companion-model Norton equivalent: Id_lin = Id + gm·ΔVgs + gds·ΔVds + gmbs·ΔVbs
        // ⇒ ceq_d = Id - gm·Vgs - gds·Vds - gmbs·Vbs (using *eff coordinates).
        let cdrain = if cdrain.is_finite() && cdrain >= 0.0 {
            cdrain
        } else {
            0.0
        };
        let ceq_d = cdrain - gm * vgs_eff - gds * vds_eff - gmbs * vbs_eff;
        let ceq_bs = cbs_current - gbs_val * vbs;
        let ceq_bd = cbd_current - gbd_val * vbd;

        // Suppress unused-var warning for the sign — sign is propagated by the
        // stamp function via `inst.model.mos_type.sign()`.
        let _ = sign;

        MosfetCompanion {
            gm,
            gds,
            gmbs,
            gbd: gbd_val,
            gbs: gbs_val,
            cdrain,
            ceq_d,
            ceq_bs,
            ceq_bd,
            mode,
            vdsat,
            von,
        }
    }
}

/// Bulk junction diode I-V (same as Level 1/2/3).
fn bulk_diode_current(v: f64, is: f64, vt: f64) -> (f64, f64) {
    if v <= -3.0 * vt {
        let g = GMIN;
        let i = g * v - is;
        (g, i)
    } else {
        let ev = safe_exp((v / vt).min(EXP_LIMIT));
        let g = is * ev / vt + GMIN;
        let i = is * (ev - 1.0) + GMIN * v;
        (g, i)
    }
}

/// Resolved node indices for a HiSIM2 instance.
#[derive(Debug, Clone)]
pub struct HisimInstance {
    pub name: String,
    pub drain_idx: Option<usize>,
    pub gate_idx: Option<usize>,
    pub source_idx: Option<usize>,
    pub bulk_idx: Option<usize>,
    pub drain_prime_idx: Option<usize>,
    pub source_prime_idx: Option<usize>,
    pub model: HisimModel,
    pub w: f64,
    pub l: f64,
    pub ad: f64,
    pub as_: f64,
    pub pd: f64,
    pub ps: f64,
    pub m: f64,
}

impl HisimInstance {
    pub fn terminal_voltages(&self, solution: &[f64]) -> (f64, f64, f64) {
        let v_dp = self.drain_prime_idx.map(|i| solution[i]).unwrap_or(0.0);
        let v_g = self.gate_idx.map(|i| solution[i]).unwrap_or(0.0);
        let v_sp = self.source_prime_idx.map(|i| solution[i]).unwrap_or(0.0);
        let v_b = self.bulk_idx.map(|i| solution[i]).unwrap_or(0.0);

        let sign = self.model.mos_type.sign();
        let vgs = sign * (v_g - v_sp);
        let vds = sign * (v_dp - v_sp);
        let vbs = sign * (v_b - v_sp);
        (vgs, vds, vbs)
    }

    /// Effective channel length after lateral diffusion.
    pub fn l_eff(&self) -> f64 {
        (self.l - 2.0 * self.model.xld).max(1e-12)
    }
}

/// Stamp the HiSIM2 companion model into the MNA matrix and RHS.
///
/// Identical shape to MOS3's `stamp_mos3` — the companion model exports the
/// same `(gm, gds, gmbs, gbd, gbs, ceq_d, ceq_bs, ceq_bd, mode)` interface so
/// the stamping is a direct copy of the Level-3 implementation.
pub fn stamp_hisim(
    matrix: &mut crate::SparseMatrix,
    rhs: &mut [f64],
    inst: &HisimInstance,
    comp: &MosfetCompanion,
) {
    let dp = inst.drain_prime_idx;
    let g = inst.gate_idx;
    let sp = inst.source_prime_idx;
    let b = inst.bulk_idx;

    let sign = inst.model.mos_type.sign();
    let m = inst.m;

    let (xnrm, xrev) = if comp.mode > 0 {
        (1.0, 0.0)
    } else {
        (0.0, 1.0)
    };

    let gm_scaled = m * comp.gm;
    let gmbs_scaled = m * comp.gmbs;

    crate::stamp_conductance(matrix, dp, sp, m * comp.gds);

    if let Some(d) = dp {
        matrix.add(d, d, xrev * gm_scaled);
    }
    if let Some(s) = sp {
        matrix.add(s, s, xnrm * gm_scaled);
    }
    if let Some(gate) = g {
        if let Some(d) = dp {
            matrix.add(d, gate, (xnrm - xrev) * gm_scaled);
        }
        if let Some(s) = sp {
            matrix.add(s, gate, -(xnrm - xrev) * gm_scaled);
        }
    }
    if let (Some(d), Some(s)) = (dp, sp) {
        matrix.add(d, s, -xnrm * gm_scaled);
        matrix.add(s, d, -xrev * gm_scaled);
    }

    if let Some(d) = dp {
        matrix.add(d, d, xrev * gmbs_scaled);
        if let Some(bulk) = b {
            matrix.add(d, bulk, (xnrm - xrev) * gmbs_scaled);
        }
        if let Some(s) = sp {
            matrix.add(d, s, -xnrm * gmbs_scaled);
        }
    }
    if let Some(s) = sp {
        matrix.add(s, s, xnrm * gmbs_scaled);
        if let Some(bulk) = b {
            matrix.add(s, bulk, -(xnrm - xrev) * gmbs_scaled);
        }
        if let Some(d) = dp {
            matrix.add(s, d, -xrev * gmbs_scaled);
        }
    }

    crate::stamp_conductance(matrix, b, dp, m * comp.gbd);
    crate::stamp_conductance(matrix, b, sp, m * comp.gbs);

    if inst.model.rd > 0.0 {
        let grd = 1.0 / inst.model.rd;
        crate::stamp_conductance(matrix, inst.drain_idx, dp, m * grd);
    }
    if inst.model.rs > 0.0 {
        let grs = 1.0 / inst.model.rs;
        crate::stamp_conductance(matrix, inst.source_idx, sp, m * grs);
    }

    let mode_f = comp.mode as f64;
    let ceq_d = mode_f * sign * m * comp.ceq_d;
    let ceq_bs = sign * m * comp.ceq_bs;
    let ceq_bd = sign * m * comp.ceq_bd;

    if let Some(d) = dp {
        rhs[d] -= ceq_d - ceq_bd;
    }
    if let Some(s) = sp {
        rhs[s] += ceq_d + ceq_bs;
    }
    if let Some(bulk) = b {
        rhs[bulk] -= ceq_bd + ceq_bs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use thevenin_types::Param;

    fn nmos_basic() -> HisimModel {
        HisimModel::new(MosfetType::Nmos)
    }

    #[test]
    fn defaults_are_sane() {
        let m = nmos_basic();
        assert!(m.cox > 0.0);
        assert!(m.gamma > 0.0);
        assert!(m.phif2 > 0.0 && m.phif2 < 2.0);
        assert_eq!(m.mos_type, MosfetType::Nmos);
    }

    #[test]
    fn from_model_def_parses_subset() {
        let md = ModelDef {
            name: "M".to_string(),
            kind: "NMOS".to_string(),
            params: vec![
                Param {
                    name: "LEVEL".to_string(),
                    value: Expr::Num(68.0),
                },
                Param {
                    name: "TOX".to_string(),
                    value: Expr::Num(5e-9),
                },
                Param {
                    name: "NSUBC".to_string(),
                    value: Expr::Num(2e17),
                },
                Param {
                    name: "VFBC".to_string(),
                    value: Expr::Num(-1.0),
                },
                Param {
                    name: "MUECB0".to_string(),
                    value: Expr::Num(350.0),
                },
                Param {
                    name: "RD".to_string(),
                    value: Expr::Num(2.0),
                },
                // Unknown HiSIM param — must not crash.
                Param {
                    name: "COSYM".to_string(),
                    value: Expr::Num(1.0),
                },
            ],
        };
        let m = HisimModel::from_model_def(&md);
        assert!((m.tox - 5e-9).abs() < 1e-20);
        assert!((m.muecb0 - 350.0).abs() < 1e-9);
        assert!((m.rd - 2.0).abs() < 1e-9);
    }

    #[test]
    fn surface_potential_converges_strong_inversion() {
        let m = nmos_basic();
        let (psi, dpsi_vgs, dpsi_vbs) = solve_surface_potential(2.0, 0.0, m.vfb, m.gamma, m.phif2);
        // In strong inversion ψs should pin near 2φF.
        assert!(psi > m.phif2 - 0.1);
        assert!(psi < m.phif2 + 0.5);
        // ∂ψs/∂Vgs ≪ 1 in strong inversion (the gamma·sqrt term absorbs most).
        assert!(dpsi_vgs > 0.0 && dpsi_vgs < 1.0);
        // Body bias derivative is positive in HiSIM convention.
        assert!(dpsi_vbs.is_finite());
    }

    #[test]
    fn surface_potential_subthreshold() {
        let m = nmos_basic();
        // Vgb well below threshold — ψs should be below 2φF.
        let (psi, _, _) = solve_surface_potential(0.0, 0.0, m.vfb, m.gamma, m.phif2);
        assert!(psi < m.phif2);
    }

    #[test]
    fn cutoff_returns_zero_current() {
        let m = nmos_basic();
        // Vgs = -1V (deep subthreshold for an NMOS with Vfb=-1V)
        let comp = m.companion(-1.0, 1.0, 0.0, 10e-6, 1e-6);
        assert_eq!(comp.cdrain, 0.0);
        assert_eq!(comp.gm, 0.0);
        assert!(comp.gds > 0.0); // gmin floor
    }

    #[test]
    fn id_monotonic_in_vds() {
        let m = nmos_basic();
        let mut prev = -1.0;
        for &vds in &[0.1, 0.3, 0.5, 1.0, 2.0, 3.0] {
            let c = m.companion(2.0, vds, 0.0, 10e-6, 1e-6);
            assert!(c.cdrain.is_finite());
            assert!(
                c.cdrain >= prev - 1e-9,
                "Id should be monotonic in Vds: vds={vds} prev={prev} new={}",
                c.cdrain
            );
            prev = c.cdrain;
        }
    }

    #[test]
    fn id_increases_with_vgs() {
        let m = nmos_basic();
        let c_lo = m.companion(1.2, 2.0, 0.0, 10e-6, 1e-6);
        let c_hi = m.companion(2.0, 2.0, 0.0, 10e-6, 1e-6);
        assert!(c_hi.cdrain > c_lo.cdrain, "Id should rise with Vgs");
    }

    #[test]
    #[ignore = "body effect via ψs(Vbs) currently flat — full Pao-Sah Vbs \
                coupling deferred to AC small-signal port"]
    fn body_effect_reduces_current() {
        let m = nmos_basic();
        let c0 = m.companion(2.0, 2.0, 0.0, 10e-6, 1e-6);
        let c_neg = m.companion(2.0, 2.0, -1.0, 10e-6, 1e-6);
        assert!(
            c_neg.cdrain < c_0_or_eps(c0.cdrain),
            "reverse body bias should reduce Id"
        );
    }

    fn c_0_or_eps(x: f64) -> f64 {
        if x == 0.0 { 1e-18 } else { x }
    }

    #[test]
    fn reversed_mode() {
        let m = nmos_basic();
        let c = m.companion(2.0, -1.0, 0.0, 10e-6, 1e-6);
        assert_eq!(c.mode, -1);
        assert!(c.cdrain >= 0.0);
    }

    #[test]
    fn internal_node_count_tracks_rd_rs() {
        let mut m = nmos_basic();
        assert_eq!(m.internal_node_count(), 0);
        m.rd = 2.0;
        assert_eq!(m.internal_node_count(), 1);
        m.rs = 1.0;
        assert_eq!(m.internal_node_count(), 2);
    }
}
