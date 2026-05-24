//! MOSFET Level 3 (semi-empirical short-channel) device model.
//!
//! Ports the ngspice MOS3 implementation (`ngspice-upstream/src/spicelib/
//! devices/mos3/mos3load.c`, `mos3temp.c`) to thevenin's companion-model
//! interface. MOS3 is the Liu/Kwok semi-empirical short-channel MOSFET model:
//! threshold voltage acquires a DIBL (drain-induced barrier lowering) term, a
//! short-channel `fshort` factor reduces the body coefficient, mobility is
//! modulated by the vertical field (`THETA`) and saturates by velocity
//! (`VMAX`), and channel-length modulation uses `KAPPA`/`ALPHA`.
//!
//! Companion-model shape (`gm`, `gds`, `gmbs`, bulk diodes, `mode`) and the
//! stamping logic mirror Level 2 — only the device-equation core differs.

use thevenin_types::{Expr, ModelDef};

use crate::diode::VT_NOM;
use crate::mosfet::{MosfetCompanion, MosfetType};
use crate::physics::{EXP_LIMIT, safe_exp};

/// Physical constants matching ngspice.
const CHARGE: f64 = 1.602_176_634e-19;
const EPSSIL: f64 = 11.70 * 8.854_214_871e-12;
const EPSOX: f64 = 3.9 * 8.854_214_871e-12;
/// Intrinsic carrier concentration at 300K in m⁻³.
const NI: f64 = 1.45e16;

/// fshort polynomial coefficients from mos3load.c (Liu/Kwok fit).
const COEFF0: f64 = 0.063_135_3;
const COEFF1: f64 = 0.801_329_2;
const COEFF2: f64 = -0.011_107_77;

/// MOSFET Level 3 model parameters.
#[derive(Debug, Clone)]
pub struct Mos3Model {
    pub mos_type: MosfetType,
    // ── Shared with Level 1/2 ────────────────────────────────────────────
    /// Threshold voltage (V).
    pub vto: f64,
    /// Transconductance parameter (A/V²).
    pub kp: f64,
    /// Body effect coefficient (sqrt(V)).
    pub gamma: f64,
    /// Surface potential (V).
    pub phi: f64,
    /// Drain resistance (Ω).
    pub rd: f64,
    /// Source resistance (Ω).
    pub rs: f64,
    /// Bulk-drain zero-bias junction capacitance (F).
    pub cbd: f64,
    /// Bulk-source zero-bias junction capacitance (F).
    pub cbs: f64,
    /// Bulk junction saturation current (A).
    pub is: f64,
    /// Bulk junction potential (V).
    pub pb: f64,
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
    /// Oxide thickness (m).
    pub tox: f64,
    /// Lateral diffusion (m).
    pub ld: f64,
    /// Substrate doping (1/cm³ in input form, converted to 1/m³ internally).
    pub nsub: f64,
    /// Surface mobility (cm²/V·s).
    pub u0: f64,
    /// Forward bias depletion cap coefficient.
    pub fc: f64,
    /// Flicker noise coefficient.
    pub kf: f64,
    /// Flicker noise exponent.
    pub af: f64,
    /// Surface state density (1/cm²).
    pub nss: f64,
    /// Gate type: 0=Al, +1=opposite, -1=same.
    pub tpg: f64,

    // ── Level 3 specific ─────────────────────────────────────────────────
    /// Junction depth (m).
    pub xj: f64,
    /// Narrow-channel effect factor (DELTA, dimensionless).
    pub delta: f64,
    /// Vertical-field mobility degradation coefficient (1/V) — `THETA`.
    pub theta: f64,
    /// Static-feedback (DIBL) coefficient — `ETA` (dimensionless).
    pub eta: f64,
    /// Channel-length modulation coefficient — `KAPPA` (dimensionless).
    pub kappa: f64,
    /// Maximum drift velocity (m/s) — `VMAX`.
    pub vmax: f64,
    /// Fast surface state density for subthreshold (1/cm²) — `NFS`.
    pub nfs: f64,

    // ── Derived (computed from process params, mirroring mos3temp.c) ────
    /// Oxide capacitance per unit area (F/m²).
    pub oxide_cap_factor: f64,
    /// Depletion-layer-width-squared factor `(2 eps_si)/(q Nsub)` (m²/V).
    pub alpha_dep: f64,
    /// `sqrt(alpha_dep)` — `coeffDepLayWidth` in ngspice (m/√V).
    pub coeff_dep_lay_width: f64,
    /// Narrow-channel pre-factor `delta * pi/2 * eps_si / Cox` (V·m).
    pub narrow_factor: f64,
    /// Built-in voltage `Vbi = VTO - sign*gamma*sqrt(phi)` (V).
    pub vbi: f64,
}

impl Mos3Model {
    pub fn new(mos_type: MosfetType) -> Self {
        let tox = 1e-7;
        let oxide_cap_factor = EPSOX / tox;
        Self {
            mos_type,
            vto: 0.0,
            kp: 2e-5,
            gamma: 0.0,
            phi: 0.6,
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
            tox,
            ld: 0.0,
            nsub: 0.0,
            u0: 600.0,
            fc: 0.5,
            kf: 0.0,
            af: 1.0,
            nss: 0.0,
            tpg: 1.0,
            xj: 0.0,
            delta: 0.0,
            theta: 0.0,
            eta: 0.0,
            kappa: 0.2,
            vmax: 0.0,
            nfs: 0.0,
            oxide_cap_factor,
            alpha_dep: 0.0,
            coeff_dep_lay_width: 0.0,
            narrow_factor: 0.0,
            vbi: 0.0,
        }
    }

    /// Build a `Mos3Model` from a netlist `.model` definition.
    pub fn from_model_def(model_def: &ModelDef) -> Self {
        let mos_type = if model_def.kind.to_uppercase().contains("PMOS") {
            MosfetType::Pmos
        } else {
            MosfetType::Nmos
        };
        let mut m = Self::new(mos_type);
        let mut vto_given = false;
        let mut gamma_given = false;
        let mut phi_given = false;
        let mut nsub_given = false;
        let mut kp_given = false;
        for p in &model_def.params {
            if let Expr::Num(v) = &p.value {
                match p.name.to_uppercase().as_str() {
                    "VTO" | "VT0" => {
                        m.vto = *v;
                        vto_given = true;
                    }
                    "KP" => {
                        m.kp = *v;
                        kp_given = true;
                    }
                    "GAMMA" => {
                        m.gamma = *v;
                        gamma_given = true;
                    }
                    "PHI" => {
                        m.phi = *v;
                        phi_given = true;
                    }
                    // LAMBDA is not used by Level 3 — ngspice silently ignores it.
                    "LAMBDA" => {}
                    "RD" => m.rd = *v,
                    "RS" => m.rs = *v,
                    "CBD" => m.cbd = *v,
                    "CBS" => m.cbs = *v,
                    "IS" => m.is = *v,
                    "PB" => m.pb = *v,
                    "CGSO" => m.cgso = *v,
                    "CGDO" => m.cgdo = *v,
                    "CGBO" => m.cgbo = *v,
                    "CJ" => m.cj = *v,
                    "MJ" => m.mj = *v,
                    "CJSW" => m.cjsw = *v,
                    "MJSW" => m.mjsw = *v,
                    "TOX" => m.tox = *v,
                    "LD" => m.ld = *v,
                    "NSUB" => {
                        m.nsub = *v;
                        nsub_given = true;
                    }
                    "U0" | "UO" => m.u0 = *v,
                    "FC" => m.fc = *v,
                    "KF" => m.kf = *v,
                    "AF" => m.af = *v,
                    "NSS" => m.nss = *v,
                    "TPG" => m.tpg = *v,
                    "XJ" => m.xj = *v,
                    "DELTA" => m.delta = *v,
                    "THETA" => m.theta = *v,
                    "ETA" => m.eta = *v,
                    "KAPPA" => m.kappa = *v,
                    "VMAX" => m.vmax = *v,
                    "NFS" => m.nfs = *v,
                    _ => {} // ignore unknown params (LEVEL, etc.)
                }
            }
        }
        m.compute_process_params(vto_given, gamma_given, phi_given, nsub_given, kp_given);
        m
    }

    /// Derive process-dependent parameters (mirrors `mos3temp.c` model loop).
    fn compute_process_params(
        &mut self,
        vto_given: bool,
        gamma_given: bool,
        phi_given: bool,
        nsub_given: bool,
        kp_given: bool,
    ) {
        let vtnom = VT_NOM;
        self.oxide_cap_factor = EPSOX / self.tox;

        if !kp_given {
            // u0 is cm²/V·s; oxide_cap_factor is F/m². Convert mobility to
            // m²/V·s via *1e-4. Matches mos3temp.c line 66.
            self.kp = self.u0 * 1e-4 * self.oxide_cap_factor;
        }

        if nsub_given {
            let nsub_m3 = self.nsub * 1e6;
            if nsub_m3 > NI {
                if !phi_given {
                    self.phi = 2.0 * vtnom * (nsub_m3 / NI).ln();
                    if self.phi < 0.1 {
                        self.phi = 0.1;
                    }
                }

                let reftemp = 300.15;
                let egfet1 = 1.16 - (7.02e-4 * reftemp * reftemp) / (reftemp + 1108.0);
                let type_sign = self.mos_type.sign();
                let fermis = type_sign * 0.5 * self.phi;
                let wkfng = if self.tpg != 0.0 {
                    let fermig = type_sign * self.tpg * 0.5 * egfet1;
                    3.25 + 0.5 * egfet1 - fermig
                } else {
                    3.2
                };
                let wkfngs = wkfng - (3.25 + 0.5 * egfet1 + fermis);

                if !gamma_given {
                    self.gamma = (2.0 * EPSSIL * CHARGE * nsub_m3).sqrt() / self.oxide_cap_factor;
                }

                if !vto_given {
                    let vfb = wkfngs - self.nss * 1e4 * CHARGE / self.oxide_cap_factor;
                    self.vto = vfb + type_sign * (self.gamma * self.phi.sqrt() + self.phi);
                }

                // alpha_dep = 2 * eps_si / (q * Nsub_m3)
                self.alpha_dep = (2.0 * EPSSIL) / (CHARGE * nsub_m3);
                self.coeff_dep_lay_width = self.alpha_dep.sqrt();
            }
        }

        // Narrow-channel factor (mos3temp.c line 113).
        self.narrow_factor =
            self.delta * 0.5 * std::f64::consts::PI * EPSSIL / self.oxide_cap_factor;

        // Vbi = Vto - sign*gamma*sqrt(phi).
        // (mos3temp.c line 219 minus the temperature-shift terms, which we
        // do not apply here since thevenin's MOSn models operate at TNOM.)
        let sign = self.mos_type.sign();
        self.vbi = self.vto - sign * self.gamma * self.phi.sqrt();
    }

    /// Returns the number of internal nodes added by series resistances
    /// (RD/RS), matching the Level 1/2/6 convention.
    pub fn internal_node_count(&self) -> usize {
        let mut count = 0;
        if self.rd > 0.0 {
            count += 1;
        }
        if self.rs > 0.0 {
            count += 1;
        }
        count
    }

    /// Compute the Level 3 MOSFET operating point and NR companion model.
    ///
    /// `beta` is the effective KP·W/L_eff from the instance; `w` and `l_eff`
    /// are needed for oxide cap and narrow/short-channel scaling.
    pub fn companion(
        &self,
        vgs: f64,
        vds: f64,
        vbs: f64,
        beta_in: f64,
        w: f64,
        l_eff: f64,
    ) -> MosfetCompanion {
        let vt = VT_NOM;
        let sign = self.mos_type.sign();

        // Source/drain swap handling: in MOS3 the "mode" only flips which
        // junction current we evaluate first — the device equations are
        // written terminal-symmetrically below. Determine mode from vds.
        let mode = if vds >= 0.0 { 1 } else { -1 };

        // ── Bulk diode currents ────────────────────────────────────────
        // (ngspice mos3load.c lines 414–433; gmin is added in `stamp_*`.)
        let vbd = vbs - vds;
        let (gbs_val, cbs_current) = bulk_diode_current(vbs, self.is, vt);
        let (gbd_val, cbd_current) = bulk_diode_current(vbd, self.is, vt);

        // For the I-V equations, the "controlling" Vbs/Vgs/Vds depend on
        // mode. ngspice mos3load.c references `(here->MOS3mode==1?vbs:vbd)`
        // etc. directly; we translate that here so the equations below
        // can be written in the "normal" frame.
        let (vbs_eff, vgs_eff, vds_eff) = if mode == 1 {
            (vbs, vgs, vds)
        } else {
            (vbd, vgs - vds, -vds)
        };

        // Effective oxide cap of this device.
        let oxide_cap = self.oxide_cap_factor * l_eff * w;

        // ── sqrt(phi - vbs) term ───────────────────────────────────────
        // mos3load.c lines 566–577. Smooth out negative-argument case
        // with the Pinto/Hodges-style 1/(1+vbs/(2phi)) hack.
        let (sqphbs, dsqdvb);
        let phibs;
        if vbs_eff <= 0.0 {
            phibs = self.phi - vbs_eff;
            sqphbs = phibs.sqrt();
            dsqdvb = -0.5 / sqphbs;
        } else {
            let sqphis = self.phi.sqrt();
            let sqphs3 = self.phi * sqphis;
            let s = sqphis / (1.0 + vbs_eff / (2.0 * self.phi));
            phibs = s * s;
            sqphbs = s;
            // dsqdvb = -phibs / (2 * phi^(3/2))
            dsqdvb = -phibs / (sqphs3 + sqphs3);
        }

        // ── Short-channel factor fshort ───────────────────────────────
        let one_over_xl = 1.0 / l_eff;
        let (fshort, dfsdvb);
        if self.xj != 0.0 && self.coeff_dep_lay_width != 0.0 {
            let wps = self.coeff_dep_lay_width * sqphbs;
            let one_over_xj = 1.0 / self.xj;
            let xjonxl = self.xj * one_over_xl;
            let djonxj = self.ld * one_over_xj;
            let wponxj = wps * one_over_xj;
            let wconxj = COEFF0 + COEFF1 * wponxj + COEFF2 * wponxj * wponxj;
            let arga = wconxj + djonxj;
            let argc = wponxj / (1.0 + wponxj);
            let argb_sq = 1.0 - argc * argc;
            // Guard against numerical noise driving argb_sq slightly negative.
            let argb = argb_sq.max(0.0).sqrt();
            fshort = 1.0 - xjonxl * (arga * argb - djonxj);
            let dwpdvb = self.coeff_dep_lay_width * dsqdvb;
            let dadvb = (COEFF1 + COEFF2 * (wponxj + wponxj)) * dwpdvb * one_over_xj;
            let dbdvb = if argb > 0.0 && wps > 0.0 {
                -argc * argc * (1.0 - argc) * dwpdvb / (argb * wps)
            } else {
                0.0
            };
            dfsdvb = -xjonxl * (dadvb * argb + arga * dbdvb);
        } else {
            fshort = 1.0;
            dfsdvb = 0.0;
        }

        // ── Body effect ────────────────────────────────────────────────
        let gammas = self.gamma * fshort;
        let fbodys = 0.5 * gammas / (sqphbs + sqphbs);
        let fbody = fbodys + self.narrow_factor / w;
        let onfbdy = 1.0 / (1.0 + fbody);
        let dfbdvb = if sqphbs > 0.0 && fshort > 0.0 {
            -fbodys * dsqdvb / sqphbs + fbodys * dfsdvb / fshort
        } else {
            0.0
        };
        let qbonco = gammas * sqphbs + self.narrow_factor * phibs / w;
        // Direct port of mos3load.c lines 610–611.
        let dqbdvb = gammas * dsqdvb + self.gamma * dfsdvb * sqphbs - self.narrow_factor / w;

        // ── Static-feedback (DIBL) threshold shift ─────────────────────
        // eta_eff = ETA * 8.15e-22 / (Cox * Leff^3)  — units work out to 1/V
        // when Leff is in meters and Cox in F/m².
        let eta_eff = self.eta * 8.15e-22
            / (self.oxide_cap_factor * l_eff * l_eff * l_eff).max(f64::MIN_POSITIVE);
        let vbix = self.vbi * sign - eta_eff * (mode as f64 * vds);
        // Threshold voltage (charge-sharing strong-inversion VT)
        let vth = vbix + qbonco;
        let dvtdvd = -eta_eff;
        let dvtdvb = dqbdvb;

        // ── Weak/strong-inversion handover ─────────────────────────────
        let mut von = vth;
        let mut xn = 1.0;
        let mut dxndvb = 0.0;
        let mut dvodvd = 0.0;
        let mut dvodvb = 0.0;
        let has_nfs = self.nfs != 0.0 && oxide_cap > 0.0;
        if has_nfs {
            let csonco = CHARGE * self.nfs * 1e4 * l_eff * w / oxide_cap;
            let cdonco = if phibs > 0.0 {
                qbonco / (phibs + phibs)
            } else {
                0.0
            };
            xn = 1.0 + csonco + cdonco;
            von = vth + vt * xn;
            dxndvb = if phibs > 0.0 && sqphbs > 0.0 {
                dqbdvb / (phibs + phibs) - qbonco * dsqdvb / (phibs * sqphbs)
            } else {
                0.0
            };
            dvodvd = dvtdvd;
            dvodvb = dvtdvb + vt * dxndvb;
        } else if vgs_eff <= von {
            // Cutoff
            return cutoff_companion(
                vbs,
                vbd,
                gbs_val,
                cbs_current,
                gbd_val,
                cbd_current,
                mode,
                von,
                sign,
            );
        }

        // Device is on (or subthreshold path will multiply later).
        let vgsx = vgs_eff.max(von);

        // ── Vertical-field mobility degradation ────────────────────────
        let onfg = 1.0 + self.theta * (vgsx - vth);
        let fgate = 1.0 / onfg;
        let us = self.u0 * 1e-4 * fgate; // m²/V·s
        let dfgdvg = -self.theta * fgate * fgate;
        let dfgdvd = -dfgdvg * dvtdvd;
        let dfgdvb = -dfgdvg * dvtdvb;

        // ── Saturation voltage (with / without velocity saturation) ────
        let mut vdsat;
        let dvsdvg;
        let dvsdvd;
        let dvsdvb;
        let mut onvdsc = 0.0;
        if self.vmax <= 0.0 {
            vdsat = (vgsx - vth) * onfbdy;
            dvsdvg = onfbdy;
            dvsdvd = -dvsdvg * dvtdvd;
            dvsdvb = -dvsdvg * dvtdvb - vdsat * dfbdvb * onfbdy;
        } else {
            let vdsc = l_eff * self.vmax / us;
            onvdsc = 1.0 / vdsc;
            let arga = (vgsx - vth) * onfbdy;
            let argb = (arga * arga + vdsc * vdsc).sqrt();
            vdsat = arga + vdsc - argb;
            let dvsdga = if argb > 0.0 {
                (1.0 - arga / argb) * onfbdy
            } else {
                onfbdy
            };
            // d(vdsat)/d(vgs) splits into a "direct" term and an indirect
            // term routed through fgate.
            dvsdvg = if argb > 0.0 {
                dvsdga - (1.0 - vdsc / argb) * vdsc * dfgdvg * onfg
            } else {
                dvsdga
            };
            dvsdvd = -dvsdvg * dvtdvd;
            dvsdvb = -dvsdvg * dvtdvb - arga * dvsdga * dfbdvb;
        }

        let vds_mode = mode as f64 * vds; // ngspice's (here->MOS3mode*vds)
        let vdsx = vds_mode.min(vdsat);

        // ── Strong-inversion drain current ─────────────────────────────
        let mut cdrain;
        let mut gm;
        let mut gds;
        let mut gmbs;
        let mut gds0 = 0.0; // dCD/dVdsat path through dvsdvg/etc.
        let mut beta = beta_in;

        if vdsx <= 0.0 {
            // mos3load.c "line900" — Vds is essentially 0.
            beta *= fgate;
            cdrain = 0.0;
            gm = 0.0;
            gds = beta * (vgsx - vth);
            gmbs = 0.0;
            if has_nfs && vgs_eff < von {
                // Weak-inversion attenuator on the linear gds.
                let attn = safe_exp(((vgs_eff - von) / (vt * xn)).min(EXP_LIMIT));
                gds *= attn;
            }
            // gds will be sanitised below; skip CLM.
            vdsat = vdsat.max(0.0);
            if gds < 1e-12 {
                gds = 1e-12;
            }
            let ceq_d = cdrain - gm * vgs_eff - gds * vds_eff - gmbs * vbs_eff;
            let ceq_bs = cbs_current - gbs_val * vbs;
            let ceq_bd = cbd_current - gbd_val * vbd;
            return MosfetCompanion {
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
            };
        }

        // Region: triode or saturation (selected by vdsx clamp).
        let cdo = vgsx - vth - 0.5 * (1.0 + fbody) * vdsx;
        let dcodvb = -dvtdvb - 0.5 * dfbdvb * vdsx;
        let cdnorm = cdo * vdsx;
        gm = vdsx;
        gds = if vds_mode > vdsat {
            -dvtdvd * vdsx
        } else {
            vgsx - vth - (1.0 + fbody + dvtdvd) * vdsx
        };
        gmbs = dcodvb * vdsx;

        let cd1 = beta * cdnorm;
        beta *= fgate;
        cdrain = beta * cdnorm;
        gm = beta * gm + dfgdvg * cd1;
        gds = beta * gds + dfgdvd * cd1;
        gmbs = beta * gmbs + dfgdvb * cd1;

        // ── Velocity saturation factor `fdrain` ────────────────────────
        let mut fdrain = 1.0;
        let mut dfddvg = 0.0;
        let mut dfddvd = 0.0;
        let mut dfddvb = 0.0;
        if self.vmax > 0.0 {
            fdrain = 1.0 / (1.0 + vdsx * onvdsc);
            let fd2 = fdrain * fdrain;
            let arga = fd2 * vdsx * onvdsc * onfg;
            dfddvg = -dfgdvg * arga;
            dfddvd = if vds_mode > vdsat {
                -dfgdvd * arga
            } else {
                -dfgdvd * arga - fd2 * onvdsc
            };
            dfddvb = -dfgdvb * arga;
            gm = fdrain * gm + dfddvg * cdrain;
            gds = fdrain * gds + dfddvd * cdrain;
            gmbs = fdrain * gmbs + dfddvb * cdrain;
            cdrain *= fdrain;
            // ngspice keeps `Beta *= fdrain` here for the CLM branch, but
            // we don't reuse beta after this point: the CLM branch scales
            // `cdrain` via `xlfact` directly. Skipping the assignment also
            // keeps clippy happy (it would otherwise flag a dead write).
        }

        // ── Channel-length modulation ──────────────────────────────────
        // Only active in the saturation region (vds_mode > vdsat). Uses
        // KAPPA + ALPHA (depletion-width squared). `coeffDepLayWidth^2 =
        // alpha`. If alpha=0 (no NSUB) the channel-shortening is skipped.
        let alpha = self.alpha_dep;
        let mut delxl = 0.0;
        if vds_mode > vdsat && alpha > 0.0 {
            let mut dldvg;
            let mut dldvd;
            let mut dldvb;
            if self.vmax <= 0.0 {
                // mos3load.c "line510" path with kappa·alpha·(Vds-Vdsat+Vdsat/8).
                let argv = vds_mode - vdsat + vdsat / 8.0;
                if argv > 0.0 {
                    delxl = (self.kappa * alpha * argv).sqrt();
                    dldvd = 0.5 * delxl / argv;
                } else {
                    dldvd = 0.0;
                }
                dldvg = 0.0;
                dldvb = 0.0;
                // The two dldsat·dvsdv* terms ngspice picks up via the
                // "line520 → diddl·ddld*" path are only nonzero when
                // VMAX>0; the VMAX≤0 branch sets ddld* to zero except
                // dldvd, which we keep as the direct contribution.
            } else {
                // VMAX>0 path (mos3load.c lines 748–781).
                let cdsat = cdrain;
                let gdsat = (cdsat * (1.0 - fdrain) * onvdsc).max(1.0e-12);
                let gdoncd = gdsat / cdsat.max(1e-30);
                let gdonfd = if fdrain < 1.0 {
                    gdsat / (1.0 - fdrain)
                } else {
                    0.0
                };
                let gdonfg = gdsat * onfg;
                let dgdvg = gdoncd * gm - gdonfd * dfddvg + gdonfg * dfgdvg;
                let dgdvd = gdoncd * gds - gdonfd * dfddvd + gdonfg * dfgdvd;
                let dgdvb = gdoncd * gmbs - gdonfd * dfddvb + gdonfg * dfgdvb;
                let emax = self.kappa * cdsat * one_over_xl / gdsat;
                let emoncd = if cdsat.abs() > 1e-30 {
                    emax / cdsat
                } else {
                    0.0
                };
                let emongd = emax / gdsat;
                let demdvg = emoncd * gm - emongd * dgdvg;
                let demdvd = emoncd * gds - emongd * dgdvd;
                let demdvb = emoncd * gmbs - emongd * dgdvb;

                let arga = 0.5 * emax * alpha;
                let argc = self.kappa * alpha;
                let argv = arga * arga + argc * (vds_mode - vdsat);
                let argb = argv.max(0.0).sqrt();
                delxl = argb - arga;
                let (dl_dvd_emax, dl_dem) = if argb > 0.0 {
                    (argc / (argb + argb), 0.5 * (arga / argb - 1.0) * alpha)
                } else {
                    (0.0, 0.0)
                };
                dldvg = dl_dem * demdvg;
                dldvd = dl_dem * demdvd - dl_dvd_emax;
                dldvb = dl_dem * demdvb;
                // ngspice keeps `dldvd` as the *direct* contribution and
                // returns the velocity-emax-routed term in `dldvd` minus
                // dl_dvd_emax. The expression above already matches.
            }

            // Punch-through approximation (mos3load.c lines 799–808).
            if delxl > 0.5 * l_eff {
                let new_delxl = l_eff - (l_eff * l_eff) / (4.0 * delxl);
                let scale = 4.0 * (l_eff - new_delxl) * (l_eff - new_delxl) / (l_eff * l_eff);
                dldvg *= scale;
                dldvd *= scale;
                dldvb *= scale;
                delxl = new_delxl;
            }

            let dlonxl = delxl * one_over_xl;
            let xlfact = 1.0 / (1.0 - dlonxl).max(1e-9);
            let cdrain_pre = cdrain;
            cdrain *= xlfact;
            let diddl = cdrain / (l_eff - delxl).max(1e-30);
            gm = gm * xlfact + diddl * dldvg;
            gmbs = gmbs * xlfact + diddl * dldvb;
            gds0 = diddl * dldvd;
            gm += gds0 * dvsdvg;
            gmbs += gds0 * dvsdvb;
            // ngspice's gds in the saturated CLM branch:
            //   gds = gds*xlfact + diddl*(direct dldvd into vdsx) + gds0*dvsdvd
            // The diddl·dldvd term is already the direct contribution
            // (already excluded the gds0 component) — see mos3load.c 824.
            gds = gds * xlfact + diddl * dldvd + gds0 * dvsdvd;
            let _ = cdrain_pre;
        }

        // ── Weak inversion (subthreshold) attenuator ───────────────────
        if has_nfs && vgs_eff < von {
            let onxn = 1.0 / xn;
            let ondvt = onxn / vt;
            let wfact = safe_exp(((vgs_eff - von) * ondvt).min(EXP_LIMIT));
            cdrain *= wfact;
            let gms = gm * wfact;
            let gmw = cdrain * ondvt;
            gm = gmw;
            if vds_mode > vdsat {
                gm += gds0 * dvsdvg * wfact;
            }
            gds = gds * wfact + (gms - gmw) * dvodvd;
            gmbs = gmbs * wfact + (gms - gmw) * dvodvb - gmw * (vgs_eff - von) * onxn * dxndvb;
        }

        // Floor gds for numerical health.
        if gds < 1e-12 {
            gds = 1e-12;
        }

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
            vdsat: vdsat.max(0.0),
            von,
        }
    }
}

/// Bulk junction diode current and conductance (same as Level 1/2).
fn bulk_diode_current(v: f64, is: f64, vt: f64) -> (f64, f64) {
    let gmin = 1e-12;
    if v <= -3.0 * vt {
        let g = gmin;
        let i = g * v - is;
        (g, i)
    } else {
        let ev = safe_exp((v / vt).min(EXP_LIMIT));
        let g = is * ev / vt + gmin;
        let i = is * (ev - 1.0) + gmin * v;
        (g, i)
    }
}

/// Cutoff companion (zero channel current, bulk diodes only).
#[allow(clippy::too_many_arguments)]
fn cutoff_companion(
    vbs: f64,
    vbd: f64,
    gbs_val: f64,
    cbs_current: f64,
    gbd_val: f64,
    cbd_current: f64,
    mode: i32,
    von: f64,
    _sign: f64,
) -> MosfetCompanion {
    MosfetCompanion {
        gm: 0.0,
        gds: 1e-12,
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
    }
}

/// Resolved node indices for a Level 3 MOSFET instance.
#[derive(Debug, Clone)]
pub struct Mos3Instance {
    pub name: String,
    pub drain_idx: Option<usize>,
    pub gate_idx: Option<usize>,
    pub source_idx: Option<usize>,
    pub bulk_idx: Option<usize>,
    pub drain_prime_idx: Option<usize>,
    pub source_prime_idx: Option<usize>,
    pub model: Mos3Model,
    pub w: f64,
    pub l: f64,
    pub ad: f64,
    pub as_: f64,
    pub pd: f64,
    pub ps: f64,
    pub m: f64,
}

impl Mos3Instance {
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

    /// Effective beta with W/L scaling: KP * W / L_eff.
    pub fn beta(&self) -> f64 {
        let l_eff = (self.l - 2.0 * self.model.ld).max(1e-12);
        self.model.kp * self.w / l_eff
    }

    /// Effective channel length.
    pub fn l_eff(&self) -> f64 {
        (self.l - 2.0 * self.model.ld).max(1e-12)
    }
}

/// Stamp the Level 3 MOSFET companion model into the MNA matrix and RHS.
///
/// Companion-model shape is identical to Levels 1/2/6, so the stamping
/// follows the same pattern as `mos2::stamp_mos2`.
pub fn stamp_mos3(
    matrix: &mut crate::SparseMatrix,
    rhs: &mut [f64],
    inst: &Mos3Instance,
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

    // 2. gm VCCS.
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

    // 4. gbd conductance between b and d'.
    crate::stamp_conductance(matrix, b, dp, m * comp.gbd);

    // 5. gbs conductance between b and s'.
    crate::stamp_conductance(matrix, b, sp, m * comp.gbs);

    // 6. Series resistances.
    if inst.model.rd > 0.0 {
        let grd = 1.0 / inst.model.rd;
        crate::stamp_conductance(matrix, inst.drain_idx, dp, m * grd);
    }
    if inst.model.rs > 0.0 {
        let grs = 1.0 / inst.model.rs;
        crate::stamp_conductance(matrix, inst.source_idx, sp, m * grs);
    }

    // 7. Equivalent current sources.
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
    use thevenin_types::Param;

    fn nmos_basic() -> Mos3Model {
        let mut m = Mos3Model::new(MosfetType::Nmos);
        m.vto = 0.7;
        m.kp = 2e-4;
        m.gamma = 0.5;
        m.phi = 0.7;
        m.tox = 1e-7;
        m.oxide_cap_factor = EPSOX / m.tox;
        m.xj = 0.5e-6;
        m.eta = 0.05;
        m.theta = 0.05;
        m.kappa = 0.5;
        m.vmax = 5e4;
        m.delta = 0.0;
        m
    }

    #[test]
    fn defaults_are_sane() {
        let m = Mos3Model::new(MosfetType::Nmos);
        assert_eq!(m.vto, 0.0);
        assert_eq!(m.kappa, 0.2);
        assert_eq!(m.theta, 0.0);
        assert_eq!(m.eta, 0.0);
        assert_eq!(m.vmax, 0.0);
        assert_eq!(m.nfs, 0.0);
    }

    #[test]
    fn from_model_def_picks_up_level3_params() {
        let md = ModelDef {
            name: "M".to_string(),
            kind: "NMOS".to_string(),
            params: vec![
                Param {
                    name: "LEVEL".to_string(),
                    value: Expr::Num(3.0),
                },
                Param {
                    name: "VTO".to_string(),
                    value: Expr::Num(0.7),
                },
                Param {
                    name: "KP".to_string(),
                    value: Expr::Num(200e-6),
                },
                Param {
                    name: "GAMMA".to_string(),
                    value: Expr::Num(0.5),
                },
                Param {
                    name: "PHI".to_string(),
                    value: Expr::Num(0.7),
                },
                Param {
                    name: "THETA".to_string(),
                    value: Expr::Num(0.05),
                },
                Param {
                    name: "ETA".to_string(),
                    value: Expr::Num(0.05),
                },
                Param {
                    name: "KAPPA".to_string(),
                    value: Expr::Num(0.3),
                },
                Param {
                    name: "VMAX".to_string(),
                    value: Expr::Num(5e4),
                },
                Param {
                    name: "NFS".to_string(),
                    value: Expr::Num(1e11),
                },
                Param {
                    name: "XJ".to_string(),
                    value: Expr::Num(0.5e-6),
                },
                Param {
                    name: "DELTA".to_string(),
                    value: Expr::Num(0.2),
                },
            ],
        };
        let m = Mos3Model::from_model_def(&md);
        assert_eq!(m.mos_type, MosfetType::Nmos);
        assert_abs_diff_eq!(m.vto, 0.7);
        assert_abs_diff_eq!(m.theta, 0.05);
        assert_abs_diff_eq!(m.eta, 0.05);
        assert_abs_diff_eq!(m.kappa, 0.3);
        assert_abs_diff_eq!(m.vmax, 5e4);
        assert!(m.narrow_factor > 0.0);
    }

    #[test]
    fn cutoff_returns_zero_current() {
        let m = nmos_basic();
        // Vgs = 0.3 < Vto ⇒ cutoff (Nfs = 0 path).
        let comp = m.companion(0.3, 1.0, 0.0, 2e-4, 10e-6, 1e-6);
        assert_abs_diff_eq!(comp.cdrain, 0.0);
        assert_abs_diff_eq!(comp.gm, 0.0);
    }

    #[test]
    fn linear_then_saturation_monotonic() {
        let m = nmos_basic();
        let beta = 2e-4;
        // Sweep Vds at fixed Vgs above threshold.
        let mut prev_id = -1.0;
        for &vds in &[0.1, 0.5, 1.0, 2.0, 4.0] {
            let comp = m.companion(2.0, vds, 0.0, beta, 10e-6, 1e-6);
            assert!(
                comp.cdrain.is_finite(),
                "Id finite at Vds={}: got {}",
                vds,
                comp.cdrain
            );
            assert!(
                comp.cdrain > prev_id - 1e-9,
                "Id should be monotonic in Vds"
            );
            prev_id = comp.cdrain;
        }
    }

    #[test]
    fn dibl_shifts_threshold() {
        // ETA > 0 must reduce Id (effective Vth lowered → no, raised by sign;
        // actually mos3load.c subtracts eta*Vds from Vbi, lowering Vth and
        // raising Id). Check that Id rises with ETA at fixed Vgs/Vds in
        // saturation.
        let mut m_lo = nmos_basic();
        m_lo.eta = 0.0;
        let mut m_hi = nmos_basic();
        m_hi.eta = 0.5;

        let beta = 2e-4;
        let id_lo = m_lo.companion(1.5, 3.0, 0.0, beta, 10e-6, 0.5e-6).cdrain;
        let id_hi = m_hi.companion(1.5, 3.0, 0.0, beta, 10e-6, 0.5e-6).cdrain;
        assert!(
            id_hi > id_lo,
            "DIBL (ETA>0) should raise Id: low={} high={}",
            id_lo,
            id_hi
        );
    }

    #[test]
    fn theta_reduces_current() {
        let mut m_lo = nmos_basic();
        m_lo.theta = 0.0;
        let mut m_hi = nmos_basic();
        m_hi.theta = 0.5;
        let beta = 2e-4;
        let id_lo = m_lo.companion(2.0, 3.0, 0.0, beta, 10e-6, 1e-6).cdrain;
        let id_hi = m_hi.companion(2.0, 3.0, 0.0, beta, 10e-6, 1e-6).cdrain;
        assert!(
            id_hi < id_lo,
            "mobility degradation (theta>0) should reduce Id: lo={} hi={}",
            id_lo,
            id_hi
        );
    }

    #[test]
    fn reversed_mode() {
        let m = nmos_basic();
        let comp = m.companion(2.0, -1.0, 0.0, 2e-4, 10e-6, 1e-6);
        assert_eq!(comp.mode, -1);
        assert!(comp.cdrain >= 0.0);
    }

    #[test]
    fn vds_zero_special_case() {
        let m = nmos_basic();
        // Vds = 0: should hit the line900 branch; gds > 0 still.
        let comp = m.companion(2.0, 0.0, 0.0, 2e-4, 10e-6, 1e-6);
        assert_abs_diff_eq!(comp.cdrain, 0.0);
        assert!(comp.gds > 0.0);
    }
}
