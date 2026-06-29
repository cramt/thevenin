//! BSIM2 MOSFET device model (LEVEL=5).
//!
//! Ports the ngspice BSIM2 implementation (Berkeley Short-Channel IGFET Model
//! version 2; `ngspice-upstream/src/spicelib/devices/bsim2/`) — the successor
//! to BSIM1 and predecessor to BSIM3. The model is parameter-rich: ~120
//! base model parameters plus L/W sensitivity coefficients for nearly every
//! one, totalling ~340 `.model`-recognised keywords. We collapse the W/L
//! binning step into a single per-instance `SizeDependParam` struct that
//! mirrors `struct bsim2SizeDependParam` from `bsim2def.h`.
//!
//! DC + companion-model NR is in scope; charge/capacitance equations
//! (`b2moscap.c`) and noise are deferred. Stamping follows the same shape
//! as the other MOSFET ports (companion-model `gm`/`gds`/`gmbs` + bulk
//! junction diodes).

use crate::model_params::ModelParams;

use crate::diode::VT_NOM;
use crate::mosfet::{MosfetCompanion, MosfetType};

/// Internal NR `gmin` floor (ngspice's `CKTgmin` default).
const GMIN: f64 = 1e-12;

/// BSIM2 model parameters. All entries here are the "base" values keyed by
/// process; per-instance W/L sensitivities are resolved into the
/// `Bsim2SizeDependParam` cache.
#[derive(Debug, Clone)]
pub struct Bsim2Model {
    pub mos_type: MosfetType,

    // ── Process / threshold (vfb, phi, k1, k2 and L/W sensitivities) ─────
    pub vfb0: f64,
    pub vfb_l: f64,
    pub vfb_w: f64,
    pub phi0: f64,
    pub phi_l: f64,
    pub phi_w: f64,
    pub k1_0: f64,
    pub k1_l: f64,
    pub k1_w: f64,
    pub k2_0: f64,
    pub k2_l: f64,
    pub k2_w: f64,
    pub eta0_0: f64,
    pub eta0_l: f64,
    pub eta0_w: f64,
    pub eta_b0: f64,
    pub eta_b_l: f64,
    pub eta_b_w: f64,
    pub delta_l: f64,
    pub delta_w: f64,

    // ── Mobility (Mob[0,s,2,3,4] with B/G/Vbs/Vgs sensitivities) ─────────
    pub mob0_0: f64,
    pub mob0b_0: f64,
    pub mob0b_l: f64,
    pub mob0b_w: f64,
    pub mobs0_0: f64,
    pub mobs0_l: f64,
    pub mobs0_w: f64,
    pub mobsb_0: f64,
    pub mobsb_l: f64,
    pub mobsb_w: f64,
    pub mob20_0: f64,
    pub mob20_l: f64,
    pub mob20_w: f64,
    pub mob2b_0: f64,
    pub mob2b_l: f64,
    pub mob2b_w: f64,
    pub mob2g_0: f64,
    pub mob2g_l: f64,
    pub mob2g_w: f64,
    pub mob30_0: f64,
    pub mob30_l: f64,
    pub mob30_w: f64,
    pub mob3b_0: f64,
    pub mob3b_l: f64,
    pub mob3b_w: f64,
    pub mob3g_0: f64,
    pub mob3g_l: f64,
    pub mob3g_w: f64,
    pub mob40_0: f64,
    pub mob40_l: f64,
    pub mob40_w: f64,
    pub mob4b_0: f64,
    pub mob4b_l: f64,
    pub mob4b_w: f64,
    pub mob4g_0: f64,
    pub mob4g_l: f64,
    pub mob4g_w: f64,

    // ── Mobility degradation (Ua/Ub/U1) ─────────────────────────────────
    pub ua0_0: f64,
    pub ua0_l: f64,
    pub ua0_w: f64,
    pub uab_0: f64,
    pub uab_l: f64,
    pub uab_w: f64,
    pub ub0_0: f64,
    pub ub0_l: f64,
    pub ub0_w: f64,
    pub ubb_0: f64,
    pub ubb_l: f64,
    pub ubb_w: f64,
    pub u10_0: f64,
    pub u10_l: f64,
    pub u10_w: f64,
    pub u1b_0: f64,
    pub u1b_l: f64,
    pub u1b_w: f64,
    pub u1d_0: f64,
    pub u1d_l: f64,
    pub u1d_w: f64,

    // ── Subthreshold ─────────────────────────────────────────────────────
    pub n00: f64,
    pub n0_l: f64,
    pub n0_w: f64,
    pub nb0: f64,
    pub nb_l: f64,
    pub nb_w: f64,
    pub nd0: f64,
    pub nd_l: f64,
    pub nd_w: f64,
    pub vof0_0: f64,
    pub vof0_l: f64,
    pub vof0_w: f64,
    pub vofb_0: f64,
    pub vofb_l: f64,
    pub vofb_w: f64,
    pub vofd_0: f64,
    pub vofd_l: f64,
    pub vofd_w: f64,

    // ── Impact ionisation ────────────────────────────────────────────────
    pub ai0_0: f64,
    pub ai0_l: f64,
    pub ai0_w: f64,
    pub aib_0: f64,
    pub aib_l: f64,
    pub aib_w: f64,
    pub bi0_0: f64,
    pub bi0_l: f64,
    pub bi0_w: f64,
    pub bib_0: f64,
    pub bib_l: f64,
    pub bib_w: f64,

    // ── Cubic-spline smoothing region ────────────────────────────────────
    pub vghigh0: f64,
    pub vghigh_l: f64,
    pub vghigh_w: f64,
    pub vglow0: f64,
    pub vglow_l: f64,
    pub vglow_w: f64,

    // ── Process parameters & overlap caps ───────────────────────────────
    /// Oxide thickness (input value is in micrometres in ngspice).
    pub tox: f64,
    pub temp: f64,
    pub vdd: f64,
    pub vgg: f64,
    pub vbb: f64,
    pub gate_source_overlap_cap: f64,
    pub gate_drain_overlap_cap: f64,
    pub gate_bulk_overlap_cap: f64,

    // ── Junction / sheet ────────────────────────────────────────────────
    pub sheet_resistance: f64,
    pub jct_sat_cur_density: f64,
    pub bulk_jct_potential: f64,
    pub bulk_jct_bot_grading_coeff: f64,
    pub bulk_jct_side_grading_coeff: f64,
    pub sidewall_jct_potential: f64,
    pub unit_area_jct_cap: f64,
    pub unit_length_sidewall_jct_cap: f64,
    pub default_width: f64,
    pub delta_length: f64,
    pub channel_charge_partition_flag: i32,

    // ── Derived (set during normalisation) ──────────────────────────────
    /// Cox in F/cm² (matches ngspice convention).
    pub cox_cm2: f64,
    /// 2·Vdd / 2·Vgg / 2·Vbb (overflow caps used by ngspice's evaluate).
    pub vdd2: f64,
    pub vgg2: f64,
    pub vbb2: f64,
    /// Thermal voltage at model temperature (V).
    pub vtm: f64,

    // Compatibility / unused-here knobs (kept to roundtrip without warnings)
    pub kf: f64,
    pub af: f64,

    // Pass-through structural fields used by Mosfet stamping helpers
    pub rd: f64,
    pub rs: f64,
    pub cbd: f64,
    pub cbs: f64,
    pub is: f64,
    pub pb: f64,
    pub cgso: f64,
    pub cgdo: f64,
    pub cgbo: f64,
    pub cj: f64,
    pub mj: f64,
    pub cjsw: f64,
    pub mjsw: f64,
    pub fc: f64,
}

impl Bsim2Model {
    /// Construct a model with ngspice's documented defaults (see `b2set.c`).
    pub fn new(mos_type: MosfetType) -> Self {
        Self {
            mos_type,
            vfb0: -1.0,
            vfb_l: 0.0,
            vfb_w: 0.0,
            phi0: 0.75,
            phi_l: 0.0,
            phi_w: 0.0,
            k1_0: 0.8,
            k1_l: 0.0,
            k1_w: 0.0,
            k2_0: 0.0,
            k2_l: 0.0,
            k2_w: 0.0,
            eta0_0: 0.0,
            eta0_l: 0.0,
            eta0_w: 0.0,
            eta_b0: 0.0,
            eta_b_l: 0.0,
            eta_b_w: 0.0,
            delta_l: 0.0,
            delta_w: 0.0,

            mob0_0: 400.0,
            mob0b_0: 0.0,
            mob0b_l: 0.0,
            mob0b_w: 0.0,
            mobs0_0: 500.0,
            mobs0_l: 0.0,
            mobs0_w: 0.0,
            mobsb_0: 0.0,
            mobsb_l: 0.0,
            mobsb_w: 0.0,
            mob20_0: 1.5,
            mob20_l: 0.0,
            mob20_w: 0.0,
            mob2b_0: 0.0,
            mob2b_l: 0.0,
            mob2b_w: 0.0,
            mob2g_0: 0.0,
            mob2g_l: 0.0,
            mob2g_w: 0.0,
            mob30_0: 10.0,
            mob30_l: 0.0,
            mob30_w: 0.0,
            mob3b_0: 0.0,
            mob3b_l: 0.0,
            mob3b_w: 0.0,
            mob3g_0: 0.0,
            mob3g_l: 0.0,
            mob3g_w: 0.0,
            mob40_0: 0.0,
            mob40_l: 0.0,
            mob40_w: 0.0,
            mob4b_0: 0.0,
            mob4b_l: 0.0,
            mob4b_w: 0.0,
            mob4g_0: 0.0,
            mob4g_l: 0.0,
            mob4g_w: 0.0,

            ua0_0: 0.2,
            ua0_l: 0.0,
            ua0_w: 0.0,
            uab_0: 0.0,
            uab_l: 0.0,
            uab_w: 0.0,
            ub0_0: 0.0,
            ub0_l: 0.0,
            ub0_w: 0.0,
            ubb_0: 0.0,
            ubb_l: 0.0,
            ubb_w: 0.0,
            u10_0: 0.1,
            u10_l: 0.0,
            u10_w: 0.0,
            u1b_0: 0.0,
            u1b_l: 0.0,
            u1b_w: 0.0,
            u1d_0: 0.0,
            u1d_l: 0.0,
            u1d_w: 0.0,

            n00: 1.4,
            n0_l: 0.0,
            n0_w: 0.0,
            nb0: 0.5,
            nb_l: 0.0,
            nb_w: 0.0,
            nd0: 0.0,
            nd_l: 0.0,
            nd_w: 0.0,
            vof0_0: 1.8,
            vof0_l: 0.0,
            vof0_w: 0.0,
            vofb_0: 0.0,
            vofb_l: 0.0,
            vofb_w: 0.0,
            vofd_0: 0.0,
            vofd_l: 0.0,
            vofd_w: 0.0,

            ai0_0: 0.0,
            ai0_l: 0.0,
            ai0_w: 0.0,
            aib_0: 0.0,
            aib_l: 0.0,
            aib_w: 0.0,
            bi0_0: 0.0,
            bi0_l: 0.0,
            bi0_w: 0.0,
            bib_0: 0.0,
            bib_l: 0.0,
            bib_w: 0.0,

            vghigh0: 0.2,
            vghigh_l: 0.0,
            vghigh_w: 0.0,
            vglow0: -0.15,
            vglow_l: 0.0,
            vglow_w: 0.0,

            tox: 0.03, // micrometres (ngspice default)
            temp: 27.0,
            vdd: 5.0,
            vgg: 5.0,
            vbb: 5.0,
            gate_source_overlap_cap: 0.0,
            gate_drain_overlap_cap: 0.0,
            gate_bulk_overlap_cap: 0.0,

            sheet_resistance: 0.0,
            jct_sat_cur_density: 0.0,
            bulk_jct_potential: 0.1,
            bulk_jct_bot_grading_coeff: 0.0,
            bulk_jct_side_grading_coeff: 0.0,
            sidewall_jct_potential: 0.1,
            unit_area_jct_cap: 0.0,
            unit_length_sidewall_jct_cap: 0.0,
            default_width: 10.0,
            delta_length: 0.0,
            channel_charge_partition_flag: 0,

            cox_cm2: 0.0,
            vdd2: 10.0,
            vgg2: 10.0,
            vbb2: 10.0,
            vtm: 8.625e-5 * (27.0 + 273.0),

            kf: 0.0,
            af: 1.0,
            rd: 0.0,
            rs: 0.0,
            cbd: 0.0,
            cbs: 0.0,
            is: 1e-14,
            pb: 0.8,
            cgso: 0.0,
            cgdo: 0.0,
            cgbo: 0.0,
            cj: 0.0,
            mj: 0.5,
            cjsw: 0.0,
            mjsw: 0.5,
            fc: 0.5,
        }
    }

    /// Build a `Bsim2Model` from a `.model` definition. Recognised keywords
    /// follow `b2.c::B2mPTable`. Unknown parameters are silently ignored.
    pub fn from_params(model: &ModelParams) -> Self {
        let mos_type = if model.kind.to_uppercase().contains("PMOS") {
            MosfetType::Pmos
        } else {
            MosfetType::Nmos
        };
        let mut m = Self::new(mos_type);
        for (name, v) in &model.params {
            let v = *v;
            match name.to_lowercase().as_str() {
                "vfb" => m.vfb0 = v,
                "lvfb" => m.vfb_l = v,
                "wvfb" => m.vfb_w = v,
                "phi" => m.phi0 = v,
                "lphi" => m.phi_l = v,
                "wphi" => m.phi_w = v,
                "k1" => m.k1_0 = v,
                "lk1" => m.k1_l = v,
                "wk1" => m.k1_w = v,
                "k2" => m.k2_0 = v,
                "lk2" => m.k2_l = v,
                "wk2" => m.k2_w = v,
                "eta0" => m.eta0_0 = v,
                "leta0" => m.eta0_l = v,
                "weta0" => m.eta0_w = v,
                "etab" => m.eta_b0 = v,
                "letab" => m.eta_b_l = v,
                "wetab" => m.eta_b_w = v,
                "dl" => m.delta_l = v,
                "dw" => m.delta_w = v,
                "mu0" => m.mob0_0 = v,
                "mu0b" => m.mob0b_0 = v,
                "lmu0b" => m.mob0b_l = v,
                "wmu0b" => m.mob0b_w = v,
                "mus0" => m.mobs0_0 = v,
                "lmus0" => m.mobs0_l = v,
                "wmus0" => m.mobs0_w = v,
                "musb" => m.mobsb_0 = v,
                "lmusb" => m.mobsb_l = v,
                "wmusb" => m.mobsb_w = v,
                "mu20" => m.mob20_0 = v,
                "lmu20" => m.mob20_l = v,
                "wmu20" => m.mob20_w = v,
                "mu2b" => m.mob2b_0 = v,
                "lmu2b" => m.mob2b_l = v,
                "wmu2b" => m.mob2b_w = v,
                "mu2g" => m.mob2g_0 = v,
                "lmu2g" => m.mob2g_l = v,
                "wmu2g" => m.mob2g_w = v,
                "mu30" => m.mob30_0 = v,
                "lmu30" => m.mob30_l = v,
                "wmu30" => m.mob30_w = v,
                "mu3b" => m.mob3b_0 = v,
                "lmu3b" => m.mob3b_l = v,
                "wmu3b" => m.mob3b_w = v,
                "mu3g" => m.mob3g_0 = v,
                "lmu3g" => m.mob3g_l = v,
                "wmu3g" => m.mob3g_w = v,
                "mu40" => m.mob40_0 = v,
                "lmu40" => m.mob40_l = v,
                "wmu40" => m.mob40_w = v,
                "mu4b" => m.mob4b_0 = v,
                "lmu4b" => m.mob4b_l = v,
                "wmu4b" => m.mob4b_w = v,
                "mu4g" => m.mob4g_0 = v,
                "lmu4g" => m.mob4g_l = v,
                "wmu4g" => m.mob4g_w = v,
                "ua0" => m.ua0_0 = v,
                "lua0" => m.ua0_l = v,
                "wua0" => m.ua0_w = v,
                "uab" => m.uab_0 = v,
                "luab" => m.uab_l = v,
                "wuab" => m.uab_w = v,
                "ub0" => m.ub0_0 = v,
                "lub0" => m.ub0_l = v,
                "wub0" => m.ub0_w = v,
                "ubb" => m.ubb_0 = v,
                "lubb" => m.ubb_l = v,
                "wubb" => m.ubb_w = v,
                "u10" => m.u10_0 = v,
                "lu10" => m.u10_l = v,
                "wu10" => m.u10_w = v,
                "u1b" => m.u1b_0 = v,
                "lu1b" => m.u1b_l = v,
                "wu1b" => m.u1b_w = v,
                "u1d" => m.u1d_0 = v,
                "lu1d" => m.u1d_l = v,
                "wu1d" => m.u1d_w = v,
                "n0" => m.n00 = v,
                "ln0" => m.n0_l = v,
                "wn0" => m.n0_w = v,
                "nb" => m.nb0 = v,
                "lnb" => m.nb_l = v,
                "wnb" => m.nb_w = v,
                "nd" => m.nd0 = v,
                "lnd" => m.nd_l = v,
                "wnd" => m.nd_w = v,
                "vof0" => m.vof0_0 = v,
                "lvof0" => m.vof0_l = v,
                "wvof0" => m.vof0_w = v,
                "vofb" => m.vofb_0 = v,
                "lvofb" => m.vofb_l = v,
                "wvofb" => m.vofb_w = v,
                "vofd" => m.vofd_0 = v,
                "lvofd" => m.vofd_l = v,
                "wvofd" => m.vofd_w = v,
                "ai0" => m.ai0_0 = v,
                "lai0" => m.ai0_l = v,
                "wai0" => m.ai0_w = v,
                "aib" => m.aib_0 = v,
                "laib" => m.aib_l = v,
                "waib" => m.aib_w = v,
                "bi0" => m.bi0_0 = v,
                "lbi0" => m.bi0_l = v,
                "wbi0" => m.bi0_w = v,
                "bib" => m.bib_0 = v,
                "lbib" => m.bib_l = v,
                "wbib" => m.bib_w = v,
                "vghigh" => m.vghigh0 = v,
                "lvghigh" => m.vghigh_l = v,
                "wvghigh" => m.vghigh_w = v,
                "vglow" => m.vglow0 = v,
                "lvglow" => m.vglow_l = v,
                "wvglow" => m.vglow_w = v,
                "tox" => m.tox = v,
                "temp" => m.temp = v,
                "vdd" => m.vdd = v,
                "vgg" => m.vgg = v,
                "vbb" => m.vbb = v,
                "cgso" => m.gate_source_overlap_cap = v,
                "cgdo" => m.gate_drain_overlap_cap = v,
                "cgbo" => m.gate_bulk_overlap_cap = v,
                "xpart" => {
                    m.channel_charge_partition_flag = v as i32;
                }
                "rsh" => m.sheet_resistance = v,
                "js" => m.jct_sat_cur_density = v,
                "pb" => m.bulk_jct_potential = v.max(0.1),
                "mj" => m.bulk_jct_bot_grading_coeff = v,
                "pbsw" => m.sidewall_jct_potential = v.max(0.1),
                "mjsw" => m.bulk_jct_side_grading_coeff = v,
                "cj" => m.unit_area_jct_cap = v,
                "cjsw" => m.unit_length_sidewall_jct_cap = v,
                "wdf" => m.default_width = v,
                "dell" => m.delta_length = v,
                "kf" => m.kf = v,
                "af" => m.af = v,
                _ => {}
            }
        }
        // Mirror b2temp.c entry steps.
        m.cox_cm2 = 3.453e-13 / (m.tox * 1.0e-4); // F/cm²
        m.vdd2 = 2.0 * m.vdd;
        m.vgg2 = 2.0 * m.vgg;
        m.vbb2 = 2.0 * m.vbb;
        m.vtm = 8.625e-5 * (m.temp + 273.0);
        if m.bulk_jct_potential < 0.1 {
            m.bulk_jct_potential = 0.1;
        }
        if m.sidewall_jct_potential < 0.1 {
            m.sidewall_jct_potential = 0.1;
        }
        // Mirror b2.c PMOS handling: ngspice `B2type = -1` for PMOS,
        // captured by `mos_type`.
        m
    }
}

/// Per-(W,L) resolved parameter cache.
///
/// Mirrors `struct bsim2SizeDependParam` in `bsim2def.h`. Built once per
/// instance during setup and reused across every NR iteration.
#[derive(Debug, Clone)]
pub struct Bsim2SizeDependParam {
    pub width: f64,
    pub length: f64,
    pub vfb: f64,
    pub phi: f64,
    pub k1: f64,
    pub k2: f64,
    pub eta0: f64,
    pub eta_b: f64,
    pub beta0: f64,
    pub beta0_b: f64,
    pub betas0: f64,
    pub betas_b: f64,
    pub beta20: f64,
    pub beta2_b: f64,
    pub beta2_g: f64,
    pub beta30: f64,
    pub beta3_b: f64,
    pub beta3_g: f64,
    pub beta40: f64,
    pub beta4_b: f64,
    pub beta4_g: f64,
    pub ua0: f64,
    pub ua_b: f64,
    pub ub0: f64,
    pub ub_b: f64,
    pub u10: f64,
    pub u1_b: f64,
    pub u1_d: f64,
    pub n0: f64,
    pub n_b: f64,
    pub n_d: f64,
    pub vof0: f64,
    pub vof_b: f64,
    pub vof_d: f64,
    pub ai0: f64,
    pub ai_b: f64,
    pub bi0: f64,
    pub bi_b: f64,
    pub vghigh: f64,
    pub vglow: f64,
    pub gd_overlap_cap: f64,
    pub gs_overlap_cap: f64,
    pub gb_overlap_cap: f64,
    pub sqrt_phi: f64,
    pub phis3: f64,
    pub cox_wl: f64,
    pub one_third_cox_wl: f64,
    pub two_third_cox_wl: f64,
    pub arg: f64,
    pub vt0: f64,
}

impl Bsim2SizeDependParam {
    /// Build the size-dependent parameter set for an instance with given W, L.
    /// Mirrors `b2temp.c` (size_not_found branch). Returns `None` if effective
    /// dimensions are non-positive (caller should treat this as an error).
    pub fn build(model: &Bsim2Model, w: f64, l: f64) -> Option<Self> {
        let effective_length = l - model.delta_l * 1.0e-6;
        let effective_width = w - model.delta_w * 1.0e-6;
        if effective_length <= 0.0 || effective_width <= 0.0 {
            return None;
        }
        let inv_l = 1.0e-6 / effective_length;
        let inv_w = 1.0e-6 / effective_width;

        let vfb = model.vfb0 + model.vfb_w * inv_w + model.vfb_l * inv_l;
        let phi = model.phi0 + model.phi_w * inv_w + model.phi_l * inv_l;
        let k1 = model.k1_0 + model.k1_w * inv_w + model.k1_l * inv_l;
        let k2 = model.k2_0 + model.k2_w * inv_w + model.k2_l * inv_l;
        let eta0 = model.eta0_0 + model.eta0_w * inv_w + model.eta0_l * inv_l;
        let eta_b = model.eta_b0 + model.eta_b_w * inv_w + model.eta_b_l * inv_l;

        let beta0 = model.mob0_0;
        let beta0_b = model.mob0b_0 + model.mob0b_w * inv_w + model.mob0b_l * inv_l;
        let mut betas0 = model.mobs0_0 + model.mobs0_w * inv_w + model.mobs0_l * inv_l;
        if betas0 < 1.01 * beta0 {
            betas0 = 1.01 * beta0;
        }
        let mut betas_b = model.mobsb_0 + model.mobsb_w * inv_w + model.mobsb_l * inv_l;
        let tmp_check = betas0 - beta0 - beta0_b * model.vbb;
        if (-betas_b * model.vbb) > tmp_check {
            betas_b = -tmp_check / model.vbb;
        }
        let beta20 = model.mob20_0 + model.mob20_w * inv_w + model.mob20_l * inv_l;
        let beta2_b = model.mob2b_0 + model.mob2b_w * inv_w + model.mob2b_l * inv_l;
        let beta2_g = model.mob2g_0 + model.mob2g_w * inv_w + model.mob2g_l * inv_l;
        let beta30 = model.mob30_0 + model.mob30_w * inv_w + model.mob30_l * inv_l;
        let beta3_b = model.mob3b_0 + model.mob3b_w * inv_w + model.mob3b_l * inv_l;
        let beta3_g = model.mob3g_0 + model.mob3g_w * inv_w + model.mob3g_l * inv_l;
        let beta40 = model.mob40_0 + model.mob40_w * inv_w + model.mob40_l * inv_l;
        let beta4_b = model.mob4b_0 + model.mob4b_w * inv_w + model.mob4b_l * inv_l;
        let beta4_g = model.mob4g_0 + model.mob4g_w * inv_w + model.mob4g_l * inv_l;

        let cox_w_over_l = model.cox_cm2 * effective_width / effective_length;
        let beta0 = beta0 * cox_w_over_l;
        let beta0_b = beta0_b * cox_w_over_l;
        let betas0 = betas0 * cox_w_over_l;
        let betas_b = betas_b * cox_w_over_l;
        let beta30 = beta30 * cox_w_over_l;
        let beta3_b = beta3_b * cox_w_over_l;
        let beta3_g = beta3_g * cox_w_over_l;
        let beta40 = beta40 * cox_w_over_l;
        let beta4_b = beta4_b * cox_w_over_l;
        let beta4_g = beta4_g * cox_w_over_l;

        let ua0 = model.ua0_0 + model.ua0_w * inv_w + model.ua0_l * inv_l;
        let ua_b = model.uab_0 + model.uab_w * inv_w + model.uab_l * inv_l;
        let ub0 = model.ub0_0 + model.ub0_w * inv_w + model.ub0_l * inv_l;
        let ub_b = model.ubb_0 + model.ubb_w * inv_w + model.ubb_l * inv_l;
        let u10 = model.u10_0 + model.u10_w * inv_w + model.u10_l * inv_l;
        let u1_b = model.u1b_0 + model.u1b_w * inv_w + model.u1b_l * inv_l;
        let u1_d = model.u1d_0 + model.u1d_w * inv_w + model.u1d_l * inv_l;

        let mut n0 = model.n00 + model.n0_w * inv_w + model.n0_l * inv_l;
        let n_b = model.nb0 + model.nb_w * inv_w + model.nb_l * inv_l;
        let n_d = model.nd0 + model.nd_w * inv_w + model.nd_l * inv_l;
        if n0 < 0.0 {
            n0 = 0.0;
        }

        let vof0 = model.vof0_0 + model.vof0_w * inv_w + model.vof0_l * inv_l;
        let vof_b = model.vofb_0 + model.vofb_w * inv_w + model.vofb_l * inv_l;
        let vof_d = model.vofd_0 + model.vofd_w * inv_w + model.vofd_l * inv_l;

        let ai0 = model.ai0_0 + model.ai0_w * inv_w + model.ai0_l * inv_l;
        let ai_b = model.aib_0 + model.aib_w * inv_w + model.aib_l * inv_l;
        let bi0 = model.bi0_0 + model.bi0_w * inv_w + model.bi0_l * inv_l;
        let bi_b = model.bib_0 + model.bib_w * inv_w + model.bib_l * inv_l;

        let vghigh = model.vghigh0 + model.vghigh_w * inv_w + model.vghigh_l * inv_l;
        let vglow = model.vglow0 + model.vglow_w * inv_w + model.vglow_l * inv_l;

        // CoxWL stored as in ngspice: Cox is F/cm², L·W are in metres but
        // ngspice multiplies by 1e4 to recover F (treats L·W as cm²·m²/m²
        // ≈ cm²·1e4).
        let cox_wl = model.cox_cm2 * effective_length * effective_width * 1.0e4;
        let one_third_cox_wl = cox_wl / 3.0;
        let two_third_cox_wl = 2.0 * one_third_cox_wl;

        let gs_overlap_cap = model.gate_source_overlap_cap * effective_width;
        let gd_overlap_cap = model.gate_drain_overlap_cap * effective_width;
        let gb_overlap_cap = model.gate_bulk_overlap_cap * effective_length;

        let sqrt_phi = phi.abs().sqrt();
        let phis3 = sqrt_phi * phi;
        let arg = betas_b - beta0_b - model.vdd * (beta3_b - model.vdd * beta4_b);

        let vt0 = vfb + phi + k1 * sqrt_phi - k2 * phi;

        Some(Self {
            width: w,
            length: l,
            vfb,
            phi,
            k1,
            k2,
            eta0,
            eta_b,
            beta0,
            beta0_b,
            betas0,
            betas_b,
            beta20,
            beta2_b,
            beta2_g,
            beta30,
            beta3_b,
            beta3_g,
            beta40,
            beta4_b,
            beta4_g,
            ua0,
            ua_b,
            ub0,
            ub_b,
            u10,
            u1_b,
            u1_d,
            n0,
            n_b,
            n_d,
            vof0,
            vof_b,
            vof_d,
            ai0,
            ai_b,
            bi0,
            bi_b,
            vghigh,
            vglow,
            gd_overlap_cap,
            gs_overlap_cap,
            gb_overlap_cap,
            sqrt_phi,
            phis3,
            cox_wl,
            one_third_cox_wl,
            two_third_cox_wl,
            arg,
            vt0,
        })
    }
}

/// Resolved BSIM2 instance: node indices + per-(W,L) cached parameters.
#[derive(Debug, Clone)]
pub struct Bsim2Instance {
    pub name: String,
    pub drain_idx: Option<usize>,
    pub gate_idx: Option<usize>,
    pub source_idx: Option<usize>,
    pub bulk_idx: Option<usize>,
    pub drain_prime_idx: Option<usize>,
    pub source_prime_idx: Option<usize>,
    pub model: Bsim2Model,
    pub size_params: Bsim2SizeDependParam,
    pub w: f64,
    pub l: f64,
    pub ad: f64,
    pub as_: f64,
    pub pd: f64,
    pub ps: f64,
    pub nrd: f64,
    pub nrs: f64,
    pub m: f64,
    /// Drain series conductance (1/(Rsh·NRD)) or 0 if Rsh·NRD = 0.
    pub drain_conductance: f64,
    /// Source series conductance.
    pub source_conductance: f64,
}

impl Bsim2Instance {
    /// Extract (vgs, vds, vbs) from the solution vector with PMOS sign-flip.
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
}

/// Core BSIM2 evaluate routine. Ports `B2evaluate` from `b2eval.c`.
///
/// Returns `(Ids, gm, gds, gmbs, von, vdsat)`. All inputs and outputs are in
/// the normal device frame (caller is responsible for flipping when `mode=-1`).
fn b2_evaluate(
    model: &Bsim2Model,
    p: &Bsim2SizeDependParam,
    vds: f64,
    vbs: f64,
    vgs: f64,
) -> B2Eval {
    // Clamp inputs to ngspice's overflow caps (b2eval.c lines 57-59).
    let mut vbs = vbs;
    let mut vgs = vgs;
    let mut vds = vds;
    if vbs < model.vbb2 {
        vbs = model.vbb2;
    }
    if vgs > model.vgg2 {
        vgs = model.vgg2;
    }
    if vds > model.vdd2 {
        vds = model.vdd2;
    }

    // Threshold voltage.
    let (phisb, d_phisb_d_vb, t1s, d_t1s_d_vb) = if vbs <= 0.0 {
        let phisb = p.phi - vbs;
        let t1s = phisb.sqrt();
        (phisb, -1.0, t1s, -0.5 / t1s)
    } else {
        let tmp = p.phi / (p.phi + vbs);
        let phisb = p.phi * tmp;
        let t1s = p.phis3 / (p.phi + 0.5 * vbs);
        (phisb, -tmp * tmp, t1s, -0.5 * t1s * t1s / p.phis3)
    };

    let eta = p.eta0 + p.eta_b * vbs;
    let ua = p.ua0 + p.ua_b * vbs;
    let ub = p.ub0 + p.ub_b * vbs;
    let u1s = p.u10 + p.u1_b * vbs;

    let vth = p.vfb + p.phi + p.k1 * t1s - p.k2 * phisb - eta * vds;
    let d_vth_d_vd = -eta;
    let d_vth_d_vb = p.k1 * d_t1s_d_vb + p.k2 - p.eta_b * vds;

    let vgst = vgs - vth;

    let tmp = 1.0 / (1.744 + 0.8364 * phisb);
    let gg = 1.0 - tmp;
    let d_gg_d_vb = 0.8364 * tmp * tmp * d_phisb_d_vb;
    let t0 = gg / t1s;
    let tmp1 = 0.5 * t0 * p.k1;
    let aa = 1.0 + tmp1;
    let d_aa_d_vb = if gg.abs() > 1e-30 && t1s.abs() > 1e-30 {
        (aa - 1.0) * (d_gg_d_vb / gg - d_t1s_d_vb / t1s)
    } else {
        0.0
    };
    let inv_aa = 1.0 / aa;

    let vghigh = p.vghigh;
    let vglow = p.vglow;

    let mut exp0 = 0.0;
    let mut exp1 = 0.0;
    let mut n_val = 0.0; // subthreshold n
    let (vgeff, d_vgeff_d_vg, d_vgeff_d_vd, d_vgeff_d_vb) = if vgst >= vghigh || p.n0 == 0.0 {
        (vgst, 1.0, -d_vth_d_vd, -d_vth_d_vb)
    } else {
        let vof = p.vof0 + p.vof_b * vbs + p.vof_d * vds;
        let n = p.n0 + p.n_b / t1s.max(1e-30) + p.n_d * vds;
        let n_safe = n.max(1e-12);
        n_val = n_safe;
        let tmp = 0.5 / (n_safe * model.vtm);
        let exp_arg1 = (-vds / model.vtm).max(-30.0);
        exp1 = exp_arg1.exp();
        let tmp1 = (1.0 - exp1).max(1.0e-18);
        let tmp2 = 2.0 * aa * tmp1;

        if vgst <= vglow {
            let exp_arg = (vgst * tmp).max(-30.0);
            exp0 = (0.5 * vof + exp_arg).exp();
            let vgeff = tmp2.max(0.0).sqrt() * model.vtm * exp0;
            let t0 = n_safe * model.vtm;
            let d_vgeff_d_vg = vgeff * tmp;
            let d_vgeff_d_vd = d_vgeff_d_vg
                * (n_safe / tmp1 * exp1 - d_vth_d_vd - vgst * p.n_d / n_safe + t0 * p.vof_d);
            let d_vgeff_d_vb = d_vgeff_d_vg
                * (p.vof_b * t0 - d_vth_d_vb
                    + p.n_b * vgst / (n_safe * t1s * t1s) * d_t1s_d_vb
                    + t0 * inv_aa * d_aa_d_vb);
            (vgeff, d_vgeff_d_vg, d_vgeff_d_vd, d_vgeff_d_vb)
        } else {
            // Cubic spline smoothing region between vglow and vghigh.
            let exp_arg = (vglow * tmp).max(-30.0);
            exp0 = (0.5 * vof + exp_arg).exp();
            let vgeff_lo = (2.0 * aa * (1.0 - exp1)).max(0.0).sqrt() * model.vtm * exp0;
            let con1 = vghigh;
            let con3 = vgeff_lo;
            let con4 = con3 * tmp;
            let sqr_vghigh = vghigh * vghigh;
            let sqr_vglow = vglow * vglow;
            let cub_vghigh = vghigh * sqr_vghigh;
            let cub_vglow = vglow * sqr_vglow;
            let s_t0 = 2.0 * vghigh;
            let s_t1 = 2.0 * vglow;
            let s_t2 = 3.0 * sqr_vghigh;
            let s_t3 = 3.0 * sqr_vglow;
            let s_t4 = vghigh - vglow;
            let s_t5 = sqr_vghigh - sqr_vglow;
            let s_t6 = cub_vghigh - cub_vglow;
            let s_t7 = con1 - con3;
            let denom =
                (s_t1 - s_t0) * s_t6 + (s_t2 - s_t3) * s_t5 + (s_t0 * s_t3 - s_t1 * s_t2) * s_t4;
            let delta_s = if denom.abs() > 1e-30 {
                1.0 / denom
            } else {
                0.0
            };
            let coeff_b = (s_t1 - con4 * s_t0) * s_t6
                + (con4 * s_t2 - s_t3) * s_t5
                + (s_t0 * s_t3 - s_t1 * s_t2) * s_t7;
            let coeff_c = (con4 - 1.0) * s_t6 + (s_t2 - s_t3) * s_t7 + (s_t3 - con4 * s_t2) * s_t4;
            let coeff_d = (s_t1 - s_t0) * s_t7 + (1.0 - con4) * s_t5 + (con4 * s_t0 - s_t1) * s_t4;
            let coeff_a = sqr_vghigh * (coeff_c + coeff_d * s_t0);
            let vgeff = (coeff_a + vgst * (coeff_b + vgst * (coeff_c + vgst * coeff_d))) * delta_s;
            let d_vgeff_d_vg = (coeff_b + vgst * (2.0 * coeff_c + 3.0 * vgst * coeff_d)) * delta_s;
            let t7 = con3 * tmp;
            let t8 = d_t1s_d_vb * p.n_b / (t1s * t1s * n_safe).max(1e-30);
            let t9 = n_safe * model.vtm;
            let d_con3_d_vd = t7 * (n_safe * exp1 / tmp1 - vglow * p.n_d / n_safe + t9 * p.vof_d);
            let d_con3_d_vb = t7 * (t9 * inv_aa * d_aa_d_vb + vglow * t8 + t9 * p.vof_b);
            let d_con4_d_vd = tmp * d_con3_d_vd - t7 * p.n_d / n_safe;
            let d_con4_d_vb = tmp * d_con3_d_vb + t7 * t8;

            let d_cb_d_vd = d_con4_d_vd * (s_t2 * s_t5 - s_t0 * s_t6)
                + d_con3_d_vd * (s_t1 * s_t2 - s_t0 * s_t3);
            let d_cc_d_vd = d_con4_d_vd * (s_t6 - s_t2 * s_t4) + d_con3_d_vd * (s_t3 - s_t2);
            let d_cd_d_vd = d_con4_d_vd * (s_t0 * s_t4 - s_t5) + d_con3_d_vd * (s_t0 - s_t1);
            let d_ca_d_vd = sqr_vghigh * (d_cc_d_vd + d_cd_d_vd * s_t0);
            let d_vgeff_d_vd = -d_vgeff_d_vg * d_vth_d_vd
                + (d_ca_d_vd + vgst * (d_cb_d_vd + vgst * (d_cc_d_vd + vgst * d_cd_d_vd)))
                    * delta_s;

            let d_cb_d_vb = d_con4_d_vb * (s_t2 * s_t5 - s_t0 * s_t6)
                + d_con3_d_vb * (s_t1 * s_t2 - s_t0 * s_t3);
            let d_cc_d_vb = d_con4_d_vb * (s_t6 - s_t2 * s_t4) + d_con3_d_vb * (s_t3 - s_t2);
            let d_cd_d_vb = d_con4_d_vb * (s_t0 * s_t4 - s_t5) + d_con3_d_vb * (s_t0 - s_t1);
            let d_ca_d_vb = sqr_vghigh * (d_cc_d_vb + d_cd_d_vb * s_t0);
            let d_vgeff_d_vb = -d_vgeff_d_vg * d_vth_d_vb
                + (d_ca_d_vb + vgst * (d_cb_d_vb + vgst * (d_cc_d_vb + vgst * d_cd_d_vb)))
                    * delta_s;

            (vgeff, d_vgeff_d_vg, d_vgeff_d_vd, d_vgeff_d_vb)
        }
    };

    if vgeff <= 0.0 {
        return B2Eval {
            ids: 0.0,
            gm: 0.0,
            gds: 1e-20,
            gmbs: 0.0,
            von: vth,
            vdsat: 0.0,
        };
    }

    // Velocity / mobility degradation.
    let uvert_raw = 1.0 + vgeff * (ua + vgeff * ub);
    let uvert = uvert_raw.max(0.2);
    let inv_uvert = 1.0 / uvert;
    let t8 = ua + 2.0 * ub * vgeff;
    let d_uvert_d_vg = t8 * d_vgeff_d_vg;
    let d_uvert_d_vd = t8 * d_vgeff_d_vd;
    let d_uvert_d_vb = t8 * d_vgeff_d_vb + vgeff * (p.ua_b + vgeff * p.ub_b);

    let t8 = u1s * inv_aa * inv_uvert;
    let vc = t8 * vgeff;
    let t9 = vc * inv_uvert;
    let d_vc_d_vg = t8 * d_vgeff_d_vg - t9 * d_uvert_d_vg;
    let d_vc_d_vd = t8 * d_vgeff_d_vd - t9 * d_uvert_d_vd;
    let d_vc_d_vb = t8 * d_vgeff_d_vb + p.u1_b * vgeff * inv_aa * inv_uvert
        - vc * inv_aa * d_aa_d_vb
        - t9 * d_uvert_d_vb;

    let tmp2 = (1.0 + 2.0 * vc).max(0.0).sqrt();
    let kk = 0.5 * (1.0 + vc + tmp2);
    let inv_kk = 1.0 / kk;
    let d_kk_d_vc = 0.5 + if tmp2 > 0.0 { 0.5 / tmp2 } else { 0.0 };
    let sqrt_kk = kk.sqrt();

    let t8 = inv_aa / sqrt_kk;
    let vdsat = (vgeff * t8).max(1.0e-18);
    let t9 = 0.5 * vdsat * inv_kk * d_kk_d_vc;
    let d_vdsat_d_vd = t8 * d_vgeff_d_vd - t9 * d_vc_d_vd;
    let d_vdsat_d_vg = t8 * d_vgeff_d_vg - t9 * d_vc_d_vg;
    let d_vdsat_d_vb = t8 * d_vgeff_d_vb - t9 * d_vc_d_vb - vdsat * inv_aa * d_aa_d_vb;

    // Beta family (mobility-shape coefficients).
    let beta0 = p.beta0 + p.beta0_b * vbs;
    let betas = p.betas0 + p.betas_b * vbs;
    let beta2 = p.beta20 + p.beta2_b * vbs + p.beta2_g * vgs;
    let beta3 = p.beta30 + p.beta3_b * vbs + p.beta3_g * vgs;
    let beta4 = p.beta40 + p.beta4_b * vbs + p.beta4_g * vgs;
    let beta1 = betas - (beta0 + model.vdd * (beta3 - model.vdd * beta4));

    let t0 = (vds * beta2 / vdsat).min(30.0);
    let t1 = t0.exp();
    let t2 = t1 * t1;
    let t3 = t2 + 1.0;
    let tanh_term = (t2 - 1.0) / t3;
    let sqr_sech = 4.0 * t2 / (t3 * t3);

    let beta = beta0 + beta1 * tanh_term + vds * (beta3 - beta4 * vds);
    let t4 = beta1 * sqr_sech / vdsat;
    let t5 = model.vdd * tanh_term;
    let d_beta_d_vd = beta3 - 2.0 * beta4 * vds + t4 * (beta2 - t0 * d_vdsat_d_vd);
    let d_beta_d_vg = t4 * (p.beta2_g * vds - t0 * d_vdsat_d_vg) + p.beta3_g * (vds - t5)
        - p.beta4_g * (vds * vds - model.vdd * t5);
    let d_beta1_d_vb = p.arg;
    let d_beta_d_vb = p.beta0_b
        + d_beta1_d_vb * tanh_term
        + vds * (p.beta3_b - vds * p.beta4_b)
        + t4 * (p.beta2_b * vds - t0 * d_vdsat_d_vb);

    let mut ids;
    let mut gm;
    let mut gds;
    let mut gmbs;

    if vgst > vglow {
        if vds <= vdsat {
            // Triode region.
            let t3 = vds / vdsat;
            let t4 = t3 - 1.0;
            let t2 = 1.0 - p.u1_d * t4 * t4;
            let u1 = u1s * t2;
            let utot = (uvert + u1 * vds).max(0.5);
            let inv_utot = 1.0 / utot;
            let t5 = 2.0 * u1s * p.u1_d / vdsat * t4;
            let d_u1_d_vd = t5 * (t3 * d_vdsat_d_vd - 1.0);
            let d_u1_d_vg = t5 * t3 * d_vdsat_d_vg;
            let d_u1_d_vb = t5 * t3 * d_vdsat_d_vb + p.u1_b * t2;
            let d_utot_d_vd = d_uvert_d_vd + u1 + vds * d_u1_d_vd;
            let d_utot_d_vg = d_uvert_d_vg + vds * d_u1_d_vg;
            let d_utot_d_vb = d_uvert_d_vb + vds * d_u1_d_vb;

            let tmp1 = vgeff - 0.5 * aa * vds;
            let tmp3 = tmp1 * vds;
            let betaeff = beta * inv_utot;
            ids = betaeff * tmp3;
            let t6 = ids / betaeff * inv_utot;
            gds = t6 * (d_beta_d_vd - betaeff * d_utot_d_vd)
                + betaeff * (tmp1 + (d_vgeff_d_vd - 0.5 * aa) * vds);
            gm = t6 * (d_beta_d_vg - betaeff * d_utot_d_vg) + betaeff * vds * d_vgeff_d_vg;
            gmbs = t6 * (d_beta_d_vb - betaeff * d_utot_d_vb)
                + betaeff * vds * (d_vgeff_d_vb - 0.5 * vds * d_aa_d_vb);
        } else {
            // Saturation region.
            let tmp1 = vgeff * inv_aa * inv_kk;
            let tmp3 = 0.5 * vgeff * tmp1;
            let betaeff = beta * inv_uvert;
            ids = betaeff * tmp3;
            let t0 = ids / betaeff * inv_uvert;
            let t1 = betaeff * vgeff * inv_aa * inv_kk;
            let t2 = ids * inv_kk * d_kk_d_vc;

            if p.ai0 != 0.0 {
                let ai = p.ai0 + p.ai_b * vbs;
                let bi = p.bi0 + p.bi_b * vbs;
                let delta_v = (vds - vdsat).max(1e-30);
                let t5 = (bi / delta_v).min(30.0);
                let t6 = (-t5).exp();
                let fr = 1.0 + ai * t6;
                let t7 = t5 / delta_v;
                let t8 = (1.0 - fr) * t7;
                let d_fr_d_vd = t8 * (d_vdsat_d_vd - 1.0);
                let d_fr_d_vg = t8 * d_vdsat_d_vg;
                let d_fr_d_vb = t8 * d_vdsat_d_vb + t6 * (p.ai_b - ai * p.bi_b / delta_v);

                gds = (t0 * (d_beta_d_vd - betaeff * d_uvert_d_vd) + t1 * d_vgeff_d_vd
                    - t2 * d_vc_d_vd)
                    * fr
                    + ids * d_fr_d_vd;
                gm = (t0 * (d_beta_d_vg - betaeff * d_uvert_d_vg) + t1 * d_vgeff_d_vg
                    - t2 * d_vc_d_vg)
                    * fr
                    + ids * d_fr_d_vg;
                gmbs = (t0 * (d_beta_d_vb - betaeff * d_uvert_d_vb) + t1 * d_vgeff_d_vb
                    - t2 * d_vc_d_vb
                    - ids * inv_aa * d_aa_d_vb)
                    * fr
                    + ids * d_fr_d_vb;
                ids *= fr;
            } else {
                gds = t0 * (d_beta_d_vd - betaeff * d_uvert_d_vd) + t1 * d_vgeff_d_vd
                    - t2 * d_vc_d_vd;
                gm = t0 * (d_beta_d_vg - betaeff * d_uvert_d_vg) + t1 * d_vgeff_d_vg
                    - t2 * d_vc_d_vg;
                gmbs = t0 * (d_beta_d_vb - betaeff * d_uvert_d_vb) + t1 * d_vgeff_d_vb
                    - t2 * d_vc_d_vb
                    - ids * inv_aa * d_aa_d_vb;
            }
        }
    } else {
        // Subthreshold region.
        let n_safe = n_val.max(1e-12);
        let t0 = exp0 * exp0;
        let t1 = exp1;
        ids = beta * model.vtm * model.vtm * t0 * (1.0 - t1);
        let t2 = if beta.abs() > 1e-30 { ids / beta } else { 0.0 };
        let t4 = n_safe * model.vtm;
        let t3 = ids / t4;
        let (fr, d_fr_d_vd, d_fr_d_vg, d_fr_d_vb) = if vds > vdsat && p.ai0 != 0.0 {
            let ai = p.ai0 + p.ai_b * vbs;
            let bi = p.bi0 + p.bi_b * vbs;
            let delta_v = (vds - vdsat).max(1e-30);
            let t5 = (bi / delta_v).min(30.0);
            let t6 = (-t5).exp();
            let fr = 1.0 + ai * t6;
            let t7 = t5 / delta_v;
            let t8 = (1.0 - fr) * t7;
            let d_fr_d_vd = t8 * (d_vdsat_d_vd - 1.0);
            let d_fr_d_vg = t8 * d_vdsat_d_vg;
            let d_fr_d_vb = t8 * d_vdsat_d_vb + t6 * (p.ai_b - ai * p.bi_b / delta_v);
            (fr, d_fr_d_vd, d_fr_d_vg, d_fr_d_vb)
        } else {
            (1.0, 0.0, 0.0, 0.0)
        };
        gds = (t2 * d_beta_d_vd
            + t3 * (p.vof_d * t4 - d_vth_d_vd - p.n_d * vgst / n_safe)
            + beta * model.vtm * t0 * t1)
            * fr
            + ids * d_fr_d_vd;
        gm = (t2 * d_beta_d_vg + t3) * fr + ids * d_fr_d_vg;
        gmbs = (t2 * d_beta_d_vb
            + t3 * (p.vof_b * t4 - d_vth_d_vb + p.n_b * vgst / (n_safe * t1s * t1s) * d_t1s_d_vb))
            * fr
            + ids * d_fr_d_vb;
        ids *= fr;
    }

    if !gds.is_finite() || gds < 1e-20 {
        gds = 1e-20;
    }
    if !ids.is_finite() {
        ids = 0.0;
    }
    if !gm.is_finite() {
        gm = 0.0;
    }
    if !gmbs.is_finite() {
        gmbs = 0.0;
    }

    let _ = (model.vof0_l, model.kf, model.af); // suppress unused warnings cleanly

    let ids = ids.max(0.0); // BSIM2 outputs Ids ≥ 0 in normal mode; sign is applied by caller

    B2Eval {
        ids,
        gm,
        gds,
        gmbs,
        von: vth,
        vdsat,
    }
}

/// Result of `b2_evaluate`.
struct B2Eval {
    ids: f64,
    gm: f64,
    gds: f64,
    gmbs: f64,
    von: f64,
    vdsat: f64,
}

/// Build the companion model for a BSIM2 instance. Handles source/drain
/// swap (mode), bulk junctions, and the strong/weak-inversion BSIM2 evaluate.
pub fn bsim2_companion(inst: &Bsim2Instance, vgs: f64, vds: f64, vbs: f64) -> MosfetCompanion {
    let vt = VT_NOM;
    let p = &inst.size_params;
    let model = &inst.model;

    // Drain/source diode junctions (b2ld.c lines 381-396).
    let drain_area = inst.ad;
    let source_area = inst.as_;
    let mut drain_sat = drain_area * model.jct_sat_cur_density;
    if drain_sat < 1e-15 {
        drain_sat = 1e-15;
    }
    let mut source_sat = source_area * model.jct_sat_cur_density;
    if source_sat < 1e-15 {
        source_sat = 1e-15;
    }

    let vbd = vbs - vds;
    let (gbs_j, cbs_curr) = bulk_diode(vbs, source_sat, vt);
    let (gbd_j, cbd_curr) = bulk_diode(vbd, drain_sat, vt);

    // Determine mode.
    let mode = if vds >= 0.0 { 1 } else { -1 };

    // Evaluate at normal-mode voltages (swap when reversed).
    let eval = if mode == 1 {
        b2_evaluate(model, p, vds, vbs, vgs)
    } else {
        b2_evaluate(model, p, -vds, vbd, vgs - vds)
    };

    // Drain current including bulk-drain reverse current.
    let cdrain_signed = mode as f64 * eval.ids;
    let cd = cdrain_signed - cbd_curr;

    // Equivalent NR current sources (b2ld.c lines 663-677).
    // ceq_d = cdrain - gds*vds - gm*vgs - gmbs*vbs (for normal mode)
    let ceq_d = if mode >= 0 {
        eval.ids - eval.gds * vds - eval.gm * vgs - eval.gmbs * vbs
    } else {
        -(eval.ids + eval.gds * vds - eval.gm * (vgs - vds) - eval.gmbs * vbd)
    };

    let ceq_bs = cbs_curr - gbs_j * vbs;
    let ceq_bd = cbd_curr - gbd_j * vbd;

    let _ = cd; // value already encoded via eval.ids + bulk current

    MosfetCompanion {
        gm: eval.gm,
        gds: eval.gds,
        gmbs: eval.gmbs,
        gbd: gbd_j,
        gbs: gbs_j,
        cdrain: cdrain_signed,
        ceq_d,
        ceq_bs,
        ceq_bd,
        mode,
        vdsat: eval.vdsat,
        von: eval.von,
    }
}

/// Bulk junction diode current/conductance (matches `b2ld.c` lines 381-396).
fn bulk_diode(v: f64, isat: f64, vt: f64) -> (f64, f64) {
    if v <= 0.0 {
        let g = isat / vt + GMIN;
        let i = g * v;
        (g, i)
    } else {
        let ev = (v / vt).min(40.0).exp();
        let g = isat * ev / vt + GMIN;
        let i = isat * (ev - 1.0) + GMIN * v;
        (g, i)
    }
}

/// Stamp the BSIM2 companion model into the MNA matrix and RHS. Mirrors the
/// stamping loop in `b2ld.c` (lines 686-713) but expressed in the same shape
/// as the other Rust MOSFET ports.
pub fn stamp_bsim2(
    matrix: &mut crate::SparseMatrix,
    rhs: &mut [f64],
    inst: &Bsim2Instance,
    comp: &MosfetCompanion,
) {
    let d = inst.drain_idx;
    let g = inst.gate_idx;
    let s = inst.source_idx;
    let b = inst.bulk_idx;
    let dp = inst.drain_prime_idx;
    let sp = inst.source_prime_idx;

    let sign = inst.model.mos_type.sign();
    let m = inst.m;

    let (xnrm, xrev) = if comp.mode > 0 {
        (1.0, 0.0)
    } else {
        (0.0, 1.0)
    };

    let gm_scaled = m * comp.gm;
    let gmbs_scaled = m * comp.gmbs;

    // gds between d' and s'.
    crate::stamp_conductance(matrix, dp, sp, m * comp.gds);

    // gm VCCS (gate->drain/source).
    if let Some(dpi) = dp {
        matrix.add(dpi, dpi, xrev * gm_scaled);
    }
    if let Some(spi) = sp {
        matrix.add(spi, spi, xnrm * gm_scaled);
    }
    if let Some(gi) = g {
        if let Some(dpi) = dp {
            matrix.add(dpi, gi, (xnrm - xrev) * gm_scaled);
        }
        if let Some(spi) = sp {
            matrix.add(spi, gi, -(xnrm - xrev) * gm_scaled);
        }
    }
    if let (Some(dpi), Some(spi)) = (dp, sp) {
        matrix.add(dpi, spi, -xnrm * gm_scaled);
        matrix.add(spi, dpi, -xrev * gm_scaled);
    }

    // gmbs body-effect VCCS.
    if let Some(dpi) = dp {
        matrix.add(dpi, dpi, xrev * gmbs_scaled);
        if let Some(bi) = b {
            matrix.add(dpi, bi, (xnrm - xrev) * gmbs_scaled);
        }
        if let Some(spi) = sp {
            matrix.add(dpi, spi, -xnrm * gmbs_scaled);
        }
    }
    if let Some(spi) = sp {
        matrix.add(spi, spi, xnrm * gmbs_scaled);
        if let Some(bi) = b {
            matrix.add(spi, bi, -(xnrm - xrev) * gmbs_scaled);
        }
        if let Some(dpi) = dp {
            matrix.add(spi, dpi, -xrev * gmbs_scaled);
        }
    }

    // Bulk-drain / bulk-source junction conductances.
    crate::stamp_conductance(matrix, b, dp, m * comp.gbd);
    crate::stamp_conductance(matrix, b, sp, m * comp.gbs);

    // Drain/source series resistances.
    if inst.drain_conductance > 0.0 {
        crate::stamp_conductance(matrix, d, dp, m * inst.drain_conductance);
    }
    if inst.source_conductance > 0.0 {
        crate::stamp_conductance(matrix, s, sp, m * inst.source_conductance);
    }

    // Equivalent NR current sources (b2ld.c lines 679-684).
    let ceq_d = sign * m * comp.ceq_d;
    let ceq_bs = sign * m * comp.ceq_bs;
    let ceq_bd = sign * m * comp.ceq_bd;

    if let Some(dpi) = dp {
        rhs[dpi] -= ceq_d - ceq_bd;
    }
    if let Some(spi) = sp {
        rhs[spi] += ceq_d + ceq_bs;
    }
    if let Some(bi) = b {
        rhs[bi] -= ceq_bd + ceq_bs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_model() -> Bsim2Model {
        let mut m = Bsim2Model::new(MosfetType::Nmos);
        // Use sensible 1990s-PDK NMOS parameters.
        m.vfb0 = -0.8;
        m.phi0 = 0.7;
        m.k1_0 = 0.7;
        m.k2_0 = -0.05;
        m.eta0_0 = 0.05;
        m.tox = 0.02; // micrometres
        m.cox_cm2 = 3.453e-13 / (m.tox * 1.0e-4);
        m.vdd2 = 2.0 * m.vdd;
        m.vgg2 = 2.0 * m.vgg;
        m.vbb2 = 2.0 * m.vbb;
        m.vtm = 8.625e-5 * (m.temp + 273.0);
        m
    }

    fn make_test_instance(model: Bsim2Model) -> Bsim2Instance {
        let w = 50e-6;
        let l = 10e-6;
        let sp = Bsim2SizeDependParam::build(&model, w, l).unwrap();
        Bsim2Instance {
            name: "M1".to_string(),
            drain_idx: Some(0),
            gate_idx: Some(1),
            source_idx: Some(2),
            bulk_idx: Some(3),
            drain_prime_idx: Some(0),
            source_prime_idx: Some(2),
            model,
            size_params: sp,
            w,
            l,
            ad: 100e-12,
            as_: 100e-12,
            pd: 40e-6,
            ps: 40e-6,
            nrd: 0.0,
            nrs: 0.0,
            m: 1.0,
            drain_conductance: 0.0,
            source_conductance: 0.0,
        }
    }

    #[test]
    fn defaults_match_ngspice() {
        let m = Bsim2Model::new(MosfetType::Nmos);
        assert_eq!(m.vfb0, -1.0);
        assert_eq!(m.phi0, 0.75);
        assert_eq!(m.k1_0, 0.8);
        assert_eq!(m.mob0_0, 400.0);
        assert_eq!(m.mobs0_0, 500.0);
        assert_eq!(m.n00, 1.4);
        assert_eq!(m.vghigh0, 0.2);
        assert_eq!(m.vglow0, -0.15);
    }

    #[test]
    fn from_model_def_picks_up_bsim2_params() {
        let md = ModelParams {
            name: "M".to_string(),
            kind: "NMOS".to_string(),
            params: vec![
                ("LEVEL".to_string(), 5.0),
                ("vfb".to_string(), -0.8),
                ("phi".to_string(), 0.7),
                ("k1".to_string(), 0.6),
                ("mu0".to_string(), 450.0),
                ("tox".to_string(), 0.02),
            ],
        };
        let m = Bsim2Model::from_params(&md);
        assert_eq!(m.mos_type, MosfetType::Nmos);
        assert_eq!(m.vfb0, -0.8);
        assert_eq!(m.phi0, 0.7);
        assert_eq!(m.k1_0, 0.6);
        assert_eq!(m.mob0_0, 450.0);
    }

    #[test]
    fn cutoff_returns_subthreshold_current() {
        let m = make_test_model();
        let inst = make_test_instance(m);
        // Vgs = 0, well below threshold. BSIM2 has subthreshold conduction
        // (default n0 = 1.4) so expect a small leakage, not zero.
        let comp = bsim2_companion(&inst, 0.0, 0.5, 0.0);
        assert!(
            comp.cdrain.abs() < 1e-3,
            "subthreshold leakage too large: {}",
            comp.cdrain
        );
        // Above-threshold current must dominate the leakage by orders of
        // magnitude — sanity check the dynamic range.
        let comp_on = bsim2_companion(&inst, 3.0, 0.5, 0.0);
        assert!(
            comp_on.cdrain > 100.0 * comp.cdrain.abs(),
            "subthreshold should be much smaller than on-state: off={} on={}",
            comp.cdrain,
            comp_on.cdrain
        );
    }

    #[test]
    fn dc_sweep_monotonic_in_vds() {
        let m = make_test_model();
        let inst = make_test_instance(m);
        let mut prev = -1.0;
        for &vds in &[0.05, 0.5, 1.0, 2.0, 4.0] {
            let comp = bsim2_companion(&inst, 3.0, vds, 0.0);
            assert!(comp.cdrain.is_finite(), "Id non-finite at Vds={}", vds);
            assert!(
                comp.cdrain >= prev - 1e-9,
                "Id should be monotonic in Vds (vds={}): prev={} new={}",
                vds,
                prev,
                comp.cdrain
            );
            prev = comp.cdrain;
        }
    }

    #[test]
    fn reversed_mode_marks_negative_vds() {
        let m = make_test_model();
        let inst = make_test_instance(m);
        let comp = bsim2_companion(&inst, 3.0, -1.0, 0.0);
        assert_eq!(comp.mode, -1);
        assert!(comp.cdrain.is_finite());
    }

    #[test]
    fn size_dep_param_builds() {
        let m = make_test_model();
        let sp = Bsim2SizeDependParam::build(&m, 50e-6, 10e-6).unwrap();
        assert!(sp.beta0 > 0.0);
        assert!(sp.cox_wl > 0.0);
        assert!(sp.vt0 > 0.0);
    }

    #[test]
    fn pmos_type_from_kind() {
        let md = ModelParams {
            name: "P".to_string(),
            kind: "PMOS".to_string(),
            params: vec![],
        };
        let m = Bsim2Model::from_params(&md);
        assert_eq!(m.mos_type, MosfetType::Pmos);
    }
}
