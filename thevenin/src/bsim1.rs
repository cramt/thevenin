//! BSIM1 — Berkeley Short-channel IGFET Model (LEVEL=4).
//!
//! Ports the ngspice BSIM1 implementation (`ngspice-upstream/src/spicelib/
//! devices/bsim1/{b1ld.c, b1eval.c, b1temp.c, b1set.c}`) to thevenin's
//! companion-model interface.
//!
//! BSIM1 is the original Berkeley short-channel IGFET model published in 1985.
//! Threshold-voltage is `Von = Vfb + Phi + K1*sqrt(Phi-Vbs) - K2*(Phi-Vbs)
//! - Eta*Vds` where Eta can depend on Vbs and Vds. Mobility degrades with
//! gate field via `Ugs` and with drain field via `Uds`. Velocity saturation
//! produces `VdsSat = Vgs_Vth / (A*sqrt(K))`. Subthreshold uses an
//! exponential fit with N0/NB/ND slope. All process parameters carry
//!   explicit `_L`/`_W` size-dependence coefficients that are binned per
//!   instance.
//!
//! Companion-model shape matches `mos2`/`mos3`/`mos6`: emit `gm`, `gds`,
//! `gmbs`, bulk-diode `gbd`/`gbs`, `cdrain`, and `ceq_*` equivalent currents,
//! then `stamp_bsim1` paints the MNA matrix using the standard MOSFET
//! conductance stencil. AC capacitance modelling (small-signal `c**b` terms)
//! is intentionally out of scope for the 1.0 DC + NR companion cut.

use crate::model_params::ModelParams;

use crate::diode::VT_NOM;
use crate::mosfet::{MosfetCompanion, MosfetType};
use crate::physics::{EXP_LIMIT, safe_exp};

/// Default oxide thickness for BSIM1 in microns (ngspice b1set.c sets no
/// default — but b1temp.c computes Cox = 3.453e-13 / (TOX*1e-4) and crashes
/// on TOX=0; we mirror by defaulting to 0.03 µm which is also what the
/// regression fixture uses).
const DEFAULT_TOX_UM: f64 = 0.03;

/// Cox numerator from ngspice b1temp.c: `Cox = 3.453e-13 / (TOX[µm] * 1e-4)`
/// in F/cm². Constant chosen so Cox·TOX = 3.453e-13 F/cm·µm.
const COX_NUMERATOR: f64 = 3.453e-13;

/// BSIM1 model parameters (per `.model` statement). Mirrors `B1model`
/// in `bsim1def.h`. Suffixes follow ngspice: bare = baseline, `_l` = inverse
/// length sensitivity (per µm), `_w` = inverse width sensitivity (per µm).
#[derive(Debug, Clone)]
pub struct Bsim1Model {
    pub mos_type: MosfetType,

    // ── Vfb (flat-band) — Vfb = vfb0 + vfb_l/Leff + vfb_w/Weff ────────────
    pub vfb0: f64,
    pub vfb_l: f64,
    pub vfb_w: f64,

    // ── Surface potential Phi (strong inversion) ──────────────────────────
    pub phi0: f64,
    pub phi_l: f64,
    pub phi_w: f64,

    // ── Body-effect coefficients K1, K2 ────────────────────────────────────
    pub k1_0: f64,
    pub k1_l: f64,
    pub k1_w: f64,
    pub k2_0: f64,
    pub k2_l: f64,
    pub k2_w: f64,

    // ── DIBL coefficient Eta (Vds dependence of Vth) ──────────────────────
    pub eta0: f64,
    pub eta_l: f64,
    pub eta_w: f64,

    // ── X2E (Vbs dependence of Eta) ───────────────────────────────────────
    pub eta_b0: f64,
    pub eta_bl: f64,
    pub eta_bw: f64,

    // ── X3E (Vds dependence of Eta) ───────────────────────────────────────
    pub eta_d0: f64,
    pub eta_dl: f64,
    pub eta_dw: f64,

    // ── Lateral and width reduction (µm) ──────────────────────────────────
    pub delta_l: f64,
    pub delta_w: f64,

    // ── Mobility @ Vds=0, Vgs=Vth: MUZ + sensitivities ────────────────────
    pub mob_zero: f64,
    pub mob_zero_b0: f64,
    pub mob_zero_bl: f64,
    pub mob_zero_bw: f64,

    // ── Mobility @ Vds=VDD, Vgs=Vth: MUS + sensitivities ──────────────────
    pub mob_vdd0: f64,
    pub mob_vdd_l: f64,
    pub mob_vdd_w: f64,
    pub mob_vdd_b0: f64,
    pub mob_vdd_bl: f64,
    pub mob_vdd_bw: f64,
    pub mob_vdd_d0: f64,
    pub mob_vdd_dl: f64,
    pub mob_vdd_dw: f64,

    // ── U0 (Vgs dep of mobility) + sensitivities ──────────────────────────
    pub ugs0: f64,
    pub ugs_l: f64,
    pub ugs_w: f64,
    pub ugs_b0: f64,
    pub ugs_bl: f64,
    pub ugs_bw: f64,

    // ── U1 (Vds dep of mobility / velocity sat) + sensitivities ───────────
    pub uds0: f64,
    pub uds_l: f64,
    pub uds_w: f64,
    pub uds_b0: f64,
    pub uds_bl: f64,
    pub uds_bw: f64,
    pub uds_d0: f64,
    pub uds_dl: f64,
    pub uds_dw: f64,

    // ── Subthreshold slope N0/NB/ND + sensitivities ───────────────────────
    pub subth_slope0: f64,
    pub subth_slope_l: f64,
    pub subth_slope_w: f64,
    pub subth_slope_b0: f64,
    pub subth_slope_bl: f64,
    pub subth_slope_bw: f64,
    pub subth_slope_d0: f64,
    pub subth_slope_dl: f64,
    pub subth_slope_dw: f64,

    // ── Process + supply ──────────────────────────────────────────────────
    /// Oxide thickness (µm — input unit, kept as-is for Cox computation).
    pub tox_um: f64,
    /// Supply voltage used to define MUS (VDD).
    pub vdd: f64,
    /// Operating temp (°C), stored but currently unused (TNOM=27°C only).
    pub temp_c: f64,

    // ── Overlap caps (F/m and F/m for CGBO) ───────────────────────────────
    pub cgso: f64,
    pub cgdo: f64,
    pub cgbo: f64,

    /// Channel charge partitioning flag (0 = 40/60, 1 = 0/100). Affects AC
    /// only; included for parameter table completeness.
    pub xpart: bool,

    // ── Junction params (shared with Level 1/2) ───────────────────────────
    /// Sheet resistance (Ω/square).
    pub rsh: f64,
    /// Junction saturation current density (A/m²).
    pub js: f64,
    /// Bulk junction built-in potential (V).
    pub pb: f64,
    /// Bottom junction grading coefficient.
    pub mj: f64,
    /// Sidewall junction built-in potential (V).
    pub pbsw: f64,
    /// Sidewall junction grading coefficient.
    pub mjsw: f64,
    /// Bottom junction capacitance per unit area (F/m²).
    pub cj: f64,
    /// Sidewall junction capacitance per unit length (F/m).
    pub cjsw: f64,
    /// Default channel width (µm).
    pub default_width_um: f64,
    /// Channel-end length reduction (µm).
    pub delta_length_um: f64,

    /// Flicker noise coefficient.
    pub kf: f64,
    /// Flicker noise exponent.
    pub af: f64,

    // ── Derived (cached after parameter parsing) ──────────────────────────
    /// Oxide capacitance per unit area in F/cm² (computed in `B1temp`).
    pub cox_f_per_cm2: f64,
}

impl Bsim1Model {
    /// Construct a `Bsim1Model` with all coefficients zeroed (matches
    /// `B1setup` defaults). Suitable for tests that hand-tweak specific
    /// fields.
    pub fn new(mos_type: MosfetType) -> Self {
        let tox_um = DEFAULT_TOX_UM;
        let cox = COX_NUMERATOR / (tox_um * 1.0e-4);
        Self {
            mos_type,
            vfb0: 0.0,
            vfb_l: 0.0,
            vfb_w: 0.0,
            phi0: 0.0,
            phi_l: 0.0,
            phi_w: 0.0,
            k1_0: 0.0,
            k1_l: 0.0,
            k1_w: 0.0,
            k2_0: 0.0,
            k2_l: 0.0,
            k2_w: 0.0,
            eta0: 0.0,
            eta_l: 0.0,
            eta_w: 0.0,
            eta_b0: 0.0,
            eta_bl: 0.0,
            eta_bw: 0.0,
            eta_d0: 0.0,
            eta_dl: 0.0,
            eta_dw: 0.0,
            delta_l: 0.0,
            delta_w: 0.0,
            mob_zero: 0.0,
            mob_zero_b0: 0.0,
            mob_zero_bl: 0.0,
            mob_zero_bw: 0.0,
            mob_vdd0: 0.0,
            mob_vdd_l: 0.0,
            mob_vdd_w: 0.0,
            mob_vdd_b0: 0.0,
            mob_vdd_bl: 0.0,
            mob_vdd_bw: 0.0,
            mob_vdd_d0: 0.0,
            mob_vdd_dl: 0.0,
            mob_vdd_dw: 0.0,
            ugs0: 0.0,
            ugs_l: 0.0,
            ugs_w: 0.0,
            ugs_b0: 0.0,
            ugs_bl: 0.0,
            ugs_bw: 0.0,
            uds0: 0.0,
            uds_l: 0.0,
            uds_w: 0.0,
            uds_b0: 0.0,
            uds_bl: 0.0,
            uds_bw: 0.0,
            uds_d0: 0.0,
            uds_dl: 0.0,
            uds_dw: 0.0,
            subth_slope0: 0.0,
            subth_slope_l: 0.0,
            subth_slope_w: 0.0,
            subth_slope_b0: 0.0,
            subth_slope_bl: 0.0,
            subth_slope_bw: 0.0,
            subth_slope_d0: 0.0,
            subth_slope_dl: 0.0,
            subth_slope_dw: 0.0,
            tox_um,
            vdd: 0.0,
            temp_c: 27.0,
            cgso: 0.0,
            cgdo: 0.0,
            cgbo: 0.0,
            xpart: false,
            rsh: 0.0,
            js: 0.0,
            pb: 0.1,
            mj: 0.0,
            pbsw: 0.1,
            mjsw: 0.0,
            cj: 0.0,
            cjsw: 0.0,
            default_width_um: 0.0,
            delta_length_um: 0.0,
            kf: 0.0,
            af: 1.0,
            cox_f_per_cm2: cox,
        }
    }

    /// Build a `Bsim1Model` from a netlist `.model` definition.
    ///
    /// Recognises all ngspice BSIM1 model keywords from `b1.c`: process
    /// parameters with L/W sensitivities, junction parameters, overlap
    /// capacitances, and flicker noise. NMOS/PMOS is determined by the
    /// `.model` kind (case-insensitive).
    pub fn from_params(model: &ModelParams) -> Self {
        let mos_type = if model.kind.to_uppercase().contains("PMOS") {
            MosfetType::Pmos
        } else {
            MosfetType::Nmos
        };
        let mut m = Self::new(mos_type);
        for (name, v) in &model.params {
            match name.to_uppercase().as_str() {
                // Vfb (X = baseline) — ngspice keyword: VFB, LVFB, WVFB
                "VFB" => m.vfb0 = *v,
                "LVFB" => m.vfb_l = *v,
                "WVFB" => m.vfb_w = *v,
                // Phi
                "PHI" => m.phi0 = *v,
                "LPHI" => m.phi_l = *v,
                "WPHI" => m.phi_w = *v,
                // K1
                "K1" => m.k1_0 = *v,
                "LK1" => m.k1_l = *v,
                "WK1" => m.k1_w = *v,
                // K2
                "K2" => m.k2_0 = *v,
                "LK2" => m.k2_l = *v,
                "WK2" => m.k2_w = *v,
                // Eta (DIBL)
                "ETA" => m.eta0 = *v,
                "LETA" => m.eta_l = *v,
                "WETA" => m.eta_w = *v,
                // X2E (Vbs dep of Eta)
                "X2E" => m.eta_b0 = *v,
                "LX2E" => m.eta_bl = *v,
                "WX2E" => m.eta_bw = *v,
                // X3E (Vds dep of Eta)
                "X3E" => m.eta_d0 = *v,
                "LX3E" => m.eta_dl = *v,
                "WX3E" => m.eta_dw = *v,
                // Δ length / width (µm)
                "DL" => m.delta_l = *v,
                "DW" => m.delta_w = *v,
                // MUZ + X2MZ (Vbs dep of MUZ)
                "MUZ" => m.mob_zero = *v,
                "X2MZ" => m.mob_zero_b0 = *v,
                "LX2MZ" => m.mob_zero_bl = *v,
                "WX2MZ" => m.mob_zero_bw = *v,
                // MUS + sensitivities
                "MUS" => m.mob_vdd0 = *v,
                "LMUS" => m.mob_vdd_l = *v,
                "WMUS" => m.mob_vdd_w = *v,
                "X2MS" => m.mob_vdd_b0 = *v,
                "LX2MS" => m.mob_vdd_bl = *v,
                "WX2MS" => m.mob_vdd_bw = *v,
                "X3MS" => m.mob_vdd_d0 = *v,
                "LX3MS" => m.mob_vdd_dl = *v,
                "WX3MS" => m.mob_vdd_dw = *v,
                // U0 (Vgs dep of mobility): ngspice keyword is U0
                "U0" => m.ugs0 = *v,
                "LU0" => m.ugs_l = *v,
                "WU0" => m.ugs_w = *v,
                "X2U0" => m.ugs_b0 = *v,
                "LX2U0" => m.ugs_bl = *v,
                "WX2U0" => m.ugs_bw = *v,
                // U1 (Vds dep / velocity sat): ngspice keyword is U1
                "U1" => m.uds0 = *v,
                "LU1" => m.uds_l = *v,
                "WU1" => m.uds_w = *v,
                "X2U1" => m.uds_b0 = *v,
                "LX2U1" => m.uds_bl = *v,
                "WX2U1" => m.uds_bw = *v,
                "X3U1" => m.uds_d0 = *v,
                "LX3U1" => m.uds_dl = *v,
                "WX3U1" => m.uds_dw = *v,
                // N0, NB, ND subthreshold
                "N0" => m.subth_slope0 = *v,
                "LN0" => m.subth_slope_l = *v,
                "WN0" => m.subth_slope_w = *v,
                "NB" => m.subth_slope_b0 = *v,
                "LNB" => m.subth_slope_bl = *v,
                "WNB" => m.subth_slope_bw = *v,
                "ND" => m.subth_slope_d0 = *v,
                "LND" => m.subth_slope_dl = *v,
                "WND" => m.subth_slope_dw = *v,
                // Process
                "TOX" => m.tox_um = *v,
                "TEMP" => m.temp_c = *v,
                "VDD" => m.vdd = *v,
                // Overlap caps
                "CGSO" => m.cgso = *v,
                "CGDO" => m.cgdo = *v,
                "CGBO" => m.cgbo = *v,
                // Channel-charge partitioning
                "XPART" => m.xpart = *v != 0.0,
                // Junction params
                "RSH" => m.rsh = *v,
                "JS" => m.js = *v,
                "PB" => m.pb = *v,
                "MJ" => m.mj = *v,
                "PBSW" => m.pbsw = *v,
                "MJSW" => m.mjsw = *v,
                "CJ" => m.cj = *v,
                "CJSW" => m.cjsw = *v,
                "WDF" => m.default_width_um = *v,
                "DELL" => m.delta_length_um = *v,
                // Flicker noise
                "KF" => m.kf = *v,
                "AF" => m.af = *v,
                // Recognised but no-op in this DC port
                "LEVEL" => {}
                _ => {} // ignore unknown
            }
        }
        // Limit junction potentials (ngspice b1temp.c lines 37-42).
        if m.pb < 0.1 {
            m.pb = 0.1;
        }
        if m.pbsw < 0.1 {
            m.pbsw = 0.1;
        }
        // Compute Cox in F/cm² (ngspice b1temp.c line 44).
        m.cox_f_per_cm2 = COX_NUMERATOR / (m.tox_um.max(1.0e-8) * 1.0e-4);
        m
    }

    /// Returns the number of internal nodes added by source-drain series
    /// resistance. BSIM1 in ngspice always splits drain/source via `Rsh` and
    /// adds prime nodes when sheet-resistance × squares is non-zero.
    pub fn internal_node_count(&self, nrd: f64, nrs: f64) -> usize {
        let mut count = 0;
        if self.rsh > 0.0 && nrd > 0.0 {
            count += 1;
        }
        if self.rsh > 0.0 && nrs > 0.0 {
            count += 1;
        }
        count
    }
}

/// Per-instance derived process parameters (output of `b1temp.c`). Stored on
/// the instance because BSIM1 binning maps the model's `_L`/`_W` coefficients
/// onto the instance's geometry. Same shape as `B1instance` fields in
/// `bsim1def.h`.
#[derive(Debug, Clone, Default)]
pub struct Bsim1Sized {
    /// Flat-band voltage (V).
    pub vfb: f64,
    /// Surface potential (V).
    pub phi: f64,
    /// Body-effect coeff 1.
    pub k1: f64,
    /// Body-effect coeff 2.
    pub k2: f64,
    /// DIBL coefficient.
    pub eta: f64,
    /// Vbs dependence of Eta.
    pub eta_b: f64,
    /// Vds dependence of Eta.
    pub eta_d: f64,
    /// Beta @ Vds=0 (A/V²).
    pub beta_zero: f64,
    /// Vbs dep of beta_zero.
    pub beta_zero_b: f64,
    /// Beta @ Vds=Vdd (A/V²).
    pub beta_vdd: f64,
    /// Vbs dep of beta_vdd.
    pub beta_vdd_b: f64,
    /// Vds dep of beta_vdd (clamped ≥ 0).
    pub beta_vdd_d: f64,
    /// U0 (Vgs dep of mobility).
    pub ugs: f64,
    /// Vbs dep of U0.
    pub ugs_b: f64,
    /// U1 (Vds dep / velocity sat) — already divided by Leff_µm.
    pub uds: f64,
    /// Vbs dep of U1 — already divided by Leff_µm.
    pub uds_b: f64,
    /// Vds dep of U1 — already divided by Leff_µm.
    pub uds_d: f64,
    /// Subthreshold slope baseline.
    pub subth_slope: f64,
    /// Vbs dep of subthreshold slope.
    pub subth_slope_b: f64,
    /// Vds dep of subthreshold slope.
    pub subth_slope_d: f64,
    /// Vth at Vbs=Vds=0 (V).
    pub vt0: f64,
    /// Effective channel length in *meters*.
    pub l_eff_m: f64,
    /// Effective channel length in *micrometers* (for U1 normalisation).
    pub l_eff_um: f64,
    /// Effective channel width in meters.
    pub w_eff_m: f64,
    /// Drain conductance (sheet resistance × NRD), in S. Zero if no Rsh.
    pub drain_conductance: f64,
    /// Source conductance (sheet resistance × NRS), in S.
    pub source_conductance: f64,
}

/// Resolved BSIM1 instance after geometry binning and `b1temp.c` setup.
#[derive(Debug, Clone)]
pub struct Bsim1Instance {
    pub name: String,
    pub drain_idx: Option<usize>,
    pub gate_idx: Option<usize>,
    pub source_idx: Option<usize>,
    pub bulk_idx: Option<usize>,
    pub drain_prime_idx: Option<usize>,
    pub source_prime_idx: Option<usize>,
    pub model: Bsim1Model,
    /// Channel width (m).
    pub w: f64,
    /// Channel length (m).
    pub l: f64,
    /// Drain area (m²).
    pub ad: f64,
    /// Source area (m²).
    pub as_: f64,
    /// Drain perimeter (m).
    pub pd: f64,
    /// Source perimeter (m).
    pub ps: f64,
    /// Squares of drain diffusion.
    pub nrd: f64,
    /// Squares of source diffusion.
    pub nrs: f64,
    /// Parallel multiplier.
    pub m: f64,
    /// Derived per-instance process parameters (output of `b1temp.c`).
    pub sized: Bsim1Sized,
}

impl Bsim1Instance {
    /// Get terminal voltages from the solution vector, handling PMOS sign.
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

    /// Compute the BSIM1 DC operating point and NR companion model.
    ///
    /// This is the b1ld.c + b1eval.c equivalent. Returns the standard
    /// `MosfetCompanion` so it can be stamped through `stamp_bsim1`.
    pub fn companion(&self, vgs: f64, vds: f64, vbs: f64) -> MosfetCompanion {
        let vt = VT_NOM; // CONSTvt0 in ngspice
        let model = &self.model;

        // Determine mode (b1ld.c line 396-401): vds >= 0 → normal, else inverse.
        let mode = if vds >= 0.0 { 1 } else { -1 };

        // Junction saturation currents (b1ld.c lines 143-149).
        let mut drain_sat = self.ad * model.js;
        if drain_sat < 1.0e-15 {
            drain_sat = 1.0e-15;
        }
        let mut source_sat = self.as_ * model.js;
        if source_sat < 1.0e-15 {
            source_sat = 1.0e-15;
        }

        // Bulk diodes (b1ld.c lines 379-394). Same shape as Level 1-6.
        let (gbs_val, cbs_current) = bulk_diode_current(vbs, source_sat, vt);
        let vbd = vbs - vds;
        let (gbd_val, cbd_current) = bulk_diode_current(vbd, drain_sat, vt);

        // For inverse mode b1ld.c calls `B1evaluate(-vds, vbd, vgd, ...)` so
        // the load function receives positive voltages and `swap source/drain`
        // happens at stamp time via `xnrm`/`xrev`.
        let (vds_eval, vbs_eval, vgs_eval) = if mode == 1 {
            (vds, vbs, vgs)
        } else {
            let vgd = vgs - vds;
            (-vds, vbd, vgd)
        };

        let eval = self.evaluate(vds_eval, vbs_eval, vgs_eval);

        // Save signed `Von` and `Vdsat` per ngspice convention (b1ld.c 417-418).
        // (We don't persist them across NR; they're returned in the companion.)
        let _ = (eval.von, eval.vdsat);

        // Equivalent drain current source — same shape as b1ld.c line 678.
        // For thevenin we keep everything in the "normal" reference frame and
        // let `stamp_bsim1` orient via mode. The drain current returned in the
        // companion is the *device-frame* drain current (positive for vds>0,
        // PMOS sign flip handled at terminal_voltages).
        let cdrain = mode as f64 * eval.drain_current;

        // For RHS construction in stamp routine we follow ngspice exactly:
        //   ceq_d   = type * (cdrain - gds*vds - gm*vgs - gmbs*vbs)
        // but our stamp wants device-frame quantities so we use the eval-frame
        // `gm`,`gds`,`gmbs` and corresponding voltages.
        let ceq_d =
            eval.drain_current - eval.gds * vds_eval - eval.gm * vgs_eval - eval.gmbs * vbs_eval;

        MosfetCompanion {
            gm: eval.gm,
            gds: eval.gds.max(1e-12),
            gmbs: eval.gmbs,
            gbd: gbd_val,
            gbs: gbs_val,
            cdrain,
            ceq_d,
            ceq_bs: cbs_current - gbs_val * vbs,
            ceq_bd: cbd_current - gbd_val * vbd,
            mode,
            vdsat: eval.vdsat.max(0.0),
            von: eval.von,
        }
    }

    /// Port of `B1evaluate` (b1eval.c). Computes the drain current and its
    /// derivatives w.r.t. terminal voltages in the *normal* reference frame
    /// (caller maps inverse mode by flipping signs). Inputs and outputs are
    /// in untyped MOSFET coordinates (Vds, Vbs, Vgs all referenced to source).
    fn evaluate(&self, vds: f64, vbs: f64, vgs: f64) -> Bsim1EvalOut {
        let s = &self.sized;
        let vt = VT_NOM;

        let vfb = s.vfb;
        let phi = s.phi;
        let k1 = s.k1;
        let k2 = s.k2;
        let vdd = self.model.vdd;

        // b1eval.c lines 159-186: process Ugs, Uds, Eta into local effective
        // values. Each carries derivatives w.r.t. Vbs / Vds.
        let ugs;
        let dugs_dvbs;
        let raw_ugs = s.ugs + s.ugs_b * vbs;
        if raw_ugs <= 0.0 {
            ugs = 0.0;
            dugs_dvbs = 0.0;
        } else {
            ugs = raw_ugs;
            dugs_dvbs = s.ugs_b;
        }

        let mut uds;
        let duds_dvbs;
        let duds_dvds;
        // Uds is already divided by Leff_µm in setup.
        let raw_uds = s.uds + s.uds_b * vbs + s.uds_d * (vds - vdd);
        if raw_uds <= 0.0 {
            uds = 0.0;
            duds_dvbs = 0.0;
            duds_dvds = 0.0;
        } else {
            uds = raw_uds;
            duds_dvbs = s.uds_b;
            duds_dvds = s.uds_d;
        }
        // Note: b1eval.c divides Uds by Leff *after* the raw_uds compare. We
        // pre-divided in `Bsim1Sized` (matching b1eval.c lines 170-173).
        // (The pre-divided values are already in sized.uds, sized.uds_b, sized.uds_d.)
        let _ = (&mut uds, &dugs_dvbs);

        let mut eta = s.eta + s.eta_b * vbs + s.eta_d * (vds - vdd);
        let mut deta_dvds = s.eta_d;
        let mut deta_dvbs = s.eta_b;
        if eta <= 0.0 {
            eta = 0.0;
            deta_dvds = 0.0;
            deta_dvbs = 0.0;
        } else if eta > 1.0 {
            eta = 1.0;
            deta_dvds = 0.0;
            deta_dvbs = 0.0;
        }

        // Vpb and sqrt(Vpb) (b1eval.c 187-192).
        let vpb = if vbs < 0.0 { phi - vbs } else { phi };
        let sqrt_vpb = vpb.max(1.0e-12).sqrt();

        // Threshold voltage (b1eval.c 193-197).
        let von = vfb + phi + k1 * sqrt_vpb - k2 * vpb - eta * vds;
        let vth = von;
        let dvth_dvds = -eta - deta_dvds * vds;
        // Match ngspice b1eval.c:196 — unconditional formula across the Vbs=0 seam.
        // ngspice intentionally keeps the K2 / K1·SqrtVpb terms in the Vbs ≥ 0 region
        // (where chain-rule would give 0 because Vpb is held at Phi) for Jacobian
        // continuity through Newton-Raphson.
        let dvth_dvbs = k2 - 0.5 * k1 / sqrt_vpb - deta_dvbs * vds;
        let vgs_vth = vgs - vth;

        // Bulk-charge factor G, A (b1eval.c 199-204).
        let g = 1.0 - 1.0 / (1.744 + 0.8364 * vpb);
        let mut a = 1.0 + 0.5 * g * k1 / sqrt_vpb;
        if a < 1.0 {
            a = 1.0;
        }
        let arg = (1.0 + ugs * vgs_vth).max(1.0);
        let dg_dvbs = -0.8364 * (1.0 - g) * (1.0 - g);
        // dA/dVbs only contributes when vbs < 0 (dVpb/dVbs = -1); when vbs ≥ 0
        // vpb is constant and dG/dVbs = dA/dVbs = 0.
        let da_dvbs = if vbs < 0.0 {
            0.25 * k1 / sqrt_vpb * (2.0 * dg_dvbs + g / vpb)
        } else {
            0.0
        };

        let mut drain_current = 0.0;
        let mut gm = 0.0;
        let mut gds = 0.0;
        let mut gmbs = 0.0;
        let vds_sat;

        if vgs_vth < 0.0 {
            // Cutoff (b1eval.c 206-213). Fall through to subthreshold.
            vds_sat = 0.0;
        } else {
            // Beta @ Vds=0 and @ Vds=Vdd (b1eval.c 217-235).
            let beta_vds_0 = s.beta_zero + s.beta_zero_b * vbs;
            let beta_vdd = s.beta_vdd + s.beta_vdd_b * vbs;
            let dbeta_vdd_dvds = s.beta_vdd_d.max(0.0);
            let (beta0, dbeta0_dvds, dbeta0_dvbs);
            if vds > vdd && vdd > 0.0 {
                beta0 = beta_vdd + dbeta_vdd_dvds * (vds - vdd);
                dbeta0_dvds = dbeta_vdd_dvds;
                dbeta0_dvbs = s.beta_vdd_b;
            } else if vdd > 0.0 {
                let vdd_sq = vdd * vdd;
                let c1 = (-beta_vdd + beta_vds_0 + dbeta_vdd_dvds * vdd) / vdd_sq;
                let c2 = 2.0 * (beta_vdd - beta_vds_0) / vdd - dbeta_vdd_dvds;
                let dbeta_vds_0_dvbs = s.beta_zero_b;
                let dbeta_vdd_dvbs = s.beta_vdd_b;
                let dc1_dvbs = (dbeta_vds_0_dvbs - dbeta_vdd_dvbs) / vdd_sq;
                let dc2_dvbs = dc1_dvbs * (-2.0) * vdd;
                beta0 = (c1 * vds + c2) * vds + beta_vds_0;
                dbeta0_dvds = 2.0 * c1 * vds + c2;
                dbeta0_dvbs = dc1_dvbs * vds * vds + dc2_dvbs * vds + dbeta_vds_0_dvbs;
            } else {
                // Vdd <= 0 — fall back to constant Beta = beta_vds_0.
                beta0 = beta_vds_0;
                dbeta0_dvds = 0.0;
                dbeta0_dvbs = s.beta_zero_b;
            }

            // Effective Beta (b1eval.c 239-243).
            let beta = beta0 / arg;
            let dbeta_dvgs = -beta * ugs / arg;
            let dbeta_dvds = dbeta0_dvds / arg - dbeta_dvgs * dvth_dvds;
            let dbeta_dvbs =
                dbeta0_dvbs / arg + beta * ugs * dvth_dvbs / arg - beta * vgs_vth * dugs_dvbs / arg;

            // VdsSat via quadratic-fit Vc/K (b1eval.c 247-250).
            let mut vc = uds * vgs_vth / a;
            if vc < 0.0 {
                vc = 0.0;
            }
            let term1 = (1.0 + 2.0 * vc).sqrt();
            let k = 0.5 * (1.0 + vc + term1);
            vds_sat = (vgs_vth / (a * k.sqrt())).max(0.0);

            if vds < vds_sat {
                // Triode (b1eval.c 252-264).
                let argl1 = (1.0 + uds * vds).max(1.0);
                let argl2 = vgs_vth - 0.5 * a * vds;
                drain_current = beta * argl2 * vds / argl1;
                gm = (dbeta_dvgs * argl2 * vds + beta * vds) / argl1;
                gds = (dbeta_dvds * argl2 * vds + beta * (vgs_vth - vds * dvth_dvds - a * vds)
                    - drain_current * (vds * duds_dvds + uds))
                    / argl1;
                gmbs = (dbeta_dvbs * argl2 * vds + beta * vds * (-dvth_dvbs - 0.5 * vds * da_dvbs)
                    - drain_current * vds * duds_dvbs)
                    / argl1;
            } else {
                // Saturation (b1eval.c 265-285).
                let args1 = 1.0 + 1.0 / term1;
                let dvc_dvgs = uds / a;
                let dvc_dvds = vgs_vth * duds_dvds / a - dvc_dvgs * dvth_dvds;
                let dvc_dvbs =
                    (vgs_vth * duds_dvbs - uds * (dvth_dvbs + vgs_vth * da_dvbs / a)) / a;
                let dk_dvc = 0.5 * args1;
                let dk_dvgs = dk_dvc * dvc_dvgs;
                let dk_dvds = dk_dvc * dvc_dvds;
                let dk_dvbs = dk_dvc * dvc_dvbs;
                let args2 = vgs_vth / a / k;
                let args3 = args2 * vgs_vth;
                drain_current = 0.5 * beta * args3;
                gm = 0.5 * args3 * dbeta_dvgs + beta * args2 - drain_current * dk_dvgs / k;
                gds = 0.5 * args3 * dbeta_dvds
                    - beta * args2 * dvth_dvds
                    - drain_current * dk_dvds / k;
                gmbs = 0.5 * dbeta_dvbs * args3
                    - beta * args2 * dvth_dvbs
                    - drain_current * (da_dvbs / a + dk_dvbs / k);
            }
        }

        // Subthreshold computation (b1eval.c 287-326).
        let n0 = s.subth_slope;
        if n0 > 0.0 && n0 < 200.0 {
            let nb = s.subth_slope_b;
            let nd = s.subth_slope_d;
            let mut n = n0 + nb * vbs + nd * vds;
            if n < 0.5 {
                n = 0.5;
            }
            let warg1 = safe_exp((-vds / vt).min(EXP_LIMIT));
            let wds = 1.0 - warg1;
            let wgs = safe_exp(((vgs_vth) / (n * vt)).min(EXP_LIMIT));
            let vt_sq = vt * vt;
            let warg2 = 6.04965 * vt_sq * s.beta_zero;
            let ilimit = 4.5 * vt_sq * s.beta_zero;
            let iexp = warg2 * wgs * wds;
            // Smooth limiter prevents Iexp blowup for very large Vgs_Vth.
            let denom = ilimit + iexp;
            let denom_safe = if denom > 0.0 { denom } else { 1.0e-30 };
            drain_current += ilimit * iexp / denom_safe;
            let temp1 = ilimit / denom_safe;
            let temp1_sq = temp1 * temp1;
            let denom2 = ilimit + wgs * warg2;
            let denom2_safe = if denom2 > 0.0 { denom2 } else { 1.0e-30 };
            let temp3 = ilimit / denom2_safe;
            let temp3 = temp3 * temp3 * warg2 * wgs;
            gm += temp1_sq * iexp / (n * vt);
            // gds: dDrainCurrent/dVds via subthreshold path.
            gds += temp3 * (-wds / n / vt * (dvth_dvds + vgs_vth * nd / n) + warg1 / vt);
            gmbs -= temp1_sq * iexp * (dvth_dvbs + vgs_vth * nb / n) / (n * vt);
        }

        // Clamp non-negative (b1eval.c 331-334, 580-582).
        if drain_current < 0.0 {
            drain_current = 0.0;
        }
        if gm < 0.0 {
            gm = 0.0;
        }
        if gds < 0.0 {
            gds = 0.0;
        }
        if gmbs < 0.0 {
            gmbs = 0.0;
        }

        Bsim1EvalOut {
            drain_current,
            gm,
            gds,
            gmbs,
            von,
            vdsat: vds_sat,
        }
    }
}

/// Output of `B1evaluate` — DC current and its derivatives plus Von/Vdsat.
struct Bsim1EvalOut {
    drain_current: f64,
    gm: f64,
    gds: f64,
    gmbs: f64,
    von: f64,
    vdsat: f64,
}

/// Compute per-instance derived parameters mirroring `b1temp.c`. The model's
/// `_L`/`_W` coefficients are binned onto the instance geometry, then `beta`
/// terms are scaled by `Cox · W/L`.
///
/// `w_m`, `l_m` are the *external* W/L in meters. ΔW and ΔL are in µm.
pub fn compute_sized(
    model: &Bsim1Model,
    w_m: f64,
    l_m: f64,
    nrd: f64,
    nrs: f64,
) -> Result<Bsim1Sized, &'static str> {
    let l_eff_m = l_m - model.delta_l * 1.0e-6;
    let w_eff_m = w_m - model.delta_w * 1.0e-6;
    if l_eff_m <= 0.0 {
        return Err("BSIM1: effective channel length <= 0");
    }
    if w_eff_m <= 0.0 {
        return Err("BSIM1: effective channel width <= 0");
    }
    let l_um = l_eff_m * 1.0e6;
    let w_um = w_eff_m * 1.0e6;
    let cox = model.cox_f_per_cm2; // F/cm²
    let cox_w_over_l = cox * w_um / l_um; // F/cm² (geometric factor)

    let mut s = Bsim1Sized {
        l_eff_m,
        w_eff_m,
        l_eff_um: l_um,
        ..Default::default()
    };

    s.vfb = model.vfb0 + model.vfb_l / l_um + model.vfb_w / w_um;
    s.phi = model.phi0 + model.phi_l / l_um + model.phi_w / w_um;
    s.k1 = model.k1_0 + model.k1_l / l_um + model.k1_w / w_um;
    s.k2 = model.k2_0 + model.k2_l / l_um + model.k2_w / w_um;
    s.eta = model.eta0 + model.eta_l / l_um + model.eta_w / w_um;
    s.eta_b = model.eta_b0 + model.eta_bl / l_um + model.eta_bw / w_um;
    s.eta_d = model.eta_d0 + model.eta_dl / l_um + model.eta_dw / w_um;

    // Beta-related terms (still as mobility before Cox·W/L scaling).
    s.beta_zero = model.mob_zero; // ngspice b1temp.c line 97 uses model.B1mobZero direct
    s.beta_zero_b = model.mob_zero_b0 + model.mob_zero_bl / l_um + model.mob_zero_bw / w_um;
    s.beta_vdd = model.mob_vdd0 + model.mob_vdd_l / l_um + model.mob_vdd_w / w_um;
    s.beta_vdd_b = model.mob_vdd_b0 + model.mob_vdd_bl / l_um + model.mob_vdd_bw / w_um;
    s.beta_vdd_d = model.mob_vdd_d0 + model.mob_vdd_dl / l_um + model.mob_vdd_dw / w_um;

    s.ugs = model.ugs0 + model.ugs_l / l_um + model.ugs_w / w_um;
    s.ugs_b = model.ugs_b0 + model.ugs_bl / l_um + model.ugs_bw / w_um;

    // Uds is pre-divided by Leff_µm (b1eval.c lines 170-173).
    let uds_raw = model.uds0 + model.uds_l / l_um + model.uds_w / w_um;
    let uds_b_raw = model.uds_b0 + model.uds_bl / l_um + model.uds_bw / w_um;
    let uds_d_raw = model.uds_d0 + model.uds_dl / l_um + model.uds_dw / w_um;
    s.uds = uds_raw / l_um;
    s.uds_b = uds_b_raw / l_um;
    s.uds_d = uds_d_raw / l_um;

    s.subth_slope = model.subth_slope0 + model.subth_slope_l / l_um + model.subth_slope_w / w_um;
    s.subth_slope_b =
        model.subth_slope_b0 + model.subth_slope_bl / l_um + model.subth_slope_bw / w_um;
    s.subth_slope_d =
        model.subth_slope_d0 + model.subth_slope_dl / l_um + model.subth_slope_dw / w_um;

    // Clamping (b1temp.c 123-125).
    if s.phi < 0.1 {
        s.phi = 0.1;
    }
    if s.k1 < 0.0 {
        s.k1 = 0.0;
    }
    if s.k2 < 0.0 {
        s.k2 = 0.0;
    }

    // Vt0 (b1temp.c 127-128).
    s.vt0 = s.vfb + s.phi + s.k1 * s.phi.sqrt() - s.k2 * s.phi;

    // Scale beta terms by Cox · W/L (b1temp.c 134-138).
    s.beta_zero *= cox_w_over_l;
    s.beta_zero_b *= cox_w_over_l;
    s.beta_vdd *= cox_w_over_l;
    s.beta_vdd_b *= cox_w_over_l;
    s.beta_vdd_d = (s.beta_vdd_d * cox_w_over_l).max(0.0);

    // Series-resistance conductances (b1temp.c 68-77).
    if model.rsh > 0.0 && nrd > 0.0 {
        s.drain_conductance = 1.0 / (model.rsh * nrd);
    } else {
        s.drain_conductance = 0.0;
    }
    if model.rsh > 0.0 && nrs > 0.0 {
        s.source_conductance = 1.0 / (model.rsh * nrs);
    } else {
        s.source_conductance = 0.0;
    }

    Ok(s)
}

/// Bulk junction diode current and conductance (matches `b1ld.c` lines
/// 379-394). Same form as Levels 1-6 but with `1/CONSTvt0` floor instead of
/// the `safe_exp` overflow clamp at -3·VT used elsewhere.
fn bulk_diode_current(v: f64, sat: f64, vt: f64) -> (f64, f64) {
    let gmin = 1e-12;
    if v <= 0.0 {
        let g = sat / vt + gmin;
        let i = g * v - sat;
        (g, i)
    } else {
        let ev = safe_exp((v / vt).min(EXP_LIMIT));
        let g = sat * ev / vt + gmin;
        let i = sat * (ev - 1.0) + gmin * v;
        (g, i)
    }
}

/// Stamp the BSIM1 companion model into the MNA matrix and RHS.
///
/// Companion-model shape matches Level 2/3/6 (same stencil), differing only
/// in how `gm`/`gds`/`gmbs` were computed and in the BSIM1-specific drain/
/// source series-resistance handling (driven by `Rsh × NRD`/`Rsh × NRS`,
/// not by element-level `RD`/`RS` since BSIM1 has no instance R params).
pub fn stamp_bsim1(
    matrix: &mut crate::SparseMatrix,
    rhs: &mut [f64],
    inst: &Bsim1Instance,
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

    // 1. gds output conductance between d' and s'.
    crate::stamp_conductance(matrix, dp, sp, m * comp.gds);

    // 2. gm VCCS (gate → drain/source).
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

    // 3. gmbs body-effect transconductance.
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

    // 4. gbd / gbs bulk-diode conductances.
    crate::stamp_conductance(matrix, b, dp, m * comp.gbd);
    crate::stamp_conductance(matrix, b, sp, m * comp.gbs);

    // 5. Series resistances (drain ↔ d' and source ↔ s').
    if inst.sized.drain_conductance > 0.0 {
        crate::stamp_conductance(matrix, inst.drain_idx, dp, m * inst.sized.drain_conductance);
    }
    if inst.sized.source_conductance > 0.0 {
        crate::stamp_conductance(
            matrix,
            inst.source_idx,
            sp,
            m * inst.sized.source_conductance,
        );
    }

    // 6. Equivalent current sources (NR linearization residual).
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
    use approx::assert_abs_diff_eq;

    fn basic_nmos() -> Bsim1Model {
        let mut m = Bsim1Model::new(MosfetType::Nmos);
        m.tox_um = 0.03;
        m.vdd = 5.0;
        m.vfb0 = -1.0;
        m.phi0 = 0.8;
        m.k1_0 = 1.3;
        m.k2_0 = 0.15;
        m.eta0 = 0.0;
        m.mob_zero = 500.0;
        m.mob_vdd0 = 500.0;
        m.ugs0 = 0.05;
        m.uds0 = 0.05;
        m.subth_slope0 = 1.5;
        m.cox_f_per_cm2 = COX_NUMERATOR / (m.tox_um * 1.0e-4);
        m
    }

    fn basic_instance() -> Bsim1Instance {
        let model = basic_nmos();
        let w = 50e-6;
        let l = 10e-6;
        let sized = compute_sized(&model, w, l, 1.0, 1.0).unwrap();
        Bsim1Instance {
            name: "M1".to_string(),
            drain_idx: Some(0),
            gate_idx: Some(1),
            source_idx: Some(2),
            bulk_idx: Some(3),
            drain_prime_idx: Some(0),
            source_prime_idx: Some(2),
            model,
            w,
            l,
            ad: 100e-12,
            as_: 100e-12,
            pd: 40e-6,
            ps: 40e-6,
            nrd: 1.0,
            nrs: 1.0,
            m: 1.0,
            sized,
        }
    }

    #[test]
    fn defaults_are_zero() {
        let m = Bsim1Model::new(MosfetType::Nmos);
        assert_eq!(m.vfb0, 0.0);
        assert_eq!(m.k1_0, 0.0);
        assert_eq!(m.eta0, 0.0);
        assert_eq!(m.mob_zero, 0.0);
        // Cox computed from default Tox.
        assert!(m.cox_f_per_cm2 > 0.0);
    }

    #[test]
    fn from_model_def_recognises_bsim1_keywords() {
        let md = ModelParams {
            name: "NCH".to_string(),
            kind: "NMOS".to_string(),
            params: vec![
                ("LEVEL".to_string(), 4.0),
                ("VFB".to_string(), -1.0),
                ("PHI".to_string(), 0.8),
                ("K1".to_string(), 1.3),
                ("ETA".to_string(), 0.01),
                ("MUZ".to_string(), 500.0),
                ("TOX".to_string(), 0.03),
                ("VDD".to_string(), 5.0),
                ("RSH".to_string(), 35.0),
            ],
        };
        let m = Bsim1Model::from_params(&md);
        assert_eq!(m.mos_type, MosfetType::Nmos);
        assert_abs_diff_eq!(m.vfb0, -1.0);
        assert_abs_diff_eq!(m.phi0, 0.8);
        assert_abs_diff_eq!(m.k1_0, 1.3);
        assert_abs_diff_eq!(m.eta0, 0.01);
        assert_abs_diff_eq!(m.mob_zero, 500.0);
        assert_abs_diff_eq!(m.tox_um, 0.03);
        assert_abs_diff_eq!(m.vdd, 5.0);
        assert_abs_diff_eq!(m.rsh, 35.0);
    }

    #[test]
    fn pmos_inferred_from_model_kind() {
        let md = ModelParams {
            name: "PCH".to_string(),
            kind: "PMOS".to_string(),
            params: vec![],
        };
        let m = Bsim1Model::from_params(&md);
        assert_eq!(m.mos_type, MosfetType::Pmos);
    }

    #[test]
    fn cutoff_returns_zero_current() {
        let inst = basic_instance();
        // Vgs below Vt0 → cutoff. With Vfb=-1, Phi=0.8, K1=1.3 at Vbs=0:
        //   Vt0 ≈ -1 + 0.8 + 1.3*sqrt(0.8) - 0.15*0.8 ≈ -0.157
        // So Vgs=-1 is clearly below threshold.
        let comp = inst.companion(-1.0, 1.0, 0.0);
        assert_abs_diff_eq!(comp.cdrain, 0.0, epsilon = 1.0e-12);
    }

    #[test]
    fn saturation_id_positive_and_finite() {
        let inst = basic_instance();
        let comp = inst.companion(2.0, 3.0, 0.0);
        assert!(comp.cdrain.is_finite());
        assert!(comp.cdrain > 0.0, "Id should be positive: {}", comp.cdrain);
        assert!(comp.gds >= 1.0e-12);
        assert!(comp.gm >= 0.0);
    }

    #[test]
    fn id_monotonic_in_vds() {
        let inst = basic_instance();
        let mut prev = -1.0;
        for &vds in &[0.1, 0.5, 1.0, 2.0, 4.0] {
            let comp = inst.companion(3.0, vds, 0.0);
            assert!(comp.cdrain.is_finite(), "Id finite at Vds={}", vds);
            assert!(
                comp.cdrain > prev - 1e-9,
                "Id should be monotonic in Vds: prev={}, cur={}",
                prev,
                comp.cdrain
            );
            prev = comp.cdrain;
        }
    }

    #[test]
    fn reversed_mode_for_negative_vds() {
        let inst = basic_instance();
        let comp = inst.companion(2.0, -1.0, 0.0);
        assert_eq!(comp.mode, -1);
        // Drain current sign follows mode·|cdrain|.
        assert!(comp.cdrain.is_finite());
    }

    #[test]
    fn binning_uses_l_and_w() {
        let mut model = basic_nmos();
        // Add length-binning term that *would* shift Vfb if applied.
        model.vfb_l = 0.5;
        let s_short = compute_sized(&model, 10e-6, 1e-6, 1.0, 1.0).unwrap();
        let s_long = compute_sized(&model, 10e-6, 100e-6, 1.0, 1.0).unwrap();
        // 0.5/Leff(µm) → short L=1µm sees +0.5, long L=100µm sees +0.005.
        assert!((s_short.vfb - s_long.vfb).abs() > 0.1);
    }

    #[test]
    fn drain_conductance_from_rsh_and_nrd() {
        let mut model = basic_nmos();
        model.rsh = 35.0;
        let s = compute_sized(&model, 50e-6, 10e-6, 2.0, 2.0).unwrap();
        // Rd = 35 * 2 = 70Ω → gd = 1/70 ≈ 0.01428.
        assert_abs_diff_eq!(s.drain_conductance, 1.0 / 70.0, epsilon = 1e-9);
        assert_abs_diff_eq!(s.source_conductance, 1.0 / 70.0, epsilon = 1e-9);
    }

    #[test]
    fn vt0_positive_for_nmos_with_typical_params() {
        let model = basic_nmos();
        let s = compute_sized(&model, 50e-6, 10e-6, 1.0, 1.0).unwrap();
        // Vt0 = -1 + 0.8 + 1.3*sqrt(0.8) - 0.15*0.8 ≈ 0.842
        assert!(s.vt0 > 0.0, "Vt0={} should be positive", s.vt0);
        assert!(s.vt0 < 1.5);
    }

    #[test]
    fn id_rises_with_vgs_above_threshold() {
        let inst = basic_instance();
        let id_low = inst.companion(1.5, 3.0, 0.0).cdrain;
        let id_high = inst.companion(3.0, 3.0, 0.0).cdrain;
        assert!(id_high > id_low);
    }
}
