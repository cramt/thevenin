//! BSIM3SOI-DD (Double Diffused Silicon-On-Insulator) MOSFET model.
//!
//! Implements the BSIM3SOI-DD v2.0 model matching ngspice level 56.
//! DD is a hybrid of PD and FD: it uses the FD-style self-consistent surface
//! potential chain (Vbs0t→Vbs0→Vbs0mos→Vthfd→Vbs0eff→Vbsmos→Vbseff) combined
//! with the PD-style 4-component junction diode model and GIDL currents.
//! Impact ionization uses ALPHA0/ALPHA1/BETA0 + AII/BII/CII/DII parameters.

#![allow(unused_variables, dead_code, clippy::too_many_arguments, unused_parens)]

use crate::model_params::ModelParams;

use crate::mosfet::MosfetType;
use crate::physics::{
    CHARGE_Q, EPSOX, EPSSI, EXP_THRESHOLD, EXPL_THRESHOLD, KBOQ, MAX_EXP, MIN_EXP, MIN_EXPL,
    bsim_safe_exp as safe_exp,
};

const DELTA_1: f64 = 0.02;
const DELTA_4: f64 = 0.02;
const DELT_VBS0EFF: f64 = 0.02;
const DELT_VBSMOS: f64 = 0.005;
const DELT_VBSEFF: f64 = 0.005;
/// Smoothing delta for Vbsdio clamp (from ngspice #define DELT_Vbsdio)
const DELT_VBSDIO: f64 = 0.01;
/// Offset for Vbsdio clamp floor (from ngspice #define OFF_Vbsdio)
const OFF_VBSDIO: f64 = 0.02;

const TEMP_DEFAULT: f64 = 300.15;

/// BSIM3SOI-DD model parameters (from .model card, Level=56).
#[derive(Debug, Clone)]
pub struct Bsim3SoiDdModel {
    pub mos_type: MosfetType,

    // Mode selection
    pub mob_mod: i32,
    pub cap_mod: i32,
    pub sh_mod: i32,

    // Oxide / SOI geometry
    pub tox: f64,
    pub tsi: f64,
    pub tbox: f64,

    // Threshold voltage
    pub vth0: f64,
    pub k1: f64,
    pub k2: f64,
    pub k3: f64,
    pub k3b: f64,
    pub w0: f64,
    pub nlx: f64,

    // SCE/DIBL
    pub dvt0: f64,
    pub dvt1: f64,
    pub dvt2: f64,
    pub dvt0w: f64,
    pub dvt1w: f64,
    pub dvt2w: f64,
    pub dsub: f64,
    pub eta0: f64,
    pub etab: f64,

    // Subthreshold
    pub voff: f64,
    pub nfactor: f64,
    pub cdsc: f64,
    pub cdscb: f64,
    pub cdscd: f64,
    pub cit: f64,

    // Doping
    pub nch: f64,
    pub npeak: f64,
    pub ngate: f64,
    pub nsub: f64,

    // Mobility
    pub u0: f64,
    pub ua: f64,
    pub ub: f64,
    pub uc: f64,
    pub ute: f64,
    pub ua1: f64,
    pub ub1: f64,
    pub uc1: f64,

    // Velocity saturation
    pub vsat: f64,
    pub a0: f64,
    pub ags: f64,
    pub a1: f64,
    pub a2: f64,
    pub at: f64,
    pub keta: f64,

    // Output resistance
    pub pclm: f64,
    pub pdiblc1: f64,
    pub pdiblc2: f64,
    pub pdiblcb: f64,
    pub drout: f64,
    pub pvag: f64,
    pub delta: f64,

    // Series resistance
    pub rdsw: f64,
    pub prwg: f64,
    pub prwb: f64,
    pub prt: f64,
    pub wr: f64,

    // Width/length effects
    pub dwg: f64,
    pub dwb: f64,
    pub b0: f64,
    pub b1: f64,
    pub wint: f64,
    pub lint: f64,
    pub dlc: f64,
    pub dwc: f64,

    // Impact ionization (DD uses ALPHA0/ALPHA1/BETA0 + AII/BII/CII/DII)
    pub alpha0: f64,
    pub alpha1: f64,
    pub beta0: f64,
    pub aii: f64,
    pub bii: f64,
    pub cii: f64,
    pub dii: f64,

    // Temperature
    pub tnom: f64,
    pub kt1: f64,
    pub kt1l: f64,
    pub kt2: f64,

    // SOI junction model (same as PD: 4-component)
    pub ndiode: f64,
    pub ntun: f64,
    pub nrecf0: f64,
    pub nrecr0: f64,
    pub vrec0: f64,
    pub ntrecf: f64,
    pub ntrecr: f64,
    pub isbjt: f64,
    pub isdif: f64,
    pub istun: f64,
    pub isrec: f64,
    pub xbjt: f64,
    pub xdif: f64,
    pub xrec: f64,
    pub xtun: f64,
    pub ahli: f64,
    pub lbjt0: f64,
    pub ln: f64,
    pub nbjt: f64,
    pub ndif: f64,
    pub aely: f64,
    pub vabjt: f64,

    // GIDL (same as PD)
    pub agidl: f64,
    pub bgidl: f64,
    pub ngidl: f64,

    // DD-specific surface potential params (shared with FD)
    pub kb1: f64,
    pub kb3: f64,
    pub dvbd0: f64,
    pub dvbd1: f64,
    pub vbsa: f64,
    pub delp: f64,
    pub abp: f64,
    pub mxc: f64,
    pub adice0: f64,
    pub xj: f64,
    pub kbjt1: f64,
    pub edl: f64,

    // Body resistance
    pub rbody: f64,
    pub rbsh: f64,

    // Gate overlap
    pub cgso: f64,
    pub cgdo: f64,

    // CV model
    pub clc: f64,
    pub cle: f64,
    pub cf: f64,
    pub ckappa: f64,
    pub cgdl: f64,
    pub cgsl: f64,

    // Junction capacitance
    pub cjswg: f64,
    pub mjswg: f64,
    pub pbswg: f64,
    pub tt: f64,
    pub csdesw: f64,
    pub asd: f64,

    // Self-heating
    pub rth0: f64,
    pub cth0: f64,

    // Binning parameters (L/W/P variants for kb3/dvbd0/dvbd1)
    // Default to 1.0 in ngspice (b3soiddset.c), not 0.0
    pub bin_unit: i32,
    pub lkb3: f64,
    pub wkb3: f64,
    pub pkb3: f64,
    pub ldvbd0: f64,
    pub wdvbd0: f64,
    pub pdvbd0: f64,
    pub ldvbd1: f64,
    pub wdvbd1: f64,
    pub pdvbd1: f64,

    // Precomputed
    pub cox: f64,
    pub vtm: f64,
    pub phi: f64,
    pub sqrt_phi: f64,
    pub vbi_default: f64,
    pub factor1: f64,
    pub ni: f64,
    pub eg: f64,
    // DD-specific precomputed (from FD)
    pub cbox: f64,
    pub csi: f64,
    pub qsi: f64,
    pub csieff: f64,
    pub qsieff: f64,
    pub vfbb: f64,
    /// Cboxt = cbox*csi/(cbox+csi) (buried oxide + silicon cap series combination)
    pub cboxt: f64,
    /// Processed adice: adice0 / (1 + Cboxt/cox), where Cboxt = cbox*csi/(cbox+csi)
    pub adice: f64,
}

/// Size-dependent parameters for BSIM3SOI-DD.
#[derive(Debug, Clone)]
pub struct Bsim3SoiDdSizeParam {
    pub leff: f64,
    pub weff: f64,
    pub leff_cv: f64,
    pub weff_cv: f64,

    // Core parameters
    pub vth0: f64,
    pub k1: f64,
    pub k2: f64,
    pub k3: f64,
    pub k3b: f64,
    pub w0: f64,
    pub nlx: f64,
    pub dvt0: f64,
    pub dvt1: f64,
    pub dvt2: f64,
    pub dvt0w: f64,
    pub dvt1w: f64,
    pub dvt2w: f64,
    pub eta0: f64,
    pub etab: f64,
    pub dsub: f64,
    pub voff: f64,
    pub nfactor: f64,
    pub cdsc: f64,
    pub cdscb: f64,
    pub cdscd: f64,
    pub cit: f64,
    pub u0: f64,
    pub ua: f64,
    pub ub: f64,
    pub uc: f64,
    pub vsat: f64,
    pub a0: f64,
    pub ags: f64,
    pub a1: f64,
    pub a2: f64,
    pub at: f64,
    pub keta: f64,
    pub pclm: f64,
    pub pdiblc1: f64,
    pub pdiblc2: f64,
    pub pdiblcb: f64,
    pub pvag: f64,
    pub delta: f64,
    pub rdsw: f64,
    pub prwg: f64,
    pub prwb: f64,
    pub dwg: f64,
    pub dwb: f64,
    pub b0: f64,
    pub b1: f64,
    pub alpha0: f64,
    pub beta0: f64,
    pub kt1: f64,
    pub kt1l: f64,
    pub kt2: f64,
    pub ndiode: f64,
    pub agidl: f64,
    pub bgidl: f64,
    pub ngidl: f64,

    // Precomputed
    pub phi: f64,
    pub sqrt_phi: f64,
    pub xdep0: f64,
    pub litl: f64,
    pub theta0vb0: f64,
    pub theta_rout: f64,
    pub k1eff: f64,
    pub npeak: f64,
    pub nsub: f64,
    pub vfb: f64,
    pub u0temp: f64,
    pub vsattemp: f64,
    pub rds0: f64,
    pub cdep0: f64,
    pub vbi: f64,
    pub lratio: f64,
    pub lratiodif: f64,
    pub vearly: f64,
    pub arfabjt: f64,
    pub wdios: f64,
    pub wdiod: f64,

    // Junction precomputed
    pub jbjt: f64,
    pub jdif: f64,
    pub jrec: f64,
    pub jtun: f64,

    // Overlap caps
    pub cgso_eff: f64,
    pub cgdo_eff: f64,

    // DD-specific (binned)
    pub kb3: f64,
    pub dvbd0: f64,
    pub dvbd1: f64,

    /// abulkCVfactor = (1 + clc/leff)^cle (ngspice b3soiddtemp.c line 194)
    pub abulk_cv_factor: f64,

    /// Minimum substrate current (convergence aid for body node).
    /// ngspice b3soiddtemp.c line 744: 5e-2 * weff * tsi * max(isdif, isrec)
    pub min_isub: f64,

    pub nseg: f64,
}

/// BSIM3SOI-DD instance with node indices.
#[derive(Debug, Clone)]
pub struct Bsim3SoiDdInstance {
    pub name: String,
    pub drain_idx: Option<usize>,
    pub gate_idx: Option<usize>,
    pub source_idx: Option<usize>,
    /// Back-gate (E) node.
    pub e_idx: Option<usize>,
    /// External body contact (B/P) node — optional.
    pub body_idx: Option<usize>,
    pub drain_prime_idx: Option<usize>,
    pub source_prime_idx: Option<usize>,
    /// Internal body node (always created for SOI).
    pub body_int_idx: Option<usize>,
    pub w: f64,
    pub l: f64,
    pub m: f64,
    pub nrd: f64,
    pub nrs: f64,
    pub model: Bsim3SoiDdModel,
    pub size_params: Bsim3SoiDdSizeParam,
    pub vth0_inst: f64,
    pub nbc: f64,
}

/// NR companion result for BSIM3SOI-DD.
#[derive(Debug, Clone)]
pub struct Bsim3SoiDdCompanion {
    pub ids: f64,
    pub gm: f64,
    pub gds: f64,
    pub gmbs: f64,
    pub gme: f64,
    pub mode: i32,
    pub vdsat: f64,

    // Junction currents and conductances (body node KCL)
    pub ibs: f64,
    pub ibd: f64,
    pub gbs_jct: f64,
    pub gbd_jct: f64,
    /// dIbs/dVd cross-coupling (Gjsd in ngspice): Vds-dependent source junction derivative
    /// from BJT base transport factor (BjtA) in Ibs3.
    pub gjsd: f64,
    /// Extra dIbd/dVd beyond the two-terminal Vbd chain rule (from BjtA in Ibd3).
    /// Equal to dibd3_dvd + dibd3_dvb = dBjtA/dVd contribution.
    pub gjdd_extra: f64,

    // Impact ionization
    pub iii: f64,
    pub gii_d: f64,
    pub gii_g: f64,
    pub gii_b: f64,
    /// dIii/dVe — back-gate coupling of impact ionization (ngspice Giie).
    pub gii_e: f64,

    // GIDL
    pub igidl: f64,
    pub ggidl_d: f64,
    pub ggidl_g: f64,
    pub isgidl: f64,
    pub gsgidl_g: f64,

    // Equivalent current sources for NR companion
    pub ceq_d: f64,
    /// Combined drain-junction CEQ matching ngspice cjd:
    /// Ibd - Iii - Igidl - (gjdb*Vbs + gjdd*Vds + gjdg*Vgs + gjde*Ves)
    pub ceq_jd: f64,
    /// Combined source-junction CEQ matching ngspice cjs:
    /// Ibs - Isgidl - (gjsb*Vbs + gjsd*Vds + gjsg*Vgs)
    pub ceq_js: f64,
    /// Combined body CEQ matching ngspice cbody (pre-computed cancellation):
    /// Iii + Igidl + Isgidl - Ibs - Ibd - (gbbs*Vbs + gbgs*Vgs + gbds*Vds + gbes*Ves)
    pub ceq_body: f64,
    /// Combined body derivatives (for matrix stamps, ngspice gbbs/gbgs/gbds/gbes)
    pub gbbs: f64,
    pub gbgs: f64,
    pub gbds: f64,
    pub gbes: f64,

    // Capacitances (intrinsic)
    pub cggb: f64,
    pub cgdb: f64,
    pub cgsb: f64,
    pub cbgb: f64,
    pub cbdb: f64,
    pub cbsb: f64,
    pub cdgb: f64,
    pub cddb: f64,
    pub cdsb: f64,
    pub capbd: f64,
    pub capbs: f64,
    pub qinv: f64,

    // E-node (substrate) capacitance derivatives for 5-terminal transient coupling
    // (ngspice b3soiddld.c lines 3400-3421)
    pub cgeb: f64, // dQgate/dVe
    pub cbeb: f64, // dQbody/dVe
    pub cdeb: f64, // dQdrn/dVe
    pub ceeb: f64, // dQsub/dVe (substrate self-coupling)
    pub cegb: f64, // dQsub/dVg
    pub cedb: f64, // dQsub/dVd
    pub cesb: f64, // dQsub/dVs (by KCL constraint)

    // Terminal charges for transient integration (ngspice b3soiddld.c lines 3688-3692)
    pub qgate: f64,
    pub qbody: f64,
    pub qdrn: f64,
    pub qsub: f64,

    // DD-computed body-source voltage (for body node feedback in floating body)
    pub vbs_dd: f64,
}

impl Bsim3SoiDdModel {
    pub fn new(mos_type: MosfetType) -> Self {
        let vth0_default = match mos_type {
            MosfetType::Nmos => 0.7,
            MosfetType::Pmos => -0.7,
        };
        let u0_default = match mos_type {
            MosfetType::Nmos => 0.067,
            MosfetType::Pmos => 0.025,
        };
        let mut m = Self {
            mos_type,
            mob_mod: 1,
            cap_mod: 2,
            sh_mod: 0,
            tox: 100.0e-10,
            tsi: 1e-7,
            tbox: 3e-7,
            vth0: vth0_default,
            k1: 0.5,
            k2: 0.0,
            k3: 0.0,
            k3b: 0.0,
            w0: 2.5e-6,
            nlx: 1.74e-7,
            dvt0: 2.2,
            dvt1: 0.53,
            dvt2: -0.032,
            dvt0w: 0.0,
            dvt1w: 5.3e6,
            dvt2w: -0.032,
            dsub: 0.56,
            eta0: 0.08,
            etab: -0.07,
            voff: -0.08,
            nfactor: 1.0,
            cdsc: 2.4e-4,
            cdscb: 0.0,
            cdscd: 0.0,
            cit: 0.0,
            nch: 1.7e17,
            npeak: 1.7e17,
            ngate: 0.0,
            nsub: 6e16,
            u0: u0_default,
            ua: 2.25e-9,
            ub: 5.87e-19,
            uc: -4.65e-11,
            ute: -1.5,
            ua1: 4.31e-9,
            ub1: -7.61e-18,
            uc1: -5.6e-11,
            vsat: 8e4,
            a0: 1.0,
            ags: 0.0,
            a1: 0.0,
            a2: 1.0,
            at: 3.3e4,
            keta: -0.6,
            pclm: 1.3,
            pdiblc1: 0.39,
            pdiblc2: 0.0086,
            pdiblcb: 0.0,
            drout: 0.56,
            pvag: 0.0,
            delta: 0.01,
            rdsw: 100.0,
            prwg: 0.0,
            prwb: 0.0,
            prt: 0.0,
            wr: 1.0,
            dwg: 0.0,
            dwb: 0.0,
            b0: 0.0,
            b1: 0.0,
            wint: 0.0,
            lint: 0.0,
            dlc: 0.0,
            dwc: 0.0,
            alpha0: 0.0,
            alpha1: 1.0,
            beta0: 30.0,
            aii: 0.0,
            bii: 0.0,
            cii: 0.0,
            dii: -1.0,
            tnom: 27.0,
            kt1: -0.11,
            kt1l: 0.0,
            kt2: 0.022,
            ndiode: 1.0,
            ntun: 10.0,
            nrecf0: 2.0,
            nrecr0: 10.0,
            vrec0: 0.0,
            ntrecf: 0.0,
            ntrecr: 0.0,
            isbjt: 1e-6,
            isdif: 0.0,
            istun: 0.0,
            isrec: 1e-5,
            xbjt: 2.0,
            xdif: 2.0,
            xrec: 20.0,
            xtun: 0.0,
            ahli: 0.0,
            lbjt0: 0.2e-6,
            ln: 2e-6,
            nbjt: 1.0,
            ndif: -1.0,
            aely: 0.0,
            vabjt: 10.0,
            agidl: 0.0,
            bgidl: 0.0,
            ngidl: 1.2,
            kb1: 1.0,
            kb3: 1.0,
            dvbd0: 0.0,
            dvbd1: 0.0,
            vbsa: 0.0,
            delp: 0.02,
            abp: 1.0,
            mxc: -0.9,
            adice0: 1.0,
            xj: -1.0, // sentinel; will default to tsi in precompute
            kbjt1: 0.0,
            edl: 2e-6,
            rbody: 0.0,
            rbsh: 0.0,
            cgso: 0.0,
            cgdo: 0.0,
            clc: 0.1e-7,
            cle: 0.0,
            cf: 0.0,
            ckappa: 0.6,
            cgdl: 0.0,
            cgsl: 0.0,
            cjswg: 1e-10,
            mjswg: 0.5,
            pbswg: 0.7,
            tt: 1e-12,
            csdesw: 0.0,
            asd: 0.3,
            rth0: 0.0,
            cth0: 0.0,
            // Binning defaults: ngspice b3soiddset.c defaults l/w/p_kb3/dvbd0/dvbd1 to 1.0
            bin_unit: 1,
            lkb3: 1.0,
            wkb3: 1.0,
            pkb3: 1.0,
            ldvbd0: 1.0,
            wdvbd0: 1.0,
            pdvbd0: 1.0,
            ldvbd1: 1.0,
            wdvbd1: 1.0,
            pdvbd1: 1.0,
            cox: 0.0,
            vtm: 0.0,
            phi: 0.0,
            sqrt_phi: 0.0,
            vbi_default: 0.0,
            factor1: 0.0,
            ni: 0.0,
            eg: 0.0,
            cbox: 0.0,
            csi: 0.0,
            qsi: 0.0,
            csieff: 0.0,
            qsieff: 0.0,
            vfbb: 0.0,
            cboxt: 0.0,
            adice: 1.0,
        };
        m.precompute();
        m
    }

    pub fn from_params(model: &ModelParams) -> Self {
        let mos_type = match model.kind.to_uppercase().as_str() {
            "PMOS" => MosfetType::Pmos,
            _ => MosfetType::Nmos,
        };
        let mut m = Self::new(mos_type);
        // Self::new() ran precompute() against the *default* tsi, resolving the
        // xj sentinel to that tsi (1e-7). Restore the sentinel so the final
        // precompute() below re-resolves XJ against the model card's TSI,
        // matching ngspice b3soiddset.c:215 (xj defaults to tsi at set time).
        // set!(xj, "XJ") overwrites the sentinel when XJ is given explicitly.
        m.xj = -1.0;

        fn pf(model: &ModelParams, name: &str) -> Option<f64> {
            model.params.iter().find_map(|(n, v)| {
                if n.eq_ignore_ascii_case(name) {
                    Some(*v)
                } else {
                    None
                }
            })
        }

        macro_rules! set {
            ($field:ident, $name:expr) => {
                if let Some(v) = pf(model, $name) {
                    m.$field = v;
                }
            };
        }
        macro_rules! seti {
            ($field:ident, $name:expr) => {
                if let Some(v) = pf(model, $name) {
                    m.$field = v as i32;
                }
            };
        }

        seti!(mob_mod, "MOBMOD");
        seti!(cap_mod, "CAPMOD");
        seti!(sh_mod, "SHMOD");
        set!(tox, "TOX");
        set!(tsi, "TSI");
        set!(tbox, "TBOX");
        set!(vth0, "VTH0");
        set!(k1, "K1");
        set!(k2, "K2");
        set!(k3, "K3");
        set!(k3b, "K3B");
        set!(w0, "W0");
        set!(nlx, "NLX");
        set!(dvt0, "DVT0");
        set!(dvt1, "DVT1");
        set!(dvt2, "DVT2");
        set!(dvt0w, "DVT0W");
        set!(dvt1w, "DVT1W");
        set!(dvt2w, "DVT2W");
        set!(dsub, "DSUB");
        set!(eta0, "ETA0");
        set!(etab, "ETAB");
        set!(voff, "VOFF");
        set!(nfactor, "NFACTOR");
        set!(cdsc, "CDSC");
        set!(cdscb, "CDSCB");
        set!(cdscd, "CDSCD");
        set!(cit, "CIT");
        // NCH and NPEAK are aliases (ngspice maps "nch" -> B3SOIDDnpeak).
        // Whichever the model card uses, copy to both fields so precompute() sees it.
        set!(nch, "NCH");
        set!(npeak, "NPEAK");
        // If NCH was given but NPEAK was not (or vice versa), synchronise.
        if m.nch != 1.7e17 && m.npeak == 1.7e17 {
            m.npeak = m.nch;
        } else if m.npeak != 1.7e17 && m.nch == 1.7e17 {
            m.nch = m.npeak;
        }
        set!(ngate, "NGATE");
        set!(nsub, "NSUB");
        set!(u0, "U0");
        set!(ua, "UA");
        set!(ub, "UB");
        set!(uc, "UC");
        set!(ute, "UTE");
        set!(ua1, "UA1");
        set!(ub1, "UB1");
        set!(uc1, "UC1");
        set!(vsat, "VSAT");
        set!(a0, "A0");
        set!(ags, "AGS");
        set!(a1, "A1");
        set!(a2, "A2");
        set!(at, "AT");
        set!(keta, "KETA");
        set!(pclm, "PCLM");
        set!(pdiblc1, "PDIBLC1");
        set!(pdiblc2, "PDIBLC2");
        set!(pdiblcb, "PDIBLCB");
        set!(drout, "DROUT");
        set!(pvag, "PVAG");
        set!(delta, "DELTA");
        set!(rdsw, "RDSW");
        set!(prwg, "PRWG");
        set!(prwb, "PRWB");
        set!(prt, "PRT");
        set!(wr, "WR");
        set!(dwg, "DWG");
        set!(dwb, "DWB");
        set!(b0, "B0");
        set!(b1, "B1");
        set!(wint, "WINT");
        set!(lint, "LINT");
        set!(dlc, "DLC");
        set!(dwc, "DWC");
        set!(alpha0, "ALPHA0");
        set!(alpha1, "ALPHA1");
        set!(beta0, "BETA0");
        set!(aii, "AII");
        set!(bii, "BII");
        set!(cii, "CII");
        set!(dii, "DII");
        set!(tnom, "TNOM");
        set!(kt1, "KT1");
        set!(kt1l, "KT1L");
        set!(kt2, "KT2");
        set!(ndiode, "NDIODE");
        set!(ntun, "NTUN");
        set!(nrecf0, "NRECF0");
        set!(nrecr0, "NRECR0");
        set!(vrec0, "VREC0");
        set!(ntrecf, "NTRECF");
        set!(ntrecr, "NTRECR");
        set!(isbjt, "ISBJT");
        set!(isdif, "ISDIF");
        set!(istun, "ISTUN");
        set!(isrec, "ISREC");
        set!(xbjt, "XBJT");
        set!(xdif, "XDIF");
        set!(xrec, "XREC");
        set!(xtun, "XTUN");
        set!(ahli, "AHLI");
        set!(lbjt0, "LBJT0");
        set!(ln, "LN");
        set!(nbjt, "NBJT");
        set!(ndif, "NDIF");
        set!(aely, "AELY");
        set!(vabjt, "VABJT");
        set!(agidl, "AGIDL");
        set!(bgidl, "BGIDL");
        set!(ngidl, "NGIDL");
        set!(kb1, "KB1");
        set!(kb3, "KB3");
        set!(dvbd0, "DVBD0");
        set!(dvbd1, "DVBD1");
        set!(vbsa, "VBSA");
        set!(delp, "DELP");
        set!(abp, "ABP");
        set!(mxc, "MXC");
        set!(adice0, "ADICE0");
        set!(xj, "XJ");
        set!(kbjt1, "KBJT1");
        set!(edl, "EDL");
        set!(rbody, "RBODY");
        set!(rbsh, "RBSH");
        set!(cgso, "CGSO");
        set!(cgdo, "CGDO");
        set!(clc, "CLC");
        set!(cle, "CLE");
        set!(cf, "CF");
        set!(ckappa, "CKAPPA");
        set!(cgdl, "CGDL");
        set!(cgsl, "CGSL");
        set!(cjswg, "CJSWG");
        set!(mjswg, "MJSWG");
        set!(pbswg, "PBSWG");
        set!(tt, "TT");
        set!(csdesw, "CSDESW");
        set!(asd, "ASD");
        set!(rth0, "RTH0");
        set!(cth0, "CTH0");

        // Binning parameters for kb3/dvbd0/dvbd1 (default 1.0 in ngspice)
        seti!(bin_unit, "BINUNIT");
        set!(lkb3, "LKB3");
        set!(wkb3, "WKB3");
        set!(pkb3, "PKB3");
        set!(ldvbd0, "LDVBD0");
        set!(wdvbd0, "WDVBD0");
        set!(pdvbd0, "PDVBD0");
        set!(ldvbd1, "LDVBD1");
        set!(wdvbd1, "WDVBD1");
        set!(pdvbd1, "PDVBD1");

        // Handle u0 units: ngspice treats u0 > 1 as cm²/Vs, converts by /1e4
        if m.u0 > 1.0 {
            m.u0 /= 1e4;
        }

        m.precompute();
        m
    }

    fn precompute(&mut self) {
        // XJ defaults to tsi in BSIM3SOI-DD (ngspice b3soiddset.c)
        if self.xj < 0.0 {
            self.xj = self.tsi;
        }
        let tnom_k = self.tnom + 273.15;
        self.cox = EPSOX / self.tox;
        self.vtm = KBOQ * TEMP_DEFAULT;

        let eg0 = 1.16 - 7.02e-4 * tnom_k * tnom_k / (tnom_k + 1108.0);
        self.eg = eg0;
        self.ni = 1.45e10
            * (tnom_k / 300.15)
            * (tnom_k / 300.15).sqrt()
            * (21.5565981 - eg0 / (2.0 * KBOQ * tnom_k)).exp();

        let npeak = if self.npeak > 1e20 {
            self.npeak * 1e-6
        } else {
            self.npeak
        };

        self.phi = 2.0 * self.vtm * (npeak / self.ni).ln();
        if self.phi < 0.4 {
            self.phi = 0.4;
        }
        self.sqrt_phi = self.phi.sqrt();

        // Built-in potential: vbi = Vt * ln(ND * NA / ni²)
        // ND = 1e20 /cm³ (n+ S/D), NA = npeak /cm³, ni in /cm³ → dimensionless ratio.
        self.vbi_default = self.vtm * (1e20 * npeak / (self.ni * self.ni)).ln();
        self.factor1 = (EPSSI / EPSOX * self.tox).sqrt();

        // DD-specific precomputed (same as FD)
        self.cbox = EPSOX / self.tbox;
        self.csi = EPSSI / self.tsi;
        let nsub = if self.nsub > 1e20 {
            self.nsub * 1e-6
        } else {
            self.nsub
        };
        self.qsi = CHARGE_Q * npeak * 1e6 * self.tsi;

        // Effective silicon capacitance (for body potential calculation).
        // ngspice b3soiddset.c lines 975-992: csieff/qsieff depend on VBSA.
        // When VBSA is too large for the given tsi, fall back to csi/qsi.
        // Otherwise compute the effective depletion-corrected thickness (tsieff).
        let tmp1_vbsa = 2.0 * EPSSI * self.vbsa / CHARGE_Q / (1e6 * npeak);
        let tmp2_tsi2 = self.tsi * self.tsi;
        if tmp2_tsi2 < tmp1_vbsa {
            // VBSA too large: ngspice prints warning and uses full tsi
            self.csieff = self.csi;
            self.qsieff = self.qsi;
        } else {
            let tsieff = (tmp2_tsi2 - tmp1_vbsa).sqrt();
            self.csieff = EPSSI / tsieff;
            self.qsieff = CHARGE_Q * npeak * 1e6 * tsieff;
        }
        // Back-gate flat-band voltage: vfbb = -type * Vtm * ln(npeak / nsub)
        // Matches ngspice b3soiddtemp.c line 587.
        self.vfbb = -self.mos_type.sign() * self.vtm * (npeak / nsub).ln();
        // adice uses local Cboxt = cbox*csi/(cbox+csi) (ngspice b3soiddset.c line 973, 997)
        let cboxt_local = self.cbox * self.csi / (self.cbox + self.csi);
        self.adice = self.adice0 / (1.0 + cboxt_local / self.cox);
        // Stored cboxt uses csieff (VBSA-adjusted), for Qe2 charge (ngspice line 994)
        self.cboxt = 1.0 / (1.0 / self.cbox + 1.0 / self.csieff);
    }

    /// Number of internal nodes this model creates.
    /// Drain/source prime nodes only created when sheet resistance (RBSH) and
    /// drain/source squares (NRD/NRS) are both positive — matching ngspice
    /// b3soiddset.c which checks `sheetResistance > 0 && drainSquares > 0`.
    /// RDSW is folded into the channel model, not external series resistance.
    pub fn internal_node_count(&self, nrd: f64, nrs: f64) -> usize {
        let mut count = 1; // Always create internal body node
        if self.rbsh > 0.0 && nrd > 0.0 {
            count += 1; // drain prime
        }
        if self.rbsh > 0.0 && nrs > 0.0 {
            count += 1; // source prime
        }
        count
    }

    pub fn size_dep_param(&self, w: f64, l: f64, temp: f64) -> Bsim3SoiDdSizeParam {
        let tnom_k = self.tnom + 273.15;
        let vtm = KBOQ * temp;

        let dl = self.lint;
        let dw = self.wint;
        let leff = l - 2.0 * dl;
        let weff = w - 2.0 * dw;
        let dlc = if self.dlc != 0.0 { self.dlc } else { dl };
        let dwc = if self.dwc != 0.0 { self.dwc } else { dw };
        let leff_cv = l - 2.0 * dlc;
        let weff_cv = w - 2.0 * dwc;

        let phi = self.phi;
        let sqrt_phi = self.sqrt_phi;
        let temp_ratio = temp / tnom_k;
        // ngspice b3soiddtemp.c line 530: T0 = (TRatio - 1.0)
        // All temperature coefficient terms use (TRatio - 1.0), not (T - Tnom)
        let t_ratio_minus1 = temp_ratio - 1.0;

        let u0temp = self.u0 * temp_ratio.powf(self.ute);
        let vsattemp = self.vsat - self.at * t_ratio_minus1;

        let rds0 = if self.rdsw > 0.0 {
            // ngspice b3soiddtemp.c: rds0 = (rdsw + prt*T0) / pow(weff*1e6, wr)
            (self.rdsw + self.prt * t_ratio_minus1) / (weff * 1e6).powf(self.wr)
        } else {
            0.0
        };

        let ua = self.ua + self.ua1 * t_ratio_minus1;
        let ub = self.ub + self.ub1 * t_ratio_minus1;
        let uc = self.uc + self.uc1 * t_ratio_minus1;

        let npeak = if self.npeak > 1e20 {
            self.npeak * 1e-6
        } else {
            self.npeak
        };
        let nsub = if self.nsub > 1e20 {
            self.nsub * 1e-6
        } else {
            self.nsub
        };

        let xdep0 = (2.0 * EPSSI / (CHARGE_Q * npeak * 1e6)).sqrt() * sqrt_phi;

        // litl: ngspice b3soiddtemp.c line 651: sqrt(3.0 * xj * tox)
        // ngspice uses hardcoded 3.0 (not EPSSI/EPSOX ≈ 2.99934).
        let litl = (3.0 * self.xj * self.tox).sqrt();

        // Characteristic length for DIBL (theta0vb0) and PDIBL (theta_rout).
        // ngspice b3soiddtemp.c line 734: T1 = sqrt(EPSSI / EPSOX * tox * Xdep0)
        let t1_soi = (EPSSI * xdep0 / self.cox).sqrt();

        // theta0vb0: ngspice uses dsub (NOT dvt1) and does NOT multiply by dvt0.
        // b3soiddtemp.c lines 736-737.
        let t0 = -0.5 * self.dsub * leff / t1_soi;
        let theta0vb0 = if t0 > -EXP_THRESHOLD {
            let t1 = t0.exp();
            t1 + 2.0 * t1 * t1
        } else {
            MIN_EXP + 2.0 * MIN_EXP * MIN_EXP
        };

        // theta_rout: ngspice uses drout with same characteristic length.
        // b3soiddtemp.c lines 739-742.
        let t0 = -0.5 * self.drout * leff / t1_soi;
        let theta_rout = if t0 > -EXP_THRESHOLD {
            let t1 = t0.exp();
            self.pdiblc1 * (t1 + 2.0 * t1 * t1) + self.pdiblc2
        } else {
            self.pdiblc2
        };

        let k1eff = self.k1;
        // ngspice b3soiddtemp.c lines 723-726: vfb = type * VTH0 - phi - k1 * sqrtPhi
        // Only when VTH0 is given; otherwise vfb = -1.0.  Since all test model cards
        // specify VTH0, and our default (0.7/-0.7) is set consistently, always compute.
        let sign = self.mos_type.sign();
        let vfb = sign * self.vth0 - phi - self.k1 * sqrt_phi;
        // ngspice b3soiddtemp.c line 655: sqrt(q * EPSSI * npeak * 1e6 / 2.0 / phi)
        let cdep0 = (CHARGE_Q * EPSSI * npeak * 1e6 / 2.0 / phi).sqrt();

        let eg = 1.16 - 7.02e-4 * temp * temp / (temp + 1108.0);
        let ni_temp = 1.45e10
            * (temp / 300.15)
            * (temp / 300.15).sqrt()
            * (21.5565981 - eg / (2.0 * vtm)).exp();
        // vbi = Vt * ln(1e20 * npeak / ni²), matching ngspice b3soiddtemp.c line ~654.
        let vbi = vtm * (1e20 * npeak / (ni_temp * ni_temp)).abs().ln();

        // SOI junction parameters — ngspice b3soiddtemp.c lines 574-584
        // DD model uses power-law + bandgap exponential, NOT the PD nrecf0/ntrecf formula.
        let eg0 = self.eg; // bandgap at Tnom, computed in set_defaults_and_derive()
        let t0_jbjt = temp_ratio.powf(self.xbjt / self.ndiode);
        let t1_jdif = temp_ratio.powf(self.xdif / self.ndiode);
        let t2_jrec = temp_ratio.powf(self.xrec / self.ndiode / 2.0);
        let t4 = -eg0 / self.ndiode / vtm * (1.0 - temp_ratio);
        let t5 = t4.exp();
        let t6 = t5.sqrt();
        let jbjt = self.isbjt * t0_jbjt * t5;
        let jdif = self.isdif * t1_jdif * t5;
        let jrec = self.isrec * t2_jrec * t6;
        let t0_jtun = temp_ratio.powf(self.xtun / self.ntun);
        let jtun = self.istun * t0_jtun;

        // ngspice DD uses weff directly for IGIDL (b3soiddld.c line 2222/2248),
        // not wdios/wdiod. The DD model has no separate wdios/wdiod variables.
        let wdios = weff;
        let wdiod = weff;

        let lratio = if self.lbjt0 > 0.0 {
            (1.0 - leff / (leff + self.lbjt0)) / (1.0 + (self.ndif * leff / (leff + self.lbjt0)))
        } else {
            0.0
        };
        let lratiodif = lratio;
        let vearly = if self.vabjt > 0.0 { self.vabjt } else { 10.0 };
        let arfabjt = self.xbjt;

        let cgso_eff = if self.cgso > 0.0 {
            self.cgso
        } else {
            0.6 * dlc * self.cox
        };
        let cgdo_eff = if self.cgdo > 0.0 {
            self.cgdo
        } else {
            0.6 * dlc * self.cox
        };

        // Parameter binning for kb3, dvbd0, dvbd1.
        // ngspice b3soiddtemp.c: binned = base + l*Inv_L + w*Inv_W + p*Inv_LW
        // L/W/P coefficients default to 1.0 in b3soiddset.c
        let (inv_l, inv_w, inv_lw) = if self.bin_unit == 1 {
            (1.0e-6 / leff, 1.0e-6 / weff, 1.0e-12 / (leff * weff))
        } else {
            (1.0 / leff, 1.0 / weff, 1.0 / (leff * weff))
        };
        let kb3_binned = self.kb3 + self.lkb3 * inv_l + self.wkb3 * inv_w + self.pkb3 * inv_lw;
        let dvbd0_binned =
            self.dvbd0 + self.ldvbd0 * inv_l + self.wdvbd0 * inv_w + self.pdvbd0 * inv_lw;
        let dvbd1_binned =
            self.dvbd1 + self.ldvbd1 * inv_l + self.wdvbd1 * inv_w + self.pdvbd1 * inv_lw;

        Bsim3SoiDdSizeParam {
            leff,
            weff,
            leff_cv,
            weff_cv,
            vth0: self.vth0,
            k1: self.k1,
            k2: self.k2,
            k3: self.k3,
            k3b: self.k3b,
            w0: self.w0,
            nlx: self.nlx,
            dvt0: self.dvt0,
            dvt1: self.dvt1,
            dvt2: self.dvt2,
            dvt0w: self.dvt0w,
            dvt1w: self.dvt1w,
            dvt2w: self.dvt2w,
            eta0: self.eta0,
            etab: self.etab,
            dsub: self.dsub,
            voff: self.voff,
            nfactor: self.nfactor,
            cdsc: self.cdsc,
            cdscb: self.cdscb,
            cdscd: self.cdscd,
            cit: self.cit,
            u0: u0temp,
            ua,
            ub,
            uc,
            vsat: vsattemp,
            a0: self.a0,
            ags: self.ags,
            a1: self.a1,
            a2: self.a2,
            at: self.at,
            keta: self.keta,
            pclm: self.pclm,
            pdiblc1: self.pdiblc1,
            pdiblc2: self.pdiblc2,
            pdiblcb: self.pdiblcb,
            pvag: self.pvag,
            delta: self.delta,
            rdsw: self.rdsw,
            prwg: self.prwg,
            prwb: self.prwb,
            dwg: self.dwg,
            dwb: self.dwb,
            b0: self.b0,
            b1: self.b1,
            alpha0: self.alpha0,
            beta0: self.beta0,
            kt1: self.kt1,
            kt1l: self.kt1l,
            kt2: self.kt2,
            ndiode: self.ndiode,
            agidl: self.agidl,
            bgidl: self.bgidl,
            ngidl: self.ngidl,
            phi,
            sqrt_phi,
            xdep0,
            litl,
            theta0vb0,
            theta_rout,
            k1eff,
            npeak,
            nsub,
            vfb,
            u0temp,
            vsattemp,
            rds0,
            cdep0,
            vbi,
            lratio,
            lratiodif,
            vearly,
            arfabjt,
            wdios,
            wdiod,
            jbjt,
            jdif,
            jrec,
            jtun,
            cgso_eff,
            cgdo_eff,
            kb3: kb3_binned,
            dvbd0: dvbd0_binned,
            dvbd1: dvbd1_binned,
            // ngspice b3soiddtemp.c line 194: abulkCVfactor = (1 + clc/leff)^cle
            abulk_cv_factor: (1.0 + self.clc / leff).powf(self.cle),
            // ngspice b3soiddtemp.c line 744: minimum substrate current for body node stability
            min_isub: 5.0e-2 * weff * self.tsi * self.isdif.max(self.isrec),
            nseg: 1.0,
        }
    }
}

impl Bsim3SoiDdInstance {
    pub fn terminal_voltages(&self, solution: &[f64]) -> (f64, f64, f64, f64) {
        let vg = self.gate_idx.map_or(0.0, |i| solution[i]);
        let vd = self.drain_eff_idx().map_or(0.0, |i| solution[i]);
        let vs = self.source_eff_idx().map_or(0.0, |i| solution[i]);
        let vb = self.body_int_idx.map_or(0.0, |i| solution[i]);

        let sign = self.model.mos_type.sign();
        let vgs = sign * (vg - vs);
        let vds = sign * (vd - vs);
        let vbs = sign * (vb - vs);
        let ves = sign * (self.e_idx.map_or(0.0, |i| solution[i]) - vs);

        (vgs, vds, vbs, ves)
    }

    pub fn drain_eff_idx(&self) -> Option<usize> {
        self.drain_prime_idx.or(self.drain_idx)
    }

    pub fn source_eff_idx(&self) -> Option<usize> {
        self.source_prime_idx.or(self.source_idx)
    }

    pub fn ac_stamp(&self, comp: &Bsim3SoiDdCompanion) -> crate::ac::BsimAcStamp {
        crate::ac::BsimAcStamp {
            dp: self.drain_eff_idx(),
            g: self.gate_idx,
            sp: self.source_eff_idx(),
            b: self.body_int_idx,
            drain_idx: self.drain_idx,
            source_idx: self.source_idx,
            m: self.m,
            gm: comp.gm,
            gds: comp.gds,
            gmbs: comp.gmbs,
            gbd: comp.gbd_jct,
            gbs: comp.gbs_jct,
            g_drain: 0.0,
            g_source: 0.0,
            cggb: comp.cggb,
            cgdb: comp.cgdb,
            cgsb: comp.cgsb,
            cbgb: comp.cbgb,
            cbdb: comp.cbdb,
            cbsb: comp.cbsb,
            cdgb: comp.cdgb,
            cddb: comp.cddb,
            cdsb: comp.cdsb,
            capbd: comp.capbd,
            capbs: comp.capbs,
        }
    }
}

/// Compute BSIM3SOI-DD companion model (NR linearization).
///
/// DD uses the FD-style self-consistent surface potential chain for body bias,
/// combined with PD-style 4-component junction currents and GIDL.
#[expect(clippy::too_many_lines)]
pub fn bsim3soi_dd_companion(
    vgs: f64,
    vds: f64,
    vbs: f64,
    ves: f64,
    sp: &Bsim3SoiDdSizeParam,
    model: &Bsim3SoiDdModel,
) -> Bsim3SoiDdCompanion {
    let sign = model.mos_type.sign();
    let cox = model.cox;
    let vtm = KBOQ * TEMP_DEFAULT;
    let phi = sp.phi;
    let sqrt_phi = sp.sqrt_phi;

    // Mode detection (forward/reverse)
    let (vgs_i, vds_i, vbs_i, ves_i, mode) = if vds >= 0.0 {
        (vgs, vds, vbs, ves, 1)
    } else {
        (vgs - vds, -vds, vbs - vds, ves - vds, -1)
    };

    let leff = sp.leff;
    let weff = sp.weff;
    let vbi = sp.vbi;
    let v0 = vbi - phi;

    let vesfb = ves_i - model.vfbb;

    // ========== DD surface potential chain (same as FD) ==========

    // Vbs0t
    let t0 = -sp.dvbd1 * leff / sp.litl;
    let t1 = sp.dvbd0 * (safe_exp(0.5 * t0) + 2.0 * safe_exp(t0));
    let t2 = t1 * v0;
    let t3 = 0.5 * model.qsi / model.csi;
    let vbs0t = phi - t3 + model.vbsa + t2;

    // Vbs0 (with back-gate coupling)
    let t0_kb = 1.0 + model.csieff / model.cbox;
    let t1_kb = model.kb1 / t0_kb;
    let t2_kb = t1_kb * (vbs0t - vesfb);
    let t6_vbs0 = vbs0t - t2_kb;

    // Limit Vbs0 below phi - delp
    let t1_lim = phi - model.delp;
    let t2_lim = t1_lim - t6_vbs0 - DELT_VBSEFF;
    let t3_lim = (t2_lim * t2_lim + 4.0 * DELT_VBSEFF).sqrt();
    let vbs0 = t1_lim - 0.5 * (t2_lim + t3_lim);

    // Vbs0mos
    let t1_mos = vbs0t - vbs0 - DELT_VBSMOS;
    let t2_mos = (t1_mos * t1_mos + DELT_VBSMOS * DELT_VBSMOS).sqrt();
    let t3_mos = 0.5 * (t1_mos + t2_mos);
    let t4_mos = t3_mos * model.csieff / model.qsieff;
    let vbs0mos = vbs0 - 0.5 * t3_mos * t4_mos;

    // Vthfd (threshold voltage using Vbs0mos)
    let phis_fd = phi - vbs0mos;
    let sqrt_phis_fd = phis_fd.abs().sqrt();
    let xdep_fd = sp.xdep0 * sqrt_phis_fd / sqrt_phi;

    let t0_dvt = sp.dvt2 * vbs0mos;
    let t1_dvt = if t0_dvt >= -0.5 {
        1.0 + t0_dvt
    } else {
        let t4 = 1.0 / (3.0 + 8.0 * t0_dvt);
        (1.0 + 3.0 * t0_dvt) * t4
    };
    let lt1_fd = model.factor1 * xdep_fd.sqrt() * t1_dvt;

    let t0_sce = -0.5 * sp.dvt1 * leff / lt1_fd;
    let theta0_fd = if t0_sce > -EXP_THRESHOLD {
        let t1 = t0_sce.exp();
        t1 * (1.0 + 2.0 * t1)
    } else {
        MIN_EXP * (1.0 + 2.0 * MIN_EXP)
    };
    let delt_vth_fd = sp.dvt0 * theta0_fd * v0;

    let t0_dvtw = sp.dvt2w * vbs0mos;
    let t1_dvtw = if t0_dvtw >= -0.5 {
        1.0 + t0_dvtw
    } else {
        let t4 = 1.0 / (3.0 + 8.0 * t0_dvtw);
        (1.0 + 3.0 * t0_dvtw) * t4
    };
    let ltw_fd = model.factor1 * xdep_fd.sqrt() * t1_dvtw;
    let t0_w_fd = -0.5 * sp.dvt1w * weff * leff / ltw_fd;
    let t2_w_fd = if t0_w_fd > -EXP_THRESHOLD {
        let t1 = t0_w_fd.exp();
        t1 * (1.0 + 2.0 * t1)
    } else {
        MIN_EXP * (1.0 + 2.0 * MIN_EXP)
    };
    let delt_vthw_fd = sp.dvt0w * t2_w_fd * v0;

    let temp_ratio_minus1 = (TEMP_DEFAULT / (model.tnom + 273.15)) - 1.0;
    let t0_nlx = (1.0 + sp.nlx / leff).sqrt();
    let t1_kt = sp.kt1 + sp.kt1l / leff + sp.kt2 * vbs0mos;
    let delt_vth_temp_fd = sp.k1 * (t0_nlx - 1.0) * sqrt_phi + t1_kt * temp_ratio_minus1;

    let tmp2_fd = model.tox * phi / (weff + sp.w0);
    let t3_eta_fd = sp.eta0 + sp.etab * vbs0mos;
    let t3_eta_fd_eff = if t3_eta_fd < 1e-4 {
        let t9 = 1.0 / (3.0 - 2e4 * t3_eta_fd);
        (2e-4 - t3_eta_fd) * t9
    } else {
        t3_eta_fd
    };
    let dibl_sft_fd = t3_eta_fd_eff * sp.theta0vb0 * vds_i;

    let vthfd = sign * sp.vth0 + sp.k1 * (sqrt_phis_fd - sqrt_phi)
        - sp.k2 * vbs0mos
        - delt_vth_fd
        - delt_vthw_fd
        + (sp.k3 + sp.k3b * vbs0mos) * tmp2_fd
        + delt_vth_temp_fd
        - dibl_sft_fd;

    // Poly gate depletion
    let t0_poly = sp.vfb + phi;
    let (vgs_eff, dvgs_eff_dvg) = if model.ngate > 1e18 && model.ngate < 1e25 && vgs_i > t0_poly {
        let t1 = 1e6 * CHARGE_Q * EPSSI * model.ngate / (cox * cox);
        let t4 = (1.0 + 2.0 * (vgs_i - t0_poly) / t1).sqrt();
        let t2 = t1 * (t4 - 1.0);
        let t3 = 0.5 * t2 * t2 / t1;
        let t7 = 1.12 - t3 - 0.05;
        let t6 = (t7 * t7 + 0.224).sqrt();
        let t5 = 1.12 - 0.5 * (t7 + t6);
        (vgs_i - t5, 1.0 - (0.5 - 0.5 / t4) * (1.0 + t7 / t6))
    } else {
        (vgs_i, 1.0)
    };

    // ========== DD Vbs0eff and Vbsmos calculation ==========

    let t1_eff = vthfd - vgs_eff - DELT_VBS0EFF;
    let t2_eff = (t1_eff * t1_eff + DELT_VBS0EFF * DELT_VBS0EFF).sqrt();

    let vbs0teff = vbs0t - 0.5 * (t1_eff + t2_eff);

    // Nfb (feedback factor)
    let k1 = sp.k1;
    let t3_nfb = 1.0 / (k1 * k1);
    let t4_nfb = sp.kb3 * model.cbox / cox;
    let t8_nfb = (phi - vbs0mos).abs().sqrt();
    let t5_nfb = (1.0 + 4.0 * t3_nfb * (phi + k1 * t8_nfb - vbs0mos))
        .abs()
        .sqrt();
    let t6_nfb = 1.0 + t4_nfb * t5_nfb;
    let nfb = 1.0 / t6_nfb;

    let vbs0eff_dd = vbs0 - nfb * 0.5 * (t1_eff + t2_eff);

    // Vbsdio = smooth_max(vbs_i, vbs0eff_dd + OFF_VBSDIO)
    // Clamps effective body-source voltage from below at the device physics floor.
    // When body is above floor (normal operation): vbsdio ≈ vbs_i.
    // When body is below floor (unphysical): vbsdio ≈ vbs0eff_dd + OFF_VBSDIO.
    let t1_vbsdio = vbs_i - (vbs0eff_dd + OFF_VBSDIO) - DELT_VBSDIO;
    let t2_vbsdio = (t1_vbsdio * t1_vbsdio + DELT_VBSDIO * DELT_VBSDIO).sqrt();
    let dvbsdio_dvb = 0.5 * (1.0 + t1_vbsdio / t2_vbsdio);
    let vbsdio = vbs0eff_dd + OFF_VBSDIO + 0.5 * (t1_vbsdio + t2_vbsdio);

    // Vbsmos (ngspice lines 1131-1150)
    let t1_bsmos = vbs0teff - vbsdio - DELT_VBSMOS;
    let t2_bsmos = (t1_bsmos * t1_bsmos + DELT_VBSMOS * DELT_VBSMOS).sqrt();
    let t3_bsmos = 0.5 * (t1_bsmos + t2_bsmos);
    let t5_bsmos = 0.5 * (1.0 + t1_bsmos / t2_bsmos);
    let t4_bsmos = t3_bsmos * model.csieff / model.qsieff;
    let vbsmos = vbsdio - 0.5 * t3_bsmos * t4_bsmos;
    // dvbsmos/dvbsdio: vbsmos = vbsdio - 0.5*T3*T4, dT3/dvbsdio = -T5
    let dvbsmos_dvbsdio = 1.0 + t5_bsmos * t4_bsmos;

    // ========== Vbseff (final body-source effective voltage) ==========
    let t1_vbseff = phi - model.delp;
    let t2_vbseff = t1_vbseff - vbsmos - DELT_VBSEFF;
    let t3_vbseff = (t2_vbseff * t2_vbseff + 4.0 * DELT_VBSEFF * t1_vbseff).sqrt();
    let vbseff = t1_vbseff - 0.5 * (t2_vbseff + t3_vbseff);
    let dvbseff_dvbsmos = 0.5 * (1.0 + t2_vbseff / t3_vbseff);

    // The DD-computed Vbs for body node feedback
    let vbs_dd = vbsdio;

    // --- Cross-derivatives: dVbseff/dVg and dVbseff/dVd ---
    // Track how gate and drain voltages affect Vbseff through the back-gate
    // coupling chain (Vbs0teff → Vbs0eff → Vbsdio → Vbsmos → Vbseff).
    // Required for correct Jacobian in floating-body devices (ngspice
    // b3soiddld.c lines 1090-1191).

    // Smoothing factor from Vbs0teff = Vbs0t - 0.5*(t1_eff + t2_eff)
    let s_teff = 0.5 * (1.0 + t1_eff / t2_eff);

    // dVthfd/dVd (DIBL in floating-body threshold)
    let dvthfd_dvd = -t3_eta_fd_eff * sp.theta0vb0;

    // dVbs0teff/dVg, dVbs0teff/dVd (ngspice lines 1090-1091)
    let dvbs0teff_dvg = s_teff * dvgs_eff_dvg;
    let dvbs0teff_dvd = -s_teff * dvthfd_dvd;

    // dVbs0eff/dVg, dVbs0eff/dVd (ngspice lines 1107-1108)
    let dvbs0eff_dvg = nfb * s_teff * dvgs_eff_dvg;
    let dvbs0eff_dvd = -nfb * s_teff * dvthfd_dvd;

    // dVbsdio/dVg, dVbsdio/dVd (ngspice lines 1124-1125)
    let dvbsdio_dvg = (1.0 - dvbsdio_dvb) * dvbs0eff_dvg;
    let dvbsdio_dvd = (1.0 - dvbsdio_dvb) * dvbs0eff_dvd;

    // dVbsmos/dVg, dVbsmos/dVd (ngspice lines 1145-1146)
    let dt1_bsmos_dvg = dvbs0teff_dvg - dvbsdio_dvg;
    let dt1_bsmos_dvd = dvbs0teff_dvd - dvbsdio_dvd;
    let dvbsmos_dvg = dvbsdio_dvg - t4_bsmos * t5_bsmos * dt1_bsmos_dvg;
    let dvbsmos_dvd = dvbsdio_dvd - t4_bsmos * t5_bsmos * dt1_bsmos_dvd;

    // dVbseff/dVg, dVbseff/dVd (ngspice lines 1188-1189)
    let dvbseff_dvg = dvbseff_dvbsmos * dvbsmos_dvg;
    let dvbseff_dvd = dvbseff_dvbsmos * dvbsmos_dvd;

    // --- Cross-derivatives: dVbseff/dVe (back-gate transconductance chain) ---
    // Ve affects Ids through the back-gate coupling chain:
    //   Ve → Vbs0 → Vbs0mos → Vthfd → Vbs0teff → Vbs0eff → Vbsdio → Vbsmos → Vbseff
    // ngspice b3soiddld.c lines 939-1191.

    // dT6/dVe = kb1/(1+csieff/Cbox) (ngspice line 939)
    let dt6_dve = t1_kb;

    // dVbs0/dVe (ngspice line 951): smoothing factor from Vbs0 limit
    let s_vbs0 = 0.5 * (1.0 + t2_lim / t3_lim);
    let dvbs0_dve = s_vbs0 * dt6_dve;

    // dVbs0mos/dVe (ngspice lines 960-961)
    // T5 in ngspice = 0.5 * T4_mos * (1 + T1_mos/T2_mos)
    let s_vbs0mos = 0.5 * t4_mos * (1.0 + t1_mos / t2_mos);
    let dvbs0mos_dve = dvbs0_dve * (1.0 + s_vbs0mos);

    // dVthfd/dVbs0mos (ngspice lines 1075-1077: T7)
    // Replicate the Vthfd sensitivity using the FD-region intermediates
    let dsqrt_phis_fd_dvbs0mos = -0.5 / sqrt_phis_fd.max(1e-20);
    let dxdep_fd_dvbs0mos = (sp.xdep0 / sqrt_phi) * dsqrt_phis_fd_dvbs0mos;

    // SCE derivative: dTheta0_fd/dVbs0mos through lt1_fd
    let dt1_dvt_dvbs0mos = if sp.dvt2 * vbs0mos >= -0.5 {
        sp.dvt2
    } else {
        let t4 = 1.0 / (3.0 + 8.0 * sp.dvt2 * vbs0mos);
        sp.dvt2 * t4 * t4
    };
    let sqrt_xdep_fd = xdep_fd.sqrt();
    let dlt1_fd_dvbs0mos = model.factor1
        * (0.5 / sqrt_xdep_fd.max(1e-20) * t1_dvt * dxdep_fd_dvbs0mos
            + sqrt_xdep_fd * dt1_dvt_dvbs0mos);
    let t0_sce_val = -0.5 * sp.dvt1 * leff / lt1_fd;
    let ddelt_vth_fd_dvbs0mos = if t0_sce_val > -EXP_THRESHOLD {
        let t1_exp = t0_sce_val.exp();
        let dtheta0_fd_dvbs0mos =
            (-t0_sce_val / lt1_fd * t1_exp * dlt1_fd_dvbs0mos) * (1.0 + 4.0 * t1_exp);
        sp.dvt0 * dtheta0_fd_dvbs0mos * v0
    } else {
        0.0
    };

    // Width SCE derivative: dDeltVthw_fd/dVbs0mos
    let dt1_dvtw_dvbs0mos = if sp.dvt2w * vbs0mos >= -0.5 {
        sp.dvt2w
    } else {
        let t4 = 1.0 / (3.0 + 8.0 * sp.dvt2w * vbs0mos);
        sp.dvt2w * t4 * t4
    };
    let dltw_fd_dvbs0mos = model.factor1
        * (0.5 / sqrt_xdep_fd.max(1e-20) * t1_dvtw * dxdep_fd_dvbs0mos
            + sqrt_xdep_fd * dt1_dvtw_dvbs0mos);
    let t0_w_fd_val = -0.5 * sp.dvt1w * weff * leff / ltw_fd;
    let ddelt_vthw_fd_dvbs0mos = if t0_w_fd_val > -EXP_THRESHOLD {
        let t1_exp = t0_w_fd_val.exp();
        let dt2_w_fd_dvbs0mos =
            (-t0_w_fd_val / ltw_fd * t1_exp * dltw_fd_dvbs0mos) * (1.0 + 4.0 * t1_exp);
        sp.dvt0w * dt2_w_fd_dvbs0mos * v0
    } else {
        0.0
    };

    // DIBL derivative in Vthfd
    let dt3_eta_fd_dvbs0mos = if t3_eta_fd < 1e-4 {
        let t9 = 1.0 / (3.0 - 2e4 * t3_eta_fd);
        t9 * t9 * sp.etab
    } else {
        sp.etab
    };
    let ddibl_sft_fd_dvbs0mos = sp.theta0vb0 * vds_i * dt3_eta_fd_dvbs0mos;

    // T6 + K3b*tmp2 - K2 + KT2*TempRatio (ngspice line 1072-1073)
    let t6_vthfd = sp.k3b * tmp2_fd - sp.k2 + sp.kt2 * temp_ratio_minus1;

    // Full dVthfd/dVbs0mos (ngspice line 1075-1077: T7)
    let dvthfd_dvbs0mos =
        sp.k1 * dsqrt_phis_fd_dvbs0mos - ddelt_vth_fd_dvbs0mos - ddelt_vthw_fd_dvbs0mos + t6_vthfd
            - ddibl_sft_fd_dvbs0mos;

    // dVthfd/dVe (ngspice line 1078)
    let dvthfd_dve = dvthfd_dvbs0mos * dvbs0mos_dve;

    // dVbs0teff/dVe (ngspice line 1092)
    let dvbs0teff_dve = -s_teff * dvthfd_dve;

    // dVbs0eff/dVe (ngspice lines 1109-1110)
    // T7 from ngspice line 1105 is the Nfb derivative factor
    let t8_nfb_val = (phi - vbs0mos).abs().sqrt();
    let t5_nfb_val = (1.0 + 4.0 * t3_nfb * (phi + k1 * t8_nfb_val - vbs0mos))
        .abs()
        .sqrt();
    let t7_nfb = 2.0 * t3_nfb * t4_nfb * nfb * nfb / t5_nfb_val * (0.5 * k1 / t8_nfb_val + 1.0);
    let dvbs0eff_dve =
        dvbs0_dve - nfb * s_teff * dvthfd_dve - t7_nfb * 0.5 * (t1_eff + t2_eff) * dvbs0mos_dve;

    // dVbsdio/dVe (ngspice line 1126)
    let dvbsdio_dve = (1.0 - dvbsdio_dvb) * dvbs0eff_dve;

    // dVbsmos/dVe (ngspice lines 1139, 1148)
    let dt1_bsmos_dve = dvbs0teff_dve - dvbsdio_dve;
    let dvbsmos_dve = dvbsdio_dve - t4_bsmos * t5_bsmos * dt1_bsmos_dve;

    // dVcs/dVe (ngspice line 1157)
    let dvcs_dve = dvbsdio_dve - dvbs0eff_dve;

    // dVbseff/dVe (ngspice line 1191)
    let dvbseff_dve = dvbseff_dvbsmos * dvbsmos_dve;

    // ========== Main MOSFET equations ==========
    let phis = phi - vbseff;
    let sqrt_phis = phis.abs().sqrt();
    let dsqrt_phis_dvb = -0.5 / sqrt_phis.max(1e-20);
    let xdep = sp.xdep0 * sqrt_phis / sqrt_phi;
    let dxdep_dvb = (sp.xdep0 / sqrt_phi) * dsqrt_phis_dvb;

    // Vth calculation
    let t3_vth = xdep.sqrt();

    let t0 = sp.dvt2 * vbseff;
    let (t1, t2_) = if t0 >= -0.5 {
        (1.0 + t0, sp.dvt2)
    } else {
        let t4 = 1.0 / (3.0 + 8.0 * t0);
        ((1.0 + 3.0 * t0) * t4, sp.dvt2 * t4 * t4)
    };
    let lt1 = model.factor1 * t3_vth * t1;
    let dlt1_dvb = model.factor1 * (0.5 / t3_vth * t1 * dxdep_dvb + t3_vth * t2_);

    let t0w = sp.dvt2w * vbseff;
    let (t1w, t2w) = if t0w >= -0.5 {
        (1.0 + t0w, sp.dvt2w)
    } else {
        let t4 = 1.0 / (3.0 + 8.0 * t0w);
        ((1.0 + 3.0 * t0w) * t4, sp.dvt2w * t4 * t4)
    };
    let ltw = model.factor1 * t3_vth * t1w;
    let dltw_dvb = model.factor1 * (0.5 / t3_vth * t1w * dxdep_dvb + t3_vth * t2w);

    let t0_sce2 = -0.5 * sp.dvt1 * leff / lt1;
    let (theta0, dtheta0_dvb) = if t0_sce2 > -EXP_THRESHOLD {
        let t1 = t0_sce2.exp();
        (
            t1 * (1.0 + 2.0 * t1),
            (-t0_sce2 / lt1 * t1 * dlt1_dvb) * (1.0 + 4.0 * t1),
        )
    } else {
        (MIN_EXP * (1.0 + 2.0 * MIN_EXP), 0.0)
    };
    let delt_vth = sp.dvt0 * theta0 * v0;
    let ddelt_vth_dvb = sp.dvt0 * dtheta0_dvb * v0;

    let t0_w = -0.5 * sp.dvt1w * weff * leff / ltw;
    let (t2_w, dt2w_dvb) = if t0_w > -EXP_THRESHOLD {
        let t1 = t0_w.exp();
        (
            t1 * (1.0 + 2.0 * t1),
            (-t0_w / ltw * t1 * dltw_dvb) * (1.0 + 4.0 * t1),
        )
    } else {
        (MIN_EXP * (1.0 + 2.0 * MIN_EXP), 0.0)
    };
    let delt_vthw = sp.dvt0w * t2_w * v0;
    let ddelt_vthw_dvb = sp.dvt0w * dt2w_dvb * v0;

    // ngspice b3soiddld.c line 1274-1276: recomputes DeltVthtemp with Vbseff
    // (not Vbs0mos as used in the Vthfd computation above)
    let t1_kt_final = sp.kt1 + sp.kt1l / leff + sp.kt2 * vbseff;
    let delt_vth_temp = sp.k1 * (t0_nlx - 1.0) * sqrt_phi + t1_kt_final * temp_ratio_minus1;

    let tmp2 = model.tox * phi / (weff + sp.w0);
    let t3_eta = sp.eta0 + sp.etab * vbseff;
    let (t3_eta_eff, dt3_dvb) = if t3_eta < 1e-4 {
        let t9 = 1.0 / (3.0 - 2e4 * t3_eta);
        ((2e-4 - t3_eta) * t9, t9 * t9 * sp.etab)
    } else {
        (t3_eta, sp.etab)
    };
    let dibl_sft = t3_eta_eff * sp.theta0vb0 * vds_i;
    let ddibl_sft_dvd = sp.theta0vb0 * t3_eta_eff;
    let ddibl_sft_dvb = sp.theta0vb0 * vds_i * dt3_dvb;

    let vth =
        sign * sp.vth0 + sp.k1 * (sqrt_phis - sqrt_phi) - sp.k2 * vbseff - delt_vth - delt_vthw
            + (sp.k3 + sp.k3b * vbseff) * tmp2
            + delt_vth_temp
            - dibl_sft;

    let t6 = sp.k3b * tmp2 - sp.k2 + sp.kt2 * temp_ratio_minus1;
    let dvth_dvb = sp.k1 * dsqrt_phis_dvb - ddelt_vth_dvb - ddelt_vthw_dvb + t6 - ddibl_sft_dvb;
    let dvth_dvd = -ddibl_sft_dvd;

    // Calculate n (subthreshold swing factor)
    let t2_n = sp.nfactor * EPSSI / xdep;
    let dt2n_dvb = -t2_n / xdep * dxdep_dvb;
    let t3_n = sp.cdsc + sp.cdscb * vbseff + sp.cdscd * vds_i;
    let dt3n_dvb = sp.cdscb;
    let dt3n_dvd = sp.cdscd;
    let t4_n = (t2_n + t3_n * theta0 + sp.cit) / cox;
    let dt4n_dvb = (dt2n_dvb + theta0 * dt3n_dvb + dtheta0_dvb * t3_n) / cox;
    let dt4n_dvd = theta0 * dt3n_dvd / cox;
    let (n, dn_dvb, dn_dvd) = if t4_n >= -0.5 {
        (1.0 + t4_n, dt4n_dvb, dt4n_dvd)
    } else {
        let t0 = 1.0 / (3.0 + 8.0 * t4_n);
        let n = (1.0 + 3.0 * t4_n) * t0;
        let t0sq = t0 * t0;
        (n, t0sq * dt4n_dvb, t0sq * dt4n_dvd)
    };

    // Vgsteff (effective gate overdrive)
    let vgst = vgs_eff - vth;
    let dvgst_dvg = dvgs_eff_dvg;
    let dvgst_dvd = -dvth_dvd;
    let dvgst_dvb = -dvth_dvb;

    let t10 = 2.0 * n * vtm;
    let vgst_nvt = vgst / t10;
    let exp_arg = (2.0 * sp.voff - vgst) / t10;

    // dvbseff_dvb: full derivative chain vbs → vbsdio → vbsmos → vbseff
    let dvbseff_dvb = dvbseff_dvbsmos * dvbsmos_dvbsdio * dvbsdio_dvb;

    let (vgsteff, dvgsteff_dvg, dvgsteff_dvd, dvgsteff_dvb, dvgsteff_dve) =
        if vgst_nvt > EXPL_THRESHOLD {
            // Strong inversion: Vgsteff = Vgst, chain-rule through Vbseff
            (
                vgst,
                dvgs_eff_dvg - dvth_dvb * dvbseff_dvg,
                -dvth_dvd - dvth_dvb * dvbseff_dvd,
                -dvth_dvb * dvbseff_dvb,
                -dvth_dvb * dvbseff_dve,
            )
        } else if exp_arg > EXPL_THRESHOLD {
            // Weak inversion
            let t0 = (vgst - sp.voff) / (n * vtm);
            let exp_vgst = t0.exp();
            let vgsteff_val = vtm * sp.cdep0 / cox * exp_vgst;
            let t3 = vgsteff_val / (n * vtm);
            let t1 = -t3 * (dvth_dvb + t0 * vtm * dn_dvb);
            (
                vgsteff_val,
                t3 * dvgs_eff_dvg + t1 * dvbseff_dvg,
                -t3 * (dvth_dvd + t0 * vtm * dn_dvd) + t1 * dvbseff_dvd,
                t1 * dvbseff_dvb,
                t1 * dvbseff_dve,
            )
        } else {
            // Moderate inversion (smooth transition)
            let exp_vgst = vgst_nvt.exp();
            let t1 = t10 * (1.0 + exp_vgst).ln();
            let dt1_dvg = exp_vgst / (1.0 + exp_vgst);
            let dt1_dvb = -dt1_dvg * (dvth_dvb + vgst / n * dn_dvb) + t1 / n * dn_dvb;
            let dt1_dvd = -dt1_dvg * (dvth_dvd + vgst / n * dn_dvd) + t1 / n * dn_dvd;

            let dt2_dvg = -cox / (vtm * sp.cdep0) * exp_arg.exp();
            let t2_val = 1.0 - t10 * dt2_dvg;
            let dt2_dvd =
                -dt2_dvg * (dvth_dvd - 2.0 * vtm * exp_arg * dn_dvd) + (t2_val - 1.0) / n * dn_dvd;
            let dt2_dvb =
                -dt2_dvg * (dvth_dvb - 2.0 * vtm * exp_arg * dn_dvb) + (t2_val - 1.0) / n * dn_dvb;

            let vgsteff_val = t1 / t2_val;
            let t3 = t2_val * t2_val;
            let t4 = (t2_val * dt1_dvb - t1 * dt2_dvb) / t3;
            (
                vgsteff_val,
                (t2_val * dt1_dvg - t1 * dt2_dvg) / t3 * dvgs_eff_dvg + t4 * dvbseff_dvg,
                (t2_val * dt1_dvd - t1 * dt2_dvd) / t3 + t4 * dvbseff_dvd,
                t4 * dvbseff_dvb,
                t4 * dvbseff_dve,
            )
        };

    let vgst2vtm = vgsteff + 2.0 * vtm;

    // Effective channel geometry
    let mut weff_ch = weff - 2.0 * (sp.dwg * vgsteff + sp.dwb * (sqrt_phis - sqrt_phi));
    let mut dweff_dvg = -2.0 * sp.dwg;
    let mut dweff_dvb = -2.0 * sp.dwb * dsqrt_phis_dvb;
    if weff_ch < 2e-8 {
        let t0 = 1.0 / (6e-8 - 2.0 * weff_ch);
        weff_ch = 2e-8 * (4e-8 - weff_ch) * t0;
        let t0sq = t0 * t0 * 4e-16;
        dweff_dvg *= t0sq;
        dweff_dvb *= t0sq;
    }

    // Series resistance Rds
    let t0_rds = sp.prwg * vgsteff + sp.prwb * (sqrt_phis - sqrt_phi);
    let (rds, drds_dvg, drds_dvb) = if t0_rds >= -0.9 {
        (
            sp.rds0 * (1.0 + t0_rds),
            sp.rds0 * sp.prwg,
            sp.rds0 * sp.prwb * dsqrt_phis_dvb,
        )
    } else {
        let t1 = 1.0 / (17.0 + 20.0 * t0_rds);
        (
            sp.rds0 * (0.8 + t0_rds) * t1,
            sp.rds0 * sp.prwg * t1 * t1,
            sp.rds0 * sp.prwb * dsqrt_phis_dvb * t1 * t1,
        )
    };

    // Abulk calculation (ngspice DD formula: keta applied multiplicatively, +1 at end)
    let (abulk0, dabulk0_dvb, abulk, dabulk_dvg, dabulk_dvb) = {
        let (mut abulk0, mut dabulk0_dvb, mut abulk, mut dabulk_dvg, mut dabulk_dvb) =
            if sp.a0 == 0.0 {
                (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64)
            } else {
                let t1 = 0.5 * sp.k1eff / phi.sqrt();

                let t9 = (model.xj * xdep).sqrt();
                let tmp1 = leff + 2.0 * t9;
                let t5 = leff / tmp1;
                let tmp2_a = sp.a0 * t5;
                let tmp3_a = weff + sp.b1;
                let tmp4_a = sp.b0 / tmp3_a;
                let t2 = tmp2_a + tmp4_a;
                let dt2_dvb = -t9 * tmp2_a / tmp1 / xdep * dxdep_dvb;
                let _t6 = t5 * t5;
                let t7 = t5 * t5 * t5;

                let abulk0 = t1 * t2; // NO +1 yet
                let dabulk0_dvb = t1 * dt2_dvb;

                let t8 = sp.ags * sp.a0 * t7;
                let dabulk_dvg = -t1 * t8;
                let abulk = abulk0 + dabulk_dvg * vgsteff; // NO +1 yet
                let dabulk_dvb = dabulk0_dvb - t8 * vgsteff * 3.0 * t1 * dt2_dvb / tmp2_a;

                (abulk0, dabulk0_dvb, abulk, dabulk_dvg, dabulk_dvb)
            };

        // Clamp before keta
        if abulk0 < 0.01 {
            let t9 = 1.0 / (3.0 - 200.0 * abulk0);
            abulk0 = (0.02 - abulk0) * t9;
            dabulk0_dvb *= t9 * t9;
        }
        if abulk < 0.01 {
            let t9 = 1.0 / (3.0 - 200.0 * abulk);
            abulk = (0.02 - abulk) * t9;
            dabulk_dvb *= t9 * t9;
        }

        // Keta multiplicative correction (applied AFTER clamp, BEFORE +1)
        let t2_k = sp.keta * vbseff;
        let (t0_k, dt0k_dvb) = if t2_k >= -0.9 {
            let t0 = 1.0 / (1.0 + t2_k);
            (t0, -sp.keta * t0 * t0)
        } else {
            let t1 = 1.0 / (0.8 + t2_k);
            ((17.0 + 20.0 * t2_k) * t1, -sp.keta * t1 * t1)
        };
        dabulk_dvg *= t0_k;
        dabulk_dvb = dabulk_dvb * t0_k + abulk * dt0k_dvb;
        dabulk0_dvb = dabulk0_dvb * t0_k + abulk0 * dt0k_dvb;
        abulk *= t0_k;
        abulk0 *= t0_k;

        // Add 1 at the end
        abulk += 1.0;
        abulk0 += 1.0;

        (abulk0, dabulk0_dvb, abulk, dabulk_dvg, dabulk_dvb)
    };

    // Xcsat / Abeff (DD-specific cross-section saturation blending)
    const DELT_XCSAT: f64 = 0.2;
    let vcs = vbsdio - vbs0eff_dd;
    let (abeff, dabeff_dvg, dabeff_dvb, dabeff_dvc) = {
        let t0 = model.abp * vgst2vtm;
        if t0.abs() < 1e-20 {
            // Avoid division by zero; Xcsat=0, Abeff=adice
            (model.adice, 0.0, 0.0, 0.0)
        } else {
            let t1 = 1.0 - vcs / t0 - DELT_XCSAT;
            let t2 = (t1 * t1 + DELT_XCSAT * DELT_XCSAT).sqrt();
            let t3 = 1.0 - 0.5 * (t1 + t2);
            let t5 = -0.5 * (1.0 + t1 / t2);
            let dt1_dvg = vcs / vgst2vtm / t0;
            let dt3_dvg = t5 * dt1_dvg;
            let dt1_dvc = -1.0 / t0; // C line 1534
            let dt3_dvc = t5 * dt1_dvc; // C line 1535

            let xcsat = model.mxc * t3 * t3 + (1.0 - model.mxc) * t3;
            let t4 = 2.0 * model.mxc * t3 + (1.0 - model.mxc);
            let dxcsat_dvg = t4 * dt3_dvg;
            let dxcsat_dvc = t4 * dt3_dvc; // C line 1540

            let abeff = xcsat * abulk + (1.0 - xcsat) * model.adice;
            let dabeff_dvg = xcsat * dabulk_dvg + abulk * dxcsat_dvg - model.adice * dxcsat_dvg;
            let dabeff_dvb = xcsat * dabulk_dvb;
            let dabeff_dvc = (abulk - model.adice) * dxcsat_dvc; // C line 1546
            (abeff, dabeff_dvg, dabeff_dvb, dabeff_dvc)
        }
    };

    // Mobility
    let t5 = if model.mob_mod == 1 {
        let t0 = vgsteff + vth + vth;
        let t2 = sp.ua + sp.uc * vbseff;
        let t3 = t0 / model.tox;
        t3 * (t2 + sp.ub * t3)
    } else if model.mob_mod == 2 {
        vgsteff / model.tox * (sp.ua + sp.uc * vbseff + sp.ub * vgsteff / model.tox)
    } else {
        let t0 = vgsteff + vth + vth;
        let t2 = 1.0 + sp.uc * vbseff;
        let t3 = t0 / model.tox;
        t3 * (sp.ua + sp.ub * t3) * t2
    };

    let denomi = if t5 >= -0.8 {
        1.0 + t5
    } else {
        let t9 = 1.0 / (7.0 + 10.0 * t5);
        (0.6 + t5) * t9
    };

    let ueff = sp.u0temp / denomi;
    let t9 = -ueff / denomi;
    // ngspice b3soiddld.c: full dDenomi derivatives matching FD/PD patterns
    let (dueff_dvg, dueff_dvd, dueff_dvb) = if model.mob_mod == 1 {
        let t0 = vgsteff + vth + vth;
        let t2 = sp.ua + sp.uc * vbseff;
        let t3 = t0 / model.tox;
        let ddenomi_dvg = (t2 + 2.0 * sp.ub * t3) / model.tox;
        let ddenomi_dvd = ddenomi_dvg * 2.0 * dvth_dvd;
        let ddenomi_dvb = ddenomi_dvg * 2.0 * dvth_dvb + sp.uc * t3;
        (t9 * ddenomi_dvg, t9 * ddenomi_dvd, t9 * ddenomi_dvb)
    } else if model.mob_mod == 2 {
        let ddenomi_dvg = (sp.ua + sp.uc * vbseff + 2.0 * sp.ub * vgsteff / model.tox) / model.tox;
        let ddenomi_dvb = vgsteff * sp.uc / model.tox;
        (t9 * ddenomi_dvg, 0.0, t9 * ddenomi_dvb)
    } else {
        // mob_mod 0/3 (else)
        let t0 = vgsteff + vth + vth;
        let t2 = 1.0 + sp.uc * vbseff;
        let t3 = t0 / model.tox;
        let t4 = t3 * (sp.ua + sp.ub * t3);
        let ddenomi_dvg = (sp.ua + 2.0 * sp.ub * t3) * t2 / model.tox;
        let ddenomi_dvd = ddenomi_dvg * 2.0 * dvth_dvd;
        let ddenomi_dvb = ddenomi_dvg * 2.0 * dvth_dvb + sp.uc * t4;
        (t9 * ddenomi_dvg, t9 * ddenomi_dvd, t9 * ddenomi_dvb)
    };

    // Saturation voltage Vdsat
    let wvcox = weff_ch * sp.vsattemp * cox;
    let wvcox_rds = wvcox * rds;
    let esat = 2.0 * sp.vsattemp / ueff;
    let esat_l = esat * leff;
    let t0_esat = -esat_l / ueff;
    let desat_l_dvg = t0_esat * dueff_dvg;
    let desat_l_dvd = t0_esat * dueff_dvd;
    let desat_l_dvb = t0_esat * dueff_dvb;

    let a1_val = sp.a1;
    let (lambda, dlambda_dvg) = if a1_val == 0.0 {
        (sp.a2, 0.0)
    } else if a1_val > 0.0 {
        let t0 = 1.0 - sp.a2;
        let t1 = t0 - a1_val * vgsteff - 0.0001;
        let t2 = (t1 * t1 + 0.0004 * t0).sqrt();
        (sp.a2 + t0 - 0.5 * (t1 + t2), 0.5 * a1_val * (1.0 + t1 / t2))
    } else {
        let t1 = sp.a2 + a1_val * vgsteff - 0.0001;
        let t2 = (t1 * t1 + 0.0004 * sp.a2).sqrt();
        (0.5 * (t1 + t2), 0.5 * a1_val * (1.0 + t1 / t2))
    };

    let vdsat;
    let dvdsat_dvg;
    let dvdsat_dvd;
    let dvdsat_dvb;
    let dvdsat_dvc;
    let tmp1_lambda; // dLambda_dVg / (Lambda * Lambda), needed for Vasat derivatives

    let (tmp2_rds, tmp3_rds) = if rds > 0.0 {
        (
            drds_dvg / rds + dweff_dvg / weff_ch,
            drds_dvb / rds + dweff_dvb / weff_ch,
        )
    } else {
        (dweff_dvg / weff_ch, dweff_dvb / weff_ch)
    };

    if rds == 0.0 && lambda == 1.0 {
        tmp1_lambda = 0.0;
        let t0 = 1.0 / (abeff * esat_l + vgst2vtm);
        let t1 = t0 * t0;
        let t2 = vgst2vtm * t0;
        let t3 = esat_l * vgst2vtm;
        vdsat = t3 * t0;
        let dt0_dvg = -(abeff * desat_l_dvg + esat_l * dabeff_dvg + 1.0) * t1;
        let dt0_dvd = -(abeff * desat_l_dvd) * t1;
        let dt0_dvb = -(abeff * desat_l_dvb + esat_l * dabeff_dvb) * t1;
        let dt0_dvc = -(esat_l * dabeff_dvc) * t1; // C line 1680
        dvdsat_dvg = t3 * dt0_dvg + t2 * desat_l_dvg + esat_l * t0;
        dvdsat_dvd = t3 * dt0_dvd + t2 * desat_l_dvd;
        dvdsat_dvb = t3 * dt0_dvb + t2 * desat_l_dvb;
        dvdsat_dvc = t3 * dt0_dvc; // C line 1688
    } else {
        tmp1_lambda = dlambda_dvg / (lambda * lambda);
        let t9 = abeff * wvcox_rds;
        let t8 = abeff * t9;
        let t7 = vgst2vtm * t9;
        let t6 = vgst2vtm * wvcox_rds;
        let t0 = 2.0 * abeff * (t9 - 1.0 + 1.0 / lambda);
        let dt0_dvg = 2.0
            * (t8 * tmp2_rds - abeff * dlambda_dvg / (lambda * lambda)
                + (2.0 * t9 + 1.0 / lambda - 1.0) * dabeff_dvg);
        let dt0_dvb =
            2.0 * (t8 * (2.0 / abeff * dabeff_dvb + tmp3_rds) + (1.0 / lambda - 1.0) * dabeff_dvb);
        let dt0_dvd = 0.0;
        let dt0_dvc = 4.0 * t9 * dabeff_dvc; // C line 1708

        let t1 = vgst2vtm * (2.0 / lambda - 1.0) + abeff * esat_l + 3.0 * t7;
        let dt1_dvg = (2.0 / lambda - 1.0) - 2.0 * vgst2vtm * dlambda_dvg / (lambda * lambda)
            + abeff * desat_l_dvg
            + esat_l * dabeff_dvg
            + 3.0 * (t9 + t7 * tmp2_rds + t6 * dabeff_dvg);
        let dt1_dvb =
            abeff * desat_l_dvb + esat_l * dabeff_dvb + 3.0 * (t6 * dabeff_dvb + t7 * tmp3_rds);
        let dt1_dvd = abeff * desat_l_dvd;
        let dt1_dvc = esat_l * dabeff_dvc + 3.0 * t6 * dabeff_dvc; // C line 1724

        let t2 = vgst2vtm * (esat_l + 2.0 * t6);
        let dt2_dvg = esat_l + vgst2vtm * desat_l_dvg + t6 * (4.0 + 2.0 * vgst2vtm * tmp2_rds);
        let dt2_dvb = vgst2vtm * (desat_l_dvb + 2.0 * t6 * tmp3_rds);
        let dt2_dvd = vgst2vtm * desat_l_dvd;
        // T2 has no dVc dependency (no dT2_dVc in C code)

        let t3 = (t1 * t1 - 2.0 * t0 * t2).sqrt();
        vdsat = (t1 - t3) / t0;
        dvdsat_dvg =
            (dt1_dvg - (t1 * dt1_dvg - dt0_dvg * t2 - t0 * dt2_dvg) / t3 - vdsat * dt0_dvg) / t0;
        dvdsat_dvb =
            (dt1_dvb - (t1 * dt1_dvb - dt0_dvb * t2 - t0 * dt2_dvb) / t3 - vdsat * dt0_dvb) / t0;
        dvdsat_dvd = (dt1_dvd - (t1 * dt1_dvd - t0 * dt2_dvd) / t3) / t0;
        dvdsat_dvc = (dt1_dvc - (t1 * dt1_dvc - dt0_dvc * t2) / t3 - vdsat * dt0_dvc) / t0;
        // C line 1752-1753
    }

    // Vdseff
    let t1 = vdsat - vds_i - sp.delta;
    let t2 = (t1 * t1 + 4.0 * sp.delta * vdsat).sqrt();
    let t0 = t1 / t2;
    let t3 = 2.0 * sp.delta / t2;
    let vdseff = vdsat - 0.5 * (t1 + t2);
    let dvdseff_dvg = dvdsat_dvg - 0.5 * (dvdsat_dvg + t0 * dvdsat_dvg + t3 * dvdsat_dvg);
    let dvdseff_dvd =
        dvdsat_dvd - 0.5 * (dvdsat_dvd - 1.0 + t0 * (dvdsat_dvd - 1.0) + t3 * dvdsat_dvd);
    let dvdseff_dvb = dvdsat_dvb - 0.5 * (dvdsat_dvb + t0 * dvdsat_dvb + t3 * dvdsat_dvb);
    // C lines 1817-1835: dT1_dVc = dVdsat_dVc, dT2_dVc = T0*dT1_dVc + T3*dVdsat_dVc
    let dvdseff_dvc = dvdsat_dvc - 0.5 * (dvdsat_dvc + t0 * dvdsat_dvc + t3 * dvdsat_dvc);

    // Clamp Vdseff to Vds but keep smooth-formula derivatives (matches
    // ngspice b3soiddld.c:1840-1843 which only clamps the value, not the
    // derivatives).  Preserving derivatives avoids a Jacobian discontinuity
    // at the clamping boundary, improving NR convergence in the linear region.
    let vdseff = if vdseff > vds_i { vds_i } else { vdseff };
    let diff_vds = vds_i - vdseff;

    // Vasat (saturation Early voltage)
    let tmp4 = 1.0 - 0.5 * abeff * vdsat / vgst2vtm;
    let t9_va = wvcox_rds * vgsteff;
    let t8_va = t9_va / vgst2vtm;
    let t0_va = esat_l + vdsat + 2.0 * t9_va * tmp4;

    let t7_va = 2.0 * wvcox_rds * tmp4;
    let dt0_va_dvg = desat_l_dvg + dvdsat_dvg + t7_va * (1.0 + tmp2_rds * vgsteff)
        - t8_va * (abeff * dvdsat_dvg - abeff * vdsat / vgst2vtm + vdsat * dabeff_dvg);
    let dt0_va_dvb = desat_l_dvb + dvdsat_dvb + t7_va * tmp3_rds * vgsteff
        - t8_va * (dabeff_dvb * vdsat + abeff * dvdsat_dvb);
    let dt0_va_dvd = desat_l_dvd + dvdsat_dvd - t8_va * abeff * dvdsat_dvd;
    let dt0_va_dvc = dvdsat_dvc - t8_va * (abeff * dvdsat_dvc + vdsat * dabeff_dvc); // C line 1882

    let t9_ab = wvcox_rds * abeff;
    let t1_ab = 2.0 / lambda - 1.0 + t9_ab;
    let dt1_ab_dvg = -2.0 * tmp1_lambda + wvcox_rds * (abeff * tmp2_rds + dabeff_dvg);
    let dt1_ab_dvb = dabeff_dvb * wvcox_rds + t9_ab * tmp3_rds;
    let dt1_ab_dvc = dabeff_dvc * wvcox_rds; // C line 1897

    let vasat = t0_va / t1_ab;
    let dvasat_dvg = (dt0_va_dvg - vasat * dt1_ab_dvg) / t1_ab;
    let dvasat_dvb = (dt0_va_dvb - vasat * dt1_ab_dvb) / t1_ab;
    let dvasat_dvd = dt0_va_dvd / t1_ab;
    let dvasat_dvc = (dt0_va_dvc - vasat * dt1_ab_dvc) / t1_ab; // C line 1907

    // VACLM (channel length modulation Early voltage)
    let (vaclm, dvaclm_dvg, dvaclm_dvd, dvaclm_dvb, dvaclm_dvc) = if sp.pclm > 0.0
        && diff_vds > 1e-10
    {
        let t0 = 1.0 / (sp.pclm * abeff * sp.litl);
        let dt0_dvb = -t0 / abeff * dabeff_dvb;
        let dt0_dvg = -t0 / abeff * dabeff_dvg;
        let dt0_dvc = -t0 / abeff * dabeff_dvc; // C line 1916

        let t2 = vgsteff / esat_l;
        let t1 = leff * (abeff + t2);
        let dt1_dvg = leff * ((1.0 - t2 * desat_l_dvg) / esat_l + dabeff_dvg);
        let dt1_dvb = leff * (dabeff_dvb - t2 * desat_l_dvb / esat_l);
        let dt1_dvd = -t2 * desat_l_dvd / esat;
        let dt1_dvc = leff * dabeff_dvc; // C line 1923

        let t9_cl = t0 * t1;
        let vaclm = t9_cl * diff_vds;
        let dvaclm_dvg = t0 * dt1_dvg * diff_vds - t9_cl * dvdseff_dvg + t1 * diff_vds * dt0_dvg;
        let dvaclm_dvb = (dt0_dvb * t1 + t0 * dt1_dvb) * diff_vds - t9_cl * dvdseff_dvb;
        let dvaclm_dvd = t0 * dt1_dvd * diff_vds + t9_cl * (1.0 - dvdseff_dvd);
        let dvaclm_dvc = (t1 * dt0_dvc + t0 * dt1_dvc) * diff_vds - t9_cl * dvdseff_dvc; // C line 1934-1935

        (vaclm, dvaclm_dvg, dvaclm_dvd, dvaclm_dvb, dvaclm_dvc)
    } else {
        (MAX_EXP, 0.0, 0.0, 0.0, 0.0)
    };

    // VADIBL (DIBL Early voltage)
    let (vadibl, dvadibl_dvg, dvadibl_dvd, dvadibl_dvb, dvadibl_dvc) = if sp.theta_rout > 0.0 {
        let t8 = abeff * vdsat;
        let t0 = vgst2vtm * t8;
        let t1 = vgst2vtm + t8;
        let dt0_dvg = vgst2vtm * abeff * dvdsat_dvg + t8 + vgst2vtm * vdsat * dabeff_dvg;
        let dt1_dvg = 1.0 + abeff * dvdsat_dvg + vdsat * dabeff_dvg;
        let dt1_dvb = dabeff_dvb * vdsat + abeff * dvdsat_dvb;
        let dt0_dvb = vgst2vtm * dt1_dvb;
        let dt1_dvd = abeff * dvdsat_dvd;
        let dt0_dvd = vgst2vtm * dt1_dvd;
        let dt1_dvc = abeff * dvdsat_dvc + vdsat * dabeff_dvc; // C line 1959
        let dt0_dvc = vgst2vtm * dt1_dvc; // C line 1960

        let t9_dibl = t1 * t1;
        let t2_dibl = sp.theta_rout;
        let vadibl = (vgst2vtm - t0 / t1) / t2_dibl;
        let mut dvadibl_dvg = (1.0 - dt0_dvg / t1 + t0 * dt1_dvg / t9_dibl) / t2_dibl;
        let mut dvadibl_dvb = (-dt0_dvb / t1 + t0 * dt1_dvb / t9_dibl) / t2_dibl;
        let mut dvadibl_dvd = (-dt0_dvd / t1 + t0 * dt1_dvd / t9_dibl) / t2_dibl;
        let mut dvadibl_dvc = (-dt0_dvc / t1 + t0 * dt1_dvc / t9_dibl) / t2_dibl; // C line 1974

        let t7 = sp.pdiblcb * vbseff;
        let (vadibl, dvadibl_dvg, dvadibl_dvd, dvadibl_dvb, dvadibl_dvc) = if t7 >= -0.9 {
            let t3 = 1.0 / (1.0 + t7);
            let vadibl = vadibl * t3;
            dvadibl_dvg *= t3;
            dvadibl_dvb = (dvadibl_dvb - vadibl * sp.pdiblcb) * t3;
            dvadibl_dvd *= t3;
            dvadibl_dvc *= t3; // C line 1987
            (vadibl, dvadibl_dvg, dvadibl_dvd, dvadibl_dvb, dvadibl_dvc)
        } else {
            let t4 = 1.0 / (0.8 + t7);
            let t3 = (17.0 + 20.0 * t7) * t4;
            dvadibl_dvg *= t3;
            dvadibl_dvb = dvadibl_dvb * t3 - vadibl * sp.pdiblcb * t4 * t4;
            dvadibl_dvd *= t3;
            dvadibl_dvc *= t3; // C line 1999
            let vadibl = vadibl * t3;
            (vadibl, dvadibl_dvg, dvadibl_dvd, dvadibl_dvb, dvadibl_dvc)
        };

        (vadibl, dvadibl_dvg, dvadibl_dvd, dvadibl_dvb, dvadibl_dvc)
    } else {
        (MAX_EXP, 0.0, 0.0, 0.0, 0.0)
    };

    // PVAG factor T0 and its derivatives
    let t8_pvag = sp.pvag / esat_l;
    let t9_pvag = t8_pvag * vgsteff;
    let (t0_pvag, dt0_pvag_dvg, dt0_pvag_dvb, dt0_pvag_dvd) = if t9_pvag > -0.9 {
        let t0 = 1.0 + t9_pvag;
        let dt0_dvg = t8_pvag * (1.0 - vgsteff * desat_l_dvg / esat_l);
        let dt0_dvb = -t9_pvag * desat_l_dvb / esat_l;
        let dt0_dvd = -t9_pvag * desat_l_dvd / esat_l;
        (t0, dt0_dvg, dt0_dvb, dt0_dvd)
    } else {
        let t1 = 1.0 / (17.0 + 20.0 * t9_pvag);
        let t0 = (0.8 + t9_pvag) * t1;
        let t1sq = t1 * t1;
        let dt0_dvg = t8_pvag * (1.0 - vgsteff * desat_l_dvg / esat_l) * t1sq;
        let t9_scaled = t9_pvag * t1sq / esat_l;
        let dt0_dvb = -t9_scaled * desat_l_dvb;
        let dt0_dvd = -t9_scaled * desat_l_dvd;
        (t0, dt0_dvg, dt0_dvb, dt0_dvd)
    };

    // Combine VACLM and VADIBL into T1_va
    let tmp3_va = vaclm + vadibl;
    let t1_va = vaclm * vadibl / tmp3_va;
    let tmp3_va_sq = tmp3_va * tmp3_va;
    let tmp1_va = vaclm * vaclm;
    let tmp2_va = vadibl * vadibl;
    let dt1_va_dvg = (tmp1_va * dvadibl_dvg + tmp2_va * dvaclm_dvg) / tmp3_va_sq;
    let dt1_va_dvd = (tmp1_va * dvadibl_dvd + tmp2_va * dvaclm_dvd) / tmp3_va_sq;
    let dt1_va_dvb = (tmp1_va * dvadibl_dvb + tmp2_va * dvaclm_dvb) / tmp3_va_sq;
    let dt1_va_dvc = (tmp1_va * dvadibl_dvc + tmp2_va * dvaclm_dvc) / tmp3_va_sq; // C line 2049

    // Va = Vasat + T0_pvag * T1_va
    let va = vasat + t0_pvag * t1_va;
    let dva_dvg = dvasat_dvg + t1_va * dt0_pvag_dvg + t0_pvag * dt1_va_dvg;
    let dva_dvd = dvasat_dvd + t1_va * dt0_pvag_dvd + t0_pvag * dt1_va_dvd;
    let dva_dvb = dvasat_dvb + t1_va * dt0_pvag_dvb + t0_pvag * dt1_va_dvb;
    // C line 2058: T0_pvag has no dVc component
    let dva_dvc = dvasat_dvc + t0_pvag * dt1_va_dvc;

    // Ids calculation
    let cox_wov_l = cox * weff_ch / leff;
    let beta = ueff * cox_wov_l;

    let t0_ids = 1.0 - 0.5 * abeff * vdseff / vgst2vtm;
    let fgche1 = vgsteff * t0_ids;
    let t9_fgche = vdseff / esat_l;
    let fgche2 = 1.0 + t9_fgche;
    let gche = beta * fgche1 / fgche2;
    let t0_gche = 1.0 + gche * rds;
    let t9_gche = vdseff / t0_gche;
    let idl = gche * t9_gche;

    let t9_ids = diff_vds / va;
    let t0_ids2 = 1.0 + t9_ids;
    let ids = idl * t0_ids2;

    // Derivatives of beta
    let dbeta_dvg = cox_wov_l * dueff_dvg + beta * dweff_dvg / weff_ch;
    let dbeta_dvd = cox_wov_l * dueff_dvd;
    let dbeta_dvb = cox_wov_l * dueff_dvb + beta * dweff_dvb / weff_ch;

    // Derivatives of T0_ids = 1 - 0.5 * Abeff * Vdseff / Vgst2Vtm
    let dt0_ids_dvg =
        -0.5 * (abeff * dvdseff_dvg - abeff * vdseff / vgst2vtm + vdseff * dabeff_dvg) / vgst2vtm;
    let dt0_ids_dvd = -0.5 * abeff * dvdseff_dvd / vgst2vtm;
    let dt0_ids_dvb = -0.5 * (abeff * dvdseff_dvb + dabeff_dvb * vdseff) / vgst2vtm;
    let dt0_ids_dvc = -0.5 * (abeff * dvdseff_dvc + dabeff_dvc * vdseff) / vgst2vtm; // C line 2078

    // Derivatives of fgche1 = Vgsteff * T0_ids
    let dfgche1_dvg = vgsteff * dt0_ids_dvg + t0_ids;
    let dfgche1_dvd = vgsteff * dt0_ids_dvd;
    let dfgche1_dvb = vgsteff * dt0_ids_dvb;
    let dfgche1_dvc = vgsteff * dt0_ids_dvc; // C line 2090

    // Derivatives of fgche2 = 1 + Vdseff / EsatL
    let dfgche2_dvg = (dvdseff_dvg - t9_fgche * desat_l_dvg) / esat_l;
    let dfgche2_dvd = (dvdseff_dvd - t9_fgche * desat_l_dvd) / esat_l;
    let dfgche2_dvb = (dvdseff_dvb - t9_fgche * desat_l_dvb) / esat_l;
    let dfgche2_dvc = dvdseff_dvc / esat_l; // C line 2099

    // Derivatives of gche = beta * fgche1 / fgche2
    let dgche_dvg = (beta * dfgche1_dvg + fgche1 * dbeta_dvg - gche * dfgche2_dvg) / fgche2;
    let dgche_dvd = (beta * dfgche1_dvd + fgche1 * dbeta_dvd - gche * dfgche2_dvd) / fgche2;
    let dgche_dvb = (beta * dfgche1_dvb + fgche1 * dbeta_dvb - gche * dfgche2_dvb) / fgche2;
    let dgche_dvc = (beta * dfgche1_dvc - gche * dfgche2_dvc) / fgche2; // C line 2110

    // Derivatives of Idl (ngspice lines 2123-2128)
    let didl_dvg =
        (gche * dvdseff_dvg + t9_gche * dgche_dvg) / t0_gche - idl * gche / t0_gche * drds_dvg;
    let didl_dvd = (gche * dvdseff_dvd + t9_gche * dgche_dvd) / t0_gche;
    let didl_dvb = (gche * dvdseff_dvb + t9_gche * dgche_dvb - idl * drds_dvb * gche) / t0_gche;
    let didl_dvc = (gche * dvdseff_dvc + t9_gche * dgche_dvc) / t0_gche; // C line 2128

    // Gm0, Gds0, Gmbs0, Gmc (ngspice lines 2138-2142)
    let gm0 = t0_ids2 * didl_dvg - idl * (dvdseff_dvg + t9_ids * dva_dvg) / va;
    let gds0 = t0_ids2 * didl_dvd + idl * (1.0 - dvdseff_dvd - t9_ids * dva_dvd) / va;
    let gmbs0 = t0_ids2 * didl_dvb - idl * (dvdseff_dvb + t9_ids * dva_dvb) / va;
    let gmc = t0_ids2 * didl_dvc - idl * (dvdseff_dvc + t9_ids * dva_dvc) / va; // C line 2142

    // Compute dVcs/dV* derivatives (Vcs = Vbsdio - Vbs0eff, C lines 1154-1156)
    let dvcs_dvg = dvbsdio_dvg - dvbs0eff_dvg;
    let dvcs_dvd = dvbsdio_dvd - dvbs0eff_dvd;
    let dvcs_dvb = dvbsdio_dvb; // Vbs0eff has no direct dVb in DD

    // Final Gm, Gds, Gmbs, Gme (ngspice lines 2148-2151)
    // Includes Gmb0 cross-coupling through dVbseff_dVg/dVd/dVe chain
    // and Gmc cross-coupling through dVcs_dV* chain
    let gm = gm0 * dvgsteff_dvg + gmbs0 * dvbseff_dvg + gmc * dvcs_dvg;
    let gds = gm0 * dvgsteff_dvd + gmbs0 * dvbseff_dvd + gmc * dvcs_dvd + gds0;
    let gmbs = gm0 * dvgsteff_dvb + gmbs0 * dvbseff_dvb + gmc * dvcs_dvb;
    let gme = gm0 * dvgsteff_dve + gmbs0 * dvbseff_dve + gmc * dvcs_dve;

    // GIDL current (drain side)
    let (igidl, ggidl_d, ggidl_g) = {
        let t0 = 3.0 * model.tox;
        let t1 = (vds_i - vgs_eff - sp.ngidl) / t0;
        if sp.agidl <= 0.0 || sp.bgidl <= 0.0 || t1 <= 0.0 {
            (0.0, 0.0, 0.0)
        } else {
            let dt1_dvd = 1.0 / t0;
            let dt1_dvg = -dt1_dvd * dvgs_eff_dvg;
            let t2 = sp.bgidl / t1;
            if t2 < EXPL_THRESHOLD {
                let igidl = sp.wdiod * sp.agidl * t1 * (-t2).exp();
                let t3 = igidl / t1 * (t2 + 1.0);
                (igidl, t3 * dt1_dvd, t3 * dt1_dvg)
            } else {
                let t3 = sp.wdiod * sp.agidl * MIN_EXPL;
                (t3 * t1, t3 * dt1_dvd, t3 * dt1_dvg)
            }
        }
    };

    // GIDL source side
    let (isgidl, gsgidl_g) = {
        let t0 = 3.0 * model.tox;
        let t1 = (-vgs_eff - sp.ngidl) / t0;
        if sp.agidl <= 0.0 || sp.bgidl <= 0.0 || t1 <= 0.0 {
            (0.0, 0.0)
        } else {
            let dt1_dvg = -dvgs_eff_dvg / t0;
            let t2 = sp.bgidl / t1;
            if t2 < EXPL_THRESHOLD {
                let isgidl = sp.wdios * sp.agidl * t1 * (-t2).exp();
                let t3 = isgidl / t1 * (t2 + 1.0);
                (isgidl, t3 * dt1_dvg)
            } else {
                let t3 = sp.wdios * sp.agidl * MIN_EXPL;
                (t3 * t1, t3 * dt1_dvg)
            }
        }
    };

    // Junction currents (4-component SOI model)
    // ngspice b3soiddld.c lines 2261-2440
    let nvtm1 = vtm * sp.ndiode;
    let vbd = vbs_i - vds_i;
    let wtsi = weff * model.tsi;

    // Compute bare exponentials upfront (shared by Ibs1/Ibs2/Ibs3)
    // ngspice b3soiddld.c lines 2266-2290
    // NOTE: ngspice DD uses a hardcoded threshold of 30 for junction exponentials
    // (b3soiddld.c line 2267: "if (T0 < 30)"), NOT the general EXP_THRESHOLD (34)
    // or EXPL_THRESHOLD (100). The PD model uses DEXP with threshold 100, but DD
    // has its own inline check. We match ngspice's DD behavior exactly.
    const DD_JCT_EXP_THRESHOLD: f64 = 30.0;
    const DD_JCT_EXP30: f64 = 1.0686474581524462e13; // exp(30)
    let t0_bs = vbs_i / nvtm1;
    let (exp_vbs1, dexp_vbs1_dvb) = if t0_bs < DD_JCT_EXP_THRESHOLD {
        let e = t0_bs.exp();
        (e, e / nvtm1)
    } else {
        // Linear extrapolation matching ngspice b3soiddld.c lines 2274-2276:
        //   dExpVbs1_dVb = exp(30) / NVtm1
        //   ExpVbs1 = dExpVbs1_dVb * Vbs - 29 * exp(30)
        let deriv = DD_JCT_EXP30 / nvtm1;
        (deriv * vbs_i - 29.0 * DD_JCT_EXP30, deriv)
    };

    let t0_bd = vbd / nvtm1;
    let (exp_vbd1, dexp_vbd1_dvb) = if t0_bd < DD_JCT_EXP_THRESHOLD {
        let e = t0_bd.exp();
        (e, e / nvtm1)
    } else {
        let deriv = DD_JCT_EXP30 / nvtm1;
        (deriv * vbd - 29.0 * DD_JCT_EXP30, deriv)
    };

    // Ibs1/Ibd1: Diffusion (uses exp-1)
    // ngspice b3soiddld.c lines 2333-2346
    let (ibs1, dibs1_dvb, ibd1, dibd1_dvb) = if sp.jdif == 0.0 {
        (0.0, 0.0, 0.0, 0.0)
    } else {
        let t5 = wtsi * sp.jdif;
        let ibs1 = t5 * (exp_vbs1 - 1.0);
        let dibs1_dvb = t5 * dexp_vbs1_dvb;
        let ibd1 = t5 * (exp_vbd1 - 1.0);
        let dibd1_dvb = t5 * dexp_vbd1_dvb;
        (ibs1, dibs1_dvb, ibd1, dibd1_dvb)
    };

    // Ibs2/Ibd2: Recombination (uses sqrt of ExpVbs1, i.e. exp(V/(2*NVtm1)))
    // ngspice b3soiddld.c lines 2354-2390: ExpVbs2 = sqrt(ExpVbs1)
    // DD model does NOT have nrecf0 — uses sqrt(exp) for ideality factor 2*ndiode
    let (ibs2, dibs2_dvb, ibd2, dibd2_dvb) = if sp.jrec == 0.0 {
        (0.0, 0.0, 0.0, 0.0)
    } else {
        let t8 = wtsi * sp.jrec;

        let exp_vbs2 = exp_vbs1.sqrt();
        let dexp_vbs2_dvb = if exp_vbs2 > 1e-20 {
            0.5 / exp_vbs2 * dexp_vbs1_dvb
        } else {
            0.0
        };
        let ibs2 = t8 * (exp_vbs2 - 1.0);
        let dibs2_dvb = t8 * dexp_vbs2_dvb;

        let exp_vbd2 = exp_vbd1.sqrt();
        let dexp_vbd2_dvb = if exp_vbd2 > 1e-20 {
            0.5 / exp_vbd2 * dexp_vbd1_dvb
        } else {
            0.0
        };
        let ibd2 = t8 * (exp_vbd2 - 1.0);
        let dibd2_dvb = t8 * dexp_vbd2_dvb;
        (ibs2, dibs2_dvb, ibd2, dibd2_dvb)
    };

    // Ibs3/Ibd3/Ibjt: BJT currents (uses BARE exp, not exp-1!)
    // ngspice b3soiddld.c lines 2392-2440
    // BjtA = 1 - 0.5 * T1² where T1 = (Leff - kbjt1*Vds) / edl
    // Ibs3 = (1-BjtA) * WTsi * jbjt * ExpVbs1  (bare exp!)
    // Ic = Ibjt - Ibs3 + Ibd3 (collector current added to drain)
    #[allow(clippy::type_complexity)]
    let (ibs3, dibs3_dvb, dibs3_dvd, ibd3, dibd3_dvb, dibd3_dvd, ic, gcd, gcb) =
        if sp.jbjt == 0.0 || vds_i == 0.0 {
            (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0)
        } else {
            let t5 = wtsi * sp.jbjt;

            // BjtA: Vds-dependent base transport factor
            // ngspice b3soiddld.c lines 2401-2413
            let t0_bjt = sp.leff - model.kbjt1 * vds_i;
            let mut t1_bjt = if model.edl > 0.0 {
                t0_bjt / model.edl
            } else {
                1.0
            };
            let mut dt1_dvd = if model.edl > 0.0 {
                -model.kbjt1 / model.edl
            } else {
                0.0
            };

            // Clamping: ngspice b3soiddld.c lines 2404-2411
            if t1_bjt < 1e-3 {
                let t2 = 1.0 / (3.0 - 2e3 * t1_bjt);
                t1_bjt = (2e-3 - t1_bjt) * t2;
                dt1_dvd *= t2 * t2;
            } else if t1_bjt > 1.0 {
                t1_bjt = 1.0;
                dt1_dvd = 0.0;
            }

            let bjt_a = 1.0 - 0.5 * t1_bjt * t1_bjt;
            let dbjt_a_dvd = -t1_bjt * dt1_dvd;

            // Ibjt = T5 * (ExpVbs1 - ExpVbd1)
            let ibjt = t5 * (exp_vbs1 - exp_vbd1);
            let dibjt_dvb = t5 * (dexp_vbs1_dvb - dexp_vbd1_dvb);
            let dibjt_dvd = t5 * dexp_vbd1_dvb; // dExpVbd1/dVd = dExpVbd1/dVb (since Vbd=Vbs-Vds, d/dVd = -d/dVb... wait)

            // Note: dExpVbd1/dVd = -dExpVbd1/dVb (since Vbd = Vbs - Vds)
            // But ngspice line 2418: dIbjt_dVd = T5 * dExpVbd1_dVb (positive!)
            // This is because ngspice uses dVbd/dVd = -1, and dExpVbd1/dVbd > 0,
            // so dExpVbd1/dVd = dExpVbd1/dVbd * dVbd/dVd = dExpVbd1_dVb * (-1) = -dExpVbd1_dVb
            // But the sign on Ibjt's ExpVbd1 term is negative:
            //   Ibjt = T5*(ExpVbs1 - ExpVbd1)
            //   dIbjt/dVd = T5 * (0 - dExpVbd1/dVd) = T5 * (-(- dExpVbd1_dVb)) = T5 * dExpVbd1_dVb
            // So ngspice line 2418 is correct.
            let dibjt_dvd = t5 * dexp_vbd1_dvb;

            let t3 = (1.0 - bjt_a) * t5;
            let t4 = -t5 * dbjt_a_dvd;

            // Ibs3 = (1-BjtA) * WTsi * jbjt * ExpVbs1 (BARE exp!)
            let ibs3 = t3 * exp_vbs1;
            let dibs3_dvb = t3 * dexp_vbs1_dvb;
            let dibs3_dvd = t4 * exp_vbs1;

            // Ibd3 = (1-BjtA) * WTsi * jbjt * ExpVbd1 (BARE exp!)
            let ibd3 = t3 * exp_vbd1;
            let dibd3_dvb = t3 * dexp_vbd1_dvb;
            // dIbd3/dVd = T4*ExpVbd1 - dIbd3_dVb (ngspice line 2432)
            // T4*ExpVbd1 from BjtA derivative, -dIbd3_dVb from Vbd=Vbs-Vds chain rule
            let dibd3_dvd = t4 * exp_vbd1 - dibd3_dvb;

            // Collector current: Ic = Ibjt - Ibs3 + Ibd3
            let ic = ibjt - ibs3 + ibd3;
            let gcd = dibjt_dvd - dibs3_dvd + dibd3_dvd;
            let gcb = dibjt_dvb - dibs3_dvb + dibd3_dvb;

            (
                ibs3, dibs3_dvb, dibs3_dvd, ibd3, dibd3_dvb, dibd3_dvd, ic, gcd, gcb,
            )
        };

    // Ibs4/Ibd4: Tunneling
    let (ibs4, dibs4_dvb, ibd4, dibd4_dvb) = if sp.jtun == 0.0 {
        (0.0, 0.0, 0.0, 0.0)
    } else {
        let nvtm_tun = vtm * model.ntun;
        // ngspice b3soiddld.c line 2297: uses same threshold 30 for tunneling exp
        let t0 = -vbs_i / nvtm_tun;
        let (exp_val, dexp_val) = if t0 < DD_JCT_EXP_THRESHOLD {
            let e = t0.exp();
            (e, e)
        } else {
            (
                DD_JCT_EXP30 * (1.0 + t0 - DD_JCT_EXP_THRESHOLD),
                DD_JCT_EXP30,
            )
        };
        // ngspice b3soiddld.c line 2449: T5 = WTsi * jtun
        let wtsi_jtun = weff * model.tsi * sp.jtun;
        let ibs4 = -wtsi_jtun * (exp_val - 1.0);
        let dibs4_dvb = wtsi_jtun * dexp_val / nvtm_tun;

        let t0 = -vbd / nvtm_tun;
        let (exp_val, dexp_val) = if t0 < DD_JCT_EXP_THRESHOLD {
            let e = t0.exp();
            (e, e)
        } else {
            (
                DD_JCT_EXP30 * (1.0 + t0 - DD_JCT_EXP_THRESHOLD),
                DD_JCT_EXP30,
            )
        };
        let ibd4 = -wtsi_jtun * (exp_val - 1.0);
        let dibd4_dvb = wtsi_jtun * dexp_val / nvtm_tun;
        (ibs4, dibs4_dvb, ibd4, dibd4_dvb)
    };

    // Total junction currents
    let ibs = ibs1 + ibs2 + ibs3 + ibs4;
    let ibd = ibd1 + ibd2 + ibd3 + ibd4;
    let gbs_jct = dibs1_dvb + dibs2_dvb + dibs3_dvb + dibs4_dvb;
    let gbd_jct = dibd1_dvb + dibd2_dvb + dibd3_dvb + dibd4_dvb;

    // Vds-dependent junction cross-coupling derivatives (ngspice b3soiddld.c lines 2472, 2477).
    // Gjsd = dIbs3/dVd: source junction Vds derivative from BJT transport factor.
    let gjsd = dibs3_dvd;
    // Gjdd_extra: the extra dIbd/dVd beyond what stamp_conductance(b,dp,gbd) handles.
    // stamp_conductance captures -gbd_jct (from Vbd = Vbs - Vds chain rule).
    // The full Gjdd = -dibd1_dvb - dibd2_dvb + dibd3_dvd - dibd4_dvb
    //              = -(gbd_jct - dibd3_dvb) + dibd3_dvd = -gbd_jct + dibd3_dvb + dibd3_dvd
    // Extra = Gjdd - (-gbd_jct) = dibd3_dvb + dibd3_dvd
    let gjdd_extra = dibd3_dvb + dibd3_dvd;

    // Vdsatii for impact ionization (b3soiddld.c lines 1761-1810)
    // When AII > 0, the ionization saturation voltage is computed from
    // AII/BII/CII/DII parameters; otherwise it defaults to Vdsat.
    let (vdsatii, dvdsatii_dvg, dvdsatii_dvd, dvdsatii_dvb);
    if model.aii > 0.0 {
        let (t0_cii, dt0_cii_dvd) = if model.cii != 0.0 {
            let t0_lim = model.cii / 3.0_f64.sqrt() + model.dii;
            let t1_lim = vds_i - t0_lim - 0.1;
            let t2_lim = (t1_lim * t1_lim + 0.4).sqrt();
            let _t3_lim = t0_lim + 0.5 * (t1_lim + t2_lim);
            let dt3_dvd = 0.5 * (1.0 + t1_lim / t2_lim);
            let t4_lim = _t3_lim - model.dii;
            let t5_cii = model.cii / t4_lim;
            let t0_v = t5_cii * t5_cii;
            let dt0_dvd = -2.0 * t0_v / t4_lim * dt3_dvd;
            (t0_v, dt0_dvd)
        } else {
            (0.0, 0.0)
        };
        let t0 = t0_cii + 1.0;
        let t3 = model.aii + model.bii / sp.leff;
        let t4 = 1.0 / (t0 * vgsteff + t3 * esat_l);
        let t5 = -t4 * t4;
        let t7 = esat_l * vgsteff;
        let t8 = t4 * vgsteff;
        vdsatii = t7 * t4;
        let dt4_dvg = t5 * (t0 + t3 * desat_l_dvg);
        let dt4_dvb = t5 * t3 * desat_l_dvb;
        let dt4_dvd = t5 * (vgsteff * dt0_cii_dvd + t3 * desat_l_dvd);
        dvdsatii_dvg = t7 * dt4_dvg + t4 * (esat_l + vgsteff * desat_l_dvg);
        dvdsatii_dvb = t7 * dt4_dvb + t8 * desat_l_dvb;
        dvdsatii_dvd = t7 * dt4_dvd + t8 * desat_l_dvd;
    } else {
        vdsatii = vdsat;
        dvdsatii_dvg = dvdsat_dvg;
        dvdsatii_dvb = dvdsat_dvb;
        dvdsatii_dvd = dvdsat_dvd;
    }

    // Effective Vdsii: smooth clamp Vdseffii ≈ min(Vdsatii, Vds)
    // (b3soiddld.c lines 1847-1866)
    let t1_ii = vdsatii - vds_i - sp.delta;
    let t2_ii_val = (t1_ii * t1_ii + 4.0 * sp.delta * vdsatii).sqrt();
    let vdseffii = vdsatii - 0.5 * (t1_ii + t2_ii_val);
    let diff_vdsii = vds_i - vdseffii;

    // dVdseffii/dV* derivatives (b3soiddld.c lines 1850-1862)
    let t0_ii = t1_ii / t2_ii_val;
    let t3_ii = 2.0 * sp.delta / t2_ii_val;
    let t4_ii = t0_ii + t3_ii;
    let dt2ii_dvg = t4_ii * dvdsatii_dvg;
    let dt2ii_dvd = t4_ii * dvdsatii_dvd - t0_ii;
    let dt2ii_dvb = t4_ii * dvdsatii_dvb;
    let dvdseffii_dvg = 0.5 * (dvdsatii_dvg - dt2ii_dvg);
    let dvdseffii_dvd = 0.5 * (dvdsatii_dvd - dt2ii_dvd + 1.0);
    let dvdseffii_dvb = 0.5 * (dvdsatii_dvb - dt2ii_dvb);

    // Impact ionization (DD: b3soiddld.c lines 2156-2200)
    // Full chain-rule derivatives matching ngspice's decomposed approach.
    let t2_alpha = model.alpha1 + sp.alpha0 / sp.leff;
    let (iii, gii_d, gii_g, gii_b, gii_e) = if t2_alpha <= 0.0 || sp.beta0 <= 0.0 {
        (0.0, 0.0, 0.0, 0.0, 0.0)
    } else if diff_vdsii > sp.beta0 / EXP_THRESHOLD {
        let t0 = -sp.beta0 / diff_vdsii;
        let t10 = t0 / diff_vdsii;
        let dt0_dvg = t10 * dvdseffii_dvg;
        let t1 = t2_alpha * diff_vdsii * t0.exp();
        let iii = t1 * ids;

        let t3 = t1 / diff_vdsii * (t0 - 1.0);
        let dt1_dvg = t1 * (dt0_dvg - dvdseffii_dvg / diff_vdsii);
        let dt1_dvd = -t3 * (1.0 - dvdseffii_dvd);
        let dt1_dvb = t3 * dvdseffii_dvb;

        // Decomposed derivatives: Iii = T1 * Ids, so
        // dIii/dV = (T1 * dIds/dV_internal + Ids * dT1/dV_internal) * chain_rule
        // ngspice lines 2191-2201
        let t2_v = t1 * gm0 + ids * dt1_dvg;
        let t3_v = t1 * gds0 + ids * dt1_dvd;
        let t4_v = t1 * gmbs0 + ids * dt1_dvb;
        let t5_v = t1 * gmc;

        let gii_g = t2_v * dvgsteff_dvg + t4_v * dvbseff_dvg + t5_v * dvcs_dvg;
        let gii_b = t2_v * dvgsteff_dvb + t4_v * dvbseff_dvb + t5_v * dvcs_dvb;
        let gii_d = t2_v * dvgsteff_dvd + t4_v * dvbseff_dvd + t5_v * dvcs_dvd + t3_v;
        let gii_e = t2_v * dvgsteff_dve + t4_v * dvbseff_dve + t5_v * dvcs_dve;
        (iii, gii_d, gii_g, gii_b, gii_e)
    } else if diff_vdsii > 0.0 {
        let t3_min = t2_alpha * MIN_EXP;
        let t1 = t3_min * diff_vdsii;
        let iii = t1 * ids;
        let dt1_dvg = -t3_min * dvdseffii_dvg;
        let dt1_dvd = t3_min * (1.0 - dvdseffii_dvd);
        let dt1_dvb = -t3_min * dvdseffii_dvb;

        let t2_v = t1 * gm0 + ids * dt1_dvg;
        let t3_v = t1 * gds0 + ids * dt1_dvd;
        let t4_v = t1 * gmbs0 + ids * dt1_dvb;
        let t5_v = t1 * gmc;

        let gii_g = t2_v * dvgsteff_dvg + t4_v * dvbseff_dvg + t5_v * dvcs_dvg;
        let gii_b = t2_v * dvgsteff_dvb + t4_v * dvbseff_dvb + t5_v * dvcs_dvb;
        let gii_d = t2_v * dvgsteff_dvd + t4_v * dvbseff_dvd + t5_v * dvcs_dvd + t3_v;
        let gii_e = t2_v * dvgsteff_dve + t4_v * dvbseff_dve + t5_v * dvcs_dve;
        (iii, gii_d, gii_g, gii_b, gii_e)
    } else {
        (0.0, 0.0, 0.0, 0.0, 0.0)
    };

    // Add BJT collector current to drain current and its derivatives to
    // output conductances, matching ngspice b3soiddld.c lines 2566-2572:
    //   cdrain = Ids + Ic; gds = Gds + Gcd; gmbs = Gmb + Gcb
    let ids = ids + ic;
    let gds = gds + gcd;
    let gmbs = gmbs + gcb;

    // Equivalent current sources for NR companion model
    let ceq_d = sign * (ids - gm * vgs_i - gds * vds_i - gmbs * vbs_i - gme * ves_i);

    // Combined drain-junction CEQ (ngspice b3soiddld.c lines 2596-2605: cjd)
    // gjdb = Gjdb - Giib (junction body derivative minus impact ionization)
    // gjdd = Gjdd - (Giid + Gdgidld) where Gjdd = -Gbd + gjdd_extra (chain rule: dVbd/dVds = -1)
    // gjdg = -(Giig + Gdgidlg) (minus Iii and GIDL gate derivs)
    // gjde = -Giie (minus impact ionization back-gate derivative)
    let gjdb = gbd_jct - gii_b;
    let gjdd = -gbd_jct + gjdd_extra - gii_d - ggidl_d;
    let gjdg = -(gii_g + ggidl_g);
    let gjde = -gii_e;
    let ceq_jd = ibd
        - iii
        - igidl
        - sp.min_isub * 0.5
        - (gjdb * vbs_i + gjdd * vds_i + gjdg * vgs_i + gjde * ves_i);

    // Combined source-junction CEQ (ngspice b3soiddld.c lines 2609-2616: cjs)
    let gjsb = gbs_jct;
    let gjsd_c = gjsd;
    let gjsg = -gsgidl_g;
    let ceq_js = ibs - isgidl - sp.min_isub * 0.5 - (gjsb * vbs_i + gjsd_c * vds_i + gjsg * vgs_i);

    // Combined body derivatives (ngspice b3soiddld.c lines 2620-2624)
    // These are the sensitivity of the NET body current to each terminal voltage.
    // Gjdd = -Gbd + gjdd_extra (chain rule from Vbd = Vbs - Vds)
    let gbbs = gii_b - gbs_jct - gbd_jct; // Giib - Gjsb - Gjdb (Gbpbs=0 for bodyMod≠1)
    let gbgs = gii_g + ggidl_g + gsgidl_g; // Giig + Gdgidlg + Gsgidlg (Gbpgs=0)
    let gbds = gii_d + ggidl_d - gjsd + gbd_jct - gjdd_extra; // Giid + Gdgidld - Gjsd - Gjdd = ... + Gbd - gjdd_extra (Gbpds=0)
    let gbes = gii_e; // Giie (Gbpes=0 for bodyMod≠1)

    // Combined body current CEQ (ngspice b3soiddld.c lines 2627-2630: cbody)
    // This is the net current flowing into the body, with linearization subtracted.
    // minIsub is a convergence aid that adds a small minimum current to the body node,
    // split equally between drain and source junctions (KCL balanced).
    let ceq_body = iii + igidl + isgidl - ibs - ibd + sp.min_isub
        - (gbbs * vbs_i + gbgs * vgs_i + gbds * vds_i + gbes * ves_i);

    // ========== Charge (CV) model — capMod=2 (ngspice b3soiddld.c lines 2637-3425) ==========
    let cox_wl = model.cox * sp.weff_cv * sp.leff_cv;

    // CV-specific Vgsteff: double the exponential argument for better
    // continuity in moderate inversion (ngspice b3soiddld.c line 2660).
    // We reuse vgsteff from IV but add the 1e-4 offset matching C code.
    let vgsteff_cv = vgsteff + 1e-4;

    // Vfb for charge model (bias-dependent, using operating-point Vth)
    // ngspice b3soiddld.c line 2675: Vfb = Vth - phi - K1*sqrtPhis
    let vfb_cv = vth - phi - sp.k1 * sqrt_phis;
    let dvfb_cv_dvb = dvth_dvb - sp.k1 * dsqrt_phis_dvb;
    let dvfb_cv_dvd = dvth_dvd;

    // Vfbeff: smooth flat-band voltage (ngspice b3soiddld.c lines 2688-2704)
    const DELTA_3: f64 = 0.02;
    let v3 = vfb_cv - vgs_eff + vbseff - DELTA_3;
    let t0_fb = if vfb_cv <= 0.0 {
        (v3 * v3 - 4.0 * DELTA_3 * vfb_cv).sqrt()
    } else {
        (v3 * v3 + 4.0 * DELTA_3 * vfb_cv).sqrt()
    };
    let t2_fb = if vfb_cv <= 0.0 {
        -DELTA_3 / t0_fb
    } else {
        DELTA_3 / t0_fb
    };
    let t1_fb = 0.5 * (1.0 + v3 / t0_fb);
    let vfbeff = vfb_cv - 0.5 * (v3 + t0_fb);
    let dvfbeff_dvd = (1.0 - t1_fb - t2_fb) * dvfb_cv_dvd;
    let dvfbeff_dvb = (1.0 - t1_fb - t2_fb) * dvfb_cv_dvb - t1_fb;
    let dvfbeff_dvrg = t1_fb * dvgs_eff_dvg;

    // Qac0 (accumulation charge, ngspice b3soiddld.c lines 2706-2711)
    let qac0 = -cox_wl * (vfbeff - vfb_cv);
    let dqac0_dvrg = -cox_wl * dvfbeff_dvrg;
    let dqac0_dvd = -cox_wl * (dvfbeff_dvd - dvfb_cv_dvd);
    let dqac0_dvb = -cox_wl * (dvfbeff_dvb - dvfb_cv_dvb);

    // Qsub0 (depletion charge, ngspice b3soiddld.c lines 2713-2735)
    let t0_k1 = 0.5 * sp.k1;
    let t3_sub = vgs_eff - vfbeff - vbseff - vgsteff_cv;
    let (t1_sub, t2_sub) = if sp.k1 == 0.0 {
        (0.0, 0.0)
    } else if t3_sub < 0.0 {
        (t0_k1 + t3_sub / sp.k1, cox_wl)
    } else {
        let s = (t0_k1 * t0_k1 + t3_sub).sqrt();
        (s, cox_wl * t0_k1 / s)
    };
    let qsub0 = cox_wl * sp.k1 * (t0_k1 - t1_sub);
    let dqsub0_dvrg = t2_sub * (dvfbeff_dvrg - dvgs_eff_dvg);
    let dqsub0_dvg = t2_sub;
    let dqsub0_dvd = t2_sub * dvfbeff_dvd;
    let dqsub0_dvb = t2_sub * (dvfbeff_dvb + 1.0);

    // AbulkCV (ngspice b3soiddld.c lines 2739-2740)
    let abulk_cv = abulk0 * sp.abulk_cv_factor;
    let dabulk_cv_dvb = dabulk0_dvb * sp.abulk_cv_factor;

    // VdsatCV for CAPMOD=2 (also used as shared VdsatCV in CAPMOD=3 for VcsCV clamping)
    let vdsat_cv = vgsteff_cv / abulk_cv + 1e-5;
    let dvdsat_cv_dvg = 1.0 / abulk_cv;
    let dvdsat_cv_dvb = -(vdsat_cv - 1e-5) * dabulk_cv_dvb / abulk_cv;

    // VdseffCV: smooth clamp of VdsatCV vs Vds (ngspice b3soiddld.c lines 2748-2756)
    let v4 = vdsat_cv - vds_i - DELTA_4;
    let t0_cv = (v4 * v4 + 4.0 * DELTA_4 * vdsat_cv).sqrt();
    let vdseff_cv = vdsat_cv - 0.5 * (v4 + t0_cv);
    let t1_cv = 0.5 * (1.0 + v4 / t0_cv);
    let t2_cv = DELTA_4 / t0_cv;
    let t3_cv = (1.0 - t1_cv - t2_cv) / abulk_cv;
    let dvdseff_cv_dvg = t3_cv;
    let dvdseff_cv_dvd = t1_cv;
    let dvdseff_cv_dvb = -t3_cv * (vdsat_cv - 1e-5) * dabulk_cv_dvb;

    // dPhis/dVb = -1 (since phis = phi - vbseff and dvbseff/dvb = 1)
    let dphis_dvb: f64 = -1.0;

    // Outputs from the capMod branch, consumed by downstream Qe1/Qe2/capacitance code.
    let qbf: f64;
    let dqbf_dvrg: f64;
    let dqbf_dvg: f64;
    let dqbf_dvd: f64;
    let dqbf_dvb: f64;
    let dqbf_dvc: f64;
    let dqbf_dve: f64;
    let xc: f64;
    let dxc_dvb: f64;
    let dxc_dvg: f64;
    let dxc_dvd: f64;
    let dxc_dvc: f64;
    let vds_cv: f64;
    let dvds_cv_dvg: f64;
    let dvds_cv_dvd: f64;
    let dvds_cv_dvb: f64;
    let dvds_cv_dvc: f64;
    let vcs_cv: f64;
    let dvcs_cv_dvb: f64;
    let dvcs_cv_dvg: f64;
    let dvcs_cv_dvd: f64;
    let dvcs_cv_dvc: f64;

    if model.cap_mod == 3 {
        // ========== CAPMOD=3 (ngspice b3soiddld.c lines 2888-3224) ==========
        const CONST_2OV3: f64 = 2.0 / 3.0;

        // VdssatCV: redefined for CAPMOD=3 (ngspice lines 2893-2904)
        let t1_cm3 = vgsteff + sp.k1 * sqrt_phis + 0.5 * sp.k1 * sp.k1;
        let t2_cm3 = vgsteff + sp.k1 * sqrt_phis + phis + 0.25 * sp.k1 * sp.k1;
        let dt1_cm3_dvb = sp.k1 * dsqrt_phis_dvb;
        let dt2_cm3_dvb = dt1_cm3_dvb + dphis_dvb;
        let sqrt_t2_cm3 = t2_cm3.abs().max(1e-30).sqrt();
        let vdsat_cv3 = t1_cm3 - sp.k1 * sqrt_t2_cm3;
        let dvdsat_cv3_dvb = dt1_cm3_dvb - sp.k1 / (2.0 * sqrt_t2_cm3) * dt2_cm3_dvb;
        let dvdsat_cv3_dvg = 1.0 - sp.k1 / (2.0 * sqrt_t2_cm3); // dT1/dVg = dT2/dVg = 1

        // VdsCV: nonlinear mapping using IV-model Vdsat (ngspice lines 2906-2978)
        let t1_vdscv = vdsat_cv3 - vdsat;
        let dt1_vdscv_dvg = dvdsat_cv3_dvg - dvdsat_dvg;
        let dt1_vdscv_dvb = dvdsat_cv3_dvb - dvdsat_dvb;
        let dt1_vdscv_dvd = -dvdsat_dvd;
        let dt1_vdscv_dvc = -dvdsat_dvc;

        let (vds_cv_raw, dvds_cv_dvg_r, dvds_cv_dvd_r, dvds_cv_dvb_r, dvds_cv_dvc_r) =
            if t1_vdscv != 0.0 {
                let t3_vm = -0.5 * vdsat / t1_vdscv; // Vdsmax
                let t2_vm = t3_vm * vdsat;
                let t4_vm = t2_vm + t1_vdscv * t3_vm * t3_vm; // fmax
                if vdseff > t2_vm && t1_vdscv < 0.0 {
                    // Saturation branch: VdsCV = fmax (flat top)
                    let t5_vm = -0.5 / (t1_vdscv * t1_vdscv);
                    let dt3_dvg = t5_vm * (t1_vdscv * dvdsat_dvg - vdsat * dt1_vdscv_dvg);
                    let dt3_dvb = t5_vm * (t1_vdscv * dvdsat_dvb - vdsat * dt1_vdscv_dvb);
                    let dt3_dvd = t5_vm * (t1_vdscv * dvdsat_dvd - vdsat * dt1_vdscv_dvd);
                    let dt3_dvc = t5_vm * (t1_vdscv * dvdsat_dvc - vdsat * dt1_vdscv_dvc);
                    (
                        t4_vm,
                        t3_vm * dvdsat_dvg
                            + vdsat * dt3_dvg
                            + t3_vm * (2.0 * t1_vdscv * dt3_dvg + t3_vm * dt1_vdscv_dvg),
                        t3_vm * dvdsat_dvd
                            + vdsat * dt3_dvd
                            + t3_vm * (2.0 * t1_vdscv * dt3_dvd + t3_vm * dt1_vdscv_dvd),
                        t3_vm * dvdsat_dvb
                            + vdsat * dt3_dvb
                            + t3_vm * (2.0 * t1_vdscv * dt3_dvb + t3_vm * dt1_vdscv_dvb),
                        t3_vm * dvdsat_dvc
                            + vdsat * dt3_dvc
                            + t3_vm * (2.0 * t1_vdscv * dt3_dvc + t3_vm * dt1_vdscv_dvc),
                    )
                } else {
                    // Parabolic branch: VdsCV = Vdseff + T1*(Vdseff/Vdsat)^2
                    let t5_vm = vdseff / vdsat;
                    let t6_vm = t5_vm * t5_vm;
                    let t8_vm = 2.0 * t1_vdscv * t5_vm / (vdsat * vdsat);
                    (
                        vdseff + t1_vdscv * t6_vm,
                        dvdseff_dvg
                            + t8_vm * (vdsat * dvdseff_dvg - vdseff * dvdsat_dvg)
                            + t6_vm * dt1_vdscv_dvg,
                        dvdseff_dvd
                            + t8_vm * (vdsat * dvdseff_dvd - vdseff * dvdsat_dvd)
                            + t6_vm * dt1_vdscv_dvd,
                        dvdseff_dvb
                            + t8_vm * (vdsat * dvdseff_dvb - vdseff * dvdsat_dvb)
                            + t6_vm * dt1_vdscv_dvb,
                        dvdseff_dvc
                            + t8_vm * (vdsat * dvdseff_dvc - vdseff * dvdsat_dvc)
                            + t6_vm * dt1_vdscv_dvc,
                    )
                }
            } else {
                // T1 == 0: VdsCV = Vdseff passthrough
                (vdseff, dvdseff_dvg, dvdseff_dvd, dvdseff_dvb, dvdseff_dvc)
            };

        // Clamp VdsCV (ngspice lines 2977-2982)
        // ngspice clamps the VALUE only, never zeroes derivatives — keeping
        // the smooth-formula derivatives preserves Jacobian continuity.
        let mut vds_cv_m = vds_cv_raw.max(0.0) + 1e-4;
        let dvds_cv_dvg_m = dvds_cv_dvg_r;
        let dvds_cv_dvd_m = dvds_cv_dvd_r;
        let dvds_cv_dvb_m = dvds_cv_dvb_r;
        let dvds_cv_dvc_m = dvds_cv_dvc_r;
        if vds_cv_m > vdsat_cv3 - 1e-7 {
            vds_cv_m = vdsat_cv3 - 1e-7;
        }

        // Surface potentials (ngspice lines 2984-3036)
        let phisd = phis + vds_cv_m;
        let dphisd_dvb = dphis_dvb + dvds_cv_dvb_m;
        let dphisd_dvd = dvds_cv_dvd_m;
        let dphisd_dvg = dvds_cv_dvg_m;
        let dphisd_dvc = dvds_cv_dvc_m;
        let sqrt_phisd = phisd.abs().max(1e-30).sqrt();

        // Qdep0: depletion charge at Vgs=Vth (ngspice lines 2992-2995)
        let t10_qdep = cox_wl * sp.k1;
        let qdep0 = t10_qdep * sqrt_phis;
        let dqdep0_dvb = t10_qdep * dsqrt_phis_dvb;

        // VcsCV for CAPMOD=3 (ngspice lines 2997-3036)
        // ngspice b3soiddld.c line 43: #define DELTA_Vcscv 0.0004
        const DELTA_VCSCV: f64 = 4e-4;
        let t5_vcscv = 2.0 * DELTA_VCSCV;
        let t1_vcscv3 = vds_cv_m - vcs - vds_cv_m * vds_cv_m * DELTA_VCSCV;
        let t2_vcscv3 = (t1_vcscv3 * t1_vcscv3 + t5_vcscv * vds_cv_m * vds_cv_m).sqrt();

        let factor_vcscv = 1.0 - 2.0 * vds_cv_m * DELTA_VCSCV;
        let dt1v_dvb = dvds_cv_dvb_m * factor_vcscv;
        let dt2v_dvb = (t1_vcscv3 * dt1v_dvb + t5_vcscv * vds_cv_m * dvds_cv_dvb_m) / t2_vcscv3;
        let dt1v_dvd = dvds_cv_dvd_m * factor_vcscv;
        let dt2v_dvd = (t1_vcscv3 * dt1v_dvd + t5_vcscv * vds_cv_m * dvds_cv_dvd_m) / t2_vcscv3;
        let dt1v_dvg = dvds_cv_dvg_m * factor_vcscv;
        let dt2v_dvg = (t1_vcscv3 * dt1v_dvg + t5_vcscv * vds_cv_m * dvds_cv_dvg_m) / t2_vcscv3;
        let dt1v_dvc = dvds_cv_dvc_m * factor_vcscv - 1.0; // -1 from dVcs/dVc
        let dt2v_dvc = (t1_vcscv3 * dt1v_dvc + t5_vcscv * vds_cv_m * dvds_cv_dvc_m) / t2_vcscv3;

        // ngspice CAPMOD=3 b3soiddld.c lines 3020-3028: no explicit clamping.
        // Match ngspice exactly — no clamp on VcsCV for CAPMOD=3.
        let vcs_cv3 = vcs + 0.5 * (t1_vcscv3 - t2_vcscv3);
        let dvcs_cv3_dvb = 0.5 * (dt1v_dvb - dt2v_dvb);
        let dvcs_cv3_dvg = 0.5 * (dt1v_dvg - dt2v_dvg);
        let dvcs_cv3_dvd = 0.5 * (dt1v_dvd - dt2v_dvd);
        let dvcs_cv3_dvc = 1.0 + 0.5 * (dt1v_dvc - dt2v_dvc);

        let phisc = phis + vcs_cv3;
        let dphisc_dvb = dphis_dvb + dvcs_cv3_dvb;
        let dphisc_dvd = dvcs_cv3_dvd;
        let dphisc_dvg = dvcs_cv3_dvg;
        let dphisc_dvc = dvcs_cv3_dvc;
        let sqrt_phisc = phisc.abs().max(1e-30).sqrt();

        // Numerically stable surface-potential power differences.
        //
        // ngspice computes Phisd^1.5 - Phis^1.5 (and Phisc^2.5 - Phis^2.5)
        // as literal differences (b3soiddld.c lines 3040-3043, 3105-3111).
        // In deep subthreshold VdsCV/VcsCV ~ -1e-7, so these subtract two
        // ~0.7 values agreeing to 7 digits; the result is then cancelled
        // AGAIN against T1*VdsCV down to ~1e-15, leaving ~0.5-2.5% FP dust
        // that re-rolls whenever any terminal voltage moves by 1 ULP. ngspice
        // tolerates this only because its bypass/NR keeps node voltages
        // bit-frozen between evaluations; our transient re-evaluates and the
        // dust becomes dQ/dt noise that pumps the floating body.
        //
        // Since phisd = phis + vds_cv_m and phisc = phis + vcs_cv3 EXACTLY
        // (by construction above), the differences are computed with the
        // increment factored out, using
        //   a^1.5 - b^1.5 = (a-b) * (a + sqrt(a*b) + b) / (sqrt(a) + sqrt(b))
        //   a^2.5 - b^2.5 = (a-b) * (u^4 + u^3 v + u^2 v^2 + u v^3 + v^4)
        //                          / (u + v),   u = sqrt(a), v = sqrt(b)
        // which are algebraically identical to ngspice's expressions but
        // carry full relative precision.
        let pow15_diff_d =
            vds_cv_m * (phisd + sqrt_phisd * sqrt_phis + phis) / (sqrt_phisd + sqrt_phis);
        let pow15_diff_c =
            vcs_cv3 * (phisc + sqrt_phisc * sqrt_phis + phis) / (sqrt_phisc + sqrt_phis);
        let pow25_diff_c = {
            let u = sqrt_phisc;
            let v = sqrt_phis;
            let u2 = u * u;
            let v2 = v * v;
            vcs_cv3 * (u2 * u2 + u2 * u * v + u2 * v2 + u * v2 * v + v2 * v2) / (u + v)
        };

        // Xc for CAPMOD=3 (ngspice lines 3038-3099)
        // Uses surface-potential-based charge partition instead of simple voltage ratios
        let xc_t1 = vgsteff + sp.k1 * sqrt_phis - 0.5 * vds_cv_m;
        let xc_t2 = CONST_2OV3 * sp.k1 * pow15_diff_d;
        let xc_t3 = vgsteff + sp.k1 * sqrt_phis - 0.5 * vcs_cv3;
        let xc_t4 = CONST_2OV3 * sp.k1 * pow15_diff_c;
        let xc_t5 = xc_t1 * vds_cv_m - xc_t2; // Denominator
        let xc_t6 = xc_t3 * vcs_cv3 - xc_t4; // Numerator
        // Xc = T6/T5 (ngspice line 3045).  No floor — matching ngspice exactly.
        // The NR convergence relies on the improved bypass (relative tolerance)
        // rather than derivative clamping, since any charge/Jacobian mismatch
        // from clamping is amplified by 1/h at small timesteps.
        let xc3 = if xc_t5.abs() > 1e-30 {
            xc_t6 / xc_t5
        } else {
            0.0
        };

        // Xc derivatives (ngspice lines 3047-3099), scaled by clamp factor.
        let dxc_t1_dvb = sp.k1 * dsqrt_phis_dvb - 0.5 * dvds_cv_dvb_m;
        let dxc_t2_dvb = sp.k1 * (sqrt_phisd * dphisd_dvb - sqrt_phis * dphis_dvb);
        let dxc_t3_dvb = sp.k1 * dsqrt_phis_dvb - 0.5 * dvcs_cv3_dvb;
        let dxc_t4_dvb = sp.k1 * (sqrt_phisc * dphisc_dvb - sqrt_phis * dphis_dvb);

        let dxc_t1_dvd = -0.5 * dvds_cv_dvd_m;
        let dxc_t2_dvd = sp.k1 * sqrt_phisd * dphisd_dvd;
        let dxc_t3_dvd = -0.5 * dvcs_cv3_dvd;
        let dxc_t4_dvd = sp.k1 * sqrt_phisc * dphisc_dvd;

        let dxc_t1_dvg = 1.0 - 0.5 * dvds_cv_dvg_m;
        let dxc_t2_dvg = sp.k1 * sqrt_phisd * dphisd_dvg;
        let dxc_t3_dvg = 1.0 - 0.5 * dvcs_cv3_dvg;
        let dxc_t4_dvg = sp.k1 * sqrt_phisc * dphisc_dvg;

        let dxc_t1_dvc = -0.5 * dvds_cv_dvc_m;
        let dxc_t2_dvc = sp.k1 * sqrt_phisd * dphisd_dvc;
        let dxc_t3_dvc = -0.5 * dvcs_cv3_dvc;
        let dxc_t4_dvc = sp.k1 * sqrt_phisc * dphisc_dvc;

        let dxc_t5_dvb = xc_t1 * dvds_cv_dvb_m + vds_cv_m * dxc_t1_dvb - dxc_t2_dvb;
        let dxc_t6_dvb = xc_t3 * dvcs_cv3_dvb + vcs_cv3 * dxc_t3_dvb - dxc_t4_dvb;
        let dxc_t5_dvd = xc_t1 * dvds_cv_dvd_m + vds_cv_m * dxc_t1_dvd - dxc_t2_dvd;
        let dxc_t6_dvd = xc_t3 * dvcs_cv3_dvd + vcs_cv3 * dxc_t3_dvd - dxc_t4_dvd;
        let dxc_t5_dvg = xc_t1 * dvds_cv_dvg_m + vds_cv_m * dxc_t1_dvg - dxc_t2_dvg;
        let dxc_t6_dvg = xc_t3 * dvcs_cv3_dvg + vcs_cv3 * dxc_t3_dvg - dxc_t4_dvg;
        let dxc_t5_dvc = xc_t1 * dvds_cv_dvc_m + vds_cv_m * dxc_t1_dvc - dxc_t2_dvc;
        let dxc_t6_dvc = xc_t3 * dvcs_cv3_dvc + vcs_cv3 * dxc_t3_dvc - dxc_t4_dvc;

        // Xc derivatives (ngspice lines 3047-3099).  No floor — matching ngspice exactly.
        // dxc3/dV = (dT6/dV - xc3 * dT5/dV) / T5
        let (dxc3_dvb, dxc3_dvg, dxc3_dvd, dxc3_dvc) = if xc_t5.abs() > 1e-30 {
            let xc3_raw = xc_t6 / xc_t5;
            (
                (dxc_t6_dvb - xc3_raw * dxc_t5_dvb) / xc_t5,
                (dxc_t6_dvg - xc3_raw * dxc_t5_dvg) / xc_t5,
                (dxc_t6_dvd - xc3_raw * dxc_t5_dvd) / xc_t5,
                (dxc_t6_dvc - xc3_raw * dxc_t5_dvc) / xc_t5,
            )
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };

        // Qsubs1 for CAPMOD=3 (ngspice lines 3101-3194)
        // Uses Nomi/Denomi formulation with surface potentials
        let phis_cubed = phis * sqrt_phis; // Phis^(3/2)
        let phisc_cubed = phisc * sqrt_phisc; // Phisc^(3/2)

        // qs_t0/qs_t2 use the numerically stable power differences (see the
        // Xc comment above); algebraically identical to ngspice's
        // Phisc^1.5 - Phis^1.5 and Phisc^2.5 - Phis^2.5.
        let qs_t0 = pow15_diff_c;
        let qs_t1 = vgsteff + sp.k1 * sqrt_phis + phis;
        let qs_t2 = pow25_diff_c; // Phi^(5/2) terms
        let qs_t3 = sp.k1 * vcs_cv3 * (phis + 0.5 * vcs_cv3);

        let dqs_t0_dvb = 1.5 * (sqrt_phisc * dphisc_dvb - sqrt_phis * dphis_dvb);
        let dqs_t1_dvb = (0.5 * sp.k1 / sqrt_phis.max(1e-30) + 1.0) * dphis_dvb;
        let dqs_t2_dvb = 2.5 * (phisc_cubed * dphisc_dvb - phis_cubed * dphis_dvb);
        let dqs_t3_dvb = sp.k1
            * (vcs_cv3 * (dphis_dvb + 0.5 * dvcs_cv3_dvb) + dvcs_cv3_dvb * (phis + 0.5 * vcs_cv3));

        let dqs_t0_dvd = 1.5 * sqrt_phisc * dphisc_dvd;
        let dqs_t2_dvd = 2.5 * phisc_cubed * dphisc_dvd;
        let dqs_t3_dvd = sp.k1 * (phis + vcs_cv3) * dvcs_cv3_dvd;

        let dqs_t0_dvg = 1.5 * sqrt_phisc * dphisc_dvg;
        let dqs_t2_dvg = 2.5 * phisc_cubed * dphisc_dvg;
        let dqs_t3_dvg =
            sp.k1 * (vcs_cv3 * 0.5 * dvcs_cv3_dvg + dvcs_cv3_dvg * (phis + 0.5 * vcs_cv3));

        let dqs_t0_dvc = 1.5 * sqrt_phisc * dphisc_dvc;
        let dqs_t2_dvc = 2.5 * phisc_cubed * dphisc_dvc;
        let dqs_t3_dvc =
            sp.k1 * (vcs_cv3 * 0.5 * dvcs_cv3_dvc + dvcs_cv3_dvc * (phis + 0.5 * vcs_cv3));

        let nomi = sp.k1 * (CONST_2OV3 * qs_t1 * qs_t0 - 0.4 * qs_t2 - qs_t3);
        let dnomi_dvb = sp.k1
            * (CONST_2OV3 * (qs_t1 * dqs_t0_dvb + qs_t0 * dqs_t1_dvb)
                - 0.4 * dqs_t2_dvb
                - dqs_t3_dvb);
        let dnomi_dvd = sp.k1 * (CONST_2OV3 * qs_t1 * dqs_t0_dvd - 0.4 * dqs_t2_dvd - dqs_t3_dvd);
        let dnomi_dvg =
            sp.k1 * (CONST_2OV3 * (qs_t1 * dqs_t0_dvg + qs_t0) - 0.4 * dqs_t2_dvg - dqs_t3_dvg);
        let dnomi_dvc = sp.k1 * (CONST_2OV3 * qs_t1 * dqs_t0_dvc - 0.4 * dqs_t2_dvc - dqs_t3_dvc);

        // Denomi (ngspice lines 3155-3184).  den_t5 uses the stable
        // Phisd^1.5 - Phis^1.5 difference (identical value to xc_t2).
        let den_t4 = vgsteff + sp.k1 * sqrt_phis - 0.5 * vds_cv_m;
        let den_t5 = CONST_2OV3 * sp.k1 * pow15_diff_d;

        let dden_t4_dvb = sp.k1 * dsqrt_phis_dvb - 0.5 * dvds_cv_dvb_m;
        let dden_t5_dvb = sp.k1 * (sqrt_phisd * dphisd_dvb - sqrt_phis * dphis_dvb);
        let dden_t4_dvd = -0.5 * dvds_cv_dvd_m;
        let dden_t5_dvd = sp.k1 * sqrt_phisd * dphisd_dvd;
        let dden_t4_dvg = 1.0 - 0.5 * dvds_cv_dvg_m;
        let dden_t5_dvg = sp.k1 * sqrt_phisd * dphisd_dvg;
        let dden_t4_dvc = -0.5 * dvds_cv_dvc_m;
        let dden_t5_dvc = sp.k1 * sqrt_phisd * dphisd_dvc;

        let denomi = den_t4 * vds_cv_m - den_t5;
        let ddenomi_dvb = vds_cv_m * dden_t4_dvb + den_t4 * dvds_cv_dvb_m - dden_t5_dvb;
        let ddenomi_dvd = vds_cv_m * dden_t4_dvd + den_t4 * dvds_cv_dvd_m - dden_t5_dvd;
        let ddenomi_dvg = vds_cv_m * dden_t4_dvg + den_t4 * dvds_cv_dvg_m - dden_t5_dvg;
        let ddenomi_dvc = vds_cv_m * dden_t4_dvc + den_t4 * dvds_cv_dvc_m - dden_t5_dvc;

        // Denomi (ngspice line 3186): T6 = -CoxWL / Denomi.  No floor —
        // matching ngspice exactly.  Convergence relies on the improved
        // soi_bypass (relative tolerance) rather than derivative clamping.
        let t6_qs1_3 = if denomi.abs() > 1e-30 {
            -cox_wl / denomi
        } else {
            0.0
        };
        let qsubs1_3 = t6_qs1_3 * nomi;
        let nomi_over_den = if denomi.abs() > 1e-30 {
            nomi / denomi
        } else {
            0.0
        };
        let dqsubs1_3_dvb = t6_qs1_3 * (dnomi_dvb - nomi_over_den * ddenomi_dvb);
        let dqsubs1_3_dvg = t6_qs1_3 * (dnomi_dvg - nomi_over_den * ddenomi_dvg);
        let dqsubs1_3_dvd = t6_qs1_3 * (dnomi_dvd - nomi_over_den * ddenomi_dvd);
        let dqsubs1_3_dvc = t6_qs1_3 * (dnomi_dvc - nomi_over_den * ddenomi_dvc);

        // Qsubs2 for CAPMOD=3 (ngspice lines 3196-3210)
        let t6_qs2 = (1e-4 + phi - vbs0eff_dd).abs().max(1e-30).sqrt();
        let t7_qs2 = sp.k1 * cox_wl;
        let t8_qs2 = 1.0 - xc3;
        let t10_qs2 = t7_qs2 * t6_qs2;
        let t11_qs2 = t7_qs2 * t8_qs2 * 0.5 / t6_qs2;
        let qsubs2_3 = -t10_qs2 * t8_qs2;
        let dqsubs2_3_dvg = t10_qs2 * dxc3_dvg;
        let dqsubs2_3_dvb = t10_qs2 * dxc3_dvb;
        let dqsubs2_3_dvd = t10_qs2 * dxc3_dvd + t11_qs2 * dvbs0eff_dvd;
        let dqsubs2_3_dvc = t10_qs2 * dxc3_dvc;
        let dqsubs2_3_dve = t11_qs2 * dvbs0eff_dve;
        let dqsubs2_3_dvrg = t11_qs2 * dvbs0eff_dvg;

        // Qbf for CAPMOD=3: adds Qdep0 (ngspice lines 3212-3224)
        qbf = qac0 + qsub0 + qsubs1_3 + qsubs2_3 + qdep0;
        dqbf_dvrg = dqac0_dvrg + dqsub0_dvrg + dqsubs2_3_dvrg;
        dqbf_dvg = dqsub0_dvg + dqsubs1_3_dvg + dqsubs2_3_dvg;
        dqbf_dvd = dqac0_dvd + dqsub0_dvd + dqsubs1_3_dvd + dqsubs2_3_dvd;
        dqbf_dvb = dqac0_dvb + dqsub0_dvb + dqsubs1_3_dvb + dqsubs2_3_dvb + dqdep0_dvb;
        dqbf_dvc = dqsubs1_3_dvc + dqsubs2_3_dvc;
        dqbf_dve = dqsubs2_3_dve;

        // Export shared outputs for downstream Qe1/Qe2 code
        xc = xc3;
        dxc_dvb = dxc3_dvb;
        dxc_dvg = dxc3_dvg;
        dxc_dvd = dxc3_dvd;
        dxc_dvc = dxc3_dvc;
        vds_cv = vds_cv_m;
        dvds_cv_dvg = dvds_cv_dvg_m;
        dvds_cv_dvd = dvds_cv_dvd_m;
        dvds_cv_dvb = dvds_cv_dvb_m;
        dvds_cv_dvc = dvds_cv_dvc_m;
        vcs_cv = vcs_cv3;
        dvcs_cv_dvb = dvcs_cv3_dvb;
        dvcs_cv_dvg = dvcs_cv3_dvg;
        dvcs_cv_dvd = dvcs_cv3_dvd;
        dvcs_cv_dvc = dvcs_cv3_dvc;
    } else {
        // ========== CAPMOD=2 (ngspice b3soiddld.c lines 2739-2886) ==========

        // VdsCV = VdseffCV for capMod=2 (ngspice b3soiddld.c lines 2761-2769)
        let mut vds_cv2 = vdseff_cv + 1e-5;
        if vds_cv2 > vdsat_cv - 1e-7 {
            vds_cv2 = vdsat_cv - 1e-7;
        }
        let dvds_cv2_dvg = dvdseff_cv_dvg;
        let dvds_cv2_dvd = dvdseff_cv_dvd;
        let dvds_cv2_dvb = dvdseff_cv_dvb;

        // VcsCV calculation (ngspice b3soiddld.c lines 2772-2796)
        // ngspice b3soiddld.c line 43: #define DELTA_Vcscv 0.0004
        const DELTA_VCSCV: f64 = 4e-4;
        let t1_vcscv = vds_cv2 - vcs - vds_cv2 * vds_cv2 * DELTA_VCSCV;
        let t5_vcscv = 2.0 * DELTA_VCSCV;
        let t2_vcscv = (t1_vcscv * t1_vcscv + t5_vcscv * vds_cv2 * vds_cv2).sqrt();

        let dt1_vcscv_dvb = dvds_cv2_dvb * (1.0 - 2.0 * vds_cv2 * DELTA_VCSCV);
        let dt2_vcscv_dvb =
            (t1_vcscv * dt1_vcscv_dvb + t5_vcscv * vds_cv2 * dvds_cv2_dvb) / t2_vcscv;
        let dt1_vcscv_dvd = dvds_cv2_dvd * (1.0 - 2.0 * vds_cv2 * DELTA_VCSCV);
        let dt2_vcscv_dvd =
            (t1_vcscv * dt1_vcscv_dvd + t5_vcscv * vds_cv2 * dvds_cv2_dvd) / t2_vcscv;
        let dt1_vcscv_dvg = dvds_cv2_dvg * (1.0 - 2.0 * vds_cv2 * DELTA_VCSCV);
        let dt2_vcscv_dvg =
            (t1_vcscv * dt1_vcscv_dvg + t5_vcscv * vds_cv2 * dvds_cv2_dvg) / t2_vcscv;
        let dt1_vcscv_dvc: f64 = -1.0;
        let dt2_vcscv_dvc = t1_vcscv * dt1_vcscv_dvc / t2_vcscv;

        // ngspice CAPMOD=2 b3soiddld.c lines 2795-2796: clamp VALUE only,
        // keep derivatives from the smooth formula for Jacobian continuity.
        let mut vcs_cv2 = vcs + 0.5 * (t1_vcscv - t2_vcscv);
        let dvcs_cv2_dvb = 0.5 * (dt1_vcscv_dvb - dt2_vcscv_dvb);
        let dvcs_cv2_dvg = 0.5 * (dt1_vcscv_dvg - dt2_vcscv_dvg);
        let dvcs_cv2_dvd = 0.5 * (dt1_vcscv_dvd - dt2_vcscv_dvd);
        let dvcs_cv2_dvc = 1.0 + 0.5 * (dt1_vcscv_dvc - dt2_vcscv_dvc);
        if vcs_cv2 < 0.0 {
            vcs_cv2 = 0.0;
        } else if vcs_cv2 > vds_cv2 {
            vcs_cv2 = vds_cv2;
        }

        // Xc calculation (cross-section parameter, ngspice b3soiddld.c lines 2798-2823)
        let t3_xc = 2.0 * vdsat_cv - vcs_cv2;
        let t4_xc = 2.0 * vdsat_cv - vds_cv2;
        let dt4_xc_dvg = 2.0 * dvdsat_cv_dvg - dvds_cv2_dvg;
        let dt4_xc_dvd = -dvds_cv2_dvd;
        let dt4_xc_dvb = 2.0 * dvdsat_cv_dvb - dvds_cv2_dvb;
        let t0_xc = t3_xc * vcs_cv2;
        let t1_xc = t4_xc * vds_cv2;
        let xc2 = if t1_xc.abs() > 1e-30 {
            t0_xc / t1_xc
        } else {
            0.0
        };

        let dt0_xc_dvb = vcs_cv2 * (2.0 * dvdsat_cv_dvb - dvcs_cv2_dvb) + t3_xc * dvcs_cv2_dvb;
        let dt0_xc_dvg = vcs_cv2 * (2.0 * dvdsat_cv_dvg - dvcs_cv2_dvg) + t3_xc * dvcs_cv2_dvg;
        let dt0_xc_dvd = 2.0 * dvcs_cv2_dvd * (vdsat_cv - vcs_cv2);
        let dt0_xc_dvc = 2.0 * dvcs_cv2_dvc * (vdsat_cv - vcs_cv2);

        let dt1_xc_dvb = vds_cv2 * dt4_xc_dvb + t4_xc * dvds_cv2_dvb;
        let dt1_xc_dvg = vds_cv2 * dt4_xc_dvg + t4_xc * dvds_cv2_dvg;
        let dt1_xc_dvd = dvds_cv2_dvd * t4_xc + vds_cv2 * dt4_xc_dvd;

        let (dxc2_dvb, dxc2_dvg, dxc2_dvd, dxc2_dvc) = if t1_xc.abs() > 1e-30 {
            (
                (dt0_xc_dvb - dt1_xc_dvb * xc2) / t1_xc,
                (dt0_xc_dvg - dt1_xc_dvg * xc2) / t1_xc,
                (dt0_xc_dvd - dt1_xc_dvd * xc2) / t1_xc,
                dt0_xc_dvc / t1_xc,
            )
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };

        // Qsubs1 (ngspice b3soiddld.c lines 2825-2866)
        let t0_qs1 = abulk_cv * vcs_cv2;
        let dt0_qs1_dvb = dabulk_cv_dvb * vcs_cv2 + dvcs_cv2_dvb * abulk_cv;
        let dt0_qs1_dvg = dvcs_cv2_dvg * abulk_cv;
        let dt0_qs1_dvd = abulk_cv * dvcs_cv2_dvd;
        let dt0_qs1_dvc = abulk_cv * dvcs_cv2_dvc;

        let t1_qs1 = 12.0 * (vgsteff_cv - 0.5 * t0_qs1 + 1e-20);
        let dt1_qs1_dvb = -6.0 * dt0_qs1_dvb;
        let dt1_qs1_dvg = 12.0 * (1.0 - 0.5 * dt0_qs1_dvg);
        let dt1_qs1_dvd = -6.0 * dt0_qs1_dvd;
        let dt1_qs1_dvc = -6.0 * dt0_qs1_dvc;

        let t2_qs1 = vcs_cv2 / t1_qs1;
        let t4_qs1 = t1_qs1 * t1_qs1;
        let dt2_qs1_dvb = (dvcs_cv2_dvb * t1_qs1 - dt1_qs1_dvb * vcs_cv2) / t4_qs1;
        let dt2_qs1_dvg = (dvcs_cv2_dvg * t1_qs1 - dt1_qs1_dvg * vcs_cv2) / t4_qs1;
        let dt2_qs1_dvd = (dvcs_cv2_dvd * t1_qs1 - dt1_qs1_dvd * vcs_cv2) / t4_qs1;
        let dt2_qs1_dvc = (dvcs_cv2_dvc * t1_qs1 - dt1_qs1_dvc * vcs_cv2) / t4_qs1;

        let t3_qs1 = t0_qs1 * t2_qs1;
        let dt3_qs1_dvb = dt0_qs1_dvb * t2_qs1 + dt2_qs1_dvb * t0_qs1;
        let dt3_qs1_dvg = dt0_qs1_dvg * t2_qs1 + dt2_qs1_dvg * t0_qs1;
        let dt3_qs1_dvd = dt0_qs1_dvd * t2_qs1 + dt2_qs1_dvd * t0_qs1;
        let dt3_qs1_dvc = dt0_qs1_dvc * t2_qs1 + dt2_qs1_dvc * t0_qs1;

        let t4_abulk = 1.0 - abulk_cv;
        let dt4_abulk_dvb = -dabulk_cv_dvb;

        let t5_qs1 = 0.5 * vcs_cv2 - t3_qs1;
        let dt5_qs1_dvb = 0.5 * dvcs_cv2_dvb - dt3_qs1_dvb;
        let dt5_qs1_dvg = 0.5 * dvcs_cv2_dvg - dt3_qs1_dvg;
        let dt5_qs1_dvd = 0.5 * dvcs_cv2_dvd - dt3_qs1_dvd;
        let dt5_qs1_dvc = 0.5 * dvcs_cv2_dvc - dt3_qs1_dvc;

        let t6_qs1 = t4_abulk * t5_qs1 * cox_wl;
        let t7_qs1 = cox_wl * xc2;

        let qsubs1_2 = cox_wl * xc2 * t4_abulk * t5_qs1;
        let dqsubs1_2_dvb =
            t6_qs1 * dxc2_dvb + t7_qs1 * (t4_abulk * dt5_qs1_dvb + dt4_abulk_dvb * t5_qs1);
        let dqsubs1_2_dvg = t6_qs1 * dxc2_dvg + t7_qs1 * t4_abulk * dt5_qs1_dvg;
        let dqsubs1_2_dvd = t6_qs1 * dxc2_dvd + t7_qs1 * t4_abulk * dt5_qs1_dvd;
        let dqsubs1_2_dvc = t6_qs1 * dxc2_dvc + t7_qs1 * t4_abulk * dt5_qs1_dvc;

        // Qsubs2 (ngspice b3soiddld.c lines 2868-2874)
        let qsubs2_2 = -cox_wl * (1.0 - xc2) * (abulk_cv - 1.0) * vcs;
        let t2_qs2 = cox_wl * (abulk_cv - 1.0) * vcs;
        let dqsubs2_2_dvb = t2_qs2 * dxc2_dvb - cox_wl * (1.0 - xc2) * vcs * dabulk_cv_dvb;
        let dqsubs2_2_dvg = t2_qs2 * dxc2_dvg;
        let dqsubs2_2_dvd = t2_qs2 * dxc2_dvd;
        let dqsubs2_2_dvc = t2_qs2 * dxc2_dvc - cox_wl * (1.0 - xc2) * (abulk_cv - 1.0);

        // Qbf: total front-gate body charge (ngspice b3soiddld.c lines 2876-2886)
        qbf = qac0 + qsub0 + qsubs1_2 + qsubs2_2;
        dqbf_dvrg = dqac0_dvrg + dqsub0_dvrg;
        dqbf_dvg = dqsub0_dvg + dqsubs1_2_dvg + dqsubs2_2_dvg;
        dqbf_dvd = dqac0_dvd + dqsub0_dvd + dqsubs1_2_dvd + dqsubs2_2_dvd;
        dqbf_dvb = dqac0_dvb + dqsub0_dvb + dqsubs1_2_dvb + dqsubs2_2_dvb;
        dqbf_dvc = dqsubs1_2_dvc + dqsubs2_2_dvc;
        dqbf_dve = 0.0;

        // Export shared outputs
        xc = xc2;
        dxc_dvb = dxc2_dvb;
        dxc_dvg = dxc2_dvg;
        dxc_dvd = dxc2_dvd;
        dxc_dvc = dxc2_dvc;
        vds_cv = vds_cv2;
        dvds_cv_dvg = dvds_cv2_dvg;
        dvds_cv_dvd = dvds_cv2_dvd;
        dvds_cv_dvb = dvds_cv2_dvb;
        dvds_cv_dvc = 0.0; // dVdsCV/dVc = 0 for capMod=2
        vcs_cv = vcs_cv2;
        dvcs_cv_dvb = dvcs_cv2_dvb;
        dvcs_cv_dvg = dvcs_cv2_dvg;
        dvcs_cv_dvd = dvcs_cv2_dvd;
        dvcs_cv_dvc = dvcs_cv2_dvc;
    }

    // Backgate charge: Qsicv, Qbf0 (ngspice b3soiddld.c lines 3228-3247)
    let cbox_wl = sp.kb3 * model.cbox * sp.weff_cv * sp.leff_cv;

    let t0_bk = 0.5 * sp.k1;
    let t2_bk1 = (phi - vbs0t).abs().sqrt();
    let t3_bk1 = phi + sp.k1 * t2_bk1 - vbs0t;
    let t4_bk1 = (t0_bk * t0_bk + t3_bk1).sqrt();
    let qsicv = sp.k1 * cox_wl * (t0_bk - t4_bk1);

    let t2_bk2 = (phi - vbs0mos).abs().sqrt();
    let t3_bk2 = phi + sp.k1 * t2_bk2 - vbs0mos;
    let t4_bk2 = (t0_bk * t0_bk + t3_bk2).sqrt();
    let qbf0 = sp.k1 * cox_wl * (t0_bk - t4_bk2);
    let t6_bk2 = cox_wl * t0_bk / t4_bk2.max(1e-30) * (1.0 + t0_bk / t2_bk2.max(1e-30));
    let dqbf0_dve = t6_bk2 * dvbs0mos_dve;

    // Qe1 (ngspice b3soiddld.c lines 3249-3264)
    let t5_e1 = -cbox_wl * (vbsdio - vbs0);
    let t6_e1 = cbox_wl * xc;
    let qe1 = -qsicv + qbf0 + t5_e1 * xc;
    let dqe1_dvg = t5_e1 * (dxc_dvg * dvgsteff_dvg + dxc_dvb * dvbseff_dvg + dxc_dvc * dvcs_dvg)
        - t6_e1 * dvbsdio_dvg;
    let dqe1_dvb = t5_e1 * (dxc_dvg * dvgsteff_dvb + dxc_dvb * dvbseff_dvb + dxc_dvc * dvcs_dvb)
        - t6_e1 * dvbsdio_dvb;
    let dqe1_dvd = t5_e1
        * (dxc_dvg * dvgsteff_dvd + dxc_dvb * dvbseff_dvd + dxc_dvc * dvcs_dvd + dxc_dvd)
        - t6_e1 * dvbsdio_dvd;
    let dqe1_dve = dqbf0_dve + t6_e1 * (dvbs0_dve - dvbsdio_dve);

    // Qe2 (ngspice b3soiddld.c lines 3266-3284)
    let t2_e2 = -model.cboxt * sp.weff_cv * sp.leff_cv;
    let t3_e2 = t2_e2 * 0.5 * (1.0 - xc);
    let t4_e2 = t2_e2 * 0.5 * (vds_cv - vcs_cv);
    let qe2 = t2_e2 * 0.5 * (1.0 - xc) * (vds_cv - vcs_cv);

    // T10=dVgsteff, T11=dVbseff, T12=dVcs transform
    let t10_e2 = t3_e2 * (dvds_cv_dvg - dvcs_cv_dvg) - t4_e2 * dxc_dvg;
    let t11_e2 = t3_e2 * (dvds_cv_dvb - dvcs_cv_dvb) - t4_e2 * dxc_dvb;
    let t12_e2_dvc = dvds_cv_dvc - dvcs_cv_dvc;
    let t12_e2 = t3_e2 * t12_e2_dvc - t4_e2 * dxc_dvc;
    let dqe2_dvg = t10_e2 * dvgsteff_dvg + t11_e2 * dvbseff_dvg + t12_e2 * dvcs_dvg;
    let dqe2_dvb = t10_e2 * dvgsteff_dvb + t11_e2 * dvbseff_dvb + t12_e2 * dvcs_dvb;
    let dqe2_dvd = t10_e2 * dvgsteff_dvd
        + t11_e2 * dvbseff_dvd
        + t12_e2 * dvcs_dvd
        + t3_e2 * (dvds_cv_dvd - dvcs_cv_dvd)
        - t4_e2 * dxc_dvd;
    let dqe2_dve = t10_e2 * dvgsteff_dve + t11_e2 * dvbseff_dve + t12_e2 * dvcs_dve;

    // Cbg, Cbb, Cbd, Cbe: transform Qbf derivatives from internal to real voltages
    // (ngspice b3soiddld.c lines 3288-3299)
    let cbg = dqbf_dvrg + dqbf_dvg * dvgsteff_dvg + dqbf_dvb * dvbseff_dvg + dqbf_dvc * dvcs_dvg;
    let cbb = dqbf_dvg * dvgsteff_dvb + dqbf_dvb * dvbseff_dvb + dqbf_dvc * dvcs_dvb;
    let cbd = dqbf_dvg * dvgsteff_dvd + dqbf_dvb * dvbseff_dvd + dqbf_dvc * dvcs_dvd + dqbf_dvd;
    let cbe = dqbf_dvg * dvgsteff_dve + dqbf_dvb * dvbseff_dve + dqbf_dvc * dvcs_dve + dqbf_dve;

    // Qex (external charge, ngspice b3soiddld.c lines 3378-3385)
    const QEX_FACT: f64 = 20.0;
    let t0_ex = QEX_FACT * sp.k1 * cox_wl;
    let qex = t0_ex * (vbs_i - vbsdio);
    let dqex_dvg = -t0_ex * dvbsdio_dvg;
    let dqex_dvb = t0_ex * (1.0 - dvbsdio_dvb);
    let dqex_dvd = -t0_ex * dvbsdio_dvd;
    let dqex_dve = -t0_ex * dvbsdio_dve;

    // Gate charge: qinv and derivatives (ngspice b3soiddld.c lines 3313-3327)
    let t0_qi = abulk_cv * vdseff_cv;
    let t1_qi = 12.0 * (vgsteff_cv - 0.5 * t0_qi + 1e-20);
    let t2_qi = vdseff_cv / t1_qi;
    let t3_qi = t0_qi * t2_qi;
    let t4_qi = 1.0 - 12.0 * t2_qi * t2_qi * abulk_cv;
    let t5_qi = 6.0 * t0_qi * (4.0 * vgsteff_cv - t0_qi) / (t1_qi * t1_qi) - 0.5;
    let t6_qi = 12.0 * t2_qi * t2_qi * vgsteff_cv;
    let qinv = cox_wl * (vgsteff_cv - 0.5 * vdseff_cv + t3_qi);
    let cgg1 = cox_wl * (t4_qi + t5_qi * dvdseff_cv_dvg);
    let cgd1 = cox_wl * t5_qi * dvdseff_cv_dvd;
    let cgb1 = cox_wl * (t5_qi * dvdseff_cv_dvb + t6_qi * dabulk_cv_dvb);

    // Source charge partition: 50/50 model (xpart=0.5 default, ngspice line 3362-3367)
    let qsrc = -0.5 * qinv;
    let csg1 = -0.5 * cgg1;
    let csb1 = -0.5 * cgb1;
    let _csd1 = -0.5 * cgd1;

    // Transform source derivatives to real voltages (ngspice b3soiddld.c lines 3370-3376)
    let csg = csg1 * dvgsteff_dvg + csb1 * dvbseff_dvg;
    let csd = _csd1 + csg1 * dvgsteff_dvd + csb1 * dvbseff_dvd;
    let csb = csg1 * dvgsteff_dvb + csb1 * dvbseff_dvb;
    let cse = csg1 * dvgsteff_dve + csb1 * dvbseff_dve;

    // Gate charge derivatives in real voltages (ngspice b3soiddld.c lines 3392-3398)
    let cgg = (cgg1 * dvgsteff_dvg + cgb1 * dvbseff_dvg) - cbg;
    let cgd = (cgd1 + cgg1 * dvgsteff_dvd + cgb1 * dvbseff_dvd) - cbd;
    let cgb_cv = (cgb1 * dvbseff_dvb + cgg1 * dvgsteff_dvb) - cbb;
    let cge = (cgg1 * dvgsteff_dve + cgb1 * dvbseff_dve) - cbe;

    // Final capacitance assignments (ngspice b3soiddld.c lines 3400-3424)
    let cggb = cgg - dqe2_dvg;
    let cgsb = -(cgg + cgd + cgb_cv + cge) + (dqe2_dvg + dqe2_dvd + dqe2_dvb + dqe2_dve);
    let cgdb = cgd - dqe2_dvd;

    let cbgb = cbg - dqe1_dvg + dqex_dvg;
    let cbsb = -(cbg + cbd + cbb + cbe) + (dqe1_dvg + dqe1_dvd + dqe1_dvb + dqe1_dve)
        - (dqex_dvg + dqex_dvd + dqex_dvb + dqex_dve);
    let cbdb = cbd - dqe1_dvd + dqex_dvd;

    let cdgb = -(cgg + cbg + csg);
    let cddb = -(cgd + cbd + csd);
    let cdsb = (cgg + cgd + cgb_cv + cge + cbg + cbd + cbb + cbe + csg + csd + csb + cse);

    // E-node (substrate) capacitance derivatives (ngspice b3soiddld.c lines 3400-3424)
    // cgeb = dQgate/dVe = Cge - Ce2e
    let cgeb = cge - dqe2_dve;
    // cbeb = dQbody/dVe = Cbe - Ce1e + dQex/dVe
    let cbeb = cbe - dqe1_dve + dqex_dve;
    // cdeb = dQdrn/dVe = -(Cge + Cbe + Cse) (by qdrn = -(qinv+qsrc), no direct Ve dep)
    let cdeb = -(cge + cbe + cse);
    // ceeb = dQsub/dVe = Ce1e + Ce2e - dQex/dVe
    let ceeb = dqe1_dve + dqe2_dve - dqex_dve;
    // cegb = dQsub/dVg = Ce1g + Ce2g - dQex/dVg
    let cegb = dqe1_dvg + dqe2_dvg - dqex_dvg;
    // cedb = dQsub/dVd = Ce1d + Ce2d - dQex/dVd
    let cedb = dqe1_dvd + dqe2_dvd - dqex_dvd;
    // cesb = dQsub/dVs (by KCL: sum of all 5 terminal derivatives = 0)
    let cesb = -(cegb + cedb + ceeb + (dqe1_dvb + dqe2_dvb - dqex_dvb));

    // Assemble terminal charges (ngspice b3soiddld.c lines 3387-3390)
    // Uses Qbf (CAPMOD body charge), NOT Qbf0 (backgate correction used only in Qe1).
    let qgate_total = qinv - (qbf + qe2);
    let mut qbody_total = qbf - qe1 + qex;
    let qsub_total = qe1 + qe2 - qex;
    let mut qdrn_total = -(qinv + qsrc);

    // qbf0 is consumed by qe1; qsicv is consumed by qe1.
    let _ = (qbf0, qsicv);

    // Intrinsic S/D junction charge (ngspice b3soiddld.c lines 3440-3494)
    // These depletion + transit-time charges couple the body node to drain/source,
    // providing essential diagonal stiffness for floating-body convergence.
    let phi_bswg = model.pbswg;
    let mjswg = model.mjswg;
    let cjsbs = model.cjswg * weff * model.tsi / 1e-7;

    // Source junction charge (Vbs-dependent)
    let (qjs, gcjsbs) = if vbs_i < 0.0 {
        let arg = 1.0 - vbs_i / phi_bswg;
        let dt3_dvb = if mjswg == 0.5 {
            1.0 / arg.sqrt()
        } else {
            (-mjswg * arg.ln()).exp()
        };
        let t3 = (1.0 - arg * dt3_dvb) * phi_bswg / (1.0 - mjswg);
        (
            cjsbs * t3 + model.tt * ibs1,
            cjsbs * dt3_dvb + model.tt * dibs1_dvb,
        )
    } else {
        let t3 = vbs_i * (1.0 + 0.5 * mjswg * vbs_i / phi_bswg);
        let dt3_dvb = 1.0 + mjswg * vbs_i / phi_bswg;
        (
            cjsbs * t3 + model.tt * ibs1,
            cjsbs * dt3_dvb + model.tt * dibs1_dvb,
        )
    };

    // Drain junction charge (Vbd-dependent)
    let dibd1_dvd = -dibd1_dvb;
    let (qjd, gcjdbs, gcjdds) = if vbd < 0.0 {
        let arg = 1.0 - vbd / phi_bswg;
        let dt3_dvb = if mjswg == 0.5 {
            1.0 / arg.sqrt()
        } else {
            (-mjswg * arg.ln()).exp()
        };
        let t3 = (1.0 - arg * dt3_dvb) * phi_bswg / (1.0 - mjswg);
        let dt3_dvd = -dt3_dvb;
        (
            cjsbs * t3 + model.tt * ibd1,
            cjsbs * dt3_dvb + model.tt * dibd1_dvb,
            cjsbs * dt3_dvd + model.tt * dibd1_dvd,
        )
    } else {
        let t3 = vbd * (1.0 + 0.5 * mjswg * vbd / phi_bswg);
        let dt3_dvb = 1.0 + mjswg * vbd / phi_bswg;
        let dt3_dvd = -dt3_dvb;
        (
            cjsbs * t3 + model.tt * ibd1,
            cjsbs * dt3_dvb + model.tt * dibd1_dvb,
            cjsbs * dt3_dvd + model.tt * dibd1_dvd,
        )
    };

    // Fold junction charges into terminal charges (ngspice lines 3483-3485)
    qdrn_total -= qjd;
    qbody_total += qjs + qjd;
    // qsrc is recomputed by KCL in the transient stamp

    // Fold junction conductance derivatives into capacitance matrix
    // (ngspice lines 3487-3494)
    let cddb = cddb - gcjdds;
    let cdsb = cdsb + gcjdds + gcjdbs;
    let cbdb = cbdb + gcjdds;
    let cbsb = cbsb - (gcjdds + gcjdbs + gcjsbs);

    Bsim3SoiDdCompanion {
        ids: ids / sp.nseg,
        gm: gm / sp.nseg,
        gds: gds / sp.nseg,
        gmbs: gmbs / sp.nseg,
        gme: gme / sp.nseg,
        mode,
        vdsat,
        ibs,
        ibd,
        gbs_jct,
        gbd_jct,
        gjsd,
        gjdd_extra,
        iii,
        gii_d,
        gii_g,
        gii_b,
        gii_e,
        igidl,
        ggidl_d,
        ggidl_g,
        isgidl,
        gsgidl_g,
        ceq_d,
        ceq_jd,
        ceq_js,
        ceq_body,
        gbbs,
        gbgs,
        gbds,
        gbes,
        cggb: cggb + sp.cgso_eff * weff + sp.cgdo_eff * weff,
        cgdb: cgdb - sp.cgdo_eff * weff,
        cgsb: cgsb - sp.cgso_eff * weff,
        cbgb,
        cbdb,
        cbsb,
        cdgb: cdgb - sp.cgdo_eff * weff,
        cddb: cddb + sp.cgdo_eff * weff,
        cdsb,
        capbd: 0.0,
        capbs: 0.0,
        qinv,
        cgeb,
        cbeb,
        cdeb,
        ceeb,
        cegb,
        cedb,
        cesb,
        qgate: qgate_total,
        qbody: qbody_total,
        qdrn: qdrn_total,
        qsub: qsub_total,
        vbs_dd,
    }
}

/// Stamp BSIM3SOI-DD companion model into the MNA matrix and RHS.
///
/// Uses direct matrix.add() for asymmetric VCCS elements (gm, gmbs) and
/// stamp_conductance() for symmetric two-terminal elements (gbd, gbs, series R).
pub fn stamp_bsim3soi_dd(
    matrix: &mut crate::SparseMatrix,
    rhs: &mut [f64],
    inst: &Bsim3SoiDdInstance,
    comp: &Bsim3SoiDdCompanion,
    gmin: f64,
) {
    let dp = inst.drain_eff_idx();
    let g = inst.gate_idx;
    let sp = inst.source_eff_idx();
    let b = inst.body_int_idx;

    let m = inst.m;

    let e = inst.e_idx;

    let (xnrm, xrev) = if comp.mode > 0 {
        (1.0, 0.0)
    } else {
        (0.0, 1.0)
    };

    // Mode-adjusted transconductances (swap gm/gds in reverse mode).
    let gm_eff = m * (comp.gm * xnrm + comp.gds * xrev);
    let gds_eff = m * (comp.gds * xnrm + comp.gm * xrev);
    let gmbs_eff = m * comp.gmbs;
    let gme_eff = m * comp.gme;
    // FwdSum includes Gme (ngspice line 3895: FwdSum = Gm + Gmbs + Gme)
    let fwd_sum = gm_eff + gds_eff + gmbs_eff + gme_eff;

    // --- Channel current (asymmetric VCCS stamps) ---
    // Ids = gm_eff*(Vg-Vsp) + gds_eff*(Vdp-Vsp) + gmbs_eff*(Vb-Vsp) + gme_eff*(Ve-Vsp)
    // D' row: dIds/d* contributions
    if let Some(d) = dp {
        matrix.add(d, d, gds_eff);
        if let Some(gate) = g {
            matrix.add(d, gate, gm_eff);
        }
        if let Some(s) = sp {
            matrix.add(d, s, -fwd_sum);
        }
        if let Some(bulk) = b {
            matrix.add(d, bulk, gmbs_eff);
        }
        if let Some(ei) = e {
            matrix.add(d, ei, gme_eff);
        }
    }
    // S' row: -dIds/d* contributions
    if let Some(s) = sp {
        if let Some(d) = dp {
            matrix.add(s, d, -gds_eff);
        }
        if let Some(gate) = g {
            matrix.add(s, gate, -gm_eff);
        }
        matrix.add(s, s, fwd_sum);
        if let Some(bulk) = b {
            matrix.add(s, bulk, -gmbs_eff);
        }
        if let Some(ei) = e {
            matrix.add(s, ei, -gme_eff);
        }
    }

    // Junction conductances and cross-coupling are now included in the combined
    // gddp*/gssp*/gbb* stamps below (matching ngspice's combined derivative approach).

    // Gate-drain gmin (ngspice b3soiddld.c lines 4103-4110: CKTgmin between G and DP)
    crate::stamp_conductance(matrix, g, dp, m * gmin);

    // Floating-body stability: add Gmin body-to-source coupling (matching ngspice
    // b3soiddld.c line 4090: Gmin = CKTgmin * 1e-6).  ngspice uses a much smaller
    // Gmin at the body node than the circuit-level gmin to avoid dominating the
    // extremely small floating-body junction currents.
    if inst.body_idx.is_none() {
        // Floor at 1e-20 so circuits with very small gmin (e.g. 1e-25) still
        // have enough body-source coupling to keep the Jacobian non-singular.
        crate::stamp_conductance(matrix, b, sp, (gmin * 1e-6).max(1e-20));
    }

    // --- Body current Jacobian stamps ---
    // Use combined derivatives matching ngspice b3soiddld.c lines 3908-3914,
    // 3916-3921, 3923-3928. This structure computes combined gbbs/gbgs/gbds/gbes
    // and gjd*/gjs* derivatives, ensuring the body node equation has the same
    // FP evaluation order as ngspice (critical for floating-body convergence).

    // Drain-junction combined derivatives (ngspice gjd*: lines 2596-2599)
    // gddp* = negated gjd* stamps.  Gjdd = dIbd/dVds = -Gbd + gjdd_extra
    // (chain rule: Vbd = Vbs - Vds → dVbd/dVds = -1)
    {
        let gddpg = m * (comp.gii_g + comp.ggidl_g); // -gjdg = Giig + Gdgidlg
        let gddpdp = m * (comp.gii_d + comp.ggidl_d + comp.gbd_jct - comp.gjdd_extra); // -gjdd = Gbd - gjdd_extra + Giid + Gdgidld
        let gddpb = m * (comp.gii_b - comp.gbd_jct); // -gjdb = Giib - Gjdb
        let gddpe = m * comp.gii_e; // -gjde = Giie
        let gddpsp = -(gddpg + gddpdp + gddpb + gddpe); // KCL balance

        if let Some(d) = dp {
            matrix.add(d, d, gddpdp);
            if let Some(gate) = g {
                matrix.add(d, gate, gddpg);
            }
            if let Some(bi) = b {
                matrix.add(d, bi, gddpb);
            }
            if let Some(ei) = e {
                matrix.add(d, ei, gddpe);
            }
            if let Some(s) = sp {
                matrix.add(d, s, gddpsp);
            }
        }
    }

    // Source-junction combined derivatives (ngspice gjs*: lines 2609-2611)
    // gssp* stamps: current flowing into source-prime from body junction paths
    {
        let gsspg = m * comp.gsgidl_g; // -gjsg = Gsgidlg
        let gsspdp = m * (-comp.gjsd); // -gjsd (negative because Gjsd is positive)
        let gsspb = m * (-comp.gbs_jct); // -gjsb
        let gsspe = 0.0_f64; // no back-gate coupling in source junction
        let gsspsp = -(gsspg + gsspdp + gsspb + gsspe); // KCL balance

        if let Some(s) = sp {
            if let Some(gate) = g {
                matrix.add(s, gate, gsspg);
            }
            if let Some(d) = dp {
                matrix.add(s, d, gsspdp);
            }
            if let Some(bi) = b {
                matrix.add(s, bi, gsspb);
            }
            matrix.add(s, s, gsspsp);
        }
    }

    // Body node combined derivatives (ngspice gbb*: lines 3908-3914)
    // These are the NEGATED body current derivatives (since rhs -= ceqbody)
    {
        let gbbg = m * (-comp.gbgs);
        let gbbdp = m * (-comp.gbds);
        let gbbb = m * (-comp.gbbs);
        let gbbe = m * (-comp.gbes);
        let gbbsp = -(gbbg + gbbdp + gbbb + gbbe); // KCL balance

        if let Some(bi) = b {
            if let Some(gate) = g {
                matrix.add(bi, gate, gbbg);
            }
            if let Some(d) = dp {
                matrix.add(bi, d, gbbdp);
            }
            matrix.add(bi, bi, gbbb);
            if let Some(ei) = e {
                matrix.add(bi, ei, gbbe);
            }
            if let Some(s) = sp {
                matrix.add(bi, s, gbbsp);
            }
        }
    }

    // --- RHS current source stamps ---
    // Matches ngspice b3soiddld.c lines 4011-4016 structure.
    // ceq_d (cdreq) has `sign` (model type) inside — no extra sign needed.
    // Junction and body CEQs are computed WITHOUT type sign in the companion,
    // but NEGATED for PMOS in the stamping section (b3soiddld.c lines 4001-4008:
    // ceqbs=-ceqbs, ceqbd=-ceqbd, ceqbody=-ceqbody for type<0).
    let sign = inst.model.mos_type.sign();
    let ceq_d = m * comp.ceq_d;
    let ceq_jd = sign * m * comp.ceq_jd;
    let ceq_js = sign * m * comp.ceq_js;
    let ceq_body = sign * m * comp.ceq_body;

    // ngspice: rhs[dNodePrime] += ceqbd - cdreq
    if let Some(d) = dp {
        rhs[d] += ceq_jd - ceq_d;
    }
    // ngspice: rhs[sNodePrime] += cdreq + ceqbs
    if let Some(s) = sp {
        rhs[s] += ceq_d + ceq_js;
    }
    // ngspice: rhs[bNode] -= ceqbody (where ceqbody = -cbody)
    // Our ceq_body = cbody, so rhs[b] -= -ceq_body = rhs[b] += ceq_body
    // But ngspice uses ceqbody = -cbody and then rhs -= ceqbody = rhs += cbody
    if let Some(bulk) = b {
        rhs[bulk] -= -ceq_body;
    }

    // --- Body resistance to external body contact (if present) ---
    if let (Some(b_int), Some(b_ext)) = (inst.body_int_idx, inst.body_idx) {
        let gbody = if inst.model.rbody > 0.0 {
            m / inst.model.rbody
        } else {
            m * 1e3
        };
        crate::stamp_conductance(matrix, Some(b_int), Some(b_ext), gbody);
    }

    // --- Series resistance: D<->D', S<->S' ---
    if inst.drain_prime_idx.is_some() && inst.drain_prime_idx != inst.drain_idx {
        let rd = if inst.nrd > 0.0 && inst.model.rbsh > 0.0 {
            inst.model.rbsh * inst.nrd
        } else {
            0.01
        };
        crate::stamp_conductance(matrix, inst.drain_idx, inst.drain_prime_idx, m / rd);
    }
    if inst.source_prime_idx.is_some() && inst.source_prime_idx != inst.source_idx {
        let rs = if inst.nrs > 0.0 && inst.model.rbsh > 0.0 {
            inst.model.rbsh * inst.nrs
        } else {
            0.01
        };
        crate::stamp_conductance(matrix, inst.source_idx, inst.source_prime_idx, m / rs);
    }
}

/// BSIM3SOI-DD voltage limiting for NR convergence.
///
/// `is_dc`: true during DC operating-point analysis, false during transient.
/// SmartVbs (clamp Vbs >= 0 for floating body) only applies during DC,
/// matching ngspice B3SOIDDSmartVbs which checks `CKTmode & (MODEDC | MODEDCOP)`.
pub fn bsim3soi_dd_limit(
    vgs_new: f64,
    vds_new: f64,
    vbs_new: f64,
    ves_new: f64,
    vgs_old: f64,
    vds_old: f64,
    vbs_old: f64,
    ves_old: f64,
    vth: f64,
    floating_body: bool,
    is_dc: bool,
) -> (f64, f64, f64, f64) {
    let vgs = crate::bsim3::fetlim(vgs_new, vgs_old, vth);
    let vds = crate::bsim3::fetlim(vds_new, vds_old, vth);
    // Body voltage limiting: ±0.2V per iteration (matching ngspice B3SOIDDlimit)
    let limit_b = 0.2;
    let vbs = if (vbs_new - vbs_old).abs() > limit_b {
        if vbs_new > vbs_old {
            vbs_old + limit_b
        } else {
            vbs_old - limit_b
        }
    } else {
        vbs_new
    };
    // SmartVbs: for floating body in DC only, Vbs cannot be negative.
    // ngspice B3SOIDDSmartVbs: only applies when CKTmode & (MODEDC | MODEDCOP).
    // During transient, the body potential can legitimately go negative.
    let vbs = if floating_body && is_dc {
        vbs.max(0.0)
    } else {
        vbs
    };
    let limit_e = 3.0;
    let ves = if (ves_new - ves_old).abs() > limit_e {
        if ves_new > ves_old {
            ves_old + limit_e
        } else {
            ves_old - limit_e
        }
    } else {
        ves_new
    };
    (vgs, vds, vbs, ves)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dd_model_defaults() {
        let model = Bsim3SoiDdModel::new(MosfetType::Nmos);
        assert_eq!(model.mos_type, MosfetType::Nmos);
        assert!((model.vth0 - 0.7).abs() < 1e-10);
        assert!(model.cox > 0.0);
        assert!(model.phi > 0.0);
        assert!(model.cbox > 0.0);
        assert!(model.csi > 0.0);
        assert!(model.qsi > 0.0);
    }

    #[test]
    fn test_dd_model_pmos_defaults() {
        let model = Bsim3SoiDdModel::new(MosfetType::Pmos);
        assert_eq!(model.mos_type, MosfetType::Pmos);
        assert!((model.vth0 - (-0.7)).abs() < 1e-10);
    }

    #[test]
    fn test_dd_size_dep_param() {
        let model = Bsim3SoiDdModel::new(MosfetType::Nmos);
        let sp = model.size_dep_param(10e-6, 0.25e-6, TEMP_DEFAULT);
        assert!(sp.leff > 0.0);
        assert!(sp.weff > 0.0);
        assert!(sp.u0 > 0.0);
        assert!(sp.vsat > 0.0);
    }

    #[test]
    fn test_dd_companion_zero_bias() {
        let model = Bsim3SoiDdModel::new(MosfetType::Nmos);
        let sp = model.size_dep_param(10e-6, 0.25e-6, TEMP_DEFAULT);
        let comp = bsim3soi_dd_companion(0.0, 0.0, 0.0, 0.0, &sp, &model);
        // At zero bias, Ids should be very small (subthreshold)
        assert!(comp.ids.abs() < 1e-3);
        assert!(comp.gm.is_finite());
        assert!(comp.gds.is_finite());
    }

    #[test]
    fn test_dd_companion_on_state() {
        let model = Bsim3SoiDdModel::new(MosfetType::Nmos);
        let sp = model.size_dep_param(10e-6, 0.25e-6, TEMP_DEFAULT);
        let comp = bsim3soi_dd_companion(1.5, 1.0, 0.0, 0.0, &sp, &model);
        // In strong inversion with positive Vds, should have meaningful current
        assert!(comp.ids > 0.0);
        assert!(comp.gm > 0.0);
        assert!(comp.gds > 0.0);
    }

    #[test]
    fn test_dd_companion_reverse() {
        let model = Bsim3SoiDdModel::new(MosfetType::Nmos);
        let sp = model.size_dep_param(10e-6, 0.25e-6, TEMP_DEFAULT);
        let comp = bsim3soi_dd_companion(1.5, -0.5, 0.0, 0.0, &sp, &model);
        assert_eq!(comp.mode, -1);
        assert!(comp.ids.is_finite());
    }

    #[test]
    fn test_dd_voltage_limiting() {
        let (vgs, vds, vbs, ves) =
            bsim3soi_dd_limit(10.0, 10.0, 10.0, 10.0, 0.5, 0.5, 0.0, 0.0, 0.7, false, true);
        // VGS and VDS should be limited
        assert!(vgs < 10.0);
        assert!(vds < 10.0);
        // VBS and VES limited to ±5V from old values
        assert!(vbs <= 5.0);
        assert!(ves <= 5.0);
    }
}
