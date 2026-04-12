//! MOSFET Level 2 (Grove-Frohman) device model.
//!
//! Implements the SPICE Level 2 MOSFET model with velocity saturation,
//! short/narrow channel effects, subthreshold conduction, and channel
//! length modulation. Translated from ngspice mos2load.c / mos2temp.c.

use thevenin_types::{Expr, ModelDef};

use crate::diode::VT_NOM;
use crate::mosfet::{MosfetCompanion, MosfetType};
use crate::physics::{EXP_LIMIT, safe_exp};

/// Physical constants matching ngspice.
const CHARGE: f64 = 1.602_176_634e-19;
const EPSSIL: f64 = 11.70 * 8.854_214_871e-12;
const EPSOX: f64 = 3.9 * 8.854_214_871e-12;
const NI: f64 = 1.45e16; // intrinsic carrier concentration at 300K in m⁻³

/// Sign arrays for Baum quartic solver (matches ngspice sig1/sig2).
const SIG1: [f64; 4] = [1.0, -1.0, 1.0, -1.0];
const SIG2: [f64; 4] = [1.0, 1.0, -1.0, -1.0];

/// MOSFET Level 2 model parameters.
#[derive(Debug, Clone)]
pub struct Mos2Model {
    pub mos_type: MosfetType,
    /// Threshold voltage (V).
    pub vto: f64,
    /// Transconductance parameter (A/V²).
    pub kp: f64,
    /// Body effect coefficient.
    pub gamma: f64,
    /// Surface potential (V).
    pub phi: f64,
    /// Channel length modulation (1/V).
    pub lambda: f64,
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
    /// Substrate doping (1/cm³).
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

    // --- Level 2 specific parameters ---
    /// Critical field for mobility degradation (V/cm).
    pub ucrit: f64,
    /// Mobility degradation exponent.
    pub uexp: f64,
    /// Maximum drift velocity (m/s).
    pub vmax: f64,
    /// Junction depth (m).
    pub xj: f64,
    /// Narrow channel effect factor (delta).
    pub delta: f64,
    /// Fast surface state density (1/cm²).
    pub nfs: f64,
    /// Total channel charge coefficient.
    pub neff: f64,

    // --- Derived parameters (computed from process params) ---
    /// Depletion width factor: sqrt(2*eps_si / (q * Nsub)).
    pub xd: f64,
    /// Oxide capacitance per unit area (F/m²).
    pub oxide_cap_factor: f64,
}

impl Mos2Model {
    pub fn new(mos_type: MosfetType) -> Self {
        let oxide_cap_factor = EPSOX / 1e-7; // default tox=1e-7
        Self {
            mos_type,
            vto: 0.0,
            kp: 2e-5,
            gamma: 0.0,
            phi: 0.6,
            lambda: 0.0,
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
            tox: 1e-7,
            ld: 0.0,
            nsub: 0.0,
            u0: 600.0,
            fc: 0.5,
            kf: 0.0,
            af: 1.0,
            nss: 0.0,
            tpg: 1.0,
            ucrit: 0.0,
            uexp: 0.0,
            vmax: 0.0,
            xj: 0.0,
            delta: 0.0,
            nfs: 0.0,
            neff: 1.0,
            xd: 0.0,
            oxide_cap_factor,
        }
    }

    /// Create a `Mos2Model` from a netlist `.model` definition.
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
                    "LAMBDA" => m.lambda = *v,
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
                    "UCRIT" => m.ucrit = *v,
                    "UEXP" => m.uexp = *v,
                    "VMAX" => m.vmax = *v,
                    "XJ" => m.xj = *v,
                    "DELTA" => m.delta = *v,
                    "NFS" => m.nfs = *v,
                    "NEFF" => m.neff = *v,
                    _ => {} // ignore unknown params (LEVEL, etc.)
                }
            }
        }
        m.compute_process_params(vto_given, gamma_given, phi_given, nsub_given, kp_given);
        m
    }

    /// Compute derived model parameters from process parameters.
    /// Matches ngspice mos2temp.c model-level processing.
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
            self.kp = self.u0 * 1e-4 * self.oxide_cap_factor;
        }

        if !nsub_given {
            return;
        }

        let nsub_m3 = self.nsub * 1e6;
        if nsub_m3 <= NI {
            return;
        }

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

        // Depletion width factor (used in short-channel effect).
        self.xd = (2.0 * EPSSIL / (CHARGE * nsub_m3)).sqrt();
    }

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

    /// Compute the Level 2 MOSFET operating point.
    ///
    /// `beta` is the effective KP * W / L_eff from the instance.
    /// `w` and `l_eff` are needed for oxide cap and narrow-channel calculations.
    pub fn companion(
        &self,
        vgs: f64,
        vds: f64,
        vbs: f64,
        beta: f64,
        w: f64,
        l_eff: f64,
    ) -> MosfetCompanion {
        let vt = VT_NOM;

        // Determine mode
        let (lvgs, lvds, lvbs, mode) = if vds >= 0.0 {
            (vgs, vds, vbs, 1)
        } else {
            (vgs - vds, -vds, vbs - vds, -1)
        };

        let phi_min_vbs = self.phi - lvbs;

        // Compute sarg1 (sqrt at source) and its derivatives
        let (sarg1, dsrgdb, d2sdb2);
        if lvbs <= 0.0 {
            sarg1 = phi_min_vbs.sqrt();
            dsrgdb = -0.5 / sarg1;
            d2sdb2 = 0.5 * dsrgdb / phi_min_vbs;
        } else {
            let sphi = self.phi.sqrt();
            let sphi3 = self.phi * sphi;
            let s = sphi / (1.0 + 0.5 * lvbs / self.phi);
            let tmp = s / sphi3;
            sarg1 = s;
            dsrgdb = -0.5 * s * tmp;
            d2sdb2 = -dsrgdb * tmp;
        }

        // Compute barg (sqrt at drain) and its derivatives
        let (barg, dbrgdb, d2bdb2);
        if (lvbs - lvds) <= 0.0 {
            barg = (phi_min_vbs + lvds).sqrt();
            dbrgdb = -0.5 / barg;
            d2bdb2 = 0.5 * dbrgdb / (phi_min_vbs + lvds);
        } else {
            let sphi = self.phi.sqrt();
            let sphi3 = self.phi * sphi;
            let b = sphi / (1.0 + 0.5 * (lvbs - lvds) / self.phi);
            let tmp = b / sphi3;
            barg = b;
            dbrgdb = -0.5 * b * tmp;
            d2bdb2 = -dbrgdb * tmp;
        }

        // Oxide cap (unscaled per-device)
        let unscaled_oxide_cap = self.oxide_cap_factor * l_eff * w;

        // Narrow-channel effect factor
        let factor =
            0.125 * self.delta * 2.0 * std::f64::consts::PI * EPSSIL / unscaled_oxide_cap * l_eff;
        let eta = 1.0 + factor;
        let sign = self.mos_type.sign();
        let vbin = sign * self.vto - sign * self.gamma * self.phi.sqrt() + factor * phi_min_vbs;

        // Short-channel effect
        let (gamasd, dgddvb, mut dgdvds, mut dgddb2);
        if self.gamma > 0.0 || self.nsub > 0.0 {
            let xwd = self.xd * barg;
            let xws = self.xd * sarg1;

            let (mut argss, mut argsd, mut dbargs, mut dbargd, mut argxs, mut argxd);
            argss = 0.0;
            argsd = 0.0;
            dbargs = 0.0;
            dbargd = 0.0;
            argxs = 0.0;
            argxd = 0.0;
            dgdvds = 0.0;
            dgddb2 = 0.0;

            if self.xj > 0.0 {
                let tmp = 2.0 / self.xj;
                argxs = 1.0 + xws * tmp;
                argxd = 1.0 + xwd * tmp;
                let args = argxs.sqrt();
                let argd = argxd.sqrt();
                let tmp2 = 0.5 * self.xj / l_eff;
                argss = tmp2 * (args - 1.0);
                argsd = tmp2 * (argd - 1.0);
            }
            gamasd = self.gamma * (1.0 - argss - argsd);
            let dbxwd = self.xd * dbrgdb;
            let dbxws = self.xd * dsrgdb;
            if self.xj > 0.0 {
                let tmp = 0.5 / l_eff;
                let args = argxs.sqrt();
                let argd = argxd.sqrt();
                dbargs = tmp * dbxws / args;
                dbargd = tmp * dbxwd / argd;
                let dasdb2 = -self.xd * (d2sdb2 + dsrgdb * dsrgdb * self.xd / (self.xj * argxs))
                    / (l_eff * args);
                let daddb2 = -self.xd * (d2bdb2 + dbrgdb * dbrgdb * self.xd / (self.xj * argxd))
                    / (l_eff * argd);
                dgddb2 = -0.5 * self.gamma * (dasdb2 + daddb2);
            }
            dgddvb = -self.gamma * (dbargs + dbargd);
            if self.xj > 0.0 {
                let ddxwd = -dbxwd;
                let argd = argxd.sqrt();
                dgdvds = -self.gamma * 0.5 * ddxwd / (l_eff * argd);
            }
        } else {
            gamasd = self.gamma;
            dgddvb = 0.0;
            dgdvds = 0.0;
            dgddb2 = 0.0;
        }

        let von_base = vbin + gamasd * sarg1;
        #[allow(unused_variables)]
        let vth = von_base;

        // Subthreshold parameters
        let (xn, argg, has_nfs);
        let von;
        let vgst;
        if self.nfs != 0.0 && unscaled_oxide_cap > 0.0 {
            let cfs = CHARGE * self.nfs * 1e4;
            let cdonco = -(gamasd * dsrgdb + dgddvb * sarg1) + factor;
            xn = 1.0 + cfs / unscaled_oxide_cap * w * l_eff + cdonco;
            let tmp = vt * xn;
            von = von_base + tmp;
            argg = 1.0 / tmp;
            vgst = lvgs - von;
            has_nfs = true;
        } else {
            von = von_base;
            vgst = lvgs - von;
            xn = 1.0;
            argg = 0.0;
            has_nfs = false;

            if lvgs <= vbin {
                // Cutoff
                return make_cutoff_companion(vbs, vds, self.is, vt, mode, von, sign);
            }
        }

        // Derived quantities
        let sarg3 = sarg1 * sarg1 * sarg1;
        let sbiarg = self.pb.sqrt().max(0.01);
        let gammad = gamasd;
        let dgdvbs = dgddvb;
        let body = barg * barg * barg - sarg3;
        let gdbdv = 2.0 * gammad * (barg * barg * dbrgdb - sarg1 * sarg1 * dsrgdb);
        let mut dodvbs = -factor + dgdvbs * sarg1 + gammad * dsrgdb;
        let mut dodvds = 0.0;
        let mut dxndvd = 0.0;
        let mut dxndvb = 0.0;

        if has_nfs && unscaled_oxide_cap > 0.0 {
            dxndvb = 2.0 * dgdvbs * dsrgdb + gammad * d2sdb2 + dgddb2 * sarg1;
            dodvbs += vt * dxndvb;
            dxndvd = dgdvds * dsrgdb;
            dodvds = dgdvds * sarg1 + vt * dxndvd;
        }

        // Effective mobility and its derivatives
        let (ufact, ueff, dudvgs, dudvds, dudvbs);
        if unscaled_oxide_cap > 0.0 && self.ucrit > 0.0 {
            let udenom = vgst;
            let tmp = self.ucrit * 100.0 * EPSSIL / self.oxide_cap_factor;
            if udenom > tmp {
                ufact = (tmp / udenom).powf(self.uexp);
                ueff = self.u0 * 1e-4 * ufact;
                dudvgs = -ufact * self.uexp / udenom;
                dudvds = 0.0;
                dudvbs = self.uexp * ufact * dodvbs / vgst;
            } else {
                ufact = 1.0;
                ueff = self.u0 * 1e-4;
                dudvgs = 0.0;
                dudvds = 0.0;
                dudvbs = 0.0;
            }
        } else {
            ufact = 1.0;
            ueff = self.u0 * 1e-4;
            dudvgs = 0.0;
            dudvds = 0.0;
            dudvbs = 0.0;
        }

        // Saturation voltage (Grove-Frohman)
        let gammad_eta = gamasd / eta;
        let (mut vdsat, mut dsdvgs, mut dsdvbs);
        let vgsx = if has_nfs && unscaled_oxide_cap > 0.0 {
            lvgs.max(von)
        } else {
            lvgs
        };

        if gammad_eta > 0.0 {
            let gammd2 = gammad_eta * gammad_eta;
            let argv = (vgsx - vbin) / eta + phi_min_vbs;
            if argv <= 0.0 {
                vdsat = 0.0;
                dsdvgs = 0.0;
                dsdvbs = 0.0;
            } else {
                let arg1 = (1.0 + 4.0 * argv / gammd2).sqrt();
                vdsat = (vgsx - vbin) / eta + gammd2 * (1.0 - arg1) / 2.0;
                vdsat = vdsat.max(0.0);
                dsdvgs = (1.0 - 1.0 / arg1) / eta;
                dsdvbs = (gammad_eta * (1.0 - arg1) + 2.0 * argv / (gammad_eta * arg1)) / eta
                    * dgdvbs
                    + 1.0 / arg1
                    + factor * dsdvgs;
            }
        } else {
            vdsat = (vgsx - vbin) / eta;
            vdsat = vdsat.max(0.0);
            dsdvgs = 1.0;
            dsdvbs = 0.0;
        }

        // Baum's velocity saturation (optional)
        if self.vmax > 0.0 {
            #[allow(unused_variables)]
            let gammd2 = gammad_eta * gammad_eta;
            let v1 = (vgsx - vbin) / eta + phi_min_vbs;
            let v2 = phi_min_vbs;
            let xv = self.vmax * l_eff / ueff;
            let a1 = gammad_eta / 0.75;
            let b1 = -2.0 * (v1 + xv);
            let c1 = -2.0 * gammad_eta * xv;
            let d1 = 2.0 * v1 * (v2 + xv) - v2 * v2 - 4.0 / 3.0 * gammad_eta * sarg3;
            let a = -b1;
            let b = a1 * c1 - 4.0 * d1;
            let c = -d1 * (a1 * a1 - 4.0 * b1) - c1 * c1;
            let r = -a * a / 3.0 + b;
            let s = 2.0 * a * a * a / 27.0 - a * b / 3.0 + c;
            let r3 = r * r * r;
            let s2 = s * s;
            let p = s2 / 4.0 + r3 / 27.0;
            let p0 = p.abs();
            let p2 = p0.sqrt();
            let y3 = if p < 0.0 {
                let ro = (s2 / 4.0 + p0).sqrt();
                let ro = (ro.ln() / 3.0).exp();
                let fi = (-2.0 * p2 / s).atan();
                2.0 * ro * (fi / 3.0).cos() - a / 3.0
            } else {
                let p3_val = -s / 2.0 + p2;
                let p3 = (p3_val.abs().ln() / 3.0).exp();
                let p4_val = -s / 2.0 - p2;
                let p4 = (p4_val.abs().ln() / 3.0).exp();
                p3 + p4 - a / 3.0
            };

            let a3_sq = a1 * a1 / 4.0 - b1 + y3;
            let b3_sq = y3 * y3 / 4.0 - d1;
            if a3_sq >= 0.0 && b3_sq >= 0.0 {
                let a3 = a3_sq.sqrt();
                let b3 = b3_sq.sqrt();
                let mut xvalid = f64::MAX;
                let mut found = false;
                for i in 0..4 {
                    let a4 = a1 / 2.0 + SIG1[i] * a3;
                    let b4 = y3 / 2.0 + SIG2[i] * b3;
                    let delta4 = a4 * a4 / 4.0 - b4;
                    if delta4 < 0.0 {
                        continue;
                    }
                    let tmp = delta4.sqrt();
                    for &x in &[-a4 / 2.0 + tmp, -a4 / 2.0 - tmp] {
                        if x <= 0.0 {
                            continue;
                        }
                        let poly = x * x * x * x + a1 * x * x * x + b1 * x * x + c1 * x + d1;
                        if poly.abs() <= 1.0e-6 && x < xvalid {
                            xvalid = x;
                            found = true;
                        }
                    }
                }
                if found {
                    vdsat = xvalid * xvalid - phi_min_vbs;
                }
            }
        }

        // Effective channel length and CLM derivatives
        let (mut dldvgs, mut dldvds, mut dldvbs);
        let mut xlamda = self.lambda;

        // bsarg, bodys, gdbdvs for saturation region
        let (bsarg, dbsrdb, bodys, gdbdvs);
        if (lvbs - vdsat) <= 0.0 {
            let bs = (vdsat + phi_min_vbs).sqrt();
            bsarg = bs;
            dbsrdb = -0.5 / bs;
        } else {
            let sphi = self.phi.sqrt();
            let sphi3 = self.phi * sphi;
            let bs = sphi / (1.0 + 0.5 * (lvbs - vdsat) / self.phi);
            bsarg = bs;
            dbsrdb = -0.5 * bs * bs / sphi3;
        }
        bodys = bsarg * bsarg * bsarg - sarg3;
        gdbdvs = 2.0 * gammad * (bsarg * bsarg * dbsrdb - sarg1 * sarg1 * dsrgdb);

        if lvds != 0.0 {
            if self.vmax <= 0.0 {
                if self.nsub == 0.0 || xlamda > 0.0 {
                    // Use given lambda, no CLM calculation
                    dldvgs = 0.0;
                    dldvds = 0.0;
                    dldvbs = 0.0;
                } else {
                    let argv = (lvds - vdsat) / 4.0;
                    let sargv = (1.0 + argv * argv).sqrt();
                    let arg1 = (argv + sargv).sqrt();
                    let xlfact = self.xd / (l_eff * lvds);
                    xlamda = xlfact * arg1;
                    let dldsat = lvds * xlamda / (8.0 * sargv);
                    dldvgs = dldsat * dsdvgs;
                    dldvds = -xlamda + dldsat;
                    dldvbs = dldsat * dsdvbs;
                }
            } else {
                let argv = (vgsx - vbin) / eta - vdsat;
                let xdv = self.xd / self.neff.sqrt();
                let xlv = self.vmax * xdv / (2.0 * ueff);
                let vqchan = argv - gammad_eta * bsarg;
                let dqdsat = -1.0 + gammad_eta * dbsrdb;
                let vl = self.vmax * l_eff;
                let dfunds = vl * dqdsat - ueff * vqchan;
                let dfundg = (vl - ueff * vdsat) / eta;
                let dfundb = -vl * (1.0 + dqdsat - factor / eta)
                    + ueff * (gdbdvs - dgdvbs * bodys / 1.5) / eta;
                dsdvgs = -dfundg / dfunds;
                dsdvbs = -dfundb / dfunds;
                if self.nsub == 0.0 || xlamda > 0.0 {
                    dldvgs = 0.0;
                    dldvds = 0.0;
                    dldvbs = 0.0;
                } else {
                    let argv2 = (lvds - vdsat).max(0.0);
                    let xls = (xlv * xlv + argv2).sqrt();
                    let dldsat = xdv / (2.0 * xls);
                    let xlfact = xdv / (l_eff * lvds);
                    xlamda = xlfact * (xls - xlv);
                    let dldsat = dldsat / l_eff;
                    dldvgs = dldsat * dsdvgs;
                    dldvds = -xlamda + dldsat;
                    dldvbs = dldsat * dsdvbs;
                }
            }
        } else {
            dldvgs = 0.0;
            dldvds = 0.0;
            dldvbs = 0.0;
        }

        // Limit channel shortening at punch-through
        let xwb = self.xd * sbiarg;
        let xwb = if self.nsub == 0.0 { 0.25e-6 } else { xwb };
        let xld = l_eff - xwb;
        let clfact;
        {
            let raw_clfact = 1.0 - xlamda * lvds;
            dldvds = -xlamda - dldvds;
            let xleff = l_eff * raw_clfact;
            let deltal = xlamda * lvds * l_eff;
            if xleff < xwb {
                let xleff_lim = xwb / (1.0 + (deltal - xld) / xwb);
                clfact = xleff_lim / l_eff;
                let dfact = xleff_lim * xleff_lim / (xwb * xwb);
                dldvgs *= dfact;
                dldvds *= dfact;
                dldvbs *= dfact;
            } else {
                clfact = raw_clfact;
            }
        }

        // Effective beta
        let beta1 = beta * ufact / clfact;

        // Branch on operating region
        let (cdrain, gm, mut gds, gmbs);

        // Near-zero Vds special case
        if lvds <= 1.0e-10 {
            if lvgs <= von {
                if !has_nfs || unscaled_oxide_cap <= 0.0 {
                    return make_cutoff_companion(vbs, vds, self.is, vt, mode, von, sign);
                }
                gds = beta1
                    * (von - vbin - gammad * sarg1)
                    * safe_exp((argg * (lvgs - von)).min(EXP_LIMIT));
                return make_companion_with_gds(
                    vbs, vds, self.is, vt, mode, 0.0, 0.0, gds, 0.0, von, 0.0, sign, lvgs, lvds,
                    lvbs,
                );
            }
            gds = beta1 * (lvgs - vbin - gammad * sarg1);
            return make_companion_with_gds(
                vbs, vds, self.is, vt, mode, 0.0, 0.0, gds, 0.0, von, 0.0, sign, lvgs, lvds, lvbs,
            );
        }

        // Check for subthreshold
        let in_subthreshold = if has_nfs && unscaled_oxide_cap > 0.0 {
            lvgs <= von
        } else {
            lvgs <= vbin
        };

        if in_subthreshold && has_nfs {
            // Subthreshold region
            if vdsat <= 0.0 {
                return make_cutoff_companion(vbs, vds, self.is, vt, mode, von, sign);
            }

            let vdson = vdsat.min(lvds);
            let (barg_sub, _dbrgdb_sub, body_sub, gdbdv_sub) = if lvds > vdsat {
                (bsarg, dbsrdb, bodys, gdbdvs)
            } else {
                (barg, dbrgdb, body, gdbdv)
            };

            let cdson =
                beta1 * ((von_base - vbin - eta * vdson * 0.5) * vdson - gammad * body_sub / 1.5);
            let didvds = beta1 * (von_base - vbin - eta * vdson - gammad * barg_sub);
            let mut gdson = -cdson * dldvds / clfact - beta1 * dgdvds * body_sub / 1.5;
            if lvds < vdsat {
                gdson += didvds;
            }
            let mut gbson = -cdson * dldvbs / clfact
                + beta1 * (dodvbs * vdson + factor * vdson - dgdvbs * body_sub / 1.5 - gdbdv_sub);
            if lvds > vdsat {
                gbson += didvds * dsdvbs;
            }

            let expg = safe_exp((argg * (lvgs - von)).min(EXP_LIMIT));
            cdrain = cdson * expg;
            let gmw = cdrain * argg;
            gm = if lvds > vdsat {
                gmw + didvds * dsdvgs * expg
            } else {
                gmw
            };
            let tmp = gmw * (lvgs - von) / xn;
            gds = gdson * expg - gm * dodvds - tmp * dxndvd;
            gmbs = gbson * expg - gm * dodvbs - tmp * dxndvb;
        } else if in_subthreshold {
            // Cutoff (no NFS)
            return make_cutoff_companion(vbs, vds, self.is, vt, mode, von, sign);
        } else if lvds <= vdsat {
            // Linear region
            cdrain = beta1 * ((lvgs - vbin - eta * lvds / 2.0) * lvds - gammad * body / 1.5);
            let arg1 = cdrain * (dudvgs / ufact - dldvgs / clfact);
            gm = arg1 + beta1 * lvds;
            let arg1 = cdrain * (dudvds / ufact - dldvds / clfact);
            gds = arg1 + beta1 * (lvgs - vbin - eta * lvds - gammad * barg - dgdvds * body / 1.5);
            let arg1 = cdrain * (dudvbs / ufact - dldvbs / clfact);
            gmbs = arg1 - beta1 * (gdbdv + dgdvbs * body / 1.5 - factor * lvds);
        } else {
            // Saturation region
            cdrain = beta1 * ((lvgs - vbin - eta * vdsat / 2.0) * vdsat - gammad * bodys / 1.5);
            let arg1 = cdrain * (dudvgs / ufact - dldvgs / clfact);
            gm = arg1
                + beta1 * vdsat
                + beta1 * (lvgs - vbin - eta * vdsat - gammad * bsarg) * dsdvgs;
            gds = -cdrain * dldvds / clfact - beta1 * dgdvds * bodys / 1.5;
            let arg1 = cdrain * (dudvbs / ufact - dldvbs / clfact);
            gmbs = arg1 - beta1 * (gdbdvs + dgdvbs * bodys / 1.5 - factor * vdsat)
                + beta1 * (lvgs - vbin - eta * vdsat - gammad * bsarg) * dsdvbs;
        }

        // Floor gds — must provide sufficient conductance to keep floating
        // internal nodes from creating a singular matrix. This matches the
        // Gmin added to the diagonal by ngspice's gmin stepping.
        if gds < 1e-12 {
            gds = 1e-12;
        }

        // Bulk junction diodes
        let vbd = vbs - vds;
        let (gbs_val, cbs_current) = bulk_diode_current(vbs, self.is, vt);
        let (gbd_val, cbd_current) = bulk_diode_current(vbd, self.is, vt);

        let ceq_d = cdrain - gm * lvgs - gds * lvds - gmbs * lvbs;
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

/// Bulk junction diode current and conductance (same as Level 1).
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

/// Build a cutoff companion (zero current, zero conductances).
fn make_cutoff_companion(
    vbs: f64,
    vds: f64,
    is: f64,
    vt: f64,
    mode: i32,
    von: f64,
    _sign: f64,
) -> MosfetCompanion {
    let vbd = vbs - vds;
    let (gbs_val, cbs_current) = bulk_diode_current(vbs, is, vt);
    let (gbd_val, cbd_current) = bulk_diode_current(vbd, is, vt);

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

/// Build a companion with given gds (for near-zero Vds special case).
#[allow(clippy::too_many_arguments)]
fn make_companion_with_gds(
    vbs: f64,
    vds: f64,
    is: f64,
    vt: f64,
    mode: i32,
    cdrain: f64,
    gm: f64,
    gds: f64,
    gmbs: f64,
    von: f64,
    vdsat: f64,
    _sign: f64,
    lvgs: f64,
    lvds: f64,
    lvbs: f64,
) -> MosfetCompanion {
    let vbd = vbs - vds;
    let (gbs_val, cbs_current) = bulk_diode_current(vbs, is, vt);
    let (gbd_val, cbd_current) = bulk_diode_current(vbd, is, vt);

    let ceq_d = cdrain - gm * lvgs - gds * lvds - gmbs * lvbs;
    let ceq_bs = cbs_current - gbs_val * vbs;
    let ceq_bd = cbd_current - gbd_val * vbd;

    MosfetCompanion {
        gm,
        gds: gds.max(1e-12),
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

/// Resolved node indices for a Level 2 MOSFET instance.
#[derive(Debug, Clone)]
pub struct Mos2Instance {
    pub name: String,
    pub drain_idx: Option<usize>,
    pub gate_idx: Option<usize>,
    pub source_idx: Option<usize>,
    pub bulk_idx: Option<usize>,
    pub drain_prime_idx: Option<usize>,
    pub source_prime_idx: Option<usize>,
    pub model: Mos2Model,
    pub w: f64,
    pub l: f64,
    pub ad: f64,
    pub as_: f64,
    pub pd: f64,
    pub ps: f64,
    pub m: f64,
}

impl Mos2Instance {
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

/// Stamp the Level 2 MOSFET companion model into the MNA matrix and RHS.
/// Uses the same stamping pattern as Level 1 (stamp_mosfet).
pub fn stamp_mos2(
    matrix: &mut crate::SparseMatrix,
    rhs: &mut [f64],
    inst: &Mos2Instance,
    comp: &MosfetCompanion,
) {
    // Delegate to Level 1 stamping — the companion model structure is identical.
    // We construct a temporary MosfetInstance wrapper for stamp_mosfet.
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

    // 1. gds output conductance between d' and s'
    crate::stamp_conductance(matrix, dp, sp, m * comp.gds);

    // 2. gm VCCS
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

    // 3. gmbs body-effect transconductance
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

    // 4. gbd conductance between b and d'
    crate::stamp_conductance(matrix, b, dp, m * comp.gbd);

    // 5. gbs conductance between b and s'
    crate::stamp_conductance(matrix, b, sp, m * comp.gbs);

    // 6. Series resistances
    if inst.model.rd > 0.0 {
        let grd = 1.0 / inst.model.rd;
        crate::stamp_conductance(matrix, inst.drain_idx, dp, m * grd);
    }
    if inst.model.rs > 0.0 {
        let grs = 1.0 / inst.model.rs;
        crate::stamp_conductance(matrix, inst.source_idx, sp, m * grs);
    }

    // 7. Equivalent current sources
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

    #[test]
    fn test_default_mos2_model() {
        let m = Mos2Model::new(MosfetType::Nmos);
        assert_eq!(m.vto, 0.0);
        assert_eq!(m.kp, 2e-5);
        assert_eq!(m.gamma, 0.0);
        assert_eq!(m.phi, 0.6);
        assert_eq!(m.lambda, 0.0);
        assert_eq!(m.ucrit, 0.0);
        assert_eq!(m.uexp, 0.0);
        assert_eq!(m.vmax, 0.0);
    }

    #[test]
    fn test_from_model_def_with_level2_params() {
        let model_def = ModelDef {
            name: "M".to_string(),
            kind: "NMOS".to_string(),
            params: vec![
                Param {
                    name: "NSUB".to_string(),
                    value: Expr::Num(2.2e15),
                },
                Param {
                    name: "UO".to_string(),
                    value: Expr::Num(575.0),
                },
                Param {
                    name: "UCRIT".to_string(),
                    value: Expr::Num(49e3),
                },
                Param {
                    name: "UEXP".to_string(),
                    value: Expr::Num(0.1),
                },
                Param {
                    name: "TOX".to_string(),
                    value: Expr::Num(0.11e-6),
                },
                Param {
                    name: "XJ".to_string(),
                    value: Expr::Num(2.95e-6),
                },
                Param {
                    name: "LEVEL".to_string(),
                    value: Expr::Num(2.0),
                },
                Param {
                    name: "LD".to_string(),
                    value: Expr::Num(2.4485e-6),
                },
                Param {
                    name: "NSS".to_string(),
                    value: Expr::Num(3.2e10),
                },
                Param {
                    name: "KP".to_string(),
                    value: Expr::Num(2e-5),
                },
                Param {
                    name: "PHI".to_string(),
                    value: Expr::Num(0.6),
                },
            ],
        };
        let m = Mos2Model::from_model_def(&model_def);
        assert_eq!(m.mos_type, MosfetType::Nmos);
        assert_abs_diff_eq!(m.ucrit, 49e3, epsilon = 1e-6);
        assert_abs_diff_eq!(m.uexp, 0.1, epsilon = 1e-15);
        assert_abs_diff_eq!(m.xj, 2.95e-6, epsilon = 1e-15);
        assert!(m.nsub > 0.0);
        assert!(m.xd > 0.0, "xd should be computed from nsub");
    }

    #[test]
    fn test_mos2_cutoff() {
        let mut m = Mos2Model::new(MosfetType::Nmos);
        m.vto = 1.0;
        m.kp = 1e-4;
        // Vgs = 0.5 < Vto = 1.0 → cutoff
        let comp = m.companion(0.5, 5.0, 0.0, 1e-4, 100e-6, 10e-6);
        assert_abs_diff_eq!(comp.cdrain, 0.0, epsilon = 1e-15);
        assert_abs_diff_eq!(comp.gm, 0.0, epsilon = 1e-15);
        assert_abs_diff_eq!(comp.gds, 1e-12, epsilon = 1e-15);
    }

    #[test]
    fn test_mos2_linear() {
        let mut m = Mos2Model::new(MosfetType::Nmos);
        m.vto = 1.0;
        m.kp = 1e-4;
        // No velocity saturation or short-channel effects, should behave like Level 1.
        // Vgs=3, Vds=1, Vbs=0 → linear region
        // Vgst = 2, Id ≈ beta * Vds * (Vgst - Vds/2) = 1e-4 * 1 * 1.5 = 1.5e-4
        let comp = m.companion(3.0, 1.0, 0.0, 1e-4, 100e-6, 10e-6);
        assert!(
            comp.cdrain > 0.0,
            "should have positive current in linear region"
        );
        assert_eq!(comp.mode, 1);
    }

    #[test]
    fn test_mos2_saturation() {
        let mut m = Mos2Model::new(MosfetType::Nmos);
        m.vto = 1.0;
        m.kp = 1e-4;
        // Vgs=3, Vds=5, Vbs=0 → saturation (Vgst=2 < Vds=5)
        let comp = m.companion(3.0, 5.0, 0.0, 1e-4, 100e-6, 10e-6);
        assert!(
            comp.cdrain > 0.0,
            "should have positive current in saturation"
        );
        assert_eq!(comp.mode, 1);
    }

    #[test]
    fn test_mos2_reversed() {
        let mut m = Mos2Model::new(MosfetType::Nmos);
        m.vto = 1.0;
        m.kp = 1e-4;
        let comp = m.companion(3.0, -1.0, 0.0, 1e-4, 100e-6, 10e-6);
        assert_eq!(comp.mode, -1);
        assert!(comp.cdrain > 0.0);
    }

    #[test]
    fn test_mos2_velocity_saturation() {
        let mut m = Mos2Model::new(MosfetType::Nmos);
        m.vto = 1.0;
        m.kp = 1e-4;
        m.ucrit = 1e4;
        m.uexp = 0.1;
        // With mobility degradation, current should be reduced vs. no degradation
        let comp_no_ucrit = {
            let mut m2 = m.clone();
            m2.ucrit = 0.0;
            m2.uexp = 0.0;
            m2.companion(3.0, 5.0, 0.0, 1e-4, 100e-6, 10e-6)
        };
        let comp_ucrit = m.companion(3.0, 5.0, 0.0, 1e-4, 100e-6, 10e-6);
        assert!(
            comp_ucrit.cdrain < comp_no_ucrit.cdrain,
            "mobility degradation should reduce current: {} vs {}",
            comp_ucrit.cdrain,
            comp_no_ucrit.cdrain
        );
    }
}
