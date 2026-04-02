//! BSIM3SOI-DD (Double Diffused Silicon-On-Insulator) MOSFET model.
//!
//! Implements the BSIM3SOI-DD v2.0 model matching ngspice level 56.
//! DD is a hybrid of PD and FD: it uses the FD-style self-consistent surface
//! potential chain (Vbs0t→Vbs0→Vbs0mos→Vthfd→Vbs0eff→Vbsmos→Vbseff) combined
//! with the PD-style 4-component junction diode model and GIDL currents.
//! Impact ionization uses ALPHA0/ALPHA1/BETA0 + AII/BII/CII/DII parameters.

#![allow(unused_variables, dead_code, clippy::too_many_arguments, unused_parens)]

use thevenin_types::{Expr, ModelDef};

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

    // GIDL
    pub igidl: f64,
    pub ggidl_d: f64,
    pub ggidl_g: f64,
    pub isgidl: f64,
    pub gsgidl_g: f64,

    // Equivalent current sources for NR companion
    pub ceq_d: f64,
    pub ceq_bs: f64,
    pub ceq_bd: f64,
    pub ceq_iii: f64,
    pub ceq_gidl: f64,
    pub ceq_sgidl: f64,

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
            adice: 1.0,
        };
        m.precompute();
        m
    }

    pub fn from_model_def(def: &ModelDef) -> Self {
        let mos_type = match def.kind.to_uppercase().as_str() {
            "PMOS" => MosfetType::Pmos,
            _ => MosfetType::Nmos,
        };
        let mut m = Self::new(mos_type);

        fn pf(def: &ModelDef, name: &str) -> Option<f64> {
            def.params.iter().find_map(|p| {
                if p.name.eq_ignore_ascii_case(name) {
                    if let Expr::Num(v) = &p.value {
                        Some(*v)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
        }

        macro_rules! set {
            ($field:ident, $name:expr) => {
                if let Some(v) = pf(def, $name) {
                    m.$field = v;
                }
            };
        }
        macro_rules! seti {
            ($field:ident, $name:expr) => {
                if let Some(v) = pf(def, $name) {
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
        // Processed adice: adice0 / (1 + Cboxt/Cox)
        let cboxt = self.cbox * self.csi / (self.cbox + self.csi);
        self.adice = self.adice0 / (1.0 + cboxt / self.cox);
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
    let (abulk0, _dabulk0_dvb, abulk, dabulk_dvg, dabulk_dvb) = {
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
    let vdsatii = if model.aii > 0.0 {
        let t0_cii = if model.cii != 0.0 {
            let t0_lim = model.cii / 3.0_f64.sqrt() + model.dii;
            let t1_lim = vds_i - t0_lim - 0.1;
            let t2_lim = (t1_lim * t1_lim + 0.4).sqrt();
            let t3_lim = t0_lim + 0.5 * (t1_lim + t2_lim);
            let t4_lim = t3_lim - model.dii;
            let t5_cii = model.cii / t4_lim;
            t5_cii * t5_cii
        } else {
            0.0
        };
        let t0 = t0_cii + 1.0;
        let t3 = model.aii + model.bii / sp.leff;
        let t4 = 1.0 / (t0 * vgsteff + t3 * esat_l);
        esat_l * vgsteff * t4
    } else {
        vdsat
    };

    // Effective Vdsii: smooth clamp Vdseffii ≈ min(Vdsatii, Vds)
    // (b3soiddld.c lines 1847-1866)
    let t1_ii = vdsatii - vds_i - sp.delta;
    let t2_ii_val = (t1_ii * t1_ii + 4.0 * sp.delta * vdsatii).sqrt();
    let vdseffii = vdsatii - 0.5 * (t1_ii + t2_ii_val);
    let diff_vdsii = vds_i - vdseffii;

    // Impact ionization (DD: b3soiddld.c lines 2156-2200)
    // Uses diffVdsii = Vds - Vdseffii (excess drain voltage beyond saturation)
    // as the electric field driving impact ionization, not Vds - beta0.
    let t2_alpha = model.alpha1 + sp.alpha0 / sp.leff;
    let (iii, gii_d, gii_g, gii_b) = if t2_alpha <= 0.0 || sp.beta0 <= 0.0 {
        (0.0, 0.0, 0.0, 0.0)
    } else if diff_vdsii > sp.beta0 / EXP_THRESHOLD {
        let t0 = -sp.beta0 / diff_vdsii;
        let t1 = t2_alpha * diff_vdsii * t0.exp();
        let iii = t1 * ids;
        // Simplified derivatives: dIii/dV ≈ Iii/Ids * dIds/dV
        // (full ngspice uses decomposed Gm0/Gds0/Gmb0 with chain rule)
        let t3 = t1 / diff_vdsii * (t0 - 1.0);
        let gii_d = t1 * gds - t3 * ids;
        let gii_g = t1 * gm;
        let gii_b = t1 * gmbs;
        (iii, gii_d, gii_g, gii_b)
    } else if diff_vdsii > 0.0 {
        let t3_min = t2_alpha * MIN_EXP;
        let t1 = t3_min * diff_vdsii;
        let iii = t1 * ids;
        let gii_d = t3_min * ids + t1 * gds;
        let gii_g = t1 * gm;
        let gii_b = t1 * gmbs;
        (iii, gii_d, gii_g, gii_b)
    } else {
        (0.0, 0.0, 0.0, 0.0)
    };

    // Add BJT collector current to drain current and its derivatives to
    // output conductances, matching ngspice b3soiddld.c lines 2566-2572:
    //   cdrain = Ids + Ic; gds = Gds + Gcd; gmbs = Gmb + Gcb
    let ids = ids + ic;
    let gds = gds + gcd;
    let gmbs = gmbs + gcb;

    // Equivalent current sources for NR companion model
    let ceq_d = sign * (ids - gm * vgs_i - gds * vds_i - gmbs * vbs_i - gme * ves_i);
    let ceq_bs = ibs - gbs_jct * vbs_i - gjsd * vds_i;
    let ceq_bd = ibd - gbd_jct * vbd - gjdd_extra * vds_i;
    let ceq_iii = iii - gii_d * vds_i - gii_g * vgs_i - gii_b * vbs_i;
    let ceq_gidl = igidl - ggidl_d * vds_i - ggidl_g * vgs_i;
    let ceq_sgidl = isgidl - gsgidl_g * vgs_i;

    // Capacitances
    let cox_wl = cox * weff_ch * leff;
    let (cggb, cgdb, cgsb) = if vgsteff > 0.0 {
        let t0 = 1.0 - abulk * vdseff / (2.0 * vgst2vtm);
        (cox_wl * (1.0 - t0 * t0), -cox_wl * t0 * 0.5, 0.0)
    } else {
        (cox_wl * 0.05, 0.0, 0.0)
    };
    let cbgb = 0.0;
    let cbdb = 0.0;
    let cbsb = 0.0;
    let cdgb = -cggb - cgdb;
    let cddb = -cgdb;
    let cdsb = -(cdgb + cddb);

    let qinv = if vgsteff > 0.0 {
        cox_wl * vgsteff * (1.0 - 0.5 * abulk * vdseff / vgst2vtm)
    } else {
        0.0
    };

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
        igidl,
        ggidl_d,
        ggidl_g,
        isgidl,
        gsgidl_g,
        ceq_d,
        ceq_bs,
        ceq_bd,
        ceq_iii,
        ceq_gidl,
        ceq_sgidl,
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
    let gbd = m * comp.gbd_jct;
    let gbs = m * comp.gbs_jct;

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

    // --- Junction conductances (symmetric two-terminal stamps) ---
    // BD junction: gbd between body and drain-prime
    crate::stamp_conductance(matrix, b, dp, gbd);
    // BS junction: gbs between body and source-prime
    crate::stamp_conductance(matrix, b, sp, gbs);

    // --- Junction Vds cross-coupling (ngspice Gjsd/Gjdd) ---
    // The BJT base transport factor BjtA depends on Vds, creating cross-coupling
    // derivatives that go beyond the two-terminal Vbd/Vbs junction model.
    // ngspice b3soiddld.c lines 2472, 2477: Gjsd = dIbs3/dVd, Gjdd includes dIbd/dVd.
    // The stamp_conductance above handles the Vbd chain rule part; these stamps add
    // the extra BjtA-dependent terms.
    let gjsd = m * comp.gjsd;
    let gjdd_extra = m * comp.gjdd_extra;
    if gjsd != 0.0 || gjdd_extra != 0.0 {
        // Source junction cross-coupling: dIbs/dVd = gjsd
        // Ibs flows sp→b, so at sp: outgoing, at b: incoming
        if let Some(s) = sp {
            if let Some(d) = dp {
                matrix.add(s, d, -gjsd);
            }
            matrix.add(s, s, gjsd);
        }
        // Drain junction extra cross-coupling: dIbd/dVd extra = gjdd_extra
        // Ibd flows dp→b, so at dp: outgoing, at b: incoming
        if let Some(d) = dp {
            matrix.add(d, d, -gjdd_extra);
            if let Some(s) = sp {
                matrix.add(d, s, gjdd_extra);
            }
        }
        // Body node: junction current enters body, Vds coupling
        if let Some(bi) = b {
            if let Some(d) = dp {
                matrix.add(bi, d, gjsd + gjdd_extra);
            }
            if let Some(s) = sp {
                matrix.add(bi, s, -(gjsd + gjdd_extra));
            }
        }
    }

    // Floating-body stability: add Gmin body-to-source coupling (matching ngspice
    // b3soiddld.c line 4090: Gmin = CKTgmin * 1e-6).  ngspice uses a much smaller
    // Gmin at the body node than the circuit-level gmin to avoid dominating the
    // extremely small floating-body junction currents.
    if inst.body_idx.is_none() {
        crate::stamp_conductance(matrix, b, sp, gmin * 1e-6);
    }

    // Impact ionization: Iii flows drain→body (OUT of drain, INTO body).
    // Iii depends on Vds, Vgs, Vbs — all relative to source-prime.
    // Each row must sum to zero (KCL), requiring SP-column balancing entries
    // (ngspice: gddpsp/gbbsp include these via combined derivative sums).
    if comp.iii != 0.0 {
        let gii_d = m * comp.gii_d;
        let gii_g = m * comp.gii_g;
        let gii_b = m * comp.gii_b;
        let gii_sum = gii_d + gii_g + gii_b;
        if let (Some(d), Some(bi)) = (dp, b) {
            matrix.add(d, d, gii_d);
            matrix.add(bi, d, -gii_d);
        }
        if let Some(gate) = g {
            if let Some(d) = dp {
                matrix.add(d, gate, gii_g);
            }
            if let Some(bi) = b {
                matrix.add(bi, gate, -gii_g);
            }
        }
        if let Some(bi) = b {
            matrix.add(bi, bi, -gii_b);
            if let Some(d) = dp {
                matrix.add(d, bi, gii_b);
            }
        }
        // KCL-balancing SP column entries (row sums must be zero).
        if let Some(s) = sp {
            if let Some(d) = dp {
                matrix.add(d, s, -gii_sum);
            }
            if let Some(bi) = b {
                matrix.add(bi, s, gii_sum);
            }
        }
    }

    // GIDL drain-side: Igidl flows drain→body.
    // Igidl depends on Vds, Vgs — relative to source-prime.
    if comp.igidl != 0.0 {
        let ggidl_d = m * comp.ggidl_d;
        let ggidl_g = m * comp.ggidl_g;
        let ggidl_sum = ggidl_d + ggidl_g;
        if let (Some(d), Some(bi)) = (dp, b) {
            matrix.add(d, d, ggidl_d);
            matrix.add(bi, d, -ggidl_d);
        }
        if let Some(gate) = g {
            if let Some(d) = dp {
                matrix.add(d, gate, ggidl_g);
            }
            if let Some(bi) = b {
                matrix.add(bi, gate, -ggidl_g);
            }
        }
        // KCL-balancing SP column entries.
        if let Some(s) = sp {
            if let Some(d) = dp {
                matrix.add(d, s, -ggidl_sum);
            }
            if let Some(bi) = b {
                matrix.add(bi, s, ggidl_sum);
            }
        }
    }

    // GIDL source-side: Isgidl flows source→body.
    // Isgidl depends on Vgs — relative to source-prime.
    if comp.isgidl != 0.0 {
        let gsgidl_g = m * comp.gsgidl_g;
        if let Some(gate) = g {
            if let Some(s) = sp {
                matrix.add(s, gate, gsgidl_g);
            }
            if let Some(bi) = b {
                matrix.add(bi, gate, -gsgidl_g);
            }
        }
        // KCL-balancing SP column entries.
        if let Some(s) = sp {
            matrix.add(s, s, -gsgidl_g);
            if let Some(bi) = b {
                matrix.add(bi, s, gsgidl_g);
            }
        }
    }

    // --- RHS current source stamps ---
    // comp.ceq_d already has `sign` inside (ngspice: cdreq = type * (...)),
    // so we multiply by `m` only. Junction, impact ionization, and GIDL ceqs
    // are NOT type-signed in ngspice BSIM3SOI-DD (b3soiddld.c lines 2602-2630).
    let ceq_d = m * comp.ceq_d;
    let ceq_bs = m * comp.ceq_bs;
    let ceq_bd = m * comp.ceq_bd;
    let ceq_iii = m * comp.ceq_iii;
    let ceq_gidl = m * comp.ceq_gidl;
    let ceq_sgidl = m * comp.ceq_sgidl;

    if let Some(d) = dp {
        rhs[d] -= ceq_d - ceq_bd + ceq_iii + ceq_gidl;
    }
    if let Some(s) = sp {
        rhs[s] += ceq_d + ceq_bs - ceq_sgidl;
    }
    if let Some(bulk) = b {
        rhs[bulk] -= ceq_bs + ceq_bd - ceq_iii - ceq_gidl - ceq_sgidl;
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
    // SmartVbs: for floating body in DC, Vbs cannot be negative.
    // Prevents body from going below source which would cause oscillation.
    let vbs = if floating_body { vbs.max(0.0) } else { vbs };
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
            bsim3soi_dd_limit(10.0, 10.0, 10.0, 10.0, 0.5, 0.5, 0.0, 0.0, 0.7, false);
        // VGS and VDS should be limited
        assert!(vgs < 10.0);
        assert!(vds < 10.0);
        // VBS and VES limited to ±5V from old values
        assert!(vbs <= 5.0);
        assert!(ves <= 5.0);
    }
}
