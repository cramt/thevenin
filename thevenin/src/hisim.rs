//! HiSIM2 surface-potential MOSFET (ngspice LEVEL=68).
//!
//! Faithful port of the HiSIM2 (Hiroshima University STARC IGFET Model)
//! bulk MOSFET I-V core from ngspice-45
//! (`ngspice-upstream/src/spicelib/devices/hisim2/`):
//!
//! 1. Temperature/geometry-derived constants per `hsm2temp.c` (pocket-blend
//!    `Nsub`, `cnst0`/`cnst1`, `Pb2`, depletion widths, mobility
//!    coefficients, `Vmax`).
//! 2. The forward I-V evaluation per `hsm2eval.c` (normal-mode MOSFET,
//!    `CORECIP=1`, `flg_qme=0`): the outer SCE loop coupling `dVth` to the
//!    surface potential, the source-side `Ps0` Newton solve, the `Vdseff`
//!    smooth clamp, the drain-side `Psl` Newton solve, the `Idd` drift+
//!    diffusion current, channel-length modulation (`Lred`), the `Qbu`/`Qiu`
//!    charges feeding the universal-mobility field, and velocity saturation.
//! 3. The NR companion (`gm`/`gds`/`gmbs`) is built by central finite
//!    differences of the forward eval, so the Jacobian is automatically
//!    consistent with `cdrain` (see docs/archive/migration/hisim2-port-plan.md
//!    for the rationale). Bulk junction diodes and series RD/RS share the
//!    Level-1/2/3 companion shape.
//!
//! Scope notes: the DC drain-current path is ported in earnest; terminal
//! charges (Qg/Qd/Qs/Qb for transient/AC), Isub/gate leakage/GIDL, STI,
//! DFM/WPE, and the depletion-mode (`CODEP=1`) core are not. Parameters
//! parsed from `.model` are the subset that the ported path consumes;
//! unknown names are silently ignored so foundry HiSIM2 cards import
//! without errors.

use crate::model_params::ModelParams;

use crate::diode::VT_NOM;
use crate::mosfet::{MosfetCompanion, MosfetType};
use crate::physics::{EXP_LIMIT, safe_exp};

// ── HiSIM2 physical constants (hsm2evalenv.h — these differ slightly from
//    ngspice's global CONSTANTS; use HiSIM's own values for faithfulness) ──
/// Elementary charge (C). HiSIM `C_QE`.
const C_QE: f64 = 1.602_191_8e-19;
/// Boltzmann constant (J/K). HiSIM `C_KB`.
const C_KB: f64 = 1.380_622_6e-23;
/// Permittivity of silicon (F/m). HiSIM `C_ESI`.
const C_ESI: f64 = 1.034_943e-10;
/// Permittivity of vacuum (F/m). HiSIM `C_VAC`.
const C_VAC: f64 = 8.854_187_8e-12;
/// Intrinsic carrier concentration at 300 K (1/m³). HiSIM `C_Nin0`.
const C_NIN0: f64 = 1.04e16;
/// Inverse thermal voltage at 300 K (1/V). HiSIM `C_b300`.
const C_B300: f64 = 3.868_283e1;

const C_M2CM: f64 = 1.0e2;
const C_M2CM_P2: f64 = 1.0e4;
const C_M2UM: f64 = 1.0e6;
/// `E0²` in the CLM equation (eval.c:4538, `C_E0_p2`).
const C_E0_P2: f64 = 1.0e9;

const C_SQRT_2: f64 = std::f64::consts::SQRT_2;
const C_1O3: f64 = 1.0 / 3.0;
const C_2O3: f64 = 2.0 / 3.0;
/// 2^(1/3).
const C_2P_1O3: f64 = 1.259_921_049_894_873;

// ── hsm2eval.c numerical constants ──
const EPSM10: f64 = 10.0 * f64::EPSILON;
const SMALL: f64 = 1.0e-50;
const PS_CONV: f64 = 5.0e-13;
const GS_CONV: f64 = 1.0e-8;
const DP_MAX: f64 = 0.1;
const ZNBD3: f64 = 3.0;
const ZNBD5: f64 = 5.0;
const CN_NC3: f64 = C_SQRT_2 / 108.0;
// The zone-D1/D2 polynomial coefficients are the exact literals from
// hsm2eval.c:525-534 (CN_NC51 is sqrt(2)/2 truncated upstream — keep the
// truncated value for bit-faithfulness).
#[allow(clippy::approx_constant)]
const CN_NC51: f64 = 0.707_106_781_186_548;
const CN_NC52: f64 = -0.117_851_130_197_758;
const CN_NC53: f64 = 0.017_880_050_633_883_3;
const CN_NC54: f64 = -0.001_637_301_627_791_91;
const CN_NC55: f64 = 6.369_649_188_663_52e-5;
const CN_IM53: f64 = 2.969_315_485_577_1e-1;
const CN_IM54: f64 = -7.053_654_284_009_761e-2;
const CN_IM55: f64 = 6.115_288_895_133_179e-3;
const C_PS0INI_2: f64 = 8.0e-4;
const C_PSLINI_1: f64 = 0.3;
const C_PSLINI_2: f64 = 3.0e-2;
const VGVT_SMALL: f64 = 1.0e-12;
const VBS_MIN_CLAMP: f64 = -10.5;
const LARGE_ARG: f64 = 80.0;
const VTH_DLT: f64 = 1.0e-3;
const C_SCE_DLT: f64 = 1.0e-2;
const LP_S0_MAX: usize = 20;
const LP_SL_MAX: usize = 20;
const MAX_LOOP_SCE: usize = 5;
const PS0_SCE_TOL: f64 = 4.0e-7;

/// Simulation temperature (K). ngspice default circuit temp (27 °C); the
/// golden corpus is generated at this temperature.
const TTEMP: f64 = 300.15;

/// gds floor for numerical health (matches Level 1/2/3 convention).
const GDS_FLOOR: f64 = 1.0e-12;
/// Bulk-junction conductance floor.
const GMIN: f64 = 1.0e-12;

// ─────────────────────────────────────────────────────────────────────────
// Smoothing helpers (hsm2eval.c macros). Each returns (y, dy/dx).
// ─────────────────────────────────────────────────────────────────────────

/// `Fn_SU`: smooth ceiling to `xmax`.
fn fn_su(x: f64, xmax: f64, delta: f64) -> (f64, f64) {
    let t1 = xmax - x - delta;
    let t2 = (t1 * t1 + (4.0 * xmax * delta).abs()).sqrt();
    (xmax - 0.5 * (t1 + t2), 0.5 * (1.0 + t1 / t2))
}

/// `Fn_SU2`: smooth ceiling returning (y, dy/dx, dy/dxmax).
fn fn_su2(x: f64, xmax: f64, delta: f64) -> (f64, f64, f64) {
    let t1 = xmax - x - delta;
    let t2 = (t1 * t1 + (4.0 * xmax * delta).abs()).sqrt();
    (
        xmax - 0.5 * (t1 + t2),
        0.5 * (1.0 + t1 / t2),
        0.5 * (1.0 - (t1 + 2.0 * delta) / t2),
    )
}

/// `Fn_SL`: smooth flooring to `xmin`.
fn fn_sl(x: f64, xmin: f64, delta: f64) -> (f64, f64) {
    let t1 = x - xmin - delta;
    let t2 = (t1 * t1 + (4.0 * xmin * delta).abs()).sqrt();
    (xmin + 0.5 * (t1 + t2), 0.5 * (1.0 + t1 / t2))
}

/// `Fn_SZ`: smooth flooring to zero.
fn fn_sz(x: f64, delta: f64) -> (f64, f64) {
    let t2 = (x * x + 4.0 * delta * delta).sqrt();
    let y = 0.5 * (x + t2);
    if y < 0.0 {
        (0.0, 0.0)
    } else {
        (y, 0.5 * (1.0 + x / t2))
    }
}

/// `Fn_CP` with `pw = 4`: ceiling `y = x·xmax/(x⁸+xmax⁸)^{1/8}`.
fn fn_cp4(x: f64, xmax: f64) -> f64 {
    let x2 = x * x;
    let xmax2 = xmax * xmax;
    let xp = x2 * x2 * x2 * x2;
    let xmp = xmax2 * xmax2 * xmax2 * xmax2;
    let arg = xp + xmp;
    // (arg)^(1/8) via three square roots.
    let dnm = arg.sqrt().sqrt().sqrt();
    x * xmax / dnm
}

/// `Fn_DclPoly4` + `Fn_SUPoly4`: polynomial smooth-ceiling to `xmax`.
fn fn_su_poly4(x: f64, xmax: f64) -> (f64, f64) {
    let t = x / xmax;
    let t2 = t * t;
    let t3 = t2 * t;
    let t4 = t2 * t2;
    let y_dcl = 1.0 / (1.0 + t + t2 + t3 + t4);
    let dx_dcl = -(1.0 + 2.0 * t + 3.0 * t2 + 4.0 * t3) * y_dcl * y_dcl;
    (xmax * (1.0 - y_dcl), -dx_dcl)
}

/// `Fn_SymAdd`: additional term for symmetry at Vds = 0.
fn fn_sym_add(x: f64, add0: f64) -> (f64, f64) {
    let t1 = 2.0 * x / add0;
    let t2 = 1.0
        + t1 * (1.0 / 2.0
            + t1 * (1.0 / 6.0
                + t1 * (1.0 / 24.0
                    + t1 * (1.0 / 120.0 + t1 * (1.0 / 720.0 + t1 * (1.0 / 5040.0))))));
    let t3 = 1.0 / 2.0
        + t1 * (1.0 / 3.0
            + t1 * (1.0 / 8.0 + t1 * (1.0 / 30.0 + t1 * (1.0 / 144.0 + t1 * (1.0 / 840.0)))));
    (add0 / t2, -2.0 * t3 / (t2 * t2))
}

/// `Fn_Pow`: 0^y := 0 (hsm2eval.c).
fn fn_pow(x: f64, y: f64) -> f64 {
    if x == 0.0 { 0.0 } else { x.powf(y) }
}

/// HiSIM2 model parameters. Names mirror the upstream `HSM2model` fields;
/// the retained subset is what the ported DC I-V path consumes. Anything
/// else in the netlist's `.model` card is parsed and discarded silently.
#[derive(Debug, Clone)]
pub struct HisimModel {
    pub mos_type: MosfetType,

    // ── Process / technology (hsm2set.c defaults) ───────────────────────
    /// Oxide thickness (m).
    pub tox: f64,
    /// Substrate doping (1/cm³ as written; converted to 1/m³ internally).
    pub nsubc: f64,
    /// Pocket-implant peak doping NSUBP (1/cm³).
    pub nsubp: f64,
    /// Maximum NSUBC clamp (1/cm³), `NSUBCMAX`.
    pub nsubcmax: f64,
    /// Pocket extension doping NPEXT (1/cm³).
    pub npext: f64,
    /// Pocket extension length LPEXT (m).
    pub lpext: f64,
    /// Pocket penetration length LP (m).
    pub lp: f64,
    /// Flat-band voltage VFBC (V).
    pub vfbc: f64,
    /// Built-in potential VBI (V).
    pub vbi: f64,
    /// PARL2: SCE effective-length reduction (m).
    pub parl2: f64,
    /// Lateral diffusion XLD (m), per side.
    pub xld: f64,
    /// Gate-oxide relative permittivity KAPPA.
    pub kappa: f64,
    /// Bandgap parameter EG0 (eV).
    pub eg0: f64,
    /// Nominal temperature TNOM (°C).
    pub tnom: f64,

    // ── Mobility / velocity ─────────────────────────────────────────────
    /// Coulomb scattering MUECB0 (cm²/V·s).
    pub muecb0: f64,
    /// Coulomb scattering MUECB1 (cm²/V·s).
    pub muecb1: f64,
    /// Phonon scattering MUEPH1.
    pub mueph1: f64,
    /// Phonon scattering exponent MUEPH0.
    pub mueph0: f64,
    /// Surface-roughness exponent MUESR0.
    pub muesr0: f64,
    /// Surface-roughness coefficient MUESR1.
    pub muesr1: f64,
    /// Mobility temperature exponent MUETMP.
    pub muetmp: f64,
    /// Saturation velocity VMAX (cm/s — HiSIM convention).
    pub vmax: f64,
    /// Velocity temperature parameter VTMP.
    pub vtmp: f64,
    /// Velocity overshoot VOVER.
    pub vover: f64,
    /// Velocity overshoot exponent VOVERP.
    pub voverp: f64,
    /// Depletion-charge Eeff coefficient NDEP.
    pub ndep: f64,
    /// Inversion-charge Eeff coefficient NINV.
    pub ninv: f64,
    /// Eeff Vds-dependence NINVD.
    pub ninvd: f64,
    /// High-field mobility exponent BB (2 electrons, 1 holes).
    pub bb: f64,

    // ── Short-channel / CLM ─────────────────────────────────────────────
    /// CLM1 blending coefficient (0..1) between Psl and Vds+Ps0.
    pub clm1: f64,
    /// CLM2 bulk-charge coefficient.
    pub clm2: f64,
    /// CLM3 inversion-charge coefficient.
    pub clm3: f64,
    /// CLM5 gate-length exponent.
    pub clm5: f64,
    /// CLM6 gate-length coefficient.
    pub clm6: f64,
    /// SC1..SC4 short-channel coefficients.
    pub sc1: f64,
    pub sc2: f64,
    pub sc3: f64,
    pub sc4: f64,
    /// SCP1..SCP3 pocket short-channel coefficients.
    pub scp1: f64,
    pub scp2: f64,
    pub scp3: f64,
    pub scp21: f64,
    pub scp22: f64,
    /// BS1/BS2 body-coefficient of pocket.
    pub bs1: f64,
    pub bs2: f64,
    /// Narrow-channel WFC (F/cm² as written; ×1e4 like ngspice).
    pub wfc: f64,
    /// Narrow-channel WVTH0.
    pub wvth0: f64,

    // ── Smoothing / numerics ────────────────────────────────────────────
    /// Vdseff smoothing DDLTMAX.
    pub ddltmax: f64,
    /// Vdseff smoothing DDLTSLP.
    pub ddltslp: f64,
    /// Vdseff smoothing DDLTICT.
    pub ddltict: f64,
    /// Symmetry bias shift VZADD0 (V).
    pub vzadd0: f64,
    /// Symmetry potential shift PZADD0 (V).
    pub pzadd0: f64,
    /// Minimum gate bias VGSMIN (V, NMOS frame; type-normalized).
    pub vgsmin: f64,

    // ── Parasitics (companion shape shared with Level 1/2/3) ────────────
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
    /// Flat-band voltage (V).
    pub vfb: f64,

    // ── Faithful HiSIM2 constants (per hsm2temp.c, long-channel NSUBC) ──
    // These are the model-level (geometry-independent) counterparts: the
    // per-instance eval recomputes them with the pocket-blend Nsub(Lgate).
    /// Thermal beta `q/(kB·T)` = 1/Vt (1/V). ngspice `HSM2_beta`.
    pub beta: f64,
    /// Intrinsic carrier concentration `Nin` at the sim temperature (m⁻³).
    pub nin: f64,
    /// Effective channel doping at NSUBC (m⁻³); the eval blends the pocket.
    pub nsub_eff: f64,
    /// Body-charge constant `cnst0 = sqrt(2·εsi·q·Nsub/beta)` (temp.c:645).
    pub cnst0: f64,
    /// Inversion-charge constant `cnst1 = (Nin/Nsub)²` (temp.c:648).
    pub cnst1: f64,
    /// Twice the bulk Fermi potential `Pb2 = (2/beta)·ln(Nsub/Nin)` (V).
    pub pb2: f64,
}

impl HisimModel {
    /// Create a HiSIM2 model with the upstream `hsm2set.c` defaults.
    /// Used as the fallback when a netlist references a model name that
    /// doesn't resolve (mirrors `Mos3Model::new`).
    pub fn new(mos_type: MosfetType) -> Self {
        let is_nmos = mos_type == MosfetType::Nmos;
        let mut m = Self {
            mos_type,
            tox: 3.0e-9,
            nsubc: 5.0e17,
            nsubp: 1.0e18,
            nsubcmax: 5.0e18,
            npext: 5.0e17,
            lpext: 1.0e-50,
            lp: 15.0e-9,
            vfbc: -1.0,
            vbi: 1.1,
            parl2: 10.0e-9,
            xld: 0.0,
            kappa: 3.9,
            eg0: 1.1785,
            tnom: 27.0,
            muecb0: 1.0e3,
            muecb1: 100.0,
            mueph1: if is_nmos { 25.0e3 } else { 9.0e3 },
            mueph0: 300.0e-3,
            muesr0: 2.0,
            muesr1: 1.0e15,
            muetmp: 1.5,
            vmax: 1.0e7,
            vtmp: 0.0,
            vover: 0.3,
            voverp: 0.3,
            ndep: 1.0,
            ninv: 0.5,
            ninvd: 0.0,
            bb: if is_nmos { 2.0 } else { 1.0 },
            clm1: 700.0e-3,
            clm2: 2.0,
            clm3: 1.0,
            clm5: 1.0,
            clm6: 0.0,
            sc1: 1.0,
            sc2: 0.0,
            sc3: 0.0,
            sc4: 0.0,
            scp1: 1.0,
            scp2: 0.0,
            scp3: 0.0,
            scp21: 0.0,
            scp22: 0.0,
            bs1: 0.0,
            bs2: 0.9,
            wfc: 0.0,
            wvth0: 0.0,
            ddltmax: 10.0,
            ddltslp: 10.0,
            ddltict: 0.0,
            vzadd0: 20.0e-3,
            pzadd0: 20.0e-3,
            vgsmin: -5.0,
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
            beta: 0.0,
            nin: 0.0,
            nsub_eff: 0.0,
            cnst0: 0.0,
            cnst1: 0.0,
            pb2: 0.0,
        };
        m.compute_derived();
        m
    }

    /// Build a `HisimModel` from a netlist `.model` definition. Unknown
    /// parameters are silently ignored — this lets real HiSIM2 / HiSIMHV2
    /// model cards from foundry PDKs import without errors even though only
    /// the DC I-V subset affects simulation results.
    pub fn from_params(model: &ModelParams) -> Self {
        let mos_type = if model.kind.to_uppercase().contains("PMOS") {
            MosfetType::Pmos
        } else {
            MosfetType::Nmos
        };
        let mut m = Self::new(mos_type);
        for (name, v) in &model.params {
            match name.to_uppercase().as_str() {
                "TOX" => m.tox = *v,
                "NSUBC" => m.nsubc = *v,
                "NSUBP" => m.nsubp = *v,
                "NSUBCMAX" => m.nsubcmax = *v,
                "NPEXT" => m.npext = *v,
                "LPEXT" => m.lpext = *v,
                "LP" => m.lp = *v,
                "VFBC" => m.vfbc = *v,
                "VBI" => m.vbi = *v,
                "PARL2" => m.parl2 = *v,
                "XLD" => m.xld = *v,
                "KAPPA" => m.kappa = *v,
                "EG0" => m.eg0 = *v,
                "TNOM" => m.tnom = *v,
                "MUECB0" => m.muecb0 = *v,
                "MUECB1" => m.muecb1 = *v,
                "MUEPH1" => m.mueph1 = *v,
                "MUEPH0" => m.mueph0 = *v,
                "MUESR0" => m.muesr0 = *v,
                "MUESR1" => m.muesr1 = *v,
                "MUETMP" => m.muetmp = *v,
                "VMAX" => m.vmax = *v,
                "VTMP" => m.vtmp = *v,
                "VOVER" => m.vover = *v,
                "VOVERP" => m.voverp = *v,
                "NDEP" => m.ndep = *v,
                "NINV" => m.ninv = *v,
                "NINVD" => m.ninvd = *v,
                "BB" => m.bb = *v,
                "CLM1" => m.clm1 = *v,
                "CLM2" => m.clm2 = *v,
                "CLM3" => m.clm3 = *v,
                "CLM5" => m.clm5 = *v,
                "CLM6" => m.clm6 = *v,
                "SC1" => m.sc1 = *v,
                "SC2" => m.sc2 = *v,
                "SC3" => m.sc3 = *v,
                "SC4" => m.sc4 = *v,
                "SCP1" => m.scp1 = *v,
                "SCP2" => m.scp2 = *v,
                "SCP3" => m.scp3 = *v,
                "SCP21" => m.scp21 = *v,
                "SCP22" => m.scp22 = *v,
                "BS1" => m.bs1 = *v,
                "BS2" => m.bs2 = *v,
                "WFC" => m.wfc = *v,
                "WVTH0" => m.wvth0 = *v,
                "DDLTMAX" => m.ddltmax = *v,
                "DDLTSLP" => m.ddltslp = *v,
                "DDLTICT" => m.ddltict = *v,
                "VZADD0" => m.vzadd0 = *v,
                "PZADD0" => m.pzadd0 = *v,
                "VGSMIN" => m.vgsmin = *v,
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
        m.compute_derived();
        m
    }

    fn compute_derived(&mut self) {
        self.cox = C_VAC * self.kappa / self.tox.max(1e-12);
        // Convert NSUBC (cm⁻³) → m⁻³ for the physics formulas.
        let nsub_m3 = (self.nsubc * 1e6).max(C_NIN0 * 1.01);
        self.vfb = self.vfbc;

        // ── Faithful HiSIM2 constants (per hsm2temp.c) ──────────────────
        // beta = q/(kB·T) at the simulation temperature.
        self.beta = C_QE / (C_KB * TTEMP);
        // Nin at TTEMP = TNOM = 300.15 K is exactly C_Nin0 (Tratio = 1).
        self.nin = C_NIN0;
        // Long-channel effective doping ≈ NSUBC; the per-instance eval
        // blends the pocket (NSUBP/LP) which depends on Lgate.
        let nsub = nsub_m3;
        self.nsub_eff = nsub;
        // cnst0 = sqrt(2·εsi·q·Nsub / beta) (temp.c:645).
        self.cnst0 = (2.0 * C_ESI * C_QE * nsub / self.beta).sqrt();
        // cnst1 = (Nin/Nsub)² (temp.c:648).
        let nin_over_nsub = self.nin / nsub;
        self.cnst1 = nin_over_nsub * nin_over_nsub;
        // Pb2 = (2/beta)·ln(Nsub/Nin) (= 2φB, temp.c:653).
        self.pb2 = (2.0 / self.beta) * (nsub / self.nin).ln();

        // Legacy convenience fields (init seeding, op printing).
        self.phif2 = self.pb2;
        self.gamma = (2.0 * C_ESI * C_QE * nsub).sqrt() / self.cox.max(1e-12);
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

/// Geometry/temperature-derived constants for one (W, Lgate) instance,
/// per `hsm2temp.c`. Recomputed per companion call (correctness first;
/// cache later if profiling demands it).
struct EvalConsts {
    lgate: f64,
    leff: f64,
    weff_nf: f64,
    wg: f64,
    beta: f64,
    beta_inv: f64,
    beta2: f64,
    /// `q · Nsub` with the pocket-blended doping (temp.c:362-377).
    qnsub: f64,
    qnsub_esi: f64,
    qnsub_esi2: f64,
    cnst0: f64,
    cnst1: f64,
    pb2: f64,
    pb20: f64,
    pb2c: f64,
    /// NSUBC after the NSUBCMAX smooth clamp (m⁻³).
    nsubc: f64,
    /// Pocket overlap term `ptovr` (V); zero for Lgate > 2·LP.
    ptovr: f64,
    wdpl: f64,
    wdplp: f64,
    /// Temperature/geometry-adjusted saturation velocity (m/s).
    vmax: f64,
    muecb0: f64,
    muecb1: f64,
    mphn0: f64,
    muesr: f64,
    ndep_o_esi: f64,
    ninv_o_esi: f64,
    ninvd: f64,
    ddlt: f64,
    clmmod: f64,
    msc: f64,
    cox0: f64,
    cox0_inv: f64,
    vgs_min: f64,
}

impl HisimModel {
    fn eval_consts(&self, w: f64, lgate: f64) -> EvalConsts {
        let lgate = lgate.max(1e-9);
        let wgate = w.max(1e-9);
        let lg = lgate * C_M2UM;
        let wg = wgate * C_M2UM;

        let leff = (lgate - 2.0 * self.xld).max(1e-9);
        let weff = wgate;

        // Temperature-dependent basics (TTEMP == ktnom for the default
        // TNOM=27 setup; the general formulas are kept for other TNOMs).
        let ktnom = self.tnom + 273.15;
        let beta = C_QE / (C_KB * TTEMP);
        let betatnom = C_QE / (C_KB * ktnom);
        let tratio = TTEMP / ktnom;
        // Band gap (temp.c:162, 546-550), BGTMP1/2 at defaults.
        let egtnom = self.eg0 - ktnom * (90.25e-6 + ktnom * 1.0e-7);
        let eg = egtnom - 90.25e-6 * (TTEMP - ktnom) - 1.0e-7 * (TTEMP * TTEMP - ktnom * ktnom);
        // Intrinsic carrier concentration (temp.c:572).
        let nin = C_NIN0 * tratio.powf(1.5) * (-eg / 2.0 * beta + egtnom / 2.0 * betatnom).exp();

        // NSUBC / NSUBP in m⁻³ with the NSUBCMAX smooth clamp (temp.c:337-342).
        let nsubc_raw = self.nsubc * 1e6;
        let nsubp = self.nsubp * 1e6;
        let t3 = self.nsubcmax * 1e6 / nsubc_raw;
        let (t1c, _) = fn_su(1.0, t3, 0.01);
        let nsubc = nsubc_raw * t1c;

        // Pocket blend (temp.c:362-376). Nsubps = Nsubpp = NSUBP at default
        // width/STI parameters.
        let nsubps = nsubp;
        let mut nsub = if lgate > self.lp {
            (nsubc * (lgate - self.lp) + nsubps * self.lp) / lgate
        } else {
            nsubps + (nsubps - nsubc) * (self.lp - lgate) / self.lp
        };
        // NPEXT/LPEXT correction (temp.c:370-376).
        let t3e = 0.5 * lgate - self.lp;
        let lpext_dlt = 1e-8 / C_M2CM;
        let t3z = 0.5 * (t3e + (t3e * t3e + 4.0 * lpext_dlt * lpext_dlt).sqrt());
        let t1e = self.lpext.max(0.0);
        let t2e = t3z * t1e / (t3z + t1e);
        let npext = self.npext * 1e6;
        nsub += t2e * (npext - nsubc) / lgate;

        let qnsub = C_QE * nsub;
        let qnsub_esi = qnsub * C_ESI;

        // Pocket overlap (temp.c:382-389, 583).
        let ptovr0 = if lgate <= 2.0 * self.lp {
            let nsubb = 2.0 * nsubps - (nsubps - nsubc) * lgate / self.lp - nsubc;
            (nsubb / nsubc).ln()
        } else {
            0.0
        };
        let ptovr = ptovr0 / beta;

        // 2·φB (temp.c:399-403, 651-653).
        let pb20 = 2.0 / C_B300 * (nsub / C_NIN0).ln();
        let pb2c = 2.0 / C_B300 * (nsubc / C_NIN0).ln();
        let pb2 = 2.0 / beta * (nsub / nin).ln();

        // cnst0/cnst1 (temp.c:645-648).
        let cnst0 = (2.0 * C_ESI * C_QE * nsub / beta).sqrt();
        let t1 = nin / nsub;
        let cnst1 = t1 * t1;

        // Depletion widths (temp.c:663-666).
        let wdpl = (2.0 * C_ESI / C_QE / nsub).sqrt();
        let wdplp = (2.0 * C_ESI / C_QE / nsubp).sqrt();

        // Mobility coefficients (temp.c:238-261, 576-579). Width/length/
        // STI factors are 1 at defaults.
        let muecb0 = self.muecb0;
        let muecb1 = self.muecb1;
        let mueph = self.mueph1;
        let mphn0 = tratio.powf(self.muetmp) / mueph;
        let muesr = self.muesr0;

        // Eeff coefficients (temp.c:263-275). NDEPL/NDEPW default 0.
        let ndep_o_esi = self.ndep / C_ESI;
        let ninv_o_esi = self.ninv / C_ESI;
        let ninvd = self.ninvd;

        // Velocity (temp.c:395-397, 586-588). VMAX in cm/s → m/s (set.c);
        // the VOVERS width factor is 1 at its 0.0 default.
        let vmax0 = 1.0 + self.vover / fn_pow(lg, self.voverp);
        let vmax_mks = self.vmax / C_M2CM;
        let vmax = vmax0 * vmax_mks
            / (1.8 + 0.4 * tratio + 0.1 * tratio * tratio - self.vtmp * (1.0 - tratio));

        // Vdseff smoothing (temp.c:491-498, CODDLT=1).
        let t1d = self.ddltslp * lg;
        let ddlt = t1d * self.ddltmax / (t1d + self.ddltmax) + self.ddltict + SMALL;

        // CLM5/CLM6 (temp.c:180).
        let clmmod = 1.0 + fn_pow(lg, self.clm5) * self.clm6;

        let cox0 = C_VAC * self.kappa / self.tox;

        EvalConsts {
            lgate,
            leff,
            weff_nf: weff,
            wg,
            beta,
            beta_inv: 1.0 / beta,
            beta2: beta * beta,
            qnsub,
            qnsub_esi,
            qnsub_esi2: 2.0 * qnsub_esi,
            cnst0,
            cnst1,
            pb2,
            pb20,
            pb2c,
            nsubc,
            ptovr,
            wdpl,
            wdplp,
            vmax,
            muecb0,
            muecb1,
            mphn0,
            muesr,
            ndep_o_esi,
            ninv_o_esi,
            ninvd,
            ddlt,
            clmmod,
            msc: self.scp22,
            cox0,
            cox0_inv: 1.0 / cox0,
            vgs_min: -self.vgsmin.abs(),
        }
    }
}

/// Result of one forward I-V evaluation.
struct EvalOut {
    ids: f64,
    vth: f64,
    vdsat: f64,
}

/// Source/drain-side surface-potential residual pieces (eval.c:3537-3577,
/// 4087-4126). Returns `(fs1, fs1_dPs, fs2, fs2_dPs, fb)`.
#[allow(clippy::too_many_arguments)]
fn fs_pieces(
    chi: f64,
    beta: f64,
    cfs1: f64,
    cnst1: f64,
    ps: f64,
    vref: f64,
    drain_side: bool,
    vd_shift: f64,
) -> (f64, f64, f64, f64, f64) {
    if chi < ZNBD5 {
        // zone-D1/D2: Qb0 approximated by a 5-degree polynomial.
        let fi = chi * chi * chi * (CN_IM53 + chi * (CN_IM54 + chi * CN_IM55));
        let fi_dchi = chi * chi * (3.0 * CN_IM53 + chi * (4.0 * CN_IM54 + chi * 5.0 * CN_IM55));
        let fs1 = cfs1 * fi * fi;
        let fs1_dps = cfs1 * beta * 2.0 * fi * fi_dchi;
        let fb =
            chi * (CN_NC51 + chi * (CN_NC52 + chi * (CN_NC53 + chi * (CN_NC54 + chi * CN_NC55))));
        let fb_dchi = CN_NC51
            + chi
                * (2.0 * CN_NC52
                    + chi * (3.0 * CN_NC53 + chi * (4.0 * CN_NC54 + chi * 5.0 * CN_NC55)));
        let fs2 = (fb * fb + fs1).sqrt();
        let fs2_dps = (beta * fb_dchi * 2.0 * fb + fs1_dps) / (fs2 + fs2);
        (fs1, fs1_dps, fs2, fs2_dps, fb)
    } else if drain_side {
        // zone-D3 (Psl): via Rho = beta·(Psl − Vdseff).
        let rho = beta * (ps - vd_shift);
        let exp_rho = safe_exp(rho.min(EXP_LIMIT));
        let fs1 = cnst1 * (exp_rho - cfs1 / cnst1);
        let fs1_dps = cnst1 * beta * exp_rho;
        let fs2 = (chi - 1.0 + fs1).sqrt();
        let fs2_dps = (beta + fs1_dps) / (fs2 + fs2);
        (fs1, fs1_dps, fs2, fs2_dps, 0.0)
    } else if chi < LARGE_ARG {
        let exp_chi = chi.exp();
        let fs1 = cfs1 * (exp_chi - 1.0);
        let fs1_dps = cfs1 * beta * exp_chi;
        let fs2 = (chi - 1.0 + fs1).sqrt();
        let fs2_dps = (beta + fs1_dps) / (fs2 + fs2);
        (fs1, fs1_dps, fs2, fs2_dps, 0.0)
    } else {
        // avoid exp(Chi) overflow (eval.c:3570-3574).
        let exp_bps = safe_exp((beta * ps).min(EXP_LIMIT));
        let exp_bv = safe_exp((beta * vref).min(EXP_LIMIT));
        let fs1 = cnst1 * (exp_bps - exp_bv);
        let fs1_dps = cnst1 * beta * exp_bps;
        let fs2 = (chi - 1.0 + fs1).sqrt();
        let fs2_dps = (beta + fs1_dps) / (fs2 + fs2);
        (fs1, fs1_dps, fs2, fs2_dps, 0.0)
    }
}

/// zone-D1/D2 analytical `Ps0` initial guess: the cubic solution of
/// Qs = Qb0 with Qb0 approximated to a 3-degree polynomial
/// (eval.c:3478-3488 / 2098-2136).
fn ps0_ini_cubic(ty: f64, beta: f64, beta_inv: f64, fac1: f64, vbs: f64) -> f64 {
    let t1 = 1.0 / (CN_NC3 * beta * fac1);
    let t2 = 81.0 + 3.0 * t1;
    let t3 = -2916.0 - 81.0 * t1 + 27.0 * t1 * ty;
    let t5 = fn_pow(t3 + (4.0 * t2 * t2 * t2 + t3 * t3).sqrt(), C_1O3);
    let tx = 3.0 - (C_2P_1O3 * t2) / (3.0 * t5) + 1.0 / (3.0 * C_2P_1O3) * t5;
    tx * beta_inv + vbs
}

impl HisimModel {
    /// Forward drain-current evaluation, faithful to `hsm2eval.c` PART-1
    /// (normal-mode MOSFET, CORECIP=1, flg_qme=0, no rsrd loop).
    ///
    /// Biases are in the NMOS normal frame: `vds >= 0` is required (the
    /// caller swaps drain/source for reverse mode).
    ///
    /// `unused_assignments` is allowed because the direct C port keeps the
    /// upstream's initialize-then-overwrite flow for loop-carried values.
    #[allow(clippy::too_many_lines, unused_assignments)]
    fn eval_ids(&self, c: &EvalConsts, vgs_in: f64, vds_in: f64, vbs_in: f64) -> EvalOut {
        let beta = c.beta;
        let beta_inv = c.beta_inv;
        let vfb = self.vfbc;

        // ── Clamp too-large biases (eval.c:1175-1240) ────────────────────
        let mut vbs_max = 0.8;
        for lim in [c.pb2, c.pb20, c.pb2c] {
            if lim - self.vzadd0 < vbs_max {
                vbs_max = lim - self.vzadd0;
            }
        }
        let mut vbs_bnd = 0.4;
        if vbs_bnd > vbs_max * 0.5 {
            vbs_bnd = 0.5 * vbs_max;
        }
        let vbse = vbs_in;
        let (vbs, vbsc_dvbse) = if vbse > vbs_bnd {
            let (ty, dy) = fn_su_poly4(vbse - vbs_bnd, vbs_max - vbs_bnd);
            (vbs_bnd + ty, dy)
        } else if vbse < VBS_MIN_CLAMP {
            (VBS_MIN_CLAMP, 0.0)
        } else {
            (vbse, 1.0)
        };
        let vds = vds_in;
        let vgs = vgs_in;

        // ── Vzadd / Vxsz symmetry shift (eval.c:1857-1880) ───────────────
        let (mut vzadd, _) = fn_sym_add(vbsc_dvbse * vds / 2.0, self.vzadd0);
        if vzadd < PS_CONV {
            vzadd = PS_CONV;
        }
        let vbsz = vbs + vzadd;
        let vdsz = vds + 2.0 * vzadd;
        let vgsz = vgs + vzadd;
        let vbsz2 = vbsz;

        // ── FMDVDS symmetry factor (eval.c:1884-1907) ────────────────────
        let t1f = c.qnsub_esi * c.cox0_inv * c.cox0_inv;
        let t2f = vgs - vfb;
        let t3f = 1.0 + 2.0 / t1f * (t2f - beta_inv - vbs);
        let (t4f, _) = fn_sz(t3f, 1e-3);
        let txf = t4f.sqrt();
        let pslsat = t2f + t1f * (1.0 - txf);
        let (vdsat_s, _) = fn_sl(pslsat - c.pb2, 0.1, 5e-2);
        let (txp, _) = fn_su_poly4(vds / vdsat_s, 1.0);
        let fmdvds = txp * txp;

        // ── flg_qme = 0 constants (eval.c:1925-1945, 3107-3112) ──────────
        let cox = c.cox0;
        let cox_inv = c.cox0_inv;
        let cnst_coxi = c.cnst0 * c.cnst0 * cox_inv * cox_inv;
        let fac1 = c.cnst0 * cox_inv;
        let fac1p2 = fac1 * fac1;
        // Ps0_min: approx. Poisson solution at Vgs_min (eval.c:2019-2021).
        let ps0_min = 2.0 * beta_inv * (-c.vgs_min / fac1).ln();

        // ── CORECIP initial value for PS0Z_SCE (eval.c:2060-2192) ────────
        let vthq = c.pb20 + vfb + (c.qnsub_esi2 * (c.pb20 - vbsz)).sqrt() * c.cox0_inv;
        let mut ps0z_sce = {
            let vth_ini = vthq;
            let mut tx = 4.0 * (beta * (vgs - vbs) - 1.0) / (fac1p2 * c.beta2);
            tx += 1.0;
            let t3 = if tx > EPSM10 {
                tx.sqrt()
            } else {
                EPSM10.sqrt()
            };
            let ps0_ini_a = vgs + fac1p2 * beta * 0.5 * (1.0 - t3);
            let chi = beta * (ps0_ini_a - vbs);
            if chi < ZNBD3 {
                ps0_ini_cubic(beta * (vgs - vbs), beta, beta_inv, fac1, vbs)
            } else if vgs <= vth_ini {
                ps0_ini_a
            } else {
                let t0 = vgs - vfb;
                let t2 = t0 * t0 / (c.cnst1 * cnst_coxi);
                let t3b = beta + 2.0 / t0;
                let ps0_ini_b = (t2 + SMALL).ln() / t3b;
                let (y, _, _) = fn_su2(ps0_ini_a, ps0_ini_b, C_PS0INI_2);
                y
            }
        };

        // ── SCE loop state ───────────────────────────────────────────────
        let mut nnn: usize = 0;
        let mut ps0 = 0.0_f64;
        let mut psl = 0.0_f64;
        let mut pds;
        let mut vth;
        let mut vgp;
        let mut vdsat_out = vdsat_s;

        // Values carried out of the loop for the current/charge evaluation.
        let (mut fs01, mut fs02, mut fs0_dps0) = (0.0, 0.0, -1.0);
        let (mut xi0, mut xi0p12, mut xi0p32) = (0.0, 0.0, 0.0);
        let mut xilp32 = 0.0;
        let mut qb0_src = 0.0;
        let mut qn0 = 0.0;
        let mut vgvt = 0.0;
        let mut fd2 = 0.0;
        let mut flg_zone = 3;

        loop {
            // ── dVth: SCE/RSCE (corecip, flg_qme=0; eval.c:2663-3095) ────
            let qb0_dvth = c.qnsub_esi2.sqrt();
            let vthp = ps0z_sce + vfb + qb0_dvth * cox_inv + c.ptovr;

            // dVthLP (eval.c:2723-2866); codqb=0 forces dqb=0.
            let (dvthlp, dvthlp_dps0z) = if self.lp != 0.0 {
                let t3b = self.bs2 - vbsz2 + SMALL;
                let t4b = (t3b * t3b + 4.0 * VTH_DLT).sqrt();
                let t5b = 0.5 * (t3b + t4b);
                let bs12 = self.bs1 / t5b;
                let t1l = 0.93 * (ps0z_sce + ps0_min - vbsz2);
                let (_t10, _, _t10_dxmax) = fn_su2(bs12, t1l, VTH_DLT);
                // dqb = 0 (codqb == 0, eval.c:1037/2784).
                let vth0 = ps0z_sce + vfb + (2.0 * C_QE * c.nsubc * C_ESI).sqrt() * cox_inv;
                let t5v =
                    2.0 * (self.vbi - c.pb20) * C_ESI * cox_inv * c.wdplp / (self.lp * self.lp);
                let (t6v, t6dx) = fn_sz(ps0z_sce - vbsz, C_SCE_DLT);
                let t6v = t6v + SMALL;
                let dvth0 = t5v * t6v.sqrt();
                let dvth0_dps0z = t5v * 0.5 / t6v.sqrt() * t6dx;
                let t1v = vthp - vth0;
                let t9v = ps0z_sce - vbsz2;
                let t3v = self.scp1 + self.scp3 * t9v / self.lp + 0.0 * vdsz; // SCP2 forced 0 (corecip)
                let t3v_dps0z = self.scp3 / self.lp;
                let vdx = self.scp21 + vdsz;
                let vdx2 = vdx * vdx + SMALL;
                (
                    t1v * dvth0 * t3v - c.msc / vdx2,
                    t1v * dvth0_dps0z * t3v + t1v * dvth0 * t3v_dps0z,
                )
            } else {
                (0.0, 0.0)
            };

            // dVthSC (eval.c:2951-3015). SC2/SC4 forced 0 when corecip.
            let t3s = c.lgate - self.parl2;
            let t4s = 1.0 / (t3s * t3s);
            let t5s = self.sc3 / c.lgate;
            let t6s = self.sc1 + t5s * (ps0z_sce - vbsz);
            let t1s = t6s;
            let t2s = C_ESI * cox_inv * c.wdpl * 2.0 * (self.vbi - c.pb20) * t4s;
            let a = t2s * t1s;
            let a_dps0z = t2s * t5s;
            let t7s = ps0z_sce - vbsz + ps0_min;
            let arg0 = 0.01;
            let (t8s, t8s_dps0z) = if t7s > arg0 {
                let s = t7s.sqrt();
                (s, 0.5 / s)
            } else {
                let s0 = arg0.sqrt();
                (s0 + 0.5 / s0 * (t7s - arg0), 0.5 / s0)
            };
            let dvthsc = a * t8s;
            let dvthsc_dps0z = a * t8s_dps0z + a_dps0z * t8s;

            // dVthW (eval.c:3067-3081). WFC in F/cm² × 1e4 (set.c:1157).
            let t1w = 1.0 / cox - 1.0 / (cox + self.wfc * C_M2CM_P2 / c.weff_nf);
            let dvthw = qb0_dvth * t1w + self.wvth0 / c.wg;

            let dvth = dvthsc + dvthlp + dvthw;
            let dvth_dps0z = dvthsc_dps0z + dvthlp_dps0z;

            // Vth for OP / zone selection (eval.c:3100-3101).
            vth = c.pb2 + vfb + (c.qnsub_esi2 * c.pb2).sqrt() * c.cox0_inv - dvth;

            // Vgp (eval.c:3161-3171); dPpg = 0 (PGD1 = 0).
            vgp = vgs - vfb + dvth;
            let vgp_dps0z = dvth_dps0z;
            let vgpz = vgsz - vfb + dvth;

            // Accumulation zone (eval.c:3183): Ids = 0.
            let vgs_fb = vfb - dvth + vbs;
            if vgs < vgs_fb {
                return EvalOut {
                    ids: 0.0,
                    vth,
                    vdsat: vdsat_out,
                };
            }

            // ── Ps0 initial guess (eval.c:3400-3521) ─────────────────────
            let mut tx0 = 1.0 + 4.0 * (beta * (vgp - vbs) - 1.0) / (fac1p2 * c.beta2);
            tx0 = tx0.max(EPSM10);
            let ps0_ini_a = vgp + fac1p2 * beta * 0.5 * (1.0 - tx0.sqrt());
            if nnn == 0 {
                let chi_ini = beta * (ps0_ini_a - vbs);
                let mut ps0_ini = if chi_ini < ZNBD3 {
                    ps0_ini_cubic(beta * (vgp - vbs), beta, beta_inv, fac1, vbs)
                } else if vgs <= vth {
                    ps0_ini_a
                } else {
                    let t2 = vgp * vgp / (c.cnst1 * cnst_coxi);
                    let t3b = beta + 2.0 / vgp;
                    let ps0_ini_b = (t2 + SMALL).ln() / t3b;
                    let (y, _) = fn_su(ps0_ini_a, ps0_ini_b, C_PS0INI_2);
                    y
                };
                let txmin = vbs + PS_CONV / 2.0;
                if ps0_ini < txmin {
                    ps0_ini = txmin;
                }
                ps0 = ps0_ini;
            }
            let psl_lim = ps0_ini_a;

            // ── Ps0 Newton solve (eval.c:3529-3606) ──────────────────────
            let exp_bvbs = safe_exp((beta * vbs).min(EXP_LIMIT));
            let cfs1 = c.cnst1 * exp_bvbs;
            let mut chi = beta * (ps0 - vbs);
            let mut fb0 = 0.0;
            let mut flg_conv = false;
            for _ in 0..=LP_S0_MAX {
                chi = beta * (ps0 - vbs);
                let (f1, _f1d, f2, f2d, fb) =
                    fs_pieces(chi, beta, cfs1, c.cnst1, ps0, vbs, false, 0.0);
                fs01 = f1;
                fs02 = f2;
                fb0 = fb;
                let fs0 = vgp - ps0 - fac1 * fs02;
                fs0_dps0 = -1.0 - fac1 * f2d;
                if flg_conv {
                    break;
                }
                let mut dps0 = -fs0 / fs0_dps0;
                let dplim = 0.5 * DP_MAX * (1.0 + ps0.abs().max(1.0));
                if dps0.abs() > dplim {
                    dps0 = dplim * dps0.signum();
                }
                ps0 += dps0;
                let txmin = vbs + PS_CONV / 2.0;
                if ps0 < txmin {
                    ps0 = txmin;
                }
                if dps0.abs() <= PS_CONV && fs0.abs() <= GS_CONV {
                    flg_conv = true;
                }
            }

            // Xi0 quantities (eval.c:3648-3698).
            if chi < ZNBD5 {
                xi0 = fb0 * fb0 + EPSM10;
                xi0p12 = fb0 + EPSM10;
                xi0p32 = fb0 * fb0 * fb0 + EPSM10;
            } else {
                xi0 = chi - 1.0;
                xi0p12 = xi0.sqrt();
                xi0p32 = xi0 * xi0p12;
            }

            // Qb0/Qn0 at the source (eval.c:3717-3723).
            qb0_src = c.cnst0 * xi0p12;
            qn0 = c.cnst0 * fs01 / (fs02 + xi0p12);

            // Zone flags + FD2 (eval.c:3735-3785).
            if chi < ZNBD5 {
                if chi < ZNBD3 {
                    flg_zone = 1; // D1
                } else {
                    flg_zone = 2; // D2
                    let txz = (chi - ZNBD3) / (ZNBD5 - ZNBD3);
                    fd2 = txz * txz * txz * (10.0 + txz * (-15.0 + txz * 6.0));
                }
            } else {
                flg_zone = 3; // D3
            }

            // VgVt (eval.c:3792) and the nonconductive fast path (3800).
            vgvt = qn0 * cox_inv;
            if vgvt <= VGVT_SMALL {
                // zone D4: Ids = 0. Do the corecip PS0Z Newton (eval.c:3843-3898).
                let ps0_dps0z = -vgp_dps0z / fs0_dps0;
                let (mut pzadd, _) = fn_sym_add(vds * 0.5, self.pzadd0);
                if pzadd < EPSM10 {
                    pzadd = EPSM10;
                }
                let ps0z = ps0 + pzadd;
                let g = ps0z_sce - ps0z;
                let delta = -g / (1.0 - ps0_dps0z);
                ps0z_sce += delta;
                nnn += 1;
                if delta.abs() > PS0_SCE_TOL && nnn < MAX_LOOP_SCE {
                    continue;
                }
                return EvalOut {
                    ids: 0.0,
                    vth,
                    vdsat: vdsat_out,
                };
            }

            // ── Vdseff (eval.c:3915-3980) ────────────────────────────────
            let t2v = c.qnsub_esi / (cox * cox);
            let t5v = vgpz - beta_inv - vbsz;
            let (t1v, _) = fn_sz(1.0 + 2.0 / t2v * t5v, 0.05);
            let t1v = t1v + SMALL;
            let t3v = t1v.sqrt();
            let (t10v, _) = fn_sz(vgpz + t2v * (1.0 - t3v), 0.01);
            let t10v = t10v + EPSM10;
            vdsat_out = t10v;
            let t1r = vds / t10v;
            let t7r = fn_pow(t1r, c.ddlt - 1.0) * t1r;
            let t6r = fn_pow(1.0 + t7r, 1.0 / c.ddlt - 1.0) * (1.0 + t7r);
            let vdseff = vds / t6r;

            let exp_bvbsvds = safe_exp((beta * (vbs - vdseff)).min(EXP_LIMIT));

            // ── Psl Newton solve (eval.c:3989-4171) ──────────────────────
            let cfs1l = c.cnst1 * exp_bvbsvds;
            let mut fsl_dpsl = -1.0;
            let mut chi_l;
            let mut fbl = 0.0;
            if vdseff <= 0.0 {
                pds = 0.0;
                psl = ps0;
                chi_l = beta * (psl - vbs);
                let (_f1, _f1d, _f2, f2d, fb) =
                    fs_pieces(chi_l, beta, cfs1l, c.cnst1, psl, vbs, true, vdseff);
                fsl_dpsl = -1.0 - fac1 * f2d;
                fbl = fb;
            } else {
                if nnn == 0 {
                    let pds_max = (psl_lim - ps0).max(0.0);
                    let (mut pds_ini, _) = fn_su(vdseff, (1.0 + C_PSLINI_1) * pds_max, C_PSLINI_2);
                    pds_ini = pds_ini.min(pds_max);
                    pds_ini = pds_ini.clamp(0.0, vdseff);
                    pds = pds_ini;
                    psl = ps0 + pds;
                } else {
                    // Take the Psl solution from the previous SCE pass.
                }
                let txmin = vbs + PS_CONV / 2.0;
                if psl < txmin {
                    psl = txmin;
                }
                let mut flg_conv_l = false;
                chi_l = beta * (psl - vbs);
                for _ in 0..=LP_SL_MAX {
                    chi_l = beta * (psl - vbs);
                    let (_f1, _f1d, f2, f2d, fb) =
                        fs_pieces(chi_l, beta, cfs1l, c.cnst1, psl, vbs, true, vdseff);
                    fbl = fb;
                    let fsl = vgp - psl - fac1 * f2;
                    fsl_dpsl = -1.0 - fac1 * f2d;
                    if flg_conv_l {
                        break;
                    }
                    let mut dpsl = -fsl / fsl_dpsl;
                    let dplim = 0.5 * DP_MAX * (1.0 + psl.abs().max(1.0));
                    if dpsl.abs() > dplim {
                        dpsl = dplim * dpsl.signum();
                    }
                    psl += dpsl;
                    let txmin = vbs + PS_CONV / 2.0;
                    if psl < txmin {
                        psl = txmin;
                    }
                    if dpsl.abs() <= PS_CONV && fsl.abs() <= GS_CONV {
                        flg_conv_l = true;
                    }
                }
            }

            // Xil quantities (eval.c:4206-4250) — only Xil^{3/2} feeds F11.
            if chi_l < ZNBD5 {
                xilp32 = fbl * fbl * fbl + EPSM10;
            } else {
                let xl = chi_l - 1.0;
                xilp32 = xl * xl.sqrt();
            }

            // Pds (eval.c:4255-4264).
            pds = psl - ps0;
            if pds < 0.0 {
                pds = 0.0;
                psl = ps0;
            }

            // ── corecip PS0Z Newton update (eval.c:4277-4357) ────────────
            let ps0_dps0z = -vgp_dps0z / fs0_dps0;
            let psl_dps0z = -vgp_dps0z / fsl_dpsl;
            let pds_dps0z = if pds < PS_CONV {
                0.0
            } else {
                psl_dps0z - ps0_dps0z
            };
            let (mut pzadd, pz_dx) = fn_sym_add((vds - pds) / 2.0, self.pzadd0);
            let mut pzadd_dps0z = pz_dx / 2.0 * (-pds_dps0z);
            if pzadd < EPSM10 {
                pzadd = EPSM10;
                pzadd_dps0z = 0.0;
            }
            let ps0z = ps0 + pzadd;
            let ps0z_dps0z = ps0_dps0z + pzadd_dps0z;
            let g = ps0z_sce - ps0z;
            let delta = -g / (1.0 - ps0z_dps0z);
            ps0z_sce += delta;
            nnn += 1;
            if delta.abs() > PS0_SCE_TOL && nnn < MAX_LOOP_SCE {
                continue;
            }
            break;
        }

        // ─────────────────────────────────────────────────────────────────
        // Idd (eval.c:4368-4460).
        // ─────────────────────────────────────────────────────────────────
        let eta = beta * pds / xi0;
        let eta1 = eta + 1.0;
        let eta1p12 = eta1.sqrt();
        let eta1p32 = eta1p12 * eta1;
        let eta1p52 = eta1p32 * eta1;
        let zeta12 = 1.0 / (eta1p12 + 1.0);
        let zeta32 = 1.0 / (eta1p32 + 1.0);
        let zeta52 = 1.0 / (eta1p52 + 1.0);

        let f00 = zeta12 / xi0p12;
        let f10 = C_2O3 * xi0p12 * zeta32 * (3.0 + eta * (3.0 + eta));
        let f30 = 4.0 / (15.0 * beta)
            * xi0p32
            * zeta52
            * (5.0 + eta * (10.0 + eta * (10.0 + eta * (5.0 + eta))));
        let f11 = ps0 * f10 + C_2O3 * beta_inv * xilp32 - f30;

        let t1i = vgp + beta_inv - 0.5 * (2.0 * ps0 + pds);
        let fdd = beta * cox * t1i + beta * c.cnst0 * (f00 - f10);
        let idd = pds * fdd;

        // ─────────────────────────────────────────────────────────────────
        // CLM + charges (skipped entirely in zone D1; eval.c:4465-4751).
        // ─────────────────────────────────────────────────────────────────
        let mut lred = 0.0;
        let (qbu, qiu);
        if flg_zone == 1 {
            qbu = qb0_src;
            qiu = qn0;
        } else {
            // Channel-length modulation (eval.c:4472-4582).
            if self.clm2 >= EPSM10 || self.clm3 >= EPSM10 {
                let wd = c.wdpl * (psl - vbs).max(EPSM10).sqrt();
                let t2c = self.clm3 * qn0 / wd;
                let t5c = self.clm2 * c.qnsub + t2c;
                let t4c = C_ESI / t5c;
                let mut psdl = self.clm1 * (vds + ps0) + (1.0 - self.clm1) * psl;
                if psdl > ps0 + vds - EPSM10 {
                    psdl = ps0 + vds - EPSM10;
                }
                let t6c = psdl - psl;
                let t5i = idd / (beta * qn0);
                let t10c = c.qnsub / C_ESI;
                let t7c = (2.0 * t5i + 2.0 * t10c * t6c * t4c + C_E0_P2 * t4c) / c.leff * t4c;
                let t8c = 4.0 * (2.0 * t10c * t6c + C_E0_P2) * t4c * t4c;
                let t9c = (t7c * t7c + t8c).sqrt();
                lred = 0.5 * (-t7c + t9c);
                lred *= fmdvds;
            }
            lred *= c.clmmod;

            // Qbu (eval.c:4588-4608).
            let t2q = (vgp + beta_inv) * f10 - f11;
            let qbnm = c.cnst0 * (c.cnst0 * (1.5 - (xi0 + 1.0) - 0.5 * beta * pds) + cox * t2q);
            let mut qbu_v = beta * qbnm / fdd;

            // Qiu via Alpha (eval.c:4615-4672).
            let dt_pds = 2.0 * fac1 * (f10 - xi0p12);
            let achi = pds + dt_pds;
            let txa = achi / vgvt;
            let tya = fn_cp4(txa, 1.0);
            let alpha = 1.0 - tya;
            let qinm = 1.0 + alpha * (1.0 + alpha);
            let qidn = (1.0 + alpha).max(EPSM10);
            let mut qiu_v = C_2O3 * vgvt * qinm / qidn * cox;

            // zone-D2 interpolation (eval.c:4708-4751).
            if flg_zone == 2 {
                qbu_v = fd2 * qbu_v + (1.0 - fd2) * qb0_src;
                if qbu_v < 0.0 {
                    qbu_v = 0.0;
                }
                qiu_v = fd2 * qiu_v + (1.0 - fd2) * qn0;
                if qiu_v < 0.0 {
                    qiu_v = 0.0;
                }
                lred *= fd2;
            }
            qbu = qbu_v;
            qiu = qiu_v;
        }

        // ─────────────────────────────────────────────────────────────────
        // Mobility (eval.c:4754-4893).
        // ─────────────────────────────────────────────────────────────────
        let mut lch = c.leff - lred;
        if lch < 1.0e-9 {
            lch = 1.0e-9;
        }

        let t1m = c.ndep_o_esi / C_M2CM;
        let t2m = c.ninv_o_esi / C_M2CM;
        let t3p = (pds * pds + self.vzadd0).sqrt();
        let pdsz = t3p - self.vzadd0.sqrt();
        let t4m = 1.0 + pdsz * c.ninvd;
        let eeff = (t1m * qbu + t2m * qiu) / t4m;

        let t8m = fn_pow(eeff, self.mueph0);
        let t6m = fn_pow(eeff, c.muesr);
        let rns = qiu / (C_QE * C_M2CM_P2);
        let muun_inv =
            1.0 / (c.muecb0 + c.muecb1 * rns / 1.0e11) + c.mphn0 * t8m + t6m / self.muesr1;
        let muun = 1.0 / muun_inv / C_M2CM_P2; // CGS → MKS

        // Velocity saturation (eval.c:4830-4893).
        let ty = idd / (beta * (qn0 + SMALL) * lch);
        let t2y = 0.2 * c.vmax / muun;
        let ey = (ty * ty + t2y * t2y).sqrt();
        let em = muun * ey;
        let t1v = em / c.vmax;
        let bb = self.bb;
        let t4v = if (bb - 1.0).abs() <= EPSM10 {
            1.0 + t1v
        } else if (bb - 2.0).abs() <= EPSM10 {
            1.0 + t1v * t1v
        } else {
            1.0 + fn_pow(t1v, bb)
        };
        let t5v = if (bb - 1.0).abs() <= EPSM10 {
            1.0 / t4v
        } else if (bb - 2.0).abs() <= EPSM10 {
            1.0 / t4v.sqrt()
        } else {
            t4v * fn_pow(t4v, -1.0 / bb - 1.0)
        };
        let mu = muun * t5v;

        // ─────────────────────────────────────────────────────────────────
        // Ids (eval.c:4900-4912); PTL/GDL/rsrd extensions off at defaults.
        // ─────────────────────────────────────────────────────────────────
        let beta_wl = c.weff_nf * beta_inv / lch;
        let ids = beta_wl * idd * mu;

        EvalOut {
            ids: if ids.is_finite() { ids } else { 0.0 },
            vth,
            vdsat: vdsat_out,
        }
    }

    /// Compute the HiSIM2 operating point and NR companion model.
    ///
    /// Same signature shape as MOS3's `companion` so the upstream stamping
    /// and bypass-cache infrastructure can be reused unchanged. `l_eff` is
    /// the gate length after the instance's lateral-diffusion adjustment;
    /// with the ported parameter subset (LL = 0) it coincides with `Lgate`.
    ///
    /// `gm`/`gds`/`gmbs` are central finite differences of the forward
    /// eval (`h ≈ 1e-4·max(1, |V|)`) so the Jacobian is consistent with
    /// `cdrain` — exactly what the outer NR loop needs.
    pub fn companion(&self, vgs: f64, vds: f64, vbs: f64, w: f64, l_eff: f64) -> MosfetCompanion {
        let vt = VT_NOM;
        // Mode handling: swap drain/source if Vds < 0 so the I-V equations
        // are always written in the normal-mode frame (as hsm2ld.c does).
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

        let c = self.eval_consts(w, l_eff);
        let out = self.eval_ids(&c, vgs_eff, vds_eff, vbs_eff);
        let cdrain = if out.ids.is_finite() && out.ids >= 0.0 {
            out.ids
        } else {
            0.0
        };

        let ids_at = |vg: f64, vd: f64, vb: f64| -> f64 {
            let o = self.eval_ids(&c, vg, vd, vb);
            if o.ids.is_finite() { o.ids } else { 0.0 }
        };

        // Central finite differences (forward-only near vds = 0 where the
        // normal-mode frame forbids negative Vds).
        let hg = 1e-4 * vgs_eff.abs().max(1.0);
        let mut gm = (ids_at(vgs_eff + hg, vds_eff, vbs_eff)
            - ids_at(vgs_eff - hg, vds_eff, vbs_eff))
            / (2.0 * hg);

        let hd = 1e-4 * vds_eff.abs().max(1.0);
        let mut gds = if vds_eff >= hd {
            (ids_at(vgs_eff, vds_eff + hd, vbs_eff) - ids_at(vgs_eff, vds_eff - hd, vbs_eff))
                / (2.0 * hd)
        } else {
            (ids_at(vgs_eff, vds_eff + hd, vbs_eff) - cdrain) / hd
        };

        let hb = 1e-4 * vbs_eff.abs().max(1.0);
        let mut gmbs = (ids_at(vgs_eff, vds_eff, vbs_eff + hb)
            - ids_at(vgs_eff, vds_eff, vbs_eff - hb))
            / (2.0 * hb);

        // Floors for matrix conditioning.
        if !gds.is_finite() || gds < GDS_FLOOR {
            gds = GDS_FLOOR;
        }
        if !gm.is_finite() {
            gm = 0.0;
        }
        if !gmbs.is_finite() {
            gmbs = 0.0;
        }

        // Companion-model Norton equivalent:
        // ceq_d = Id - gm·Vgs - gds·Vds - gmbs·Vbs (in *eff coordinates).
        let ceq_d = cdrain - gm * vgs_eff - gds * vds_eff - gmbs * vbs_eff;
        let ceq_bs = cbs_current - gbs_val * vbs;
        let ceq_bd = cbd_current - gbd_val * vbd;

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
            vdsat: out.vdsat,
            von: out.vth,
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
    fn faithful_constants_match_hsm2temp() {
        // Validate the HiSIM2 constants against hand-computed values from
        // hsm2temp.c (using HiSIM's own physical constants from
        // hsm2evalenv.h) for the golden card's NSUBC=5e17 cm^-3, TOX=2nm.
        let md = ModelParams {
            name: "nch".into(),
            kind: "NMOS".into(),
            params: vec![
                ("TOX".into(), 2.0e-9),
                ("NSUBC".into(), 5.0e17),
                ("VFBC".into(), -0.5),
            ],
        };
        let m = HisimModel::from_params(&md);

        // beta = q/(kB·T) at 300.15 K ≈ 38.66 /V (HiSIM C_QE/C_KB).
        assert!((m.beta - 38.66).abs() < 0.2, "beta = {}", m.beta);
        // Nsub_eff (long-channel model constant) = 5e17 cm^-3 → 5e23 m^-3.
        assert!((m.nsub_eff - 5.0e23).abs() / 5.0e23 < 1e-9);
        // cnst0 = sqrt(2·εsi·q·Nsub/beta) with HiSIM C_ESI/C_QE.
        let expect_cnst0 = (2.0 * super::C_ESI * super::C_QE * 5.0e23 / m.beta).sqrt();
        assert!((m.cnst0 - expect_cnst0).abs() / expect_cnst0 < 1e-9);
        // Pb2 = (2/beta)·ln(Nsub/Nin), with Nin = C_Nin0 = 1.04e16 m^-3.
        let expect_pb2 = (2.0 / m.beta) * (5.0e23_f64 / 1.04e16).ln();
        assert!((m.pb2 - expect_pb2).abs() < 1e-9, "pb2 = {}", m.pb2);
        // Physically Pb2 (= 2φB) should sit around 0.9 V for this doping.
        assert!(m.pb2 > 0.85 && m.pb2 < 1.0, "pb2 = {}", m.pb2);
        // cnst1 = (Nin/Nsub)² — tiny.
        assert!(m.cnst1 > 0.0 && m.cnst1 < 1e-12);
    }

    #[test]
    fn from_params_parses_subset() {
        let md = ModelParams {
            name: "M".to_string(),
            kind: "NMOS".to_string(),
            params: vec![
                ("LEVEL".to_string(), 68.0),
                ("TOX".to_string(), 5e-9),
                ("NSUBC".to_string(), 2e17),
                ("VFBC".to_string(), -1.0),
                ("MUECB0".to_string(), 350.0),
                ("RD".to_string(), 2.0),
                // Unknown HiSIM param — must not crash.
                ("COSYM".to_string(), 1.0),
            ],
        };
        let m = HisimModel::from_params(&md);
        assert!((m.tox - 5e-9).abs() < 1e-20);
        assert!((m.muecb0 - 350.0).abs() < 1e-9);
        assert!((m.rd - 2.0).abs() < 1e-9);
    }

    #[test]
    fn ps0_newton_converges_strong_inversion() {
        // The faithful eval must produce a surface potential pinned near
        // 2φB + a few Vt in strong inversion — observable through a finite,
        // positive drain current that grows sub-quadratically.
        let m = nmos_basic();
        let c = m.eval_consts(10e-6, 1e-6);
        let out = m.eval_ids(&c, 2.0, 1.0, 0.0);
        assert!(out.ids.is_finite());
        assert!(out.ids > 0.0, "strong inversion must conduct");
        assert!(out.vth.is_finite());
    }

    #[test]
    fn subthreshold_current_is_exponential() {
        // In weak inversion Id should scale ~exp(Vgs/nVt): two 60mV steps
        // must give nearly the same current ratio.
        let m = nmos_basic();
        let c = m.eval_consts(10e-6, 1e-6);
        let vth = m.eval_ids(&c, 0.5, 0.1, 0.0).vth;
        let i1 = m.eval_ids(&c, vth - 0.30, 0.1, 0.0).ids;
        let i2 = m.eval_ids(&c, vth - 0.24, 0.1, 0.0).ids;
        let i3 = m.eval_ids(&c, vth - 0.18, 0.1, 0.0).ids;
        assert!(i1 > 0.0 && i2 > i1 && i3 > i2);
        let r1 = i2 / i1;
        let r2 = i3 / i2;
        assert!(
            (r1 / r2 - 1.0).abs() < 0.2,
            "subthreshold slope should be constant: r1={r1} r2={r2}"
        );
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
    fn fd_jacobian_consistent_with_cdrain() {
        // gm from the companion must match a direct secant of cdrain.
        let m = nmos_basic();
        let dv = 1e-3;
        let c1 = m.companion(1.5, 1.0, 0.0, 10e-6, 1e-6);
        let c2 = m.companion(1.5 + dv, 1.0, 0.0, 10e-6, 1e-6);
        let gm_secant = (c2.cdrain - c1.cdrain) / dv;
        assert!(
            (c1.gm - gm_secant).abs() <= 0.05 * gm_secant.abs().max(1e-12),
            "gm={} secant={}",
            c1.gm,
            gm_secant
        );
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
