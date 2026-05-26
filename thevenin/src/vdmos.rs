//! Vertical-DMOS power MOSFET device model.
//!
//! Implements ngspice's VDMOS model (Holger Vogt / Dietmar Warning, 2018-2020).
//! Distinct from the lateral MOSFET hierarchy (Level 1, 2, 3, 6, BSIM*): VDMOS
//! is selected by `.model NAME VDMOS (…)` (or `VDMOSN` / `VDMOSP`) — NOT by a
//! `LEVEL=` parameter — so dispatch happens off the *model kind string* in
//! [`crate::mna_ir`] rather than the BSIM-style level number.
//!
//! Topology, derived faithfully from `ngspice-upstream/src/spicelib/devices/vdmos/`:
//!   * Standard 4-terminal MOSFET (D, G, S, B); B is tied to S in most VDMOS
//!     devices but accepted as a separate node for SOI-style layouts.
//!   * Built-in body-drain diode (reverse-conduction + breakdown).
//!   * Strongly Vgd-dependent gate-drain capacitance (Miller plateau).
//!   * Channel I-V uses a smooth triode/saturation blend with `mtr`, `theta`,
//!     `lambda`, and the weak-inversion log shaper (`ksubthres`).
//!
//! The DC + companion-model port (this file) is faithful to `vdmosload.c`
//! lines 320-405 (channel I-V) and lines 698-832 (body diode). Self-heating,
//! quasi-saturation, and noise are deferred — see the TODO markers.
//!
//! Series resistances RD, RS, RG produce internal nodes when nonzero (same
//! pattern as `mos2.rs`). Body-diode currents are stamped between the source
//! and drain external terminals directly; no extra diode-prime node is used
//! in this port since the body diode's series R (`rb`) is folded into the
//! source resistance for simplicity.

use thevenin_types::{Expr, ModelDef};

use crate::diode::VT_NOM;
use crate::physics::{EXP_LIMIT, safe_exp};

/// VDMOS polarity: NMOS (vdmos / vdmosn) or PMOS (vdmosp).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VdmosType {
    Nmos,
    Pmos,
}

impl VdmosType {
    pub fn sign(self) -> f64 {
        match self {
            VdmosType::Nmos => 1.0,
            VdmosType::Pmos => -1.0,
        }
    }
}

/// VDMOS model parameters. Defaults match ngspice's `vdmosset.c`.
#[derive(Debug, Clone)]
pub struct VdmosModel {
    pub mos_type: VdmosType,
    /// Threshold voltage.
    pub vto: f64,
    /// Transconductance parameter.
    pub kp: f64,
    /// Surface potential.
    pub phi: f64,
    /// Channel length modulation (1/V).
    pub lambda: f64,
    /// Vgs-dependent mobility degradation (1/V).
    pub theta: f64,
    /// Drain ohmic resistance.
    pub rd: f64,
    /// Source ohmic resistance.
    pub rs: f64,
    /// Gate ohmic resistance.
    pub rg: f64,
    /// Drain-source shunt resistance (default 1e15 — effectively open).
    pub rds: f64,
    /// Conductance multiplier in triode region.
    pub mtr: f64,
    /// Subthreshold slope (V/decade-ish — used in log shaper).
    pub ksubthres: f64,
    /// Shift of weak-inversion plot along the Vgs axis.
    pub subshift: f64,
    /// Threshold-voltage temperature coefficient (linear, V/K).
    pub tcvth: f64,
    /// Reference temperature (K).
    pub tnom: f64,
    /// Mobility-temperature exponent (Beta scales as (T/Tnom)^mu).
    pub mu: f64,

    // ----- Gate capacitance (Miller plateau) -----
    /// Minimum gate-drain capacitance (large Vgd).
    pub cgdmin: f64,
    /// Maximum gate-drain capacitance (Vgd ~ 0).
    pub cgdmax: f64,
    /// Sharpness of the Cgd(Vgd) transition.
    pub a: f64,
    /// Gate-source capacitance (constant).
    pub cgs: f64,

    // ----- Body diode -----
    /// Body-diode saturation current.
    pub is_dio: f64,
    /// Body-diode emission coefficient.
    pub n_dio: f64,
    /// Body-diode reverse-breakdown voltage. `None` disables breakdown.
    pub bv: Option<f64>,
    /// Body-diode current at breakdown.
    pub ibv: f64,
    /// Body-diode breakdown emission coefficient.
    pub nbv: f64,
    /// Body-diode zero-bias junction capacitance.
    pub cjo: f64,
    /// Body-diode grading coefficient.
    pub mj_dio: f64,
    /// Body-diode junction potential.
    pub vj_dio: f64,
    /// Body-diode forward-bias depletion cap coefficient.
    pub fc_dio: f64,
    /// Body-diode transit time.
    pub tt_dio: f64,
}

impl VdmosModel {
    pub fn new(mos_type: VdmosType) -> Self {
        Self {
            mos_type,
            vto: 0.0,
            kp: 2e-5,
            phi: 0.6,
            lambda: 0.0,
            theta: 0.0,
            rd: 0.0,
            rs: 0.0,
            rg: 0.0,
            rds: 1e15,
            mtr: 1.0,
            ksubthres: 0.1,
            subshift: 0.0,
            tcvth: 0.0,
            tnom: 300.15,
            mu: -1.5,
            cgdmin: 2e-11,
            cgdmax: 2e-9,
            a: 1.0,
            cgs: 1.4e-9,
            is_dio: 1e-14,
            n_dio: 1.0,
            bv: None,
            ibv: 1e-10,
            nbv: 1.0,
            cjo: 0.0,
            mj_dio: 0.5,
            vj_dio: 1.0,
            fc_dio: 0.5,
            tt_dio: 0.0,
        }
    }

    /// Build a `VdmosModel` from a SPICE `.model` definition. The model kind
    /// string disambiguates polarity:
    ///   * `VDMOS` / `VDMOSN` / `NMOS` → NMOS (default if ambiguous)
    ///   * `VDMOSP` / `PMOS`             → PMOS
    ///
    /// The kind string can also carry a `PCHAN` / `NCHAN` flag among the
    /// params, which ngspice recognises via `vdmospar.c`.
    pub fn from_model_def(model_def: &ModelDef) -> Self {
        let upper_kind = model_def.kind.to_uppercase();
        let mos_type = if upper_kind.contains("VDMOSP") || upper_kind == "PMOS" {
            VdmosType::Pmos
        } else {
            VdmosType::Nmos
        };

        let mut m = Self::new(mos_type);

        for p in &model_def.params {
            // Bare flag parameters: PMOS/NMOS/PCHAN/NCHAN polarity hints.
            let name_upper = p.name.to_uppercase();
            if matches!(name_upper.as_str(), "PCHAN" | "PMOS" | "VDMOSP") {
                m.mos_type = VdmosType::Pmos;
                continue;
            }
            if matches!(name_upper.as_str(), "NCHAN" | "NMOS" | "VDMOSN") {
                m.mos_type = VdmosType::Nmos;
                continue;
            }

            if let Expr::Num(v) = &p.value {
                match name_upper.as_str() {
                    "VTO" | "VT0" | "VTH" | "VTH0" => m.vto = *v,
                    "KP" => m.kp = *v,
                    "PHI" => m.phi = *v,
                    "LAMBDA" => m.lambda = *v,
                    "THETA" => m.theta = *v,
                    "RD" => m.rd = *v,
                    "RS" => m.rs = *v,
                    "RG" => m.rg = *v,
                    "RDS" => m.rds = *v,
                    "MTRIODE" => m.mtr = *v,
                    "KSUBTHRES" => m.ksubthres = *v,
                    "SUBSHIFT" => m.subshift = *v,
                    "TCVTH" | "VTOTC" => m.tcvth = *v,
                    "TNOM" => m.tnom = *v + 273.15,
                    "MU" | "BEX" => m.mu = *v,
                    "CGDMIN" => m.cgdmin = *v,
                    "CGDMAX" => m.cgdmax = *v,
                    "A" => m.a = *v,
                    "CGS" => m.cgs = *v,
                    // Body diode
                    "IS" => m.is_dio = *v,
                    "N" => m.n_dio = *v,
                    "BV" => m.bv = Some(*v),
                    "IBV" => m.ibv = *v,
                    "NBV" => m.nbv = *v,
                    "CJO" | "CJ" | "CJ0" => m.cjo = *v,
                    "MJ" => m.mj_dio = *v,
                    "VJ" => m.vj_dio = *v,
                    "FC" => m.fc_dio = *v,
                    "TT" => m.tt_dio = *v,
                    _ => {}
                }
            }
        }
        m
    }

    /// Number of internal nodes needed (RD prime, RS prime, RG prime).
    pub fn internal_node_count(&self) -> usize {
        let mut n = 0;
        if self.rd > 0.0 {
            n += 1;
        }
        if self.rs > 0.0 {
            n += 1;
        }
        if self.rg > 0.0 {
            n += 1;
        }
        n
    }
}

/// Newton-Raphson companion model for a VDMOS channel + body diode at an
/// operating point.
#[derive(Debug, Clone)]
pub struct VdmosCompanion {
    /// dId/dVgs.
    pub gm: f64,
    /// dId/dVds.
    pub gds: f64,
    /// Channel drain current.
    pub cdrain: f64,
    /// Equivalent NR current source for drain.
    pub ceq_d: f64,
    /// Body-diode conductance.
    pub gd: f64,
    /// Body-diode current (positive = forward from drain to source for NMOS).
    pub cd: f64,
    /// Body-diode equivalent NR current source.
    pub ceq_dio: f64,
    /// Operating mode: +1 normal (Vds>=0), -1 reversed.
    pub mode: i32,
    /// Saturation voltage Vdsat.
    pub vdsat: f64,
    /// Threshold voltage in signed space (for fetlim limiting).
    pub von: f64,
}

impl VdmosModel {
    /// Compute the VDMOS NR companion at (vgs, vds).
    ///
    /// `vgs` and `vds` are the channel terminal voltages already adjusted for
    /// device-type sign (positive Vgs above threshold turns the device on for
    /// both NMOS and PMOS at the call site). The body diode is evaluated from
    /// the bulk-to-drain voltage `vbd_raw = -vds` (matching ngspice's choice
    /// of `vd = type * (V(posPrime) - V(drain))` in vdmosload.c:749).
    pub fn companion(&self, vgs: f64, vds: f64) -> VdmosCompanion {
        // ----- Mode detection (vdmosload.c:329-339) -----
        let (vgs_eff, vds_eff, mode) = if vds >= 0.0 {
            (vgs, vds, 1)
        } else {
            // Reversed: swap S/D. Vgd is the "effective Vgs" of the reversed
            // device, |Vds| is the effective Vds.
            (vgs - vds, -vds, -1)
        };

        // ----- Channel I-V (vdmosload.c:341-402) -----
        let von = self.mos_type.sign() * self.vto;
        let mut vgst = vgs_eff - von;
        let vdsat = vgst.max(0.0);

        // Weak-inversion log smoothing of vgst (vdmosload.c:371-373).
        // t2 = exp((vgst - shift)/slope); vgst = slope * log(1 + t2).
        // Guard against extreme negative arguments — for very deep subthreshold
        // operation the contribution is negligibly small and we can clamp.
        let slope = self.ksubthres.max(1e-6);
        let arg = ((vgst - self.subshift) / slope).clamp(-EXP_LIMIT, EXP_LIMIT);
        let t2 = safe_exp(arg);
        // d(vgst_smoothed)/d(vgs_eff) = t2 / (t2 + 1).
        let dvgst_dvgs = t2 / (t2 + 1.0);
        vgst = slope * (1.0 + t2).ln();

        // mtr scales Vds in the triode boundary check (vdmosload.c:364).
        let vdss = vds_eff * self.mtr;
        // ngspice vdmosload.c:365 uses the raw signed `vds` here (not the
        // magnitude `vds_eff`). In reversed-mode body-diode conduction with
        // a non-zero LAMBDA, ngspice's t0 < 1 (CLM factor reduced); using
        // `vds_eff` would make it > 1 with the opposite sign.
        let t0 = 1.0 + self.lambda * vds;
        let t1 = 1.0 + self.theta * vdsat;
        let beta = self.kp;
        let betap = beta * t0 / t1;
        let dbetap_dvgs = -beta * self.theta * t0 / (t1 * t1);
        let dbetap_dvds = beta * self.lambda / t1;

        let (cdrain_eff, gm_eff, mut gds_eff);
        if vgst <= vdss {
            // Saturation region.
            cdrain_eff = 0.5 * betap * vgst * vgst;
            gm_eff = betap * vgst * dvgst_dvgs + 0.5 * dbetap_dvgs * vgst * vgst;
            gds_eff = 0.5 * dbetap_dvds * vgst * vgst;
        } else {
            // Triode / linear region.
            cdrain_eff = betap * vdss * (vgst - 0.5 * vdss);
            gm_eff = betap * vdss * dvgst_dvgs + vdss * dbetap_dvgs * (vgst - 0.5 * vdss);
            gds_eff = vdss * dbetap_dvds * (vgst - 0.5 * vdss)
                + betap * self.mtr * (vgst - 0.5 * vdss)
                - 0.5 * vdss * betap * self.mtr;
        }

        // Floor gds to avoid singular Jacobian when lambda=0 + saturation.
        if gds_eff < 1e-12 {
            gds_eff = 1e-12;
        }

        // Flip the sign and orientation back for reversed mode. cdrain itself
        // is the magnitude of the channel current in either case; the mode
        // flag is consumed by the stamp.
        let cdrain = cdrain_eff;
        let gm = gm_eff;
        let gds = gds_eff;

        // NR linearization residual for the drain (matches ngspice cdreq
        // computation at vdmosload.c:614).
        let ceq_d = cdrain - gm * vgs_eff - gds * vds_eff;

        // ----- Body diode (vdmosload.c:698-832) -----
        // The diode sits between source (anode) and drain (cathode) for an
        // NMOS VDMOS — so the bulk-to-drain voltage drives it. With our sign
        // convention, vbd = -vds is the diode forward voltage seen at the
        // source-drain pair before polarity-sign flip.
        let vt = VT_NOM;
        let vte = self.n_dio * vt;
        let vd = -vds; // forward bias when Vds < 0 (drain pulled below source)
        let (cd, gd) = if vd >= -3.0 * vte {
            // Forward conduction region.
            let arg = (vd / vte).clamp(-EXP_LIMIT, EXP_LIMIT);
            let evd = safe_exp(arg);
            let cd = self.is_dio * (evd - 1.0);
            let gd = self.is_dio * evd / vte + 1e-12;
            (cd, gd)
        } else if let Some(bv) = self.bv {
            // Breakdown region.
            let vtebrk = self.nbv * vt;
            if vd >= -bv {
                // Reverse leakage tail (cubic asymptote, matches ngspice).
                let r = 3.0 * vte / (vd * std::f64::consts::E);
                let r3 = r * r * r;
                let cd = -self.is_dio * (1.0 + r3);
                let gd = self.is_dio * 3.0 * r / vd + 1e-12;
                (cd, gd)
            } else {
                // Reverse breakdown — exponential conduction.
                let arg = (-(bv + vd) / vtebrk).clamp(-EXP_LIMIT, EXP_LIMIT);
                let evrev = safe_exp(arg);
                let cd = -self.is_dio * evrev;
                let gd = self.is_dio * evrev / vtebrk + 1e-12;
                (cd, gd)
            }
        } else {
            // No breakdown specified — flat reverse leakage.
            (-self.is_dio, 1e-12)
        };

        let ceq_dio = cd - gd * vd;

        VdmosCompanion {
            gm,
            gds,
            cdrain,
            ceq_d,
            gd,
            cd,
            ceq_dio,
            mode,
            vdsat,
            von,
        }
    }
}

/// Resolved VDMOS instance: node indices, model, and per-instance scalars.
#[derive(Debug, Clone)]
pub struct VdmosInstance {
    pub name: String,
    /// External drain node (None = ground).
    pub drain_idx: Option<usize>,
    /// Gate node (None = ground).
    pub gate_idx: Option<usize>,
    /// External source node (None = ground).
    pub source_idx: Option<usize>,
    /// Bulk node (None = ground). For most VDMOS devices this is tied to
    /// source externally, but the IR still gives it an Id.
    pub bulk_idx: Option<usize>,
    /// Internal drain prime (when RD>0), else same as drain_idx.
    pub drain_prime_idx: Option<usize>,
    /// Internal source prime (when RS>0), else same as source_idx.
    pub source_prime_idx: Option<usize>,
    /// Internal gate prime (when RG>0), else same as gate_idx.
    pub gate_prime_idx: Option<usize>,
    /// Resolved VDMOS model parameters.
    pub model: VdmosModel,
    /// Parallel-instance multiplier (drain current scales with M, no W/L).
    pub m: f64,
}

impl VdmosInstance {
    /// Extract terminal voltages (signed for device type) from the solver
    /// state vector.
    pub fn terminal_voltages(&self, solution: &[f64]) -> (f64, f64) {
        let v_dp = self.drain_prime_idx.map(|i| solution[i]).unwrap_or(0.0);
        let v_gp = self.gate_prime_idx.map(|i| solution[i]).unwrap_or(0.0);
        let v_sp = self.source_prime_idx.map(|i| solution[i]).unwrap_or(0.0);
        let sign = self.model.mos_type.sign();
        let vgs = sign * (v_gp - v_sp);
        let vds = sign * (v_dp - v_sp);
        (vgs, vds)
    }
}

/// Stamp the VDMOS companion model into the MNA matrix and RHS.
///
/// Stamps:
///   * Channel gm / gds VCCS between dp, gp, sp (gate-controlled current
///     source between drain' and source').
///   * Body diode between drain (external) and source (external).
///   * Series resistances RD, RS, RG between external and prime nodes.
///   * Drain-source shunt Rds between dp and sp (a large resistor by default).
///   * Equivalent current sources on the RHS.
pub fn stamp_vdmos(
    matrix: &mut crate::SparseMatrix,
    rhs: &mut [f64],
    inst: &VdmosInstance,
    comp: &VdmosCompanion,
) {
    let dp = inst.drain_prime_idx;
    let gp = inst.gate_prime_idx;
    let sp = inst.source_prime_idx;

    let sign = inst.model.mos_type.sign();
    let m = inst.m;

    // Channel xnrm/xrev routing (matches mosfet::stamp_mosfet).
    let (xnrm, xrev) = if comp.mode > 0 {
        (1.0, 0.0)
    } else {
        (0.0, 1.0)
    };

    let gm_s = m * comp.gm;
    let gds_s = m * comp.gds;

    // gds: ordinary conductance between dp and sp.
    crate::stamp_conductance(matrix, dp, sp, gds_s);

    // gm: VCCS between dp / sp controlled by Vgp.
    if let Some(d) = dp {
        matrix.add(d, d, xrev * gm_s);
    }
    if let Some(s) = sp {
        matrix.add(s, s, xnrm * gm_s);
    }
    if let Some(g) = gp {
        if let Some(d) = dp {
            matrix.add(d, g, (xnrm - xrev) * gm_s);
        }
        if let Some(s) = sp {
            matrix.add(s, g, -(xnrm - xrev) * gm_s);
        }
    }
    if let (Some(d), Some(s)) = (dp, sp) {
        matrix.add(d, s, -xnrm * gm_s);
        matrix.add(s, d, -xrev * gm_s);
    }

    // Drain-source shunt rds (default 1e15 — effectively open but provides
    // a numerical floor between dp and sp so the matrix can't go singular).
    if inst.model.rds > 0.0 && inst.model.rds.is_finite() {
        let g_rds = 1.0 / inst.model.rds;
        crate::stamp_conductance(matrix, dp, sp, m * g_rds);
    }

    // Body diode between source (anode for NMOS) and drain (cathode).
    // Stamped as a conductance + RHS contribution.
    let gd_s = m * comp.gd;
    crate::stamp_conductance(matrix, inst.drain_idx, inst.source_idx, gd_s);

    // Series resistances RD (between external drain and dp), RS (external
    // source and sp), RG (external gate and gp).
    if inst.model.rd > 0.0 {
        let g_rd = 1.0 / inst.model.rd;
        crate::stamp_conductance(matrix, inst.drain_idx, dp, m * g_rd);
    }
    if inst.model.rs > 0.0 {
        let g_rs = 1.0 / inst.model.rs;
        crate::stamp_conductance(matrix, inst.source_idx, sp, m * g_rs);
    }
    if inst.model.rg > 0.0 {
        let g_rg = 1.0 / inst.model.rg;
        crate::stamp_conductance(matrix, inst.gate_idx, gp, m * g_rg);
    }

    // RHS: channel equivalent current source (Norton companion).
    // ngspice mos1load: cdreq = +type * ceq_d in normal mode,
    //                   cdreq = -type * ceq_d in reversed mode.
    let mode_f = comp.mode as f64;
    let ceq_d_rhs = mode_f * sign * m * comp.ceq_d;
    if let Some(d) = dp {
        rhs[d] -= ceq_d_rhs;
    }
    if let Some(s) = sp {
        rhs[s] += ceq_d_rhs;
    }

    // RHS: body diode equivalent current source.
    // For NMOS: positive diode current flows source → drain (forward bias when
    // Vd < Vs). For PMOS: signs reverse.
    let ceq_dio_rhs = sign * m * comp.ceq_dio;
    if let Some(d) = inst.drain_idx {
        rhs[d] += ceq_dio_rhs;
    }
    if let Some(s) = inst.source_idx {
        rhs[s] -= ceq_dio_rhs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    use thevenin_types::{Expr, Param};
    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::wasm_bindgen_test as test;

    fn nmos_model() -> VdmosModel {
        let mut m = VdmosModel::new(VdmosType::Nmos);
        m.vto = 3.0;
        m.kp = 5.0; // big to land in the amps range typical for power MOSFETs
        m.lambda = 0.0;
        m.theta = 0.0;
        m.ksubthres = 0.1;
        m
    }

    #[test]
    fn type_sign_matches_polarity() {
        assert_eq!(VdmosType::Nmos.sign(), 1.0);
        assert_eq!(VdmosType::Pmos.sign(), -1.0);
    }

    #[test]
    fn defaults_match_ngspice() {
        let m = VdmosModel::new(VdmosType::Nmos);
        assert_eq!(m.ksubthres, 0.1);
        assert_eq!(m.mtr, 1.0);
        assert_eq!(m.cgdmin, 2e-11);
        assert_eq!(m.cgdmax, 2e-9);
        assert_eq!(m.cgs, 1.4e-9);
        assert_eq!(m.rds, 1e15);
    }

    #[test]
    fn from_model_def_picks_pmos() {
        let mdef = ModelDef {
            name: "PWR".into(),
            kind: "VDMOSP".into(),
            params: vec![Param {
                name: "VTO".into(),
                value: Expr::Num(-3.0),
            }],
        };
        let m = VdmosModel::from_model_def(&mdef);
        assert_eq!(m.mos_type, VdmosType::Pmos);
        assert_abs_diff_eq!(m.vto, -3.0, epsilon = 1e-15);
    }

    #[test]
    fn from_model_def_pchan_flag() {
        let mdef = ModelDef {
            name: "PWR".into(),
            kind: "VDMOS".into(),
            params: vec![Param {
                name: "PCHAN".into(),
                value: Expr::Num(1.0),
            }],
        };
        let m = VdmosModel::from_model_def(&mdef);
        assert_eq!(m.mos_type, VdmosType::Pmos);
    }

    #[test]
    fn channel_below_threshold_is_near_zero() {
        let m = nmos_model();
        // Vgs = 1V, well below vto=3V → only subthreshold leakage.
        let comp = m.companion(1.0, 5.0);
        // Smoothed vgst is small but non-zero; current should be tiny.
        assert!(comp.cdrain.abs() < 1e-3, "got {}", comp.cdrain);
    }

    #[test]
    fn channel_saturation_gives_finite_peak() {
        let m = nmos_model();
        // Vgs=10, Vto=3 → strong inversion. Vds=20 → saturation since Vgst≈7<<20.
        let comp = m.companion(10.0, 20.0);
        // Id ≈ kp/2 * vgst^2 = 2.5 * 49 ≈ 122 A. The smoothing skews this
        // slightly upward at high vgst; bound it loosely.
        assert!(
            (50.0..400.0).contains(&comp.cdrain),
            "saturation Id out of band: {}",
            comp.cdrain
        );
        assert_eq!(comp.mode, 1);
        assert!(comp.gm > 0.0);
    }

    #[test]
    fn channel_triode_gives_lower_current_than_saturation() {
        let m = nmos_model();
        let sat = m.companion(10.0, 20.0);
        let tri = m.companion(10.0, 0.5);
        assert!(
            tri.cdrain < sat.cdrain,
            "triode current ({}) should be below saturation ({})",
            tri.cdrain,
            sat.cdrain
        );
    }

    #[test]
    fn reversed_mode_is_detected() {
        let m = nmos_model();
        let comp = m.companion(10.0, -2.0);
        assert_eq!(comp.mode, -1);
    }

    #[test]
    fn body_diode_forward_conducts() {
        let m = nmos_model();
        // Vds negative → vd = +0.7V drives the body diode hard into conduction.
        let comp = m.companion(0.0, -0.7);
        assert!(
            comp.cd > 1e-3,
            "body diode should conduct heavily at vd=0.7V: {}",
            comp.cd
        );
        assert!(comp.gd > 1e-6);
    }

    #[test]
    fn body_diode_off_in_normal_operation() {
        let m = nmos_model();
        // Vds > 0 → diode is reverse-biased → leakage only.
        let comp = m.companion(5.0, 5.0);
        assert!(comp.cd.abs() < 1e-10);
    }

    #[test]
    fn internal_node_count_tracks_series_r() {
        let mut m = VdmosModel::new(VdmosType::Nmos);
        assert_eq!(m.internal_node_count(), 0);
        m.rd = 0.1;
        assert_eq!(m.internal_node_count(), 1);
        m.rs = 0.05;
        m.rg = 1.0;
        assert_eq!(m.internal_node_count(), 3);
    }

    #[test]
    fn breakdown_clamps_below_bv() {
        let mut m = nmos_model();
        m.bv = Some(100.0);
        // Vd = -150V (way past bv): diode goes into reverse breakdown.
        let comp = m.companion(0.0, 150.0); // Vds=150 → vd=-150
        assert!(
            comp.cd < -1e-3,
            "breakdown current should be substantial, got {}",
            comp.cd
        );
    }
}
