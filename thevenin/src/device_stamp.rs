//! Shared Newton-Raphson device stamping logic.
//!
//! This module extracts the nonlinear device companion stamping loop that is
//! shared between DC operating point (`simulate.rs`) and transient analysis
//! (`transient.rs`). Each device type follows the same pattern: extract terminal
//! voltages → apply voltage limiting → compute companion model → stamp MNA.

use std::cell::RefCell;

use crate::LinearSystem;
use crate::bjt::stamp_bjt;
use crate::bsim3::{bsim3_companion, bsim3_limit, stamp_bsim3};
use crate::bsim3soi_dd::{bsim3soi_dd_companion, bsim3soi_dd_limit, stamp_bsim3soi_dd};
use crate::bsim3soi_fd::{bsim3soi_fd_companion, bsim3soi_fd_limit, stamp_bsim3soi_fd};
use crate::bsim3soi_pd::{bsim3soi_pd_companion, bsim3soi_pd_limit, stamp_bsim3soi_pd};
use crate::bsim4::{bsim4_companion, bsim4_limit, stamp_bsim4};
use crate::diode::{VT_NOM, pnjlim, vcrit};
use crate::hfet::hfet_companion_full;
use crate::jfet::{jfet_limit, stamp_jfet};
use crate::mesa::{mesa_companion, stamp_mesa};
use crate::mesfet::{mesfet_companion, stamp_mesfet_with_voltages};
use crate::mna::{MnaSystem, stamp_conductance};
use crate::mosfet::{mos_limit, stamp_mosfet};
use crate::newton::NrMode;
use crate::vbic::{compute_self_heating_power, stamp_vbic_with_voltages};

/// Stamp a current source into the RHS vector.
/// Current flows from ni (pos) to nj (neg) externally:
/// subtract from ni, add to nj.
pub(crate) fn stamp_current_source(
    rhs: &mut [f64],
    ni: Option<usize>,
    nj: Option<usize>,
    i_val: f64,
) {
    if let Some(i) = ni {
        rhs[i] -= i_val;
    }
    if let Some(j) = nj {
        rhs[j] += i_val;
    }
}

/// FET voltage limiting (ngspice `DEVfetlim` from devsup.c).
///
/// Prevents large voltage jumps during NR iteration that could cause
/// divergence in FET device models. Used by BSIM3, BSIM4, and SOI variants.
pub(crate) fn fetlim(vnew: f64, vold: f64, vto: f64) -> f64 {
    let vtsthi = (2.0 * (vold - vto)).abs() + 2.0;
    let vtstlo = (vold - vto).abs() + 1.0;
    let vtox = vto + 3.5;
    let delv = vnew - vold;

    if vold >= vto {
        if vold >= vtox {
            if delv <= 0.0 {
                // going off
                if vnew >= vtox {
                    if -delv > vtstlo { vold - vtstlo } else { vnew }
                } else {
                    vnew.max(vto + 2.0)
                }
            } else {
                // staying on
                if delv >= vtsthi { vold + vtsthi } else { vnew }
            }
        } else {
            // middle region
            if delv <= 0.0 {
                vnew.max(vto - 0.5)
            } else {
                vnew.min(vto + 4.0)
            }
        }
    } else {
        // off
        if delv <= 0.0 {
            if -delv > vtsthi { vold - vtsthi } else { vnew }
        } else {
            let vtemp = vto + 0.5;
            if vnew <= vtemp {
                if delv > vtstlo { vold + vtstlo } else { vnew }
            } else {
                vtemp
            }
        }
    }
}

/// PN junction voltage limiting for BSIM models.
///
/// Uses `DEVpnjlim` formula (same as `diode::pnjlim`) with hardcoded
/// defaults (IS=1e-14, Vt at 300.15K) matching ngspice BSIM3/BSIM4
/// which call `DEVpnjlim(vbs, ..., CONSTvt0, model->vcrit, ...)`.
pub(crate) fn bsim_pnjlim(vnew: f64, vold: f64) -> f64 {
    const TEMP_DEFAULT: f64 = 300.15;
    let vt = crate::physics::KBOQ * TEMP_DEFAULT;
    let vcrit_val = vt * (vt / (std::f64::consts::SQRT_2 * 1e-14)).ln();
    crate::diode::pnjlim(vnew, vold, vt, vcrit_val)
}

/// Tracks previous junction/terminal voltages for NR voltage limiting.
///
/// Each device type needs its previous voltages preserved between NR iterations
/// to compute limiting deltas. This struct bundles all device voltage state and
/// the precomputed critical voltages.
pub(crate) struct DeviceVoltageState {
    prev_jct: RefCell<Vec<f64>>,
    vcrits: Vec<f64>,
    prev_bjt: RefCell<Vec<(f64, f64)>>,
    prev_mos: RefCell<Vec<(f64, f64, f64, f64)>>,
    prev_mos6: RefCell<Vec<(f64, f64, f64, f64)>>,
    prev_jfet: RefCell<Vec<(f64, f64)>>,
    jfet_vcrits: Vec<f64>,
    prev_bsim3: RefCell<Vec<(f64, f64, f64)>>,
    prev_bsim3soi_pd: RefCell<Vec<(f64, f64, f64, f64)>>,
    prev_bsim3soi_fd: RefCell<Vec<(f64, f64, f64, f64)>>,
    prev_bsim3soi_dd: RefCell<Vec<(f64, f64, f64, f64)>>,
    prev_bsim4: RefCell<Vec<(f64, f64, f64)>>,
    prev_vbic: RefCell<Vec<[f64; 6]>>,
    prev_mesa: RefCell<Vec<(f64, f64)>>,
    mesa_vcrits: Vec<f64>,
    mesa_vcritd: Vec<f64>,
    prev_mesfet: RefCell<Vec<(f64, f64)>>,
    mesfet_vcrits: Vec<f64>,
    prev_hfet: RefCell<Vec<(f64, f64)>>,
}

impl DeviceVoltageState {
    /// Create state with all previous voltages set to zero (for DC operating point).
    pub fn new_zero(mna: &MnaSystem) -> Self {
        let vcrits: Vec<f64> = mna
            .diodes
            .iter()
            .map(|d| vcrit(d.model.n * VT_NOM, d.model.is))
            .collect();
        let jfet_vcrits: Vec<f64> = mna
            .jfets
            .iter()
            .map(|j| vcrit(j.model.n * VT_NOM, j.model.is))
            .collect();

        Self {
            prev_jct: RefCell::new(vec![0.0; mna.diodes.len()]),
            vcrits,
            prev_bjt: RefCell::new(vec![(0.0, 0.0); mna.bjts.len()]),
            prev_mos: RefCell::new(vec![(0.0, 0.0, 0.0, 0.0); mna.mosfets.len()]),
            prev_mos6: RefCell::new(vec![(0.0, 0.0, 0.0, 0.0); mna.mos6s.len()]),
            prev_jfet: RefCell::new(vec![(0.0, 0.0); mna.jfets.len()]),
            jfet_vcrits,
            prev_bsim3: RefCell::new(vec![(0.0, 0.0, 0.0); mna.bsim3s.len()]),
            prev_bsim3soi_pd: RefCell::new(vec![(0.0, 0.0, 0.0, 0.0); mna.bsim3soi_pds.len()]),
            prev_bsim3soi_fd: RefCell::new(vec![(0.0, 0.0, 0.0, 0.0); mna.bsim3soi_fds.len()]),
            prev_bsim3soi_dd: RefCell::new(vec![(0.0, 0.0, 0.0, 0.0); mna.bsim3soi_dds.len()]),
            prev_bsim4: RefCell::new(vec![(0.0, 0.0, 0.0); mna.bsim4s.len()]),
            prev_vbic: RefCell::new(vec![[0.0; 6]; mna.vbics.len()]),
            prev_mesa: RefCell::new(vec![(0.0, 0.0); mna.mesas.len()]),
            mesa_vcrits: mna.mesas.iter().map(|m| m.precomp.vcrits).collect(),
            mesa_vcritd: mna.mesas.iter().map(|m| m.precomp.vcritd).collect(),
            prev_mesfet: RefCell::new(vec![(0.0, 0.0); mna.mesfets.len()]),
            mesfet_vcrits: mna.mesfets.iter().map(|m| m.model.vcrit).collect(),
            prev_hfet: RefCell::new(vec![(0.0, 0.0); mna.hfets.len()]),
        }
    }

    /// Create state initialized from a previous solution vector (for transient).
    pub fn from_solution(mna: &MnaSystem, prev_solution: &[f64]) -> Self {
        let vcrits: Vec<f64> = mna
            .diodes
            .iter()
            .map(|d| vcrit(d.model.n * VT_NOM, d.model.is))
            .collect();
        let jfet_vcrits: Vec<f64> = mna
            .jfets
            .iter()
            .map(|j| vcrit(j.model.n * VT_NOM, j.model.is))
            .collect();

        let prev_jct = mna
            .diodes
            .iter()
            .map(|d| {
                let (ja, jc) = if d.internal_idx.is_some() {
                    (d.internal_idx, d.cathode_idx)
                } else {
                    (d.anode_idx, d.cathode_idx)
                };
                let va = ja.map(|i| prev_solution[i]).unwrap_or(0.0);
                let vc = jc.map(|i| prev_solution[i]).unwrap_or(0.0);
                va - vc
            })
            .collect();

        Self {
            prev_jct: RefCell::new(prev_jct),
            vcrits,
            prev_bjt: RefCell::new(
                mna.bjts
                    .iter()
                    .map(|b| b.junction_voltages(prev_solution))
                    .collect(),
            ),
            prev_mos: RefCell::new(
                mna.mosfets
                    .iter()
                    .map(|m| {
                        let (vgs, vds, vbs) = m.terminal_voltages(prev_solution);
                        // Initialize von to 0.0 — it will be updated after the
                        // first companion call, matching ngspice's MOS1von default.
                        (vgs, vds, vbs, 0.0)
                    })
                    .collect(),
            ),
            prev_mos6: RefCell::new(
                mna.mos6s
                    .iter()
                    .map(|m| {
                        let (vgs, vds, vbs) = m.terminal_voltages(prev_solution);
                        (vgs, vds, vbs, 0.0)
                    })
                    .collect(),
            ),
            prev_jfet: RefCell::new(
                mna.jfets
                    .iter()
                    .map(|j| j.junction_voltages(prev_solution))
                    .collect(),
            ),
            jfet_vcrits,
            prev_bsim3: RefCell::new(
                mna.bsim3s
                    .iter()
                    .map(|b| b.terminal_voltages(prev_solution))
                    .collect(),
            ),
            prev_bsim3soi_pd: RefCell::new(
                mna.bsim3soi_pds
                    .iter()
                    .map(|b| b.terminal_voltages(prev_solution))
                    .collect(),
            ),
            prev_bsim3soi_fd: RefCell::new(
                mna.bsim3soi_fds
                    .iter()
                    .map(|b| b.terminal_voltages(prev_solution))
                    .collect(),
            ),
            prev_bsim3soi_dd: RefCell::new(
                mna.bsim3soi_dds
                    .iter()
                    .map(|b| b.terminal_voltages(prev_solution))
                    .collect(),
            ),
            prev_bsim4: RefCell::new(
                mna.bsim4s
                    .iter()
                    .map(|b| b.terminal_voltages(prev_solution))
                    .collect(),
            ),
            prev_vbic: RefCell::new(
                mna.vbics
                    .iter()
                    .map(|v| {
                        let (vbei, vbex, vbci, vbcx, vbep, _, _, _, vbcp) =
                            v.junction_voltages(prev_solution);
                        [vbei, vbex, vbci, vbcx, vbep, vbcp]
                    })
                    .collect(),
            ),
            prev_mesa: RefCell::new(
                mna.mesas
                    .iter()
                    .map(|m| m.junction_voltages(prev_solution))
                    .collect(),
            ),
            mesa_vcrits: mna.mesas.iter().map(|m| m.precomp.vcrits).collect(),
            mesa_vcritd: mna.mesas.iter().map(|m| m.precomp.vcritd).collect(),
            prev_mesfet: RefCell::new(
                mna.mesfets
                    .iter()
                    .map(|m| m.junction_voltages(prev_solution))
                    .collect(),
            ),
            mesfet_vcrits: mna.mesfets.iter().map(|m| m.model.vcrit).collect(),
            prev_hfet: RefCell::new(
                mna.hfets
                    .iter()
                    .map(|h| h.junction_voltages(prev_solution))
                    .collect(),
            ),
        }
    }

    /// Reset previous voltage states from a solution vector.
    /// Called before each NR attempt to prevent state corruption from carrying
    /// over between failed attempts (direct NR → gmin stepping → source stepping).
    pub fn reset_from_solution(&self, mna: &MnaSystem, solution: &[f64]) {
        // Reset VBIC prev voltages (most critical — self-heating + many internal
        // nodes makes VBIC very sensitive to corrupted prev state).
        {
            let mut prev = self.prev_vbic.borrow_mut();
            for (vi, vbic) in mna.vbics.iter().enumerate() {
                let (vbei, vbex, vbci, vbcx, vbep, _, _, _, vbcp) =
                    vbic.junction_voltages(solution);
                prev[vi] = [vbei, vbex, vbci, vbcx, vbep, vbcp];
            }
        }
        // Reset BJT prev voltages
        {
            let mut prev = self.prev_bjt.borrow_mut();
            for (i, bjt) in mna.bjts.iter().enumerate() {
                prev[i] = bjt.junction_voltages(solution);
            }
        }
        // Reset MOSFET prev voltages (von reset to 0.0 for fresh NR sequence)
        {
            let mut prev = self.prev_mos.borrow_mut();
            for (i, mos) in mna.mosfets.iter().enumerate() {
                let (vgs, vds, vbs) = mos.terminal_voltages(solution);
                prev[i] = (vgs, vds, vbs, 0.0);
            }
        }
        // Reset BSIM3SOI-FD/DD/PD prev voltages to prevent stale values from
        // a failed NR attempt poisoning the next one (same issue as MOSFET/BJT).
        {
            let mut prev = self.prev_bsim3soi_fd.borrow_mut();
            for (i, bsim) in mna.bsim3soi_fds.iter().enumerate() {
                prev[i] = bsim.terminal_voltages(solution);
            }
        }
        {
            let mut prev = self.prev_bsim3soi_dd.borrow_mut();
            for (i, bsim) in mna.bsim3soi_dds.iter().enumerate() {
                prev[i] = bsim.terminal_voltages(solution);
            }
        }
        {
            let mut prev = self.prev_bsim3soi_pd.borrow_mut();
            for (i, bsim) in mna.bsim3soi_pds.iter().enumerate() {
                prev[i] = bsim.terminal_voltages(solution);
            }
        }
        // Reset diode prev voltages
        {
            let mut prev = self.prev_jct.borrow_mut();
            for (di, diode) in mna.diodes.iter().enumerate() {
                let (ja, jc) = if diode.internal_idx.is_some() {
                    (diode.internal_idx, diode.cathode_idx)
                } else {
                    (diode.anode_idx, diode.cathode_idx)
                };
                let va = ja.map(|i| solution[i]).unwrap_or(0.0);
                let vc = jc.map(|i| solution[i]).unwrap_or(0.0);
                prev[di] = va - vc;
            }
        }
        // Reset MOS6 prev voltages (von reset to 0.0 for fresh NR sequence)
        {
            let mut prev = self.prev_mos6.borrow_mut();
            for (i, mos) in mna.mos6s.iter().enumerate() {
                let (vgs, vds, vbs) = mos.terminal_voltages(solution);
                prev[i] = (vgs, vds, vbs, 0.0);
            }
        }
        // Reset JFET prev voltages
        {
            let mut prev = self.prev_jfet.borrow_mut();
            for (i, jfet) in mna.jfets.iter().enumerate() {
                prev[i] = jfet.junction_voltages(solution);
            }
        }
        // Reset BSIM3v3 prev voltages
        {
            let mut prev = self.prev_bsim3.borrow_mut();
            for (i, bsim) in mna.bsim3s.iter().enumerate() {
                prev[i] = bsim.terminal_voltages(solution);
            }
        }
        // Reset BSIM4 prev voltages
        {
            let mut prev = self.prev_bsim4.borrow_mut();
            for (i, bsim) in mna.bsim4s.iter().enumerate() {
                prev[i] = bsim.terminal_voltages(solution);
            }
        }
        // Reset MESA prev voltages
        {
            let mut prev = self.prev_mesa.borrow_mut();
            for (i, mesa) in mna.mesas.iter().enumerate() {
                prev[i] = mesa.junction_voltages(solution);
            }
        }
        // Reset MESFET prev voltages
        {
            let mut prev = self.prev_mesfet.borrow_mut();
            for (i, mesfet) in mna.mesfets.iter().enumerate() {
                prev[i] = mesfet.junction_voltages(solution);
            }
        }
        // Reset HFET prev voltages
        {
            let mut prev = self.prev_hfet.borrow_mut();
            for (i, hfet) in mna.hfets.iter().enumerate() {
                prev[i] = hfet.junction_voltages(solution);
            }
        }
    }

    /// Stamp all nonlinear device companion models into the MNA system.
    ///
    /// This is the shared NR load logic used by both DC operating point and
    /// transient analysis. For each device type: extract terminal voltages from
    /// the current solution, apply voltage limiting, compute the companion
    /// model, and stamp it into the linear system.
    ///
    /// Get the current limited BJT junction voltages (vbe, vbc) for each BJT.
    /// Call after `stamp_devices()` to read the voltages used for the last stamp.
    pub fn prev_bjt_voltages(&self) -> Vec<(f64, f64)> {
        self.prev_bjt.borrow().clone()
    }

    /// Get the current limited MESA junction voltages (vgs, vgd) for each MESA FET.
    /// Call after `stamp_devices()` to read the voltages used for the last stamp.
    pub fn prev_mesa_voltages(&self) -> Vec<(f64, f64)> {
        self.prev_mesa.borrow().clone()
    }

    /// Get the current limited MOSFET voltages (vgs, vds, vbs, von) for each Level 1 MOSFET.
    /// Call after `stamp_devices()` to read the voltages used for the last stamp.
    pub fn prev_mos_voltages(&self) -> Vec<(f64, f64, f64, f64)> {
        self.prev_mos.borrow().clone()
    }

    /// Get the current limited MOS6 voltages (vgs, vds, vbs, von) for each Level 6 MOSFET.
    /// Call after `stamp_devices()` to read the voltages used for the last stamp.
    pub fn prev_mos6_voltages(&self) -> Vec<(f64, f64, f64, f64)> {
        self.prev_mos6.borrow().clone()
    }

    /// Get the current limited HFET junction voltages (vgs, vgd) for each HFET.
    /// Call after `stamp_devices()` to read the voltages used for the last stamp.
    pub fn prev_hfet_voltages(&self) -> Vec<(f64, f64)> {
        self.prev_hfet.borrow().clone()
    }

    /// Get the current limited BSIM3SOI-DD voltages (vgs, vds, vbs, ves) for each instance.
    /// Call after `stamp_devices()` to read the voltages used for the last stamp.
    #[allow(dead_code)]
    pub fn prev_bsim3soi_dd_voltages(&self) -> Vec<(f64, f64, f64, f64)> {
        self.prev_bsim3soi_dd.borrow().clone()
    }

    /// Stamp all nonlinear device companion models into the MNA system.
    ///
    /// In `NrMode::InitJct` (first iteration of each NR attempt), junction
    /// voltages are initialized to built-in potentials (vcrit for PN junctions,
    /// Vto for FETs) matching ngspice's MODEINITJCT.  This gives a physically
    /// reasonable starting point and prevents singular matrices from all-zeros.
    ///
    /// In `NrMode::Float` (subsequent iterations), voltage limiting (pnjlim,
    /// fetlim) is applied against previous iteration values, matching ngspice's
    /// MODEINITFLOAT.
    pub fn stamp_devices(
        &self,
        solution: &[f64],
        system: &mut LinearSystem,
        mna: &MnaSystem,
        gmin: f64,
        mode: NrMode,
    ) {
        let init_jct = mode == NrMode::InitJct;
        // Diodes
        {
            let mut prev = self.prev_jct.borrow_mut();
            for (di, diode) in mna.diodes.iter().enumerate() {
                let (jct_anode, jct_cathode) = if diode.internal_idx.is_some() {
                    (diode.internal_idx, diode.cathode_idx)
                } else {
                    (diode.anode_idx, diode.cathode_idx)
                };

                let v_jct = if init_jct {
                    // MODEINITJCT: initialize to critical voltage (built-in potential).
                    // Matches ngspice dioload.c: vd = here->DIOtVcrit
                    self.vcrits[di]
                } else {
                    let v_anode = jct_anode.map(|i| solution[i]).unwrap_or(0.0);
                    let v_cathode = jct_cathode.map(|i| solution[i]).unwrap_or(0.0);
                    let raw = v_anode - v_cathode;
                    pnjlim(raw, prev[di], diode.model.n * VT_NOM, self.vcrits[di])
                };
                prev[di] = v_jct;

                let (g_d, i_eq) = diode.model.companion(v_jct);
                stamp_conductance(&mut system.matrix, jct_anode, jct_cathode, g_d);
                stamp_current_source(&mut system.rhs, jct_anode, jct_cathode, i_eq);

                if let Some(int_idx) = diode.internal_idx {
                    let g_rs = 1.0 / diode.model.rs;
                    stamp_conductance(&mut system.matrix, diode.anode_idx, Some(int_idx), g_rs);
                }
            }
        }

        // BJTs
        {
            let mut prev = self.prev_bjt.borrow_mut();
            for (bi, bjt) in mna.bjts.iter().enumerate() {
                let (vbe, vbc) = if init_jct {
                    // MODEINITJCT: matches ngspice bjtload.c:
                    //   if (!here->BJToff) { vbe = type * vcrit; vbc = 0; }
                    //   else               { vbe = 0; vbc = 0; }
                    if bjt.off {
                        (0.0, 0.0)
                    } else {
                        let sign = bjt.model.bjt_type.sign();
                        (sign * bjt.model.vcrit_be(), 0.0)
                    }
                } else {
                    let (raw_vbe, raw_vbc) = bjt.junction_voltages(solution);
                    let vbe = bjt.model.limit_vbe(raw_vbe, prev[bi].0);
                    let vbc = bjt.model.limit_vbc(raw_vbc, prev[bi].1);
                    (vbe, vbc)
                };
                prev[bi] = (vbe, vbc);

                let comp = bjt.model.companion(vbe, vbc);
                stamp_bjt(&mut system.matrix, &mut system.rhs, bjt, &comp);
            }
        }

        // MOSFETs (level 1)
        {
            let mut prev = self.prev_mos.borrow_mut();
            for (mi, mos) in mna.mosfets.iter().enumerate() {
                let (vgs, vds, vbs) = if init_jct {
                    // MODEINITJCT: vgs = vds = type * Vto, vbs = 0.
                    // Matches ngspice mos1load.c:
                    //   vds = model->MOS1type * here->MOS1tVto;
                    //   vgs = vds;
                    //   vbs = 0;
                    let sign = mos.model.mos_type.sign();
                    let vto = sign * mos.model.vto;
                    (vto, vto, 0.0)
                } else {
                    let (raw_vgs, raw_vds, raw_vbs) = mos.terminal_voltages(solution);

                    // Apply voltage limiting for Level 1 MOSFETs, matching
                    // ngspice's MODEINITFLOAT which enables DEVfetlim/DEVlimvds.
                    let von_prev = prev[mi].3;
                    let (vgs, vds) = mos_limit(raw_vgs, raw_vds, prev[mi].0, prev[mi].1, von_prev);
                    let vbs = if vds >= 0.0 {
                        bsim_pnjlim(raw_vbs, prev[mi].2)
                    } else {
                        bsim_pnjlim(raw_vbs - vds, prev[mi].2 - prev[mi].1) + vds
                    };
                    (vgs, vds, vbs)
                };

                let mut eff_model = mos.model.clone();
                eff_model.kp = mos.beta();

                let comp = eff_model.companion(vgs, vds, vbs);
                // Store von for next iteration's fetlim (ngspice mos1load.c line 535):
                //   here->MOS1von = model->MOS1type * von;
                prev[mi] = (vgs, vds, vbs, comp.von);
                stamp_mosfet(&mut system.matrix, &mut system.rhs, mos, &comp);
            }
        }

        // MOS6 (level 6)
        {
            let mut prev = self.prev_mos6.borrow_mut();
            for (mi, mos) in mna.mos6s.iter().enumerate() {
                let (vgs, vds, vbs) = if init_jct {
                    // MODEINITJCT: same as Level 1.
                    let sign = mos.model.mos_type.sign();
                    let vto = sign * mos.model.vto;
                    (vto, vto, 0.0)
                } else {
                    let (raw_vgs, raw_vds, raw_vbs) = mos.terminal_voltages(solution);
                    let von_prev = prev[mi].3;
                    let (vgs, vds) = mos_limit(raw_vgs, raw_vds, prev[mi].0, prev[mi].1, von_prev);
                    let vbs = if vds >= 0.0 {
                        bsim_pnjlim(raw_vbs, prev[mi].2)
                    } else {
                        let vbd = raw_vbs - vds;
                        let vbd_lim = bsim_pnjlim(vbd, prev[mi].2 - prev[mi].1);
                        vbd_lim + vds
                    };
                    (vgs, vds, vbs)
                };

                let betac = mos.betac();
                let comp = mos.model.companion(vgs, vds, vbs, betac);
                prev[mi] = (vgs, vds, vbs, comp.von);
                crate::mos6::stamp_mos6(&mut system.matrix, &mut system.rhs, mos, &comp);
            }
        }

        // JFETs
        {
            let mut prev = self.prev_jfet.borrow_mut();
            for (ji, jfet) in mna.jfets.iter().enumerate() {
                let (vgs, vgd) = if init_jct {
                    // MODEINITJCT: vgs = vgd = type * Vto.
                    // Matches ngspice jfetload.c: vds = vgs = type * tThreshold.
                    // vgd = vgs - vds = 0 (but we set both to Vto for symmetry).
                    let sign = jfet.model.jfet_type.sign();
                    let vto = sign * jfet.model.vto;
                    (vto, vto)
                } else {
                    let (raw_vgs, raw_vgd) = jfet.junction_voltages(solution);
                    let vt = jfet.model.n * VT_NOM;
                    jfet_limit(
                        raw_vgs,
                        raw_vgd,
                        prev[ji].0,
                        prev[ji].1,
                        vt,
                        self.jfet_vcrits[ji],
                    )
                };
                prev[ji] = (vgs, vgd);

                let comp = jfet.model.companion(vgs, vgd);
                stamp_jfet(&mut system.matrix, &mut system.rhs, jfet, &comp);
            }
        }

        // BSIM3
        {
            let mut prev = self.prev_bsim3.borrow_mut();
            for (bi, bsim) in mna.bsim3s.iter().enumerate() {
                let (vgs, vds, vbs) = if init_jct {
                    // MODEINITJCT: vgs = type * vth0 + 0.1, vds = 0.1, vbs = 0.
                    // Matches ngspice b3ld.c:212: vgs = type * vth0 + 0.1; vds = 0.1;
                    let sign = bsim.model.mos_type.sign();
                    (sign * bsim.vth0_inst + 0.1, 0.1, 0.0)
                } else {
                    let (raw_vgs, raw_vds, raw_vbs) = bsim.terminal_voltages(solution);
                    bsim3_limit(
                        raw_vgs,
                        raw_vds,
                        raw_vbs,
                        prev[bi].0,
                        prev[bi].1,
                        prev[bi].2,
                        bsim.vth0_inst,
                    )
                };
                prev[bi] = (vgs, vds, vbs);

                let comp = bsim3_companion(vgs, vds, vbs, &bsim.size_params, &bsim.model);
                stamp_bsim3(&mut system.matrix, &mut system.rhs, bsim, &comp);
            }
        }

        // BSIM3SOI-PD
        {
            let mut prev = self.prev_bsim3soi_pd.borrow_mut();
            for (bi, bsim) in mna.bsim3soi_pds.iter().enumerate() {
                let (vgs, vds, vbs, ves) = if init_jct {
                    // MODEINITJCT: vgs = type*0.1 + vth0, vds = 0, vbs = 0, ves = 0.
                    // Matches ngspice b3soipdld.c:369: vgs = type*0.1 + vth0; vds = 0;
                    let sign = bsim.model.mos_type.sign();
                    (sign * 0.1 + bsim.vth0_inst, 0.0, 0.0, 0.0)
                } else {
                    let (raw_vgs, raw_vds, raw_vbs, raw_ves) = bsim.terminal_voltages(solution);
                    bsim3soi_pd_limit(
                        raw_vgs,
                        raw_vds,
                        raw_vbs,
                        raw_ves,
                        prev[bi].0,
                        prev[bi].1,
                        prev[bi].2,
                        prev[bi].3,
                        bsim.vth0_inst,
                    )
                };
                prev[bi] = (vgs, vds, vbs, ves);

                let comp =
                    bsim3soi_pd_companion(vgs, vds, vbs, ves, &bsim.size_params, &bsim.model);
                stamp_bsim3soi_pd(&mut system.matrix, &mut system.rhs, bsim, &comp, gmin);
            }
        }

        // BSIM3SOI-FD
        {
            let mut prev = self.prev_bsim3soi_fd.borrow_mut();
            for (bi, bsim) in mna.bsim3soi_fds.iter().enumerate() {
                let floating_body = bsim.body_idx.is_none();
                let (vgs, vds, vbs, ves) = if init_jct {
                    // MODEINITJCT: vgs = type*0.1 + vth0, vds = 0, vbs = 0, ves = 0.
                    // Matches ngspice b3soifdld.c:371: vgs = type*0.1 + vth0; vds = 0;
                    let sign = bsim.model.mos_type.sign();
                    (sign * 0.1 + bsim.vth0_inst, 0.0, 0.0, 0.0)
                } else {
                    let (raw_vgs, raw_vds, raw_vbs, raw_ves) = bsim.terminal_voltages(solution);
                    bsim3soi_fd_limit(
                        raw_vgs,
                        raw_vds,
                        raw_vbs,
                        raw_ves,
                        prev[bi].0,
                        prev[bi].1,
                        prev[bi].2,
                        prev[bi].3,
                        bsim.vth0_inst,
                        floating_body,
                    )
                };
                prev[bi] = (vgs, vds, vbs, ves);

                let comp = bsim3soi_fd_companion(
                    vgs,
                    vds,
                    vbs,
                    ves,
                    &bsim.size_params,
                    &bsim.model,
                    floating_body,
                );
                stamp_bsim3soi_fd(&mut system.matrix, &mut system.rhs, bsim, &comp, gmin);
            }
        }

        // BSIM3SOI-DD
        {
            let mut prev = self.prev_bsim3soi_dd.borrow_mut();
            for (bi, bsim) in mna.bsim3soi_dds.iter().enumerate() {
                let (vgs, vds, vbs, ves) = if init_jct {
                    // MODEINITJCT: vgs = type*0.1 + vth0, vds = 0, vbs = 0, ves = 0.
                    // Matches ngspice b3soiddld.c:400: vgs = type*0.1 + vth0; vds = 0;
                    let sign = bsim.model.mos_type.sign();
                    (sign * 0.1 + bsim.vth0_inst, 0.0, 0.0, 0.0)
                } else {
                    let (raw_vgs, raw_vds, raw_vbs, raw_ves) = bsim.terminal_voltages(solution);
                    bsim3soi_dd_limit(
                        raw_vgs,
                        raw_vds,
                        raw_vbs,
                        raw_ves,
                        prev[bi].0,
                        prev[bi].1,
                        prev[bi].2,
                        prev[bi].3,
                        bsim.vth0_inst,
                        bsim.body_idx.is_none(),
                    )
                };
                prev[bi] = (vgs, vds, vbs, ves);

                let comp =
                    bsim3soi_dd_companion(vgs, vds, vbs, ves, &bsim.size_params, &bsim.model);
                stamp_bsim3soi_dd(&mut system.matrix, &mut system.rhs, bsim, &comp, gmin);
            }
        }

        // BSIM4
        {
            let mut prev = self.prev_bsim4.borrow_mut();
            for (bi, bsim) in mna.bsim4s.iter().enumerate() {
                let (vgs, vds, vbs) = if init_jct {
                    // MODEINITJCT: vgs = type * vth0 + 0.1, vds = 0.1, vbs = 0.
                    // Matches ngspice b4v5ld.c:298-299: vgs = type * vth0 + 0.1;
                    let sign = bsim.model.mos_type.sign();
                    (sign * bsim.size_params.vth0 + 0.1, 0.1, 0.0)
                } else {
                    let (raw_vgs, raw_vds, raw_vbs) = bsim.terminal_voltages(solution);
                    bsim4_limit(
                        raw_vgs,
                        raw_vds,
                        raw_vbs,
                        prev[bi].0,
                        prev[bi].1,
                        prev[bi].2,
                        bsim.size_params.vth0,
                    )
                };
                prev[bi] = (vgs, vds, vbs);

                let comp = bsim4_companion(vgs, vds, vbs, &bsim.size_params, &bsim.model);
                stamp_bsim4(&mut system.matrix, &mut system.rhs, bsim, &comp);
            }
        }

        // VBIC
        {
            let mut prev = self.prev_vbic.borrow_mut();
            for (vi, vbic) in mna.vbics.iter().enumerate() {
                // Shared helper closures for voltage clamping.
                let clamp_jct = |v: f64| {
                    if v.is_nan() || v.is_infinite() {
                        0.0
                    } else {
                        v.clamp(-5.0, 5.0)
                    }
                };
                let clamp_res = |v: f64| {
                    if v.is_nan() || v.is_infinite() {
                        0.0
                    } else {
                        v.clamp(-50.0, 50.0)
                    }
                };

                // Self-heating: clone model and adjust temperature based on Vrth
                let vrth = if !init_jct && vbic.model.rth > 0.0 && vbic.rth_idx.is_some() {
                    let v = vbic.vrth(solution);
                    if v.is_nan() || v.is_infinite() {
                        0.0
                    } else {
                        v.clamp(-1e3, 1e3)
                    }
                } else {
                    0.0
                };
                let model = if vrth != 0.0 {
                    let mut m = vbic.model.clone();
                    m.temperature_adjust(vbic.t_ambient + vrth);
                    m
                } else {
                    vbic.model.clone()
                };

                let (vbei, vbex, vbci, vbcx, vbep, vrci, vrbi, vrbp, vbcp) = if init_jct {
                    // MODEINITJCT: initialize junctions for NR starting point.
                    // ngspice vbicload.c lines 250-258:
                    //   Vbe=Vbei=Vbex = type * VBICtVcrit
                    //   Vbci = -type * VBICtVcrit
                    //   Vbc=Vbcx=Vbep = 0
                    //   Vbcp = Vbc - Vbe = -type * VBICtVcrit
                    //   Vrci=Vrbi=Vrbp = 0
                    let sign = vbic.model.vbic_type.sign();
                    let vc = sign * model.vcrit_is();
                    let init_vbei = vc;
                    let init_vbex = vc;
                    let init_vbci = -vc;
                    let init_vbcp = -vc;
                    prev[vi] = [init_vbei, init_vbex, init_vbci, 0.0, 0.0, init_vbcp];
                    (
                        init_vbei, init_vbex, init_vbci, 0.0, 0.0, 0.0, 0.0, 0.0, init_vbcp,
                    )
                } else {
                    let (
                        raw_vbei,
                        raw_vbex,
                        raw_vbci,
                        raw_vbcx,
                        raw_vbep,
                        raw_vrci,
                        raw_vrbi,
                        raw_vrbp,
                        raw_vbcp,
                    ) = vbic.junction_voltages(solution);

                    let vrci = clamp_res(raw_vrci);
                    let vrbi = clamp_res(raw_vrbi);
                    let vrbp = clamp_res(raw_vrbp);

                    // If prev voltages are corrupted (NaN/Inf from a prior failed NR
                    // attempt), reset them to zero so that pnjlim works correctly.
                    if prev[vi].iter().any(|v| v.is_nan() || v.is_infinite()) {
                        prev[vi] = [0.0; 6];
                    }

                    // Apply pnjlim to all 6 junction voltages matching ngspice
                    // vbicload.c lines 656-665 where all use DEVpnjlim.
                    let vbei = model.limit_vbei(clamp_jct(raw_vbei), prev[vi][0]);
                    let vbex = model.limit_junction(clamp_jct(raw_vbex), prev[vi][1]);
                    let vbci = model.limit_vbci(clamp_jct(raw_vbci), prev[vi][2]);
                    let vbcx = model.limit_junction(clamp_jct(raw_vbcx), prev[vi][3]);
                    let vbep = model.limit_junction(clamp_jct(raw_vbep), prev[vi][4]);
                    let vbcp = model.limit_junction(clamp_jct(raw_vbcp), prev[vi][5]);
                    prev[vi] = [vbei, vbex, vbci, vbcx, vbep, vbcp];
                    (vbei, vbex, vbci, vbcx, vbep, vrci, vrbi, vrbp, vbcp)
                };

                let comp =
                    model.companion(vbei, vbex, vbci, vbcx, vbep, vrci, vrbi, vrbp, vbcp, gmin);

                stamp_vbic_with_voltages(
                    &mut system.matrix,
                    &mut system.rhs,
                    vbic,
                    &model,
                    &comp,
                    vbei,
                    vbex,
                    vbci,
                    vbcx,
                    vbep,
                    vrci,
                    vrbi,
                    vrbp,
                    vbcp,
                );

                // Self-heating thermal stamps (full electro-thermal Jacobian)
                //
                // ngspice (vbicload.c lines 1297-1464) stamps:
                // 1. Thermal diagonal: G_th = 1/RTH (Irth)
                // 2. Thermal RHS: Ith (power dissipation)
                // 3. Reverse coupling: dI_branch/dVrth in electrical rows
                //    (how each branch current changes with temperature)
                // 4. Thermal self-derivative: dIth/dVrth on thermal diagonal
                // 5. Forward coupling: dIth/dV_j in thermal row
                //    (how power changes with each electrical voltage)
                //
                // We compute derivatives numerically by perturbing Vrth.
                if let Some(rth_idx) = vbic.rth_idx {
                    let sign = model.vbic_type.sign();
                    let scale = vbic.m * vbic.area;
                    // Conductance: Irth = SCALE * Vrth / RTH
                    // ngspice multiplies Irth_Vrth by SCALE (vbicload.c line 4096),
                    // so the thermal conductance must include the area*m factor.
                    let g_th = scale / model.rth;
                    system.matrix.add(rth_idx, rth_idx, g_th);

                    // Compute external resistance voltages for power calculation
                    let node = |idx: Option<usize>| idx.map(|i| solution[i]).unwrap_or(0.0);
                    let v_c = node(vbic.coll_idx);
                    let v_b = node(vbic.base_idx);
                    let v_e = node(vbic.emit_idx);
                    let v_s = node(vbic.subs_idx);
                    let v_cx = node(vbic.coll_cx_idx);
                    let v_bx = node(vbic.base_bx_idx);
                    let v_ei = node(vbic.emit_ei_idx);
                    let v_si = node(vbic.subs_si_idx);
                    let vrcx = sign * (v_c - v_cx);
                    let vrbx = sign * (v_b - v_bx);
                    // ngspice convention: Vre = type*(V(emit) - V(emitEI)),
                    // Vrs = type*(V(subs) - V(subsSI)).
                    // Previous code had (v_ei - v_e) and (v_si - v_s) which
                    // is the opposite sign.  The power computation (vre²/RE)
                    // is unaffected, but the thermal derivative direction for
                    // the RE/RS reverse-coupling stamps was inverted.
                    let vre = sign * (v_e - v_ei);
                    let vrs = sign * (v_s - v_si);

                    let ith = compute_self_heating_power(
                        &comp, &model, vbei, vbex, vbci, vbep, vbcp, vrci, vrbi, vrbp, vrcx, vrbx,
                        vre, vrs, vbic.area, vbic.m,
                    );

                    // RHS: Ith flows into thermal node (power source).
                    // G_th * Vrth is handled by the matrix stamp above.
                    system.rhs[rth_idx] += ith;

                    // --- Full thermal Jacobian via numerical differentiation ---
                    // Compute companion at perturbed temperature to get dI/dVrth
                    // for all branch currents. This enables proper NR coupling
                    // between thermal and electrical domains, matching ngspice's
                    // auto-generated kernel derivatives.
                    if vrth.abs() > 1e-20 || ith.abs() > 1e-20 {
                        let delta = 1e-6; // 1μK temperature perturbation
                        let inv_delta = 1.0 / delta;

                        let mut model_pert = vbic.model.clone();
                        model_pert.temperature_adjust(vbic.t_ambient + vrth + delta);
                        let comp_pert = model_pert
                            .companion(vbei, vbex, vbci, vbcx, vbep, vrci, vrbi, vrbp, vbcp, gmin);

                        // Node aliases
                        let rth = Some(rth_idx);
                        let bi = vbic.base_bi_idx;
                        let bx = vbic.base_bx_idx;
                        let bp = vbic.base_bp_idx;
                        let ci = vbic.coll_ci_idx;
                        let cx = vbic.coll_cx_idx;
                        let ei = vbic.emit_ei_idx;
                        let si = vbic.subs_si_idx;
                        let c_ext = vbic.coll_idx;
                        let b_ext = vbic.base_idx;
                        let e_ext = vbic.emit_idx;
                        let s_ext = vbic.subs_idx;

                        // Helper: stamp reverse coupling for branch from np→nm
                        // dI/dVrth into electrical rows at thermal column
                        let mut m_add = |r: Option<usize>, c: Option<usize>, val: f64| {
                            if let (Some(r), Some(c)) = (r, c) {
                                system.matrix.add(r, c, val);
                            }
                        };
                        let mut r_add = |r: Option<usize>, val: f64| {
                            if let Some(r) = r {
                                system.rhs[r] += val;
                            }
                        };

                        // --- Reverse stamps: dI_branch/dVrth (electrical rows) ---
                        // Matches ngspice vbicload.c lines 1297-1414.
                        // Each branch: matrix[np, rth] += g, matrix[nm, rth] -= g
                        //              rhs[np] += g*vrth, rhs[nm] -= g*vrth
                        // NOTE: No VBICtype sign multiplier — ngspice's self-heating
                        // stamps don't apply model->VBICtype because type^2=1 cancels
                        // in the derivative (the kernel uses type-adjusted voltages).

                        // Helper macro for stamping a branch dI/dVrth
                        macro_rules! stamp_thermal_branch {
                            ($np:expr, $nm:expr, $di:expr) => {
                                let g = scale * $di;
                                m_add($np, rth, g);
                                m_add($nm, rth, -g);
                                r_add($np, g * vrth);
                                r_add($nm, -g * vrth);
                            };
                        }

                        // 1. Ibe: bi → ei
                        let d_ibe = (comp_pert.ibe - comp.ibe) * inv_delta;
                        stamp_thermal_branch!(bi, ei, d_ibe);

                        // 2. Ibex: bx → ei
                        let d_ibex = (comp_pert.ibex - comp.ibex) * inv_delta;
                        stamp_thermal_branch!(bx, ei, d_ibex);

                        // 3. Iciei: ci → ei (transport current)
                        let d_iciei = (comp_pert.iciei - comp.iciei) * inv_delta;
                        stamp_thermal_branch!(ci, ei, d_iciei);

                        // 4. Ibc: bi → ci (with avalanche)
                        let d_ibc = (comp_pert.ibc - comp.ibc) * inv_delta;
                        stamp_thermal_branch!(bi, ci, d_ibc);

                        // 5. Ibep: bx → bp
                        let d_ibep = (comp_pert.ibep - comp.ibep) * inv_delta;
                        stamp_thermal_branch!(bx, bp, d_ibep);

                        // 6. Rcx: c_ext → cx
                        if model.rcx > 0.0 && model.rcx_t > 0.0 {
                            let i_rcx = vrcx / model.rcx_t;
                            let i_rcx_pert = if model_pert.rcx_t > 0.0 {
                                vrcx / model_pert.rcx_t
                            } else {
                                0.0
                            };
                            let d_ircx = (i_rcx_pert - i_rcx) * inv_delta;
                            stamp_thermal_branch!(c_ext, cx, d_ircx);
                        }

                        // 7. Rci: cx → ci
                        let d_irci = (comp_pert.irci - comp.irci) * inv_delta;
                        stamp_thermal_branch!(cx, ci, d_irci);

                        // 8. Rbx: b_ext → bx
                        if model.rbx > 0.0 && model.rbx_t > 0.0 {
                            let i_rbx = vrbx / model.rbx_t;
                            let i_rbx_pert = if model_pert.rbx_t > 0.0 {
                                vrbx / model_pert.rbx_t
                            } else {
                                0.0
                            };
                            let d_irbx = (i_rbx_pert - i_rbx) * inv_delta;
                            stamp_thermal_branch!(b_ext, bx, d_irbx);
                        }

                        // 9. Rbi: bx → bi
                        let d_irbi = (comp_pert.irbi - comp.irbi) * inv_delta;
                        stamp_thermal_branch!(bx, bi, d_irbi);

                        // 10. Re: e_ext → ei (ngspice: emitNode → emitEINode)
                        if model.re > 0.0 && model.re_t > 0.0 {
                            let i_re = vre / model.re_t;
                            let i_re_pert = if model_pert.re_t > 0.0 {
                                vre / model_pert.re_t
                            } else {
                                0.0
                            };
                            let d_ire = (i_re_pert - i_re) * inv_delta;
                            stamp_thermal_branch!(e_ext, ei, d_ire);
                        }

                        // 11. Rbp: bp → cx
                        let d_irbp = (comp_pert.irbp - comp.irbp) * inv_delta;
                        stamp_thermal_branch!(bp, cx, d_irbp);

                        // 12. Ibcp: si → bp (parasitic B-C junction)
                        let d_ibcp = (comp_pert.ibcp - comp.ibcp) * inv_delta;
                        stamp_thermal_branch!(si, bp, d_ibcp);

                        // 13. Iccp: bx → si (parasitic transport)
                        let d_iccp = (comp_pert.iccp - comp.iccp) * inv_delta;
                        stamp_thermal_branch!(bx, si, d_iccp);

                        // 14. Rs: s_ext → si (ngspice: subsNode → subsSINode)
                        if model.rs > 0.0 && model.rs_t > 0.0 {
                            let i_rs = vrs / model.rs_t;
                            let i_rs_pert = if model_pert.rs_t > 0.0 {
                                vrs / model_pert.rs_t
                            } else {
                                0.0
                            };
                            let d_irs = (i_rs_pert - i_rs) * inv_delta;
                            stamp_thermal_branch!(s_ext, si, d_irs);
                        }

                        // --- Thermal self-derivative: dIth/dVrth ---
                        // Matches ngspice vbicload.c line 1433:
                        //   *(here->VBICtempTempPtr) += -Ith_Vrth;
                        let ith_pert = compute_self_heating_power(
                            &comp_pert,
                            &model_pert,
                            vbei,
                            vbex,
                            vbci,
                            vbep,
                            vbcp,
                            vrci,
                            vrbi,
                            vrbp,
                            vrcx,
                            vrbx,
                            vre,
                            vrs,
                            vbic.area,
                            vbic.m,
                        );
                        let d_ith = (ith_pert - ith) * inv_delta;
                        // Ith enters thermal node as positive (our convention).
                        // NR linearization: Ith ≈ Ith0 + dIth/dVrth*(Vrth-Vrth0)
                        // Matrix gets +dIth/dVrth, RHS gets -(Ith0 - dIth/dVrth*Vrth0)
                        // which is already accounted for by adding dIth/dVrth to diagonal
                        // and dIth/dVrth*vrth to RHS.
                        m_add(rth, rth, d_ith);
                        r_add(rth, d_ith * vrth);
                    }
                }
            }
        }

        // MESA FETs
        {
            let mut prev = self.prev_mesa.borrow_mut();
            for (mi, mesa) in mna.mesas.iter().enumerate() {
                let (vgs, vgd) = if init_jct {
                    // MODEINITJCT: vgs = Vto, vgd = Vto.
                    // Matches ngspice mesaload.c: vgs = type * tVto, vds = type * 0.1.
                    // MESA is always NMF (sign=+1).  Use vgd ≈ vgs for init.
                    let vto = mesa.precomp.t_vto;
                    (vto, vto)
                } else {
                    let (raw_vgs, raw_vgd) = mesa.junction_voltages(solution);
                    let vtes = mesa.model.n * crate::mesa::K_OVER_Q * mesa.precomp.ts;
                    let vted = mesa.model.n * crate::mesa::K_OVER_Q * mesa.precomp.td;
                    let vgs = pnjlim(raw_vgs, prev[mi].0, vtes, self.mesa_vcrits[mi]);
                    let vgd = pnjlim(raw_vgd, prev[mi].1, vted, self.mesa_vcritd[mi]);
                    let vgs = fetlim(vgs, prev[mi].0, mesa.precomp.t_vto);
                    let vgd = fetlim(vgd, prev[mi].1, mesa.precomp.t_vto);
                    (vgs, vgd)
                };
                prev[mi] = (vgs, vgd);

                let comp = mesa_companion(mesa, vgs, vgd, 1e-12);
                stamp_mesa(&comp, mesa, &mut system.matrix, &mut system.rhs);
            }
        }

        // MESFETs
        {
            let mut prev = self.prev_mesfet.borrow_mut();
            for (mi, mes) in mna.mesfets.iter().enumerate() {
                let (vgs, vgd) = if init_jct {
                    // MODEINITJCT: vgs = vgd = Vt0.
                    // Matches ngspice mesfetload.c MODEINITJCT behavior.
                    let vto = mes.model.vt0;
                    (vto, vto)
                } else {
                    let (raw_vgs, raw_vgd) = mes.junction_voltages(solution);
                    let vgs = pnjlim(raw_vgs, prev[mi].0, VT_NOM, self.mesfet_vcrits[mi]);
                    let vgd = pnjlim(raw_vgd, prev[mi].1, VT_NOM, self.mesfet_vcrits[mi]);
                    let vgs = fetlim(vgs, prev[mi].0, mes.model.vt0);
                    let vgd = fetlim(vgd, prev[mi].1, mes.model.vt0);
                    (vgs, vgd)
                };
                prev[mi] = (vgs, vgd);

                let comp = mesfet_companion(mes, vgs, vgd, 1e-12);
                stamp_mesfet_with_voltages(
                    &comp,
                    mes,
                    vgs,
                    vgd,
                    &mut system.matrix,
                    &mut system.rhs,
                );
            }
        }

        // HFETs
        {
            let mut prev = self.prev_hfet.borrow_mut();
            for (hi, hfet) in mna.hfets.iter().enumerate() {
                let (vgs, vgd) = if init_jct {
                    // MODEINITJCT with device ON (HFETAoff==0):
                    // ngspice hfetload.c lines 114-119 uses vgs=vgd=-1
                    // (reverse bias) — NOT vt0. This is critical for
                    // bistable circuits like the DCFL inverter where the
                    // initial guess determines which OP the NR solver finds.
                    (-1.0, -1.0)
                } else {
                    let (raw_vgs, raw_vgd) = hfet.junction_voltages(solution);
                    if hfet.model.level == 6 {
                        // HFET2 (hfet2load.c lines 179-185) applies both
                        // DEVpnjlim and DEVfetlim voltage limiting.
                        let vgs_lim = pnjlim(raw_vgs, prev[hi].0, VT_NOM, hfet.precomp.vcrit);
                        let vgd_lim = pnjlim(raw_vgd, prev[hi].1, VT_NOM, hfet.precomp.vcrit);
                        let vgs = fetlim(vgs_lim, prev[hi].0, hfet.precomp.t_vto);
                        let vgd = fetlim(vgd_lim, prev[hi].1, hfet.precomp.t_vto);
                        (vgs, vgd)
                    } else {
                        // HFET1 (hfetload.c line 268-269) applies only DEVfetlim.
                        let vgs = fetlim(raw_vgs, prev[hi].0, hfet.precomp.t_vto);
                        let vgd = fetlim(raw_vgd, prev[hi].1, hfet.precomp.t_vto);
                        (vgs, vgd)
                    }
                };
                prev[hi] = (vgs, vgd);

                let comp = hfet_companion_full(hfet, vgs, vgd, gmin);
                crate::hfet::stamp_hfet_with_voltages(
                    &comp,
                    hfet,
                    vgs,
                    vgd,
                    &mut system.matrix,
                    &mut system.rhs,
                );
            }
        }

        // Behavioral current sources (B-elements with I=...)
        for bsrc in &mna.behavioral_sources {
            // Build node voltage map from current solution
            let mut node_voltages: std::collections::BTreeMap<String, f64> =
                std::collections::BTreeMap::new();
            node_voltages.insert("0".to_string(), 0.0);
            for (name, idx) in mna.node_map.iter() {
                node_voltages.insert(name.to_string(), solution[idx]);
            }

            let i0_raw = crate::expr::evaluate_bsrc_expr(&bsrc.expr, &node_voltages).unwrap_or(0.0);
            // Apply temperature coefficient scaling
            let i0 = i0_raw * bsrc.tc_factor;

            // Numerical Jacobian: perturb each node by DV and measure dI/dV
            const DV: f64 = 1e-8;
            let mut i_eq = i0;

            for (name, idx) in mna.node_map.iter() {
                let v_old = node_voltages[name];
                let mut perturbed = node_voltages.clone();
                *perturbed.get_mut(name).unwrap() = v_old + DV;
                let i1_raw = crate::expr::evaluate_bsrc_expr(&bsrc.expr, &perturbed).unwrap_or(0.0);
                let g = (i1_raw * bsrc.tc_factor - i0) / DV;

                if g.abs() > 1e-30 {
                    if let Some(ni) = bsrc.pos_idx {
                        system.matrix.add(ni, idx, g);
                    }
                    if let Some(nj) = bsrc.neg_idx {
                        system.matrix.add(nj, idx, -g);
                    }
                    i_eq -= g * v_old;
                }
            }

            stamp_current_source(&mut system.rhs, bsrc.pos_idx, bsrc.neg_idx, i_eq);
        }

        // Behavioral voltage sources (B-elements with V=...)
        for bvsrc in &mna.behavioral_voltage_sources {
            // Build node voltage map from current solution
            let mut node_voltages: std::collections::BTreeMap<String, f64> =
                std::collections::BTreeMap::new();
            node_voltages.insert("0".to_string(), 0.0);
            for (name, idx) in mna.node_map.iter() {
                node_voltages.insert(name.to_string(), solution[idx]);
            }

            let f0_raw =
                crate::expr::evaluate_bsrc_expr(&bvsrc.expr, &node_voltages).unwrap_or(0.0);
            // Guard against NaN/Inf from expressions like ln(0) at initial guess,
            // then apply temperature coefficient scaling
            let f0 = if f0_raw.is_finite() {
                f0_raw * bvsrc.tc_factor
            } else {
                0.0
            };

            // Numerical Jacobian: perturb each node by DV and measure df/dV
            const DV: f64 = 1e-8;
            let branch = bvsrc.branch_idx;
            let mut rhs_adjust = 0.0;

            for (name, idx) in mna.node_map.iter() {
                let v_old = node_voltages[name];
                let mut perturbed = node_voltages.clone();
                *perturbed.get_mut(name).unwrap() = v_old + DV;
                let f1_raw =
                    crate::expr::evaluate_bsrc_expr(&bvsrc.expr, &perturbed).unwrap_or(0.0);
                let f1 = if f1_raw.is_finite() {
                    f1_raw * bvsrc.tc_factor
                } else {
                    f0
                };
                let dfdv = (f1 - f0) / DV;

                if dfdv.is_finite() && dfdv.abs() > 1e-30 {
                    // Branch equation: V(pos) - V(neg) - f(V*) = 0
                    // Jacobian contribution: -df/dVi
                    system.matrix.add(branch, idx, -dfdv);
                    rhs_adjust -= dfdv * v_old;
                }
            }

            // RHS: f(V*) - sum(df/dVi * Vi*) is the linearized constant term.
            // The base matrix already has +1/-1 for V(pos)-V(neg).
            system.rhs[branch] += f0 + rhs_adjust;
        }

        // XSPICE code model instances
        if let Some(ref registry) = mna.xspice_registry {
            for inst in &mna.xspice_instances {
                let cm_def = match registry.get(&inst.model_type) {
                    Some(def) => def,
                    None => continue,
                };

                // Extract port values from current solution
                let port_values: Vec<f64> = inst
                    .port_connections
                    .iter()
                    .map(|conn| {
                        let port_def = &cm_def.ports[conn.port_def_index];
                        match port_def.port_type {
                            thevenin_xspice::PortType::Voltage
                            | thevenin_xspice::PortType::Conductance
                            | thevenin_xspice::PortType::DifferentialVoltage => {
                                let vp = conn.pos_idx.map(|i| solution[i]).unwrap_or(0.0);
                                let vn = conn.neg_idx.map(|i| solution[i]).unwrap_or(0.0);
                                vp - vn
                            }
                            thevenin_xspice::PortType::Current
                            | thevenin_xspice::PortType::Resistance => {
                                conn.branch_idx.map(|i| solution[i]).unwrap_or(0.0)
                            }
                        }
                    })
                    .collect();

                let mut state_borrow = inst.state.borrow_mut();
                let inputs = thevenin_xspice::CmInputs {
                    port_values: &port_values,
                    params: &inst.params,
                    mode: thevenin_xspice::AnalysisMode::DcOp,
                };

                let outputs = cm_def.evaluate(&inputs, &mut **state_borrow);

                thevenin_xspice::stamp_xspice_instance(
                    &mut system.matrix,
                    &mut system.rhs,
                    inst,
                    &cm_def.ports,
                    &outputs,
                    &port_values,
                );
            }
        }
    }
}
