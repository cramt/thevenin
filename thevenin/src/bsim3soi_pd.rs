//! BSIM3SOI-PD (Partially Depleted Silicon-On-Insulator) MOSFET model.
//!
//! Implements the BSIM3SOI-PD v2.2.3 model matching ngspice level 57.
//! Based on the BSIM3v3 core equations with SOI-specific extensions:
//! floating body, back-gate (E node), optional body contact (B node),
//! SOI junction diode model (4 components), and self-heating (disabled via SHMOD=0).

#![allow(unused_variables, dead_code, clippy::too_many_arguments, unused_parens)]

use crate::model_params::ModelParams;

use crate::mosfet::MosfetType;
use crate::physics::{
    CHARGE_Q, EG300, EPSOX, EPSSI, EXP_THRESHOLD, EXPL_THRESHOLD, KBOQ, MAX_EXP, MIN_EXP, MIN_EXPL,
    soi_dexp,
};

const DELTA_1: f64 = 0.02;
const DELTA_3_SOI: f64 = 0.08;
const DELTA_4: f64 = 0.02;
const DELT_VBSEFF: f64 = 0.005;

const TEMP_DEFAULT: f64 = 300.15;

/// BSIM3SOI-PD model parameters (from .model card, Level=57).
#[derive(Debug, Clone)]
pub struct Bsim3SoiPdModel {
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
    pub xj: f64,

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
    pub ketas: f64,

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

    // Substrate current / impact ionization
    pub alpha0: f64,
    pub beta0: f64,
    pub beta1: f64,
    pub beta2: f64,
    pub vdsatii0: f64,
    pub esatii: f64,
    pub sii0: f64,
    pub sii1: f64,
    pub sii2: f64,
    pub siid: f64,
    pub lii: f64,

    // Temperature
    pub tnom: f64,
    pub kt1: f64,
    pub kt1l: f64,
    pub kt2: f64,

    // SOI junction model
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
    pub ldif0: f64,
    pub aely: f64,
    pub vabjt: f64,

    // GIDL
    pub agidl: f64,
    pub bgidl: f64,
    pub ngidl: f64,

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
    pub tcjswg: f64,
    pub tt: f64,
    pub csdesw: f64,
    pub asd: f64,

    // Self-heating
    pub rth0: f64,
    pub cth0: f64,

    // Precomputed
    pub cox: f64,
    pub vtm: f64,
    pub phi: f64,
    pub sqrt_phi: f64,
    pub vbi_default: f64,
    pub factor1: f64,
    pub ni: f64,
    pub eg: f64,
}

/// Size-dependent parameters for BSIM3SOI-PD.
#[derive(Debug, Clone)]
pub struct Bsim3SoiPdSizeParam {
    pub leff: f64,
    pub weff: f64,
    pub leff_cv: f64,
    pub weff_cv: f64,

    // Binned core parameters (no L/W binning for now, just base values)
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

    // nseg (number of body segments)
    pub nseg: f64,
}

/// BSIM3SOI-PD instance with node indices.
#[derive(Debug, Clone)]
pub struct Bsim3SoiPdInstance {
    pub name: String,
    pub drain_idx: Option<usize>,
    pub gate_idx: Option<usize>,
    pub source_idx: Option<usize>,
    /// Back-gate (E) node — always present for SOI.
    pub e_idx: Option<usize>,
    /// External body contact (B/P) node — optional (floating body when None).
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
    pub model: Bsim3SoiPdModel,
    pub size_params: Bsim3SoiPdSizeParam,
    pub vth0_inst: f64,
    /// Number of body contacts (0=floating, 1=single, 2=double)
    pub nbc: f64,
}

/// NR companion result for BSIM3SOI-PD.
#[derive(Debug, Clone)]
pub struct Bsim3SoiPdCompanion {
    pub ids: f64,
    pub gm: f64,
    pub gds: f64,
    pub gmbs: f64,
    pub mode: i32,
    pub vdsat: f64,

    // Junction currents and conductances (body node KCL)
    pub ibs: f64,
    pub ibd: f64,
    pub gbs_jct: f64,
    pub gbd_jct: f64,

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
    /// Impact ionization equivalent current: iii - gii_d*vds - gii_g*vgs - gii_b*vbs
    pub ceq_iii: f64,
    /// GIDL drain-side equivalent current: igidl - ggidl_d*vds - ggidl_g*vgs
    pub ceq_gidl: f64,
    /// GIDL source-side equivalent current: isgidl - gsgidl_g*vgs
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
}

impl Bsim3SoiPdModel {
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
            tox: 150e-10,
            tsi: 1e-7,
            tbox: 8e-8,
            vth0: vth0_default,
            k1: 0.5,
            k2: 0.0,
            k3: 80.0,
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
            xj: -1.0, // sentinel; will default to tsi in precompute
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
            keta: -0.047,
            ketas: 0.0,
            pclm: 1.3,
            pdiblc1: 0.39,
            pdiblc2: 0.0086,
            pdiblcb: 0.0,
            drout: 0.56,
            pvag: 0.0,
            delta: 0.01,
            rdsw: 0.0,
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
            beta0: 30.0,
            beta1: 0.0,
            beta2: 0.1,
            vdsatii0: 0.9,
            esatii: 1e7,
            sii0: 0.5,
            sii1: 0.1,
            sii2: 0.0,
            siid: 0.0,
            lii: 0.0,
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
            isrec: 0.0,
            xbjt: 1.0,
            xdif: 1.0,
            xrec: 1.0,
            xtun: 0.0,
            ahli: 0.0,
            lbjt0: 0.2e-6,
            ln: 2e-6,
            nbjt: 1.0,
            ndif: -1.0,
            ldif0: 1.0,
            aely: 0.0,
            vabjt: 10.0,
            agidl: 0.0,
            bgidl: 2.3e9,
            ngidl: 1.2,
            rbody: 0.0,
            rbsh: 0.0,
            cgso: 0.0,
            cgdo: 0.0,
            clc: 1e-7,
            cle: 0.6,
            cf: 0.0,
            ckappa: 0.6,
            cgdl: 0.0,
            cgsl: 0.0,
            cjswg: 0.0,
            mjswg: 0.5,
            pbswg: 1.0,
            tcjswg: 0.0,
            tt: 0.0,
            csdesw: 0.0,
            asd: 0.3,
            rth0: 0.0,
            cth0: 0.0,
            cox: 0.0,
            vtm: 0.0,
            phi: 0.0,
            sqrt_phi: 0.0,
            vbi_default: 0.0,
            factor1: 0.0,
            ni: 0.0,
            eg: 0.0,
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
        // NCH and NPEAK are aliases in ngspice (IOP "nch" -> B3SOIPD_MOD_NPEAK).
        set!(nch, "NCH");
        set!(npeak, "NPEAK");
        if m.nch != 1.7e17 && m.npeak == 1.7e17 {
            m.npeak = m.nch;
        } else if m.npeak != 1.7e17 && m.nch == 1.7e17 {
            m.nch = m.npeak;
        }
        set!(ngate, "NGATE");
        set!(nsub, "NSUB");
        set!(xj, "XJ");
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
        set!(ketas, "KETAS");
        set!(pclm, "PCLM");
        if let Some(v) = pf(model, "PDIBLC1") {
            m.pdiblc1 = v;
        }
        if let Some(v) = pf(model, "PDIBLC2") {
            m.pdiblc2 = v;
        }
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
        set!(beta0, "BETA0");
        set!(beta1, "BETA1");
        set!(beta2, "BETA2");
        set!(vdsatii0, "VDSATII0");
        set!(sii0, "SII0");
        set!(sii1, "SII1");
        set!(sii2, "SII2");
        set!(siid, "SIID");
        set!(lii, "LII");
        set!(esatii, "ESATII");
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
        set!(ldif0, "LDIF0");
        set!(aely, "AELY");
        set!(vabjt, "VABJT");
        set!(agidl, "AGIDL");
        set!(bgidl, "BGIDL");
        set!(ngidl, "NGIDL");
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
        set!(tcjswg, "TCJSWG");
        set!(tt, "TT");
        set!(csdesw, "CSDESW");
        set!(asd, "ASD");
        set!(rth0, "RTH0");
        set!(cth0, "CTH0");

        // Handle u0 units: ngspice treats u0 > 1 as cm²/Vs, converts by /1e4
        if m.u0 > 1.0 {
            m.u0 /= 1e4;
        }

        m.precompute();
        m
    }

    fn precompute(&mut self) {
        // XJ defaults to tsi in BSIM3SOI-PD (ngspice b3soipdset.c)
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
            self.npeak * 1e-6 // Convert from cm⁻³ to m⁻³ if in SI
        } else {
            self.npeak
        };

        self.phi = 2.0 * self.vtm * (npeak / self.ni).ln();
        if self.phi < 0.4 {
            self.phi = 0.4;
        }
        self.sqrt_phi = self.phi.sqrt();

        // vbi = Vt * ln(ND * NA / ni²), ND = 1e20 /cm³ (n+ S/D), NA = npeak /cm³.
        self.vbi_default = self.vtm * (1e20 * npeak / (self.ni * self.ni)).ln();
        self.factor1 = (EPSSI / EPSOX * self.tox).sqrt();
    }

    /// Number of internal nodes this model creates.
    /// Drain/source prime nodes only created when sheet resistance (RBSH) and
    /// drain/source squares (NRD/NRS) are both positive — matching ngspice
    /// b3soipdset.c which checks `sheetResistance > 0 && drainSquares > 0`.
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

    pub fn size_dep_param(&self, w: f64, l: f64, temp: f64) -> Bsim3SoiPdSizeParam {
        let tnom_k = self.tnom + 273.15;
        let vtm = KBOQ * temp;

        // Effective length/width
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

        // Temperature ratio
        let temp_ratio = temp / tnom_k;
        // ngspice b3soipdtemp.c line 624: T0 = (TempRatio - 1.0)
        // All temperature coefficient terms use (TRatio - 1.0), not (T - Tnom)
        let t_ratio_minus1 = temp_ratio - 1.0;

        // Mobility temperature dependence
        let u0temp = self.u0 * temp_ratio.powf(self.ute);

        // Velocity saturation temperature dependence
        let vsattemp = self.vsat - self.at * t_ratio_minus1;

        // Series resistance
        let rds0 = if self.rdsw > 0.0 {
            // ngspice b3soipdtemp.c: rds0 = (rdsw + prt*T0) / pow(weff*1e6, wr)
            (self.rdsw + self.prt * t_ratio_minus1) / (weff * 1e6).powf(self.wr)
        } else {
            0.0
        };

        // ua, ub, uc temperature dependence
        let ua = self.ua + self.ua1 * t_ratio_minus1;
        let ub = self.ub + self.ub1 * t_ratio_minus1;
        let uc = self.uc + self.uc1 * t_ratio_minus1;

        // Xdep0
        let xdep0 = (2.0 * EPSSI / (CHARGE_Q * self.npeak * 1e6)).sqrt() * sqrt_phi;

        // litl: ngspice b3soipdtemp.c line 755: sqrt(3.0 * xj * tox)
        // ngspice uses hardcoded 3.0 (not EPSSI/EPSOX ≈ 2.99934).
        let litl = (3.0 * self.xj * self.tox).sqrt();

        // Characteristic length for DIBL (theta0vb0) and PDIBL (theta_rout).
        // ngspice b3soipdtemp.c: T1 = sqrt(EPSSI / EPSOX * tox * Xdep0)
        let t1_soi = (EPSSI * xdep0 / self.cox).sqrt();

        // theta0vb0: ngspice uses dsub (NOT dvt1) and does NOT multiply by dvt0.
        // b3soipdtemp.c lines 844-845.
        let t0 = -0.5 * self.dsub * leff / t1_soi;
        let theta0vb0 = if t0 > -EXP_THRESHOLD {
            let t1 = t0.exp();
            t1 + 2.0 * t1 * t1
        } else {
            MIN_EXP + 2.0 * MIN_EXP * MIN_EXP
        };

        // theta_rout for VADIBL: ngspice uses drout with same characteristic length.
        // b3soipdtemp.c lines 847-850.
        let t0 = -0.5 * self.drout * leff / t1_soi;
        let theta_rout = if t0 > -EXP_THRESHOLD {
            let t1 = t0.exp();
            self.pdiblc1 * (t1 + 2.0 * t1 * t1) + self.pdiblc2
        } else {
            self.pdiblc2
        };

        // k1eff (from k1, or compute from nch if not given)
        let k1eff = self.k1;

        // vfb: ngspice b3soipdtemp.c lines 831-834: vfb = type * VTH0 - phi - k1eff * sqrtPhi
        let sign = self.mos_type.sign();
        let vfb = sign * self.vth0 - phi - k1eff * sqrt_phi;

        // cdep0: ngspice b3soipdtemp.c line 759: sqrt(q * EPSSI * npeak * 1e6 / 2.0 / phi)
        let cdep0 = (CHARGE_Q * EPSSI * self.npeak * 1e6 / 2.0 / phi).sqrt();

        // V0 = vbi - phi
        let eg = 1.16 - 7.02e-4 * temp * temp / (temp + 1108.0);
        let ni_temp = 1.45e10
            * (temp / 300.15)
            * (temp / 300.15).sqrt()
            * (21.5565981 - eg / (2.0 * vtm)).exp();
        let vbi = vtm * (1e20 * self.npeak / (ni_temp * ni_temp)).abs().ln();

        // SOI junction parameters — matching ngspice b3soipdtemp.c lines 683-700
        // PD uses DEXP formula: jrec = isrec * exp(xrec * Eg300/(vtm0*nrecf0) * (TRatio-1))
        let vtm_tnom = KBOQ * tnom_k;
        let t4_temp = EG300 / vtm_tnom * (temp_ratio - 1.0);

        let t7_bjt = self.xbjt * t4_temp / self.ndiode;
        let t7_dif = self.xdif * t4_temp / self.ndiode;
        let t7_rec = self.xrec * t4_temp / self.nrecf0;
        let t7_tun = self.xtun * (temp_ratio - 1.0);

        let jbjt = self.isbjt * t7_bjt.exp();
        let jdif = self.isdif * t7_dif.exp();
        let jrec = self.isrec * t7_rec.exp();
        let jtun = self.istun * t7_tun.exp();

        // Diode widths: ngspice b3soipdtemp.c line 181-182:
        //   wdiod = weff / nseg + pdbcp; wdios = weff / nseg + psbcp;
        // nseg defaults to 1, psbcp/pdbcp default to 0 (instance parameters not yet parsed).
        // NOTE: ASD is NOT used for junction current width in PD — it controls
        // the source/drain bottom diffusion capacitance smoothing (b3soipdtemp.c line 862).
        let wdios = weff;
        let wdiod = weff;

        // BJT-related ratios (ngspice b3soipdtemp.c lines 932-942)
        let ln_clamped = self.ln.max(1e-15);
        // arfabjt = exp(-0.5 * Leff^2 / LN^2) — BJT transport fraction
        let arfabjt = {
            let t0 = -0.5 * leff * leff / (ln_clamped * ln_clamped);
            soi_dexp(t0).0
        };
        // lratio = (lbjt0 * (1/Leff + 1/LN))^nbjt
        let t0_bjt = self.lbjt0 * (1.0 / leff + 1.0 / ln_clamped);
        let lratio = t0_bjt.powf(self.nbjt);
        // lratiodif = 1 + ldif0 * (lbjt0 * (1/Leff + 1/LN))^ndif
        let lratiodif = 1.0 + self.ldif0 * t0_bjt.powf(self.ndif);
        // vearly = vabjt + aely * Leff, clamped to >= 1
        let vearly = (self.vabjt + self.aely * leff).max(1.0);

        // Overlap caps
        let cgso_eff = if self.cgso > 0.0 {
            self.cgso
        } else {
            0.6 * self.dlc * self.cox
        };
        let cgdo_eff = if self.cgdo > 0.0 {
            self.cgdo
        } else {
            0.6 * self.dlc * self.cox
        };

        Bsim3SoiPdSizeParam {
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
            nseg: 1.0,
        }
    }
}

impl Bsim3SoiPdInstance {
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

    pub fn ac_stamp(&self, comp: &Bsim3SoiPdCompanion) -> crate::ac::BsimAcStamp {
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

/// Compute BSIM3SOI-PD companion model (NR linearization).
///
/// Core load function computing drain current, junction currents, conductances,
/// and capacitances at the given operating point.
#[expect(clippy::too_many_lines)]
pub fn bsim3soi_pd_companion(
    vgs: f64,
    vds: f64,
    vbs: f64,
    _ves: f64,
    sp: &Bsim3SoiPdSizeParam,
    model: &Bsim3SoiPdModel,
) -> Bsim3SoiPdCompanion {
    let sign = model.mos_type.sign();
    let cox = model.cox;
    let vtm = KBOQ * TEMP_DEFAULT;
    let phi = sp.phi;
    let sqrt_phi = sp.sqrt_phi;

    // Mode detection (forward/reverse)
    let (vgs_i, vds_i, vbs_i, mode) = if vds >= 0.0 {
        (vgs, vds, vbs, 1)
    } else {
        (vgs - vds, -vds, vbs - vds, -1)
    };

    let leff = sp.leff;
    let weff = sp.weff;

    // Poly gate depletion
    let (vgs_eff, dvgs_eff_dvg) =
        if model.ngate > 1e18 && model.ngate < 1e25 && vgs_i > (sp.vfb + phi) {
            let t1 = 1e6 * CHARGE_Q * EPSSI * model.ngate / (cox * cox);
            let t4 = (1.0 + 2.0 * (vgs_i - sp.vfb - phi) / t1).sqrt();
            let t2 = t1 * (t4 - 1.0);
            let t3 = 0.5 * t2 * t2 / t1;
            let t7 = 1.12 - t3 - 0.05;
            let t6 = (t7 * t7 + 0.224).sqrt();
            let t5 = 1.12 - 0.5 * (t7 + t6);
            (vgs_i - t5, 1.0 - (0.5 - 0.5 / t4) * (1.0 + t7 / t6))
        } else {
            (vgs_i, 1.0)
        };

    // Vbseff limiting (SOI-specific: clamp between -5 and 0.95*phi)
    // Step 1: Vbs limited above -5
    let t0 = vbs_i + 5.0 - 0.001;
    let t1 = (t0 * t0 + 0.004 * 5.0).sqrt();
    let t2 = -5.0 + 0.5 * (t0 + t1);
    let dt2_dvb = 0.5 * (1.0 + t0 / t1);

    // Step 2: Vbsh limited below 1.5
    let t0_2 = 1.5;
    let t1_2 = t0_2 - t2 - 0.002;
    let t3_2 = (t1_2 * t1_2 + 0.008 * t0_2).sqrt();
    let vbsh = t0_2 - 0.5 * (t1_2 + t3_2);
    let dvbsh_dvb = 0.5 * (1.0 + t1_2 / t3_2) * dt2_dvb;

    // Step 3: Vbseff limited to 0.95*phi
    let t0_3 = 0.95 * phi;
    let t1_3 = t0_3 - vbsh - 0.002;
    let t2_3 = (t1_3 * t1_3 + 0.008 * t0_3).sqrt();
    let vbseff = t0_3 - 0.5 * (t1_3 + t2_3);
    let mut dvbseff_dvb = 0.5 * (1.0 + t1_3 / t2_3) * dvbsh_dvb;

    if dvbseff_dvb < 1e-20 {
        dvbseff_dvb = 1e-20;
    }

    // Surface potential and depletion
    let phis = phi - vbseff;
    let sqrt_phis = phis.sqrt();
    let dsqrt_phis_dvb = -0.5 / sqrt_phis;
    let xdep = sp.xdep0 * sqrt_phis / sqrt_phi;
    let dxdep_dvb = (sp.xdep0 / sqrt_phi) * dsqrt_phis_dvb;

    // V0 = vbi - phi
    let v0 = sp.vbi - phi;

    // Vth calculation (following b3soipdld.c lines 869-984)
    let t3_vth = xdep.sqrt();

    // SCE: dvt1/dvt2 contribution to threshold
    let t0 = sp.dvt2 * vbseff;
    let (t1, t2_) = if t0 >= -0.5 {
        (1.0 + t0, sp.dvt2)
    } else {
        let t4 = 1.0 / (3.0 + 8.0 * t0);
        ((1.0 + 3.0 * t0) * t4, sp.dvt2 * t4 * t4)
    };
    let lt1 = model.factor1 * t3_vth * t1;
    let dlt1_dvb = model.factor1 * (0.5 / t3_vth * t1 * dxdep_dvb + t3_vth * t2_);

    // Width effect on Vth
    let t0w = sp.dvt2w * vbseff;
    let (t1w, t2w) = if t0w >= -0.5 {
        (1.0 + t0w, sp.dvt2w)
    } else {
        let t4 = 1.0 / (3.0 + 8.0 * t0w);
        ((1.0 + 3.0 * t0w) * t4, sp.dvt2w * t4 * t4)
    };
    let ltw = model.factor1 * t3_vth * t1w;
    let dltw_dvb = model.factor1 * (0.5 / t3_vth * t1w * dxdep_dvb + t3_vth * t2w);

    // Delt_vth (short-channel)
    let t0_sce = -0.5 * sp.dvt1 * leff / lt1;
    let (theta0, dtheta0_dvb) = if t0_sce > -EXP_THRESHOLD {
        let t1 = t0_sce.exp();
        (
            t1 * (1.0 + 2.0 * t1),
            (-t0_sce / lt1 * t1 * dlt1_dvb) * (1.0 + 4.0 * t1),
        )
    } else {
        (MIN_EXP * (1.0 + 2.0 * MIN_EXP), 0.0)
    };
    let delt_vth = sp.dvt0 * theta0 * v0;
    let ddelt_vth_dvb = sp.dvt0 * dtheta0_dvb * v0;

    // Width effect on Vth
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

    // Temperature contribution to Vth
    let temp_ratio_minus1 = (TEMP_DEFAULT / (model.tnom + 273.15)) - 1.0;
    let t0_nlx = (1.0 + sp.nlx / leff).sqrt();
    let t1_kt = sp.kt1 + sp.kt1l / leff + sp.kt2 * vbseff;
    let delt_vth_temp = sp.k1eff * (t0_nlx - 1.0) * sqrt_phi + t1_kt * temp_ratio_minus1;

    // DIBL
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

    // sqrtPhisExt correction (SOI-specific)
    let t9_ext = 2.2361 / sqrt_phi;
    let sqrt_phis_ext = sqrt_phis - t9_ext * (vbsh - vbseff);
    let dsqrt_phis_ext_dvb = dsqrt_phis_dvb - t9_ext * (dvbsh_dvb / dvbseff_dvb - 1.0);

    // Final Vth
    let vth = sign * sp.vth0 + sp.k1eff * (sqrt_phis_ext - sqrt_phi)
        - sp.k2 * vbseff
        - delt_vth
        - delt_vthw
        + (sp.k3 + sp.k3b * vbseff) * tmp2
        + delt_vth_temp
        - dibl_sft;

    let t6 = sp.k3b * tmp2 - sp.k2 + sp.kt2 * temp_ratio_minus1;
    let dvth_dvb =
        sp.k1eff * dsqrt_phis_ext_dvb - ddelt_vth_dvb - ddelt_vthw_dvb + t6 - ddibl_sft_dvb;
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

    let (vgsteff, dvgsteff_dvg, dvgsteff_dvd, dvgsteff_dvb) = if vgst_nvt > EXPL_THRESHOLD {
        let dvgsteff_dvb_raw = -dvth_dvb;
        (
            vgst,
            dvgs_eff_dvg,
            -dvth_dvd,
            dvgsteff_dvb_raw * dvbseff_dvb,
        )
    } else if exp_arg > EXPL_THRESHOLD {
        let t0 = (vgst - sp.voff) / (n * vtm);
        let exp_vgst = t0.exp();
        let vgsteff_val = vtm * sp.cdep0 / cox * exp_vgst;
        let t3 = vgsteff_val / (n * vtm);
        let t1 = -t3 * (dvth_dvb + t0 * vtm * dn_dvb);
        (
            vgsteff_val,
            t3 * dvgs_eff_dvg,
            -t3 * (dvth_dvd + t0 * vtm * dn_dvd),
            t1 * dvbseff_dvb,
        )
    } else {
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
            (t2_val * dt1_dvg - t1 * dt2_dvg) / t3 * dvgs_eff_dvg,
            (t2_val * dt1_dvd - t1 * dt2_dvd) / t3,
            t4 * dvbseff_dvb,
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
        let rds = sp.rds0 * (0.8 + t0_rds) * t1;
        let t1sq = t1 * t1;
        (
            rds,
            sp.rds0 * sp.prwg * t1sq,
            sp.rds0 * sp.prwb * dsqrt_phis_dvb * t1sq,
        )
    };

    // Abulk calculation (with SOI KETAS modification)
    let (abulk0, dabulk0_dvb, abulk, dabulk_dvg, dabulk_dvb) = if sp.a0 == 0.0 {
        (1.0, 0.0, 1.0, 0.0, 0.0)
    } else {
        // SOI uses ketas in addition to keta
        let t10_k = sp.keta * vbsh;
        let (t11, dt11_dvb) = if t10_k >= -0.9 {
            let t11 = 1.0 / (1.0 + t10_k);
            (t11, -sp.keta * t11 * t11 * dvbsh_dvb)
        } else {
            let t12 = 1.0 / (0.8 + t10_k);
            let t11 = (17.0 + 20.0 * t10_k) * t12;
            (t11, -sp.keta * t12 * t12 * dvbsh_dvb)
        };

        let t10_phi = phi + model.ketas;
        let t13 = (vbsh * t11) / t10_phi;
        let dt13_dvb = (vbsh * dt11_dvb + t11 * dvbsh_dvb) / t10_phi;

        let (t14, dt14_dvb) = if t13 < 0.96 {
            let t14 = 1.0 / (1.0 - t13).sqrt();
            let t10 = 0.5 * t14 / (1.0 - t13);
            (t14, t10 * dt13_dvb)
        } else {
            let t11 = 1.0 / (1.0 - 1.043406 * t13);
            let t14 = (6.00167 - 6.26044 * t13) * t11;
            let t10 = 0.001742 * t11 * t11;
            (t14, t10 * dt13_dvb)
        };

        let t10_k1 = 0.5 * sp.k1eff / (phi + model.ketas).sqrt();
        let t1 = t10_k1 * t14;
        let dt1_dvb = t10_k1 * dt14_dvb;

        let t9 = (model.xj * xdep).sqrt();
        let tmp1 = leff + 2.0 * t9;
        let t5 = leff / tmp1;
        let tmp2_a = sp.a0 * t5;
        let tmp3_a = weff + sp.b1;
        let tmp4_a = sp.b0 / tmp3_a;
        let t2 = tmp2_a + tmp4_a;
        let dt2_dvb = -t9 * tmp2_a / tmp1 / xdep * dxdep_dvb;
        let t6 = t5 * t5;
        let t7 = t5 * t6;

        let mut abulk0 = 1.0 + t1 * t2;
        let mut dabulk0_dvb = t1 * dt2_dvb + t2 * dt1_dvb;

        let t8 = sp.ags * sp.a0 * t7;
        let dabulk_dvg = -t1 * t8;
        let mut abulk = abulk0 + dabulk_dvg * vgsteff;
        let mut dabulk_dvb = dabulk0_dvb - t8 * vgsteff * (dt1_dvb + 3.0 * t1 * dt2_dvb / tmp2_a);

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

        (abulk0, dabulk0_dvb, abulk, dabulk_dvg, dabulk_dvb)
    };

    // Mobility
    let (ueff, dueff_dvg, dueff_dvd, dueff_dvb) = {
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
        // ngspice b3soipdld.c lines 1261-1297: full dDenomi derivatives
        if model.mob_mod == 1 {
            let t0 = vgsteff + vth + vth;
            let t2 = sp.ua + sp.uc * vbseff;
            let t3 = t0 / model.tox;
            let ddenomi_dvg = (t2 + 2.0 * sp.ub * t3) / model.tox;
            let ddenomi_dvd = ddenomi_dvg * 2.0 * dvth_dvd;
            let ddenomi_dvb = ddenomi_dvg * 2.0 * dvth_dvb + sp.uc * t3;
            (ueff, t9 * ddenomi_dvg, t9 * ddenomi_dvd, t9 * ddenomi_dvb)
        } else if model.mob_mod == 2 {
            let ddenomi_dvg =
                (sp.ua + sp.uc * vbseff + 2.0 * sp.ub * vgsteff / model.tox) / model.tox;
            let ddenomi_dvb = vgsteff * sp.uc / model.tox;
            (ueff, t9 * ddenomi_dvg, 0.0, t9 * ddenomi_dvb)
        } else {
            // mob_mod 0/3 (else)
            let t0 = vgsteff + vth + vth;
            let t2 = 1.0 + sp.uc * vbseff;
            let t3 = t0 / model.tox;
            let t4 = t3 * (sp.ua + sp.ub * t3);
            let ddenomi_dvg = (sp.ua + 2.0 * sp.ub * t3) * t2 / model.tox;
            let ddenomi_dvd = ddenomi_dvg * 2.0 * dvth_dvd;
            let ddenomi_dvb = ddenomi_dvg * 2.0 * dvth_dvb + sp.uc * t4;
            (ueff, t9 * ddenomi_dvg, t9 * ddenomi_dvd, t9 * ddenomi_dvb)
        }
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

    // Lambda
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

    let (tmp2_rds, tmp3_rds) = if rds > 0.0 {
        (
            drds_dvg / rds + dweff_dvg / weff_ch,
            drds_dvb / rds + dweff_dvb / weff_ch,
        )
    } else {
        (dweff_dvg / weff_ch, dweff_dvb / weff_ch)
    };

    if rds == 0.0 && lambda == 1.0 {
        let t0 = 1.0 / (abulk * esat_l + vgst2vtm);
        let t1 = t0 * t0;
        let t2 = vgst2vtm * t0;
        let t3 = esat_l * vgst2vtm;
        vdsat = t3 * t0;
        let dt0_dvg = -(abulk * desat_l_dvg + esat_l * dabulk_dvg + 1.0) * t1;
        let dt0_dvd = -(abulk * desat_l_dvd) * t1;
        let dt0_dvb = -(abulk * desat_l_dvb + esat_l * dabulk_dvb) * t1;
        dvdsat_dvg = t3 * dt0_dvg + t2 * desat_l_dvg + esat_l * t0;
        dvdsat_dvd = t3 * dt0_dvd + t2 * desat_l_dvd;
        dvdsat_dvb = t3 * dt0_dvb + t2 * desat_l_dvb;
    } else {
        let t9 = abulk * wvcox_rds;
        let t8 = abulk * t9;
        let t7 = vgst2vtm * t9;
        let t6 = vgst2vtm * wvcox_rds;
        let t0 = 2.0 * abulk * (t9 - 1.0 + 1.0 / lambda);
        let dt0_dvg = 2.0
            * (t8 * tmp2_rds - abulk * dlambda_dvg / (lambda * lambda)
                + (2.0 * t9 + 1.0 / lambda - 1.0) * dabulk_dvg);
        let dt0_dvb =
            2.0 * (t8 * (2.0 / abulk * dabulk_dvb + tmp3_rds) + (1.0 / lambda - 1.0) * dabulk_dvb);
        let dt0_dvd = 0.0;

        let t1 = vgst2vtm * (2.0 / lambda - 1.0) + abulk * esat_l + 3.0 * t7;
        let dt1_dvg = (2.0 / lambda - 1.0) - 2.0 * vgst2vtm * dlambda_dvg / (lambda * lambda)
            + abulk * desat_l_dvg
            + esat_l * dabulk_dvg
            + 3.0 * (t9 + t7 * tmp2_rds + t6 * dabulk_dvg);
        let dt1_dvb =
            abulk * desat_l_dvb + esat_l * dabulk_dvb + 3.0 * (t6 * dabulk_dvb + t7 * tmp3_rds);
        let dt1_dvd = abulk * desat_l_dvd;

        let t2 = vgst2vtm * (esat_l + 2.0 * t6);
        let dt2_dvg = esat_l + vgst2vtm * desat_l_dvg + t6 * (4.0 + 2.0 * vgst2vtm * tmp2_rds);
        let dt2_dvb = vgst2vtm * (desat_l_dvb + 2.0 * t6 * tmp3_rds);
        let dt2_dvd = vgst2vtm * desat_l_dvd;

        let t3 = (t1 * t1 - 2.0 * t0 * t2).sqrt();
        vdsat = (t1 - t3) / t0;
        dvdsat_dvg =
            (dt1_dvg - (t1 * dt1_dvg - dt0_dvg * t2 - t0 * dt2_dvg) / t3 - vdsat * dt0_dvg) / t0;
        dvdsat_dvb =
            (dt1_dvb - (t1 * dt1_dvb - dt0_dvb * t2 - t0 * dt2_dvb) / t3 - vdsat * dt0_dvb) / t0;
        dvdsat_dvd = (dt1_dvd - (t1 * dt1_dvd - t0 * dt2_dvd) / t3) / t0;
    }

    // Vdseff (effective Vds)
    let t1 = vdsat - vds_i - sp.delta;
    let t2 = (t1 * t1 + 4.0 * sp.delta * vdsat).sqrt();
    let t0 = t1 / t2;
    let t3 = 2.0 * sp.delta / t2;
    let vdseff = vdsat - 0.5 * (t1 + t2);
    let dvdseff_dvg = dvdsat_dvg - 0.5 * (dvdsat_dvg + t0 * dvdsat_dvg + t3 * dvdsat_dvg);
    let dvdseff_dvd =
        dvdsat_dvd - 0.5 * (dvdsat_dvd - 1.0 + t0 * (dvdsat_dvd - 1.0) + t3 * dvdsat_dvd);
    let dvdseff_dvb = dvdsat_dvb - 0.5 * (dvdsat_dvb + t0 * dvdsat_dvb + t3 * dvdsat_dvb);

    // Clamp Vdseff to Vds but keep smooth-formula derivatives (matches
    // ngspice b3soipdld.c which only clamps the value, not derivatives).
    let vdseff = if vdseff > vds_i { vds_i } else { vdseff };
    let diff_vds = vds_i - vdseff;

    // Vasat and its derivatives (ngspice b3soipdld.c lines 1503-1544)
    let tmp4 = 1.0 - 0.5 * abulk * vdsat / vgst2vtm;
    let t9_va = wvcox_rds * vgsteff;
    let t8_va = t9_va / vgst2vtm;
    let t0_va = esat_l + vdsat + 2.0 * t9_va * tmp4;

    let t7_va = 2.0 * wvcox_rds * tmp4;
    let dt0_va_dvg = desat_l_dvg + dvdsat_dvg + t7_va * (1.0 + tmp2_rds * vgsteff)
        - t8_va * (abulk * dvdsat_dvg - abulk * vdsat / vgst2vtm + vdsat * dabulk_dvg);
    let dt0_va_dvb = desat_l_dvb + dvdsat_dvb + t7_va * tmp3_rds * vgsteff
        - t8_va * (dabulk_dvb * vdsat + abulk * dvdsat_dvb);
    let dt0_va_dvd = desat_l_dvd + dvdsat_dvd - t8_va * abulk * dvdsat_dvd;

    let t9_ab = wvcox_rds * abulk;
    let t1_ab = 2.0 / lambda - 1.0 + t9_ab;
    let tmp1_lambda = dlambda_dvg / (lambda * lambda);
    let dt1_ab_dvg = -2.0 * tmp1_lambda + wvcox_rds * (abulk * tmp2_rds + dabulk_dvg);
    let dt1_ab_dvb = dabulk_dvb * wvcox_rds + t9_ab * tmp3_rds;

    let vasat = t0_va / t1_ab;
    let dvasat_dvg = (dt0_va_dvg - vasat * dt1_ab_dvg) / t1_ab;
    let dvasat_dvb = (dt0_va_dvb - vasat * dt1_ab_dvb) / t1_ab;
    let dvasat_dvd = dt0_va_dvd / t1_ab;

    // VACLM (channel length modulation Early voltage) with derivatives
    let (vaclm, dvaclm_dvg, dvaclm_dvd, dvaclm_dvb) = if sp.pclm > 0.0 && diff_vds > 1e-10 {
        let t0 = 1.0 / (sp.pclm * abulk * sp.litl);
        let dt0_dvb = -t0 / abulk * dabulk_dvb;
        let dt0_dvg = -t0 / abulk * dabulk_dvg;

        let t2 = vgsteff / esat_l;
        let t1 = leff * (abulk + t2);
        let dt1_dvg = leff * ((1.0 - t2 * desat_l_dvg) / esat_l + dabulk_dvg);
        let dt1_dvb = leff * (dabulk_dvb - t2 * desat_l_dvb / esat_l);
        let dt1_dvd = -t2 * desat_l_dvd / esat;

        let t9_cl = t0 * t1;
        let vaclm = t9_cl * diff_vds;
        let dvaclm_dvg = t0 * dt1_dvg * diff_vds - t9_cl * dvdseff_dvg + t1 * diff_vds * dt0_dvg;
        let dvaclm_dvb = (dt0_dvb * t1 + t0 * dt1_dvb) * diff_vds - t9_cl * dvdseff_dvb;
        let dvaclm_dvd = t0 * dt1_dvd * diff_vds + t9_cl * (1.0 - dvdseff_dvd);

        (vaclm, dvaclm_dvg, dvaclm_dvd, dvaclm_dvb)
    } else {
        (MAX_EXP, 0.0, 0.0, 0.0)
    };

    // VADIBL (DIBL Early voltage) with derivatives
    let (vadibl, dvadibl_dvg, dvadibl_dvd, dvadibl_dvb) = if sp.theta_rout > 0.0 {
        let t8 = abulk * vdsat;
        let t0 = vgst2vtm * t8;
        let t1 = vgst2vtm + t8;
        let dt0_dvg = vgst2vtm * abulk * dvdsat_dvg + t8 + vgst2vtm * vdsat * dabulk_dvg;
        let dt1_dvg = 1.0 + abulk * dvdsat_dvg + vdsat * dabulk_dvg;
        let dt1_dvb = dabulk_dvb * vdsat + abulk * dvdsat_dvb;
        let dt0_dvb = vgst2vtm * dt1_dvb;
        let dt1_dvd = abulk * dvdsat_dvd;
        let dt0_dvd = vgst2vtm * dt1_dvd;

        let t9_dibl = t1 * t1;
        let t2_dibl = sp.theta_rout;
        let vadibl = (vgst2vtm - t0 / t1) / t2_dibl;
        let mut dvadibl_dvg = (1.0 - dt0_dvg / t1 + t0 * dt1_dvg / t9_dibl) / t2_dibl;
        let mut dvadibl_dvb = (-dt0_dvb / t1 + t0 * dt1_dvb / t9_dibl) / t2_dibl;
        let mut dvadibl_dvd = (-dt0_dvd / t1 + t0 * dt1_dvd / t9_dibl) / t2_dibl;

        let t7 = sp.pdiblcb * vbseff;
        let (vadibl, dvadibl_dvg, dvadibl_dvd, dvadibl_dvb) = if t7 >= -0.9 {
            let t3 = 1.0 / (1.0 + t7);
            let vadibl = vadibl * t3;
            dvadibl_dvg *= t3;
            dvadibl_dvb = (dvadibl_dvb - vadibl * sp.pdiblcb) * t3;
            dvadibl_dvd *= t3;
            (vadibl, dvadibl_dvg, dvadibl_dvd, dvadibl_dvb)
        } else {
            let t4 = 1.0 / (0.8 + t7);
            let t3 = (17.0 + 20.0 * t7) * t4;
            dvadibl_dvg *= t3;
            dvadibl_dvb = dvadibl_dvb * t3 - vadibl * sp.pdiblcb * t4 * t4;
            dvadibl_dvd *= t3;
            let vadibl = vadibl * t3;
            (vadibl, dvadibl_dvg, dvadibl_dvd, dvadibl_dvb)
        };

        (vadibl, dvadibl_dvg, dvadibl_dvd, dvadibl_dvb)
    } else {
        (MAX_EXP, 0.0, 0.0, 0.0)
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
    let tmp1_va = vaclm * vaclm;
    let tmp2_va = vadibl * vadibl;
    let tmp3_va = vaclm + vadibl;
    let t1_va = vaclm * vadibl / tmp3_va;
    let tmp3_va_sq = tmp3_va * tmp3_va;
    let dt1_va_dvg = (tmp1_va * dvadibl_dvg + tmp2_va * dvaclm_dvg) / tmp3_va_sq;
    let dt1_va_dvd = (tmp1_va * dvadibl_dvd + tmp2_va * dvaclm_dvd) / tmp3_va_sq;
    let dt1_va_dvb = (tmp1_va * dvadibl_dvb + tmp2_va * dvaclm_dvb) / tmp3_va_sq;

    // Va = Vasat + T0_pvag * T1_va
    let va = vasat + t0_pvag * t1_va;
    let dva_dvg = dvasat_dvg + t1_va * dt0_pvag_dvg + t0_pvag * dt1_va_dvg;
    let dva_dvd = dvasat_dvd + t1_va * dt0_pvag_dvd + t0_pvag * dt1_va_dvd;
    let dva_dvb = dvasat_dvb + t1_va * dt0_pvag_dvb + t0_pvag * dt1_va_dvb;

    // Ids calculation
    let cox_wov_l = cox * weff_ch / leff;
    let beta = ueff * cox_wov_l;

    let t0_ids = 1.0 - 0.5 * abulk * vdseff / vgst2vtm;
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

    // Derivatives of T0_ids = 1 - 0.5 * Abulk * Vdseff / Vgst2Vtm
    let dt0_ids_dvg =
        -0.5 * (abulk * dvdseff_dvg - abulk * vdseff / vgst2vtm + vdseff * dabulk_dvg) / vgst2vtm;
    let dt0_ids_dvd = -0.5 * abulk * dvdseff_dvd / vgst2vtm;
    let dt0_ids_dvb = -0.5 * (abulk * dvdseff_dvb + dabulk_dvb * vdseff) / vgst2vtm;

    // Derivatives of fgche1 = Vgsteff * T0_ids
    let dfgche1_dvg = vgsteff * dt0_ids_dvg + t0_ids;
    let dfgche1_dvd = vgsteff * dt0_ids_dvd;
    let dfgche1_dvb = vgsteff * dt0_ids_dvb;

    // Derivatives of fgche2 = 1 + Vdseff / EsatL
    let dfgche2_dvg = (dvdseff_dvg - t9_fgche * desat_l_dvg) / esat_l;
    let dfgche2_dvd = (dvdseff_dvd - t9_fgche * desat_l_dvd) / esat_l;
    let dfgche2_dvb = (dvdseff_dvb - t9_fgche * desat_l_dvb) / esat_l;

    // Derivatives of gche = beta * fgche1 / fgche2
    let dgche_dvg = (beta * dfgche1_dvg + fgche1 * dbeta_dvg - gche * dfgche2_dvg) / fgche2;
    let dgche_dvd = (beta * dfgche1_dvd + fgche1 * dbeta_dvd - gche * dfgche2_dvd) / fgche2;
    let dgche_dvb = (beta * dfgche1_dvb + fgche1 * dbeta_dvb - gche * dfgche2_dvb) / fgche2;

    // Derivatives of Idl (ngspice b3soipdld.c lines 1741-1745)
    let didl_dvg =
        (gche * dvdseff_dvg + t9_gche * dgche_dvg) / t0_gche - idl * gche / t0_gche * drds_dvg;
    let didl_dvd = (gche * dvdseff_dvd + t9_gche * dgche_dvd) / t0_gche;
    let didl_dvb = (gche * dvdseff_dvb + t9_gche * dgche_dvb - idl * drds_dvb * gche) / t0_gche;

    // Gm0, Gds0, Gmbs0 (ngspice b3soipdld.c lines 1755-1758)
    let gm0 = t0_ids2 * didl_dvg - idl * (dvdseff_dvg + t9_ids * dva_dvg) / va;
    let gds0 = t0_ids2 * didl_dvd + idl * (1.0 - dvdseff_dvd - t9_ids * dva_dvd) / va;
    let gmbs0 = t0_ids2 * didl_dvb - idl * (dvdseff_dvb + t9_ids * dva_dvb) / va;

    // Final Gm, Gds, Gmbs (ngspice b3soipdld.c lines 1766-1768)
    let gm = gm0 * dvgsteff_dvg;
    let gmbs = gm0 * dvgsteff_dvb + gmbs0 * dvbseff_dvb;
    let gds = gm0 * dvgsteff_dvd + gds0;

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
    let nvtm1 = vtm * sp.ndiode;
    let vbd = vbs_i - vds_i;

    // Ibs1/Ibd1: Diffusion current
    let (ibs1, dibs1_dvb, ibd1, dibd1_dvb, dibd1_dvd) = if sp.jdif == 0.0 {
        (0.0, 0.0, 0.0, 0.0, 0.0)
    } else {
        let t0 = vbs_i / nvtm1;
        let (exp_vbs, dexp) = soi_dexp(t0);
        let t0_ws = sp.wdios * model.tsi * sp.jdif;
        let ibs1 = t0_ws * (exp_vbs - 1.0);
        let dibs1_dvb = t0_ws * dexp / nvtm1;

        let t0 = vbd / nvtm1;
        let (exp_vbd, dexp) = soi_dexp(t0);
        let t0_wd = sp.wdiod * model.tsi * sp.jdif;
        let ibd1 = t0_wd * (exp_vbd - 1.0);
        let dibd1_dvb = t0_wd * dexp / nvtm1;
        (ibs1, dibs1_dvb, ibd1, dibd1_dvb, -dibd1_dvb)
    };

    // Ibs2/Ibd2: Recombination/trap-assisted tunneling
    // ngspice b3soipdld.c lines 1893-1990: Ibs2 = T3 * (T10 + T11)
    // T10 = forward bias term: exp(Vbs/NVtmf)
    // T11 = reverse bias term: -exp(-Vbs * vrec0 / (NVtmr * (vrec0 - Vbs)))
    let (ibs2, dibs2_dvb, ibd2, dibd2_dvb, dibd2_dvd) = if sp.jrec == 0.0 {
        (0.0, 0.0, 0.0, 0.0, 0.0)
    } else {
        let nvtmf = 0.026 * model.nrecf0;
        let nvtmr = 0.026 * model.nrecr0;

        // === Ibs2 ===
        // Forward bias component (T10)
        let t0_bs = vbs_i / nvtmf;
        let (t10_bs, t2_bs) = soi_dexp(t0_bs);
        let dt10_bs_dvb = t2_bs / nvtmf;

        // Reverse bias component (T11) — ngspice b3soipdld.c lines 1921-1944
        let (t11_bs, dt11_bs_dvb) = if model.vrec0 == 0.0 {
            (0.0, 0.0)
        } else if (model.vrec0 - vbs_i) < 1e-3 {
            // Clamp to avoid singularity (ngspice lines 1922-1929)
            let t1 = 1e3;
            let t0 = -vbs_i / nvtmr * model.vrec0 * t1;
            (-t0.exp(), 0.0)
        } else {
            let t1 = 1.0 / (model.vrec0 - vbs_i);
            let t0 = -vbs_i / nvtmr * model.vrec0 * t1;
            let dt0_dvb = -model.vrec0 / nvtmr * (t1 + vbs_i * t1 * t1);
            let (exp_t0, dexp_t0) = soi_dexp(t0);
            (-exp_t0, -dexp_t0 * dt0_dvb)
        };

        let t3_bs = sp.wdios * model.tsi * sp.jrec;
        let ibs2 = t3_bs * (t10_bs + t11_bs);
        let dibs2_dvb = t3_bs * (dt10_bs_dvb + dt11_bs_dvb);

        // === Ibd2 ===
        // Forward bias component (T10)
        let t0_bd = vbd / nvtmf;
        let (t10_bd, t2_bd) = soi_dexp(t0_bd);
        let dt10_bd_dvb = t2_bd / nvtmf;

        // Reverse bias component (T11) — same formula with Vbd instead of Vbs
        let (t11_bd, dt11_bd_dvb) = if model.vrec0 == 0.0 {
            (0.0, 0.0)
        } else if (model.vrec0 - vbd) < 1e-3 {
            let t1 = 1e3;
            let t0 = -vbd / nvtmr * model.vrec0 * t1;
            (-t0.exp(), 0.0)
        } else {
            let t1 = 1.0 / (model.vrec0 - vbd);
            let t0 = -vbd / nvtmr * model.vrec0 * t1;
            let dt0_dvb = -model.vrec0 / nvtmr * (t1 + vbd * t1 * t1);
            let (exp_t0, dexp_t0) = soi_dexp(t0);
            (-exp_t0, -dexp_t0 * dt0_dvb)
        };

        let t3_bd = sp.wdiod * model.tsi * sp.jrec;
        let ibd2 = t3_bd * (t10_bd + t11_bd);
        let dibd2_dvb = t3_bd * (dt10_bd_dvb + dt11_bd_dvb);
        (ibs2, dibs2_dvb, ibd2, dibd2_dvb, -dibd2_dvb)
    };

    // Ibs3/Ibd3: BJT recombination current + Ic: BJT collector transport current
    // (ngspice b3soipdld.c lines 2007-2192)
    //
    // The total BJT current splits into:
    //   - Recombination fraction (1-arfabjt): flows as Ibs3/Ibd3 body junction currents
    //   - Transport fraction (arfabjt): flows as Ic collector current added to drain
    //
    // Both include high-level injection factors (EhlisFactor/EhlidFactor) and
    // Ic includes a second-order Early effect factor (E2ndFactor).
    let (ibs3, dibs3_dvb, ibd3, dibd3_dvb, dibd3_dvd, ic, gcd, gcb) = if sp.jbjt == 0.0
        || sp.lratio == 0.0
    {
        (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0)
    } else {
        let t0_bs = vbs_i / nvtm1;
        let (exp_vbs, dexp_bs) = soi_dexp(t0_bs);
        let t0_bd = vbd / nvtm1;
        let (exp_vbd, dexp_bd) = soi_dexp(t0_bd);

        let ien = weff / sp.nseg * model.tsi * sp.jbjt * sp.lratio;
        let ahli = model.ahli;

        // High-level injection factor — source side (ngspice lines 2014-2033)
        let ehlis = ahli * (exp_vbs - 1.0);
        let (ehlis_factor, dehlis_dvb) = if ehlis < 1e-5 {
            (1.0, 0.0)
        } else {
            let f = 1.0 / (1.0 + ehlis).sqrt();
            let t = -0.5 * f / (1.0 + ehlis);
            (f, t * ahli * dexp_bs / nvtm1)
        };

        // High-level injection factor — drain side (ngspice lines 2035-2056)
        let ehlid = ahli * (exp_vbd - 1.0);
        let (ehlid_factor, dehlid_dvb, dehlid_dvd) = if ehlid < 1e-5 {
            (1.0, 0.0, 0.0)
        } else {
            let f = 1.0 / (1.0 + ehlid).sqrt();
            let t = -0.5 * f / (1.0 + ehlid);
            let dehlid_dvb = t * ahli * dexp_bd / nvtm1;
            (f, dehlid_dvb, -dehlid_dvb)
        };

        // Ibs3/Ibd3: recombination (1-arfabjt fraction) with EhlisFactor
        // (ngspice lines 2058-2093)
        let t0_recomb = 1.0 - sp.arfabjt;
        let (ibs3, dibs3_dvb, ibd3, dibd3_dvb, dibd3_dvd) = if t0_recomb < 1e-2 {
            (0.0, 0.0, 0.0, 0.0, 0.0)
        } else {
            let t1 = t0_recomb * ien;
            let ibs3 = t1 * (exp_vbs - 1.0) * ehlis_factor;
            let dibs3_dvb = t1 * (dexp_bs / nvtm1 * ehlis_factor + (exp_vbs - 1.0) * dehlis_dvb);
            let ibd3 = t1 * (exp_vbd - 1.0) * ehlid_factor;
            let dibd3_dvb = t1 * (dexp_bd / nvtm1 * ehlid_factor + (exp_vbd - 1.0) * dehlid_dvb);
            (ibs3, dibs3_dvb, ibd3, dibd3_dvb, -dibd3_dvb)
        };

        // Ic: BJT collector current (arfabjt fraction) with E2ndFactor
        // (ngspice lines 2123-2192)
        let (ic, gcd, gcb) = if sp.arfabjt < 1e-2 || vds_i == 0.0 {
            (0.0, 0.0, 0.0)
        } else {
            // Second-order Early effect (ngspice lines 2128-2171)
            let t0_e = 1.0 + (vbs_i + vbd) / sp.vearly;
            let dt0_e_dvb = 2.0 / sp.vearly;
            let dt0_e_dvd = -1.0 / sp.vearly;

            let ehlis_raw = if ehlis < 1e-5 { 0.0 } else { ehlis };
            let ehlid_raw = if ehlid < 1e-5 { 0.0 } else { ehlid };
            let t1_e = ehlis_raw + ehlid_raw;
            let dt1_e_dvb = if ehlis >= 1e-5 {
                ahli * dexp_bs / nvtm1
            } else {
                0.0
            } + if ehlid >= 1e-5 {
                ahli * dexp_bd / nvtm1
            } else {
                0.0
            };
            let dt1_e_dvd = if ehlid >= 1e-5 {
                -(ahli * dexp_bd / nvtm1)
            } else {
                0.0
            };

            let t3_e = (t0_e * t0_e + 4.0 * t1_e).sqrt();
            let dt3_e_dvb = 0.5 / t3_e * (2.0 * t0_e * dt0_e_dvb + 4.0 * dt1_e_dvb);
            let dt3_e_dvd = 0.5 / t3_e * (2.0 * t0_e * dt0_e_dvd + 4.0 * dt1_e_dvd);

            let t2_e = (t0_e + t3_e) / 2.0;
            let dt2_e_dvb = (dt0_e_dvb + dt3_e_dvb) / 2.0;
            let dt2_e_dvd = (dt0_e_dvd + dt3_e_dvd) / 2.0;

            let (e2nd, de2nd_dvb, de2nd_dvd) = if t2_e < 0.1 {
                (10.0, 0.0, 0.0)
            } else {
                let e = 1.0 / t2_e;
                (e, -e / t2_e * dt2_e_dvb, -e / t2_e * dt2_e_dvd)
            };

            let t0_ic = sp.arfabjt * ien;
            let dexp_bs_dvb = dexp_bs / nvtm1;
            let dexp_bd_dvb = dexp_bd / nvtm1;
            let ic = t0_ic * (exp_vbs - exp_vbd) * e2nd;
            let gcb =
                t0_ic * ((dexp_bs_dvb - dexp_bd_dvb) * e2nd + (exp_vbs - exp_vbd) * de2nd_dvb);
            let gcd = t0_ic * ((-dexp_bd_dvb) * e2nd + (exp_vbs - exp_vbd) * de2nd_dvd);

            (ic, gcd, gcb)
        };

        (ibs3, dibs3_dvb, ibd3, dibd3_dvb, dibd3_dvd, ic, gcd, gcb)
    };

    // Ibs4/Ibd4: Tunneling current
    let (ibs4, dibs4_dvb, ibd4, dibd4_dvb, dibd4_dvd) = if sp.jtun == 0.0 {
        (0.0, 0.0, 0.0, 0.0, 0.0)
    } else {
        let nvtm_tun = vtm * model.ntun;
        let t0 = -vbs_i / nvtm_tun;
        let (exp_val, dexp_val) = soi_dexp(t0);
        let t3 = sp.wdios * model.tsi * sp.jtun;
        let ibs4 = -t3 * (exp_val - 1.0);
        let dibs4_dvb = t3 * dexp_val / nvtm_tun;

        let t0 = -vbd / nvtm_tun;
        let (exp_val, dexp_val) = soi_dexp(t0);
        let t3 = sp.wdiod * model.tsi * sp.jtun;
        let ibd4 = -t3 * (exp_val - 1.0);
        let dibd4_dvb = t3 * dexp_val / nvtm_tun;
        (ibs4, dibs4_dvb, ibd4, dibd4_dvb, -dibd4_dvb)
    };

    // Total junction currents
    let ibs = ibs1 + ibs2 + ibs3 + ibs4;
    let ibd = ibd1 + ibd2 + ibd3 + ibd4;
    let gbs_jct = dibs1_dvb + dibs2_dvb + dibs3_dvb + dibs4_dvb;
    let gbd_jct = dibd1_dvb + dibd2_dvb + dibd3_dvb + dibd4_dvb;

    // Impact ionization current (Iii) — PD Vdsatii-based model
    // (ngspice b3soipdld.c lines 2536-2642)
    let (iii, gii_d, gii_g, gii_b) = if sp.alpha0 <= 0.0 {
        (0.0, 0.0, 0.0, 0.0)
    } else {
        // Vdsatii0: saturation voltage for impact ionization (at T=Tnom)
        let vdsatii0 = model.vdsatii0 - model.lii / leff;

        // VgsStep: gate-voltage-dependent shift (ngspice lines 2549-2570)
        let t0_esat = model.esatii * leff;
        let t1_sii = model.sii0 * t0_esat / (1.0 + t0_esat);
        let t0_sii1 = 1.0 / (1.0 + model.sii1 * vgsteff);
        let t3_sii = t0_sii1 + model.sii2;
        let t4_sii = vgst * model.sii1 * t0_sii1 * t0_sii1; // -dT3/dVgsteff * Vgst
        let t2_vgst = vgst * t3_sii;
        let dt2_dvg = t3_sii * dvgst_dvg - t4_sii * dvgsteff_dvg;
        let dt2_dvb = t3_sii * dvgst_dvb * dvbseff_dvb - t4_sii * dvgsteff_dvb;
        let dt2_dvd = t3_sii * dvgst_dvd - t4_sii * dvgsteff_dvd;

        let t3_siid = 1.0 / (1.0 + model.siid * vds_i);
        let dt3_siid_dvd = -model.siid * t3_siid * t3_siid;

        let vgs_step = t1_sii * t2_vgst * t3_siid;
        let _vdsatii = vdsatii0 + vgs_step;
        let vdiff = vds_i - vdsatii0 - vgs_step;
        let dvdiff_dvg = -t1_sii * t3_siid * dt2_dvg;
        let dvdiff_dvb = -t1_sii * t3_siid * dt2_dvb;
        let dvdiff_dvd = 1.0 - t1_sii * (t3_siid * dt2_dvd + t2_vgst * dt3_siid_dvd);

        // Polynomial denominator (ngspice lines 2583-2600)
        let t0_poly = model.beta2 + model.beta1 * vdiff + sp.beta0 * vdiff * vdiff;
        let (t0_poly, dt0_poly_dvg, dt0_poly_dvb, dt0_poly_dvd) = if t0_poly < 1e-5 {
            (1e-5, 0.0, 0.0, 0.0)
        } else {
            let t1_coeff = model.beta1 + 2.0 * sp.beta0 * vdiff;
            (
                t0_poly,
                t1_coeff * dvdiff_dvg,
                t1_coeff * dvdiff_dvb,
                t1_coeff * dvdiff_dvd,
            )
        };

        // Ratio = alpha0 * exp(Vdiff/T0_poly) with clamping (ngspice lines 2602-2624)
        let (ratio, dratio_dvg, dratio_dvb, dratio_dvd) =
            if t0_poly < vdiff / EXPL_THRESHOLD && vdiff > 0.0 {
                (sp.alpha0 * MAX_EXP, 0.0, 0.0, 0.0)
            } else if t0_poly < -vdiff / EXPL_THRESHOLD && vdiff < 0.0 {
                (sp.alpha0 * MIN_EXP, 0.0, 0.0, 0.0)
            } else {
                let ratio = sp.alpha0 * (vdiff / t0_poly).exp();
                if ratio > 10.0 {
                    (10.0, 0.0, 0.0, 0.0)
                } else {
                    let t1_deriv = ratio / (t0_poly * t0_poly);
                    (
                        ratio,
                        t1_deriv * (t0_poly * dvdiff_dvg - vdiff * dt0_poly_dvg),
                        t1_deriv * (t0_poly * dvdiff_dvb - vdiff * dt0_poly_dvb),
                        t1_deriv * (t0_poly * dvdiff_dvd - vdiff * dt0_poly_dvd),
                    )
                }
            };

        let iii = ratio * ids;
        let gii_g = ratio * gm + ids * dratio_dvg;
        let gii_b = ratio * gmbs + ids * dratio_dvb;
        let gii_d = ratio * gds + ids * dratio_dvd;
        (iii, gii_d, gii_g, gii_b)
    };

    // Add BJT collector current to drain current and its derivatives to
    // output conductance / body transconductance (ngspice b3soipdld.c lines 2679-2685):
    //   cdrain = Ids + Ic
    //   gds    = Gds + Gcd
    //   gmbs   = Gmb + Gcb
    let ids = ids + ic;
    let gds = gds + gcd;
    let gmbs = gmbs + gcb;

    // Equivalent current sources for NR companion model
    let ceq_d = sign * (ids - gm * vgs_i - gds * vds_i - gmbs * vbs_i);
    let ceq_bs = ibs - gbs_jct * vbs_i;
    let ceq_bd = ibd - gbd_jct * vbd;
    // Impact ionization: Iii = f(Vgs, Vds, Vbs); ceq = Iii - dI/dV * V0
    let ceq_iii = iii - gii_d * vds_i - gii_g * vgs_i - gii_b * vbs_i;
    // GIDL: Igidl = f(Vds, Vgs); ceq = Igidl - dI/dV * V0
    let ceq_gidl = igidl - ggidl_d * vds_i - ggidl_g * vgs_i;
    let ceq_sgidl = isgidl - gsgidl_g * vgs_i;

    // Capacitances (simplified — gate overlap + basic intrinsic)
    let cox_wl = cox * weff_ch * leff;
    let (cggb, cgdb, cgsb) = if vgsteff > 0.0 {
        // Strong inversion: Gate cap split
        let t0 = 1.0 - abulk * vdseff / (2.0 * vgst2vtm);
        (cox_wl * (1.0 - t0 * t0), -cox_wl * t0 * 0.5, 0.0)
    } else {
        // Subthreshold
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

    Bsim3SoiPdCompanion {
        ids: ids / sp.nseg,
        gm: gm / sp.nseg,
        gds: gds / sp.nseg,
        gmbs: gmbs / sp.nseg,
        mode,
        vdsat,
        ibs,
        ibd,
        gbs_jct,
        gbd_jct,
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
    }
}

/// Stamp BSIM3SOI-PD companion model into the MNA matrix and RHS.
///
/// Nodes: D' (drain prime), G (gate), S' (source prime), B_int (internal body).
/// The E (back-gate) and external B nodes are handled via body resistance in mna.rs.
pub fn stamp_bsim3soi_pd(
    matrix: &mut crate::SparseMatrix,
    rhs: &mut [f64],
    inst: &Bsim3SoiPdInstance,
    comp: &Bsim3SoiPdCompanion,
    gmin: f64,
) {
    let dp = inst.drain_eff_idx();
    let g = inst.gate_idx;
    let sp = inst.source_eff_idx();
    let b = inst.body_int_idx;

    let m = inst.m;

    let (xnrm, xrev) = if comp.mode > 0 {
        (1.0, 0.0)
    } else {
        (0.0, 1.0)
    };

    let gm_eff = m * (comp.gm * xnrm + comp.gds * xrev);
    let gds_eff = m * (comp.gds * xnrm + comp.gm * xrev);
    let gmbs_eff = m * comp.gmbs;
    // Channel current: asymmetric VCCS stamps (must use matrix.add, not stamp_conductance)
    if let Some(d) = dp {
        matrix.add(d, d, gds_eff);
        if let Some(gate) = g {
            matrix.add(d, gate, gm_eff);
        }
        if let Some(s) = sp {
            matrix.add(d, s, -(gm_eff + gds_eff + gmbs_eff));
        }
        if let Some(bulk) = b {
            matrix.add(d, bulk, gmbs_eff);
        }
    }
    if let Some(s) = sp {
        if let Some(d) = dp {
            matrix.add(s, d, -gds_eff);
        }
        if let Some(gate) = g {
            matrix.add(s, gate, -gm_eff);
        }
        matrix.add(s, s, gm_eff + gds_eff + gmbs_eff);
        if let Some(bulk) = b {
            matrix.add(s, bulk, -gmbs_eff);
        }
    }

    // Gate-drain gmin (ngspice b3soipdld.c lines 4062-4063: CKTgmin between G and DP)
    crate::stamp_conductance(matrix, g, dp, m * gmin);

    // Floating-body stability: add Gmin body-to-source coupling (matching ngspice
    // b3soipdld.c line 4038: Gmin = CKTgmin * 1e-6).  ngspice uses a much smaller
    // Gmin at the body node than the circuit-level gmin to avoid dominating the
    // extremely small floating-body junction currents.
    // Floor at 1e-20 so circuits with very small gmin (e.g. 1e-25) still
    // have enough body-source coupling to keep the Jacobian non-singular.
    let body_gmin = (gmin * 1e-6).max(1e-20);
    if inst.body_idx.is_none() {
        crate::stamp_conductance(matrix, b, sp, body_gmin);
    }

    // --- Combined junction / Iii / GIDL derivative stamps (matching ngspice) ---
    //
    // Instead of stamping junction, Iii, and GIDL terms separately (which breaks
    // KCL at the source-prime column), use combined derivative stamps with
    // KCL-computed SP entries.  This matches ngspice b3soipdld.c lines 3894-3911
    // and 4040-4059, and the DD model pattern (bsim3soi_dd.rs lines 3085-3159).
    //
    // For PD model: Gjsd = 0 (no Vds dependence of source junction current)
    // and Gjdd = -Gjdb (all junction components depend on Vbd = Vbs - Vds only),
    // so the combined stamps simplify accordingly.

    // Drain-junction combined derivatives (ngspice gjd*: lines 2705-2714)
    // gddp* = negated stored_gjd* stamps.
    // stored_gjdb = Gjdb - Giib, stored_gjdd = Gjdd - (Giid+Gdgidld),
    // stored_gjdg = -(Giig+Gdgidlg)
    {
        let gddpg = m * (comp.gii_g + comp.ggidl_g);
        let gddpdp = m * (comp.gii_d + comp.ggidl_d + comp.gbd_jct);
        let gddpb = m * (comp.gii_b - comp.gbd_jct);
        let gddpsp = -(gddpg + gddpdp + gddpb); // KCL balance

        if let Some(d) = dp {
            matrix.add(d, d, gddpdp);
            if let Some(gate) = g {
                matrix.add(d, gate, gddpg);
            }
            if let Some(bi) = b {
                matrix.add(d, bi, gddpb);
            }
            if let Some(s) = sp {
                matrix.add(d, s, gddpsp);
            }
        }
    }

    // Source-junction combined derivatives (ngspice gjs*: lines 2716-2725)
    // gssp* stamps: current flowing into source-prime from body junction paths
    {
        let gsspg = m * comp.gsgidl_g;
        let gsspdp = 0.0_f64; // Gjsd = 0 for PD model
        let gsspb = m * (-comp.gbs_jct);
        let gsspsp = -(gsspg + gsspdp + gsspb); // KCL balance

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

    // Body node combined derivatives (ngspice gbb*: lines 2727-2731)
    // These are the NEGATED body current derivatives (since rhs -= ceqbody)
    // gbbs = Giib - Gjsb - Gjdb, gbgs = Giig + Gdgidlg + Gsgidlg,
    // gbds = Giid + Gdgidld + Gjdb (since Gjsd=0, Gjdd=-Gjdb)
    {
        let gbbg = m * (-(comp.gii_g + comp.ggidl_g + comp.gsgidl_g));
        let gbbdp = m * (-(comp.gii_d + comp.ggidl_d + comp.gbd_jct));
        let gbbb = m * (-comp.gii_b + comp.gbs_jct + comp.gbd_jct);
        let gbbsp = -(gbbg + gbbdp + gbbb); // KCL balance

        if let Some(bi) = b {
            if let Some(gate) = g {
                matrix.add(bi, gate, gbbg);
            }
            if let Some(d) = dp {
                matrix.add(bi, d, gbbdp);
            }
            matrix.add(bi, bi, gbbb);
            if let Some(s) = sp {
                matrix.add(bi, s, gbbsp);
            }
        }
    }

    // RHS: equivalent current sources for NR companion linearization.
    // Convention: rhs[node] -= ceq for current OUT, rhs[node] += ceq for current IN.
    // comp.ceq_d already has `sign` inside (ngspice: cdreq = type * (...)),
    // so we multiply by `m` only.
    // Junction, Iii, and GIDL ceqs are computed WITHOUT type sign in the companion
    // (b3soipdld.c lines 2665-2696), but are NEGATED for PMOS in the stamping
    // section (b3soipdld.c lines 3981-3991: ceqbs=-ceqbs, ceqbd=-ceqbd,
    // ceqbody=-ceqbody for type<0). Apply `sign` to match this behavior.
    let sign = inst.model.mos_type.sign();
    let ceq_d = m * comp.ceq_d;
    let ceq_bs = sign * m * comp.ceq_bs;
    let ceq_bd = sign * m * comp.ceq_bd;
    let ceq_iii = sign * m * comp.ceq_iii;
    let ceq_gidl = sign * m * comp.ceq_gidl;
    let ceq_sgidl = sign * m * comp.ceq_sgidl;

    if let Some(d) = dp {
        // Channel (out) + BD junction (out) + Iii (out) + GIDL drain (out)
        rhs[d] -= ceq_d - ceq_bd + ceq_iii + ceq_gidl;
    }
    if let Some(s) = sp {
        // Channel (in) + BS junction (in) + GIDL source (out)
        rhs[s] += ceq_d + ceq_bs - ceq_sgidl;
    }
    if let Some(bulk) = b {
        // BS junction (out) + BD junction (out) - Iii (in) - GIDL drain (in) - GIDL source (in)
        rhs[bulk] -= ceq_bs + ceq_bd - ceq_iii - ceq_gidl - ceq_sgidl;
    }

    // Body resistance to external body contact
    if let (Some(b_int), Some(b_ext)) = (inst.body_int_idx, inst.body_idx) {
        let gbody = if inst.model.rbody > 0.0 {
            m / inst.model.rbody
        } else {
            m * 1e3
        };
        crate::stamp_conductance(matrix, Some(b_int), Some(b_ext), gbody);
    }

    // Series resistance: single stamp_conductance per resistor
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

/// BSIM3SOI-PD voltage limiting for NR convergence.
pub fn bsim3soi_pd_limit(
    vgs_new: f64,
    vds_new: f64,
    vbs_new: f64,
    ves_new: f64,
    vgs_old: f64,
    vds_old: f64,
    vbs_old: f64,
    ves_old: f64,
    vth: f64,
) -> (f64, f64, f64, f64) {
    let vgs = crate::bsim3::fetlim(vgs_new, vgs_old, vth);
    let vds = crate::bsim3::fetlim(vds_new, vds_old, vth);
    // Body voltage limiting: simple ±0.2V clamp matching ngspice B3SOIPDlimit.
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
