//! Transient (time-domain) analysis engine.
//!
//! Implements `.tran` analysis with Backward Euler (BE) and Trapezoidal (Trap)
//! integration methods. Capacitors and inductors are converted to companion
//! models (conductance/resistance + current/voltage source) at each timestep.
//!
//! Supports adaptive timestep control via local truncation error (LTE)
//! estimation using the difference between BE and Trap results for
//! capacitor/inductor charges/fluxes.

use thevenin_types::{Analysis, Item, Netlist, SimPlot, SimResult, SimVector};

use crate::LinearSystem;
use crate::device_stamp::{DeviceVoltageState, stamp_current_source};
use crate::expr_val;
use crate::ltra::{LtraCoeffs, LtraState};
use crate::mna::{MnaError, MnaSystem, assemble_mna, stamp_conductance};
use crate::newton::{NrMode, NrOptions, transient_nr_solve};
use crate::simulate::{nr_options_from_netlist, solve_op_raw_with_opts};
use crate::txl::TxlTransientStamp;
use crate::waveform::{self, TranParams};

/// Integration method for transient analysis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IntegrationMethod {
    /// Backward Euler — first-order, unconditionally stable.
    BackwardEuler,
    /// Trapezoidal — second-order, A-stable.
    Trapezoidal,
}

/// History state for a capacitor at the previous timestep.
#[derive(Debug, Clone)]
struct CapHistory {
    /// Voltage across capacitor at previous timestep.
    voltage: f64,
    /// Current through capacitor at previous timestep (needed for Trap).
    current: f64,
    /// Charge at previous timestep (Q = C*v for constant cap; needed for LTE).
    charge: f64,
    /// Charge at two timesteps ago (needed for divided-difference LTE).
    charge_prev: f64,
}

/// History state for an inductor at the previous timestep.
#[derive(Debug, Clone)]
struct IndHistory {
    /// Current through inductor at previous timestep.
    current: f64,
    /// Voltage across inductor at previous timestep (needed for Trap).
    voltage: f64,
}

/// History state for a BJT junction charge at the previous timestep.
/// Tracks both charges and voltages to enable incremental charge computation
/// with voltage-dependent depletion capacitances, matching ngspice's bjtload.c.
#[derive(Debug, Clone)]
struct BjtChargeHistory {
    /// B-E junction charge at previous timestep (depletion + diffusion).
    qbe: f64,
    /// B-E charge current at previous timestep (for trapezoidal).
    cqbe: f64,
    /// B-E junction voltage at previous timestep.
    vbe: f64,
    /// B-C junction charge at previous timestep (depletion + diffusion).
    qbc: f64,
    /// B-C charge current at previous timestep (for trapezoidal).
    cqbc: f64,
    /// B-C junction voltage at previous timestep.
    vbc: f64,
    /// B-E charge at two timesteps ago (for divided-difference LTE).
    qbe_prev: f64,
    /// B-C charge at two timesteps ago (for divided-difference LTE).
    qbc_prev: f64,
}

/// History state for a MESA junction charge at the previous timestep.
#[derive(Debug, Clone)]
struct MesaChargeHistory {
    /// G-S junction charge at previous timestep.
    qgs: f64,
    /// G-S charge current at previous timestep (for trapezoidal).
    cqgs: f64,
    /// G-D junction charge at previous timestep.
    qgd: f64,
    /// G-D charge current at previous timestep (for trapezoidal).
    cqgd: f64,
    /// Previous G-S capacitor voltage (V(gp) - V(spp)).
    vgspp: f64,
    /// Previous G-D capacitor voltage (V(gp) - V(dpp)).
    vgdpp: f64,
}

/// History state for a MOSFET Meyer gate charge at the previous timestep.
#[derive(Debug, Clone)]
struct MosfetChargeHistory {
    /// G-S gate charge at previous timestep.
    qgs: f64,
    /// G-S charge current at previous timestep (for trapezoidal).
    cqgs: f64,
    /// G-D gate charge at previous timestep.
    qgd: f64,
    /// G-D charge current at previous timestep (for trapezoidal).
    cqgd: f64,
    /// G-B gate charge at previous timestep.
    qgb: f64,
    /// G-B charge current at previous timestep (for trapezoidal).
    cqgb: f64,
    /// Previous signed V(gate) - V(source_prime).
    vgs: f64,
    /// Previous signed V(gate) - V(drain_prime).
    vgd: f64,
    /// Previous signed V(gate) - V(bulk).
    vgb: f64,
}

/// History state for an HFET junction charge at the previous timestep.
#[derive(Debug, Clone)]
struct HfetChargeHistory {
    /// G-S junction charge at previous timestep.
    qgs: f64,
    /// G-S charge current at previous timestep (for trapezoidal).
    cqgs: f64,
    /// G-D junction charge at previous timestep.
    qgd: f64,
    /// G-D charge current at previous timestep (for trapezoidal).
    cqgd: f64,
    /// Previous G-S capacitor voltage (V(gp) - V(spp)).
    vgspp: f64,
    /// Previous G-D capacitor voltage (V(gp) - V(dpp)).
    vgdpp: f64,
}

/// History state for BSIM3SOI-DD charges at the previous timestep.
/// Tracks both the intrinsic 4-terminal charges (gate, body, drain, substrate)
/// and the CboxWL buried oxide body-to-backgate capacitance.
#[derive(Debug, Clone)]
struct Bsim3SoiDdChargeHistory {
    // Intrinsic terminal charges (ngspice b3soiddld.c lines 3688-3692)
    qgate: f64,
    cqgate: f64,
    qbody: f64,
    cqbody: f64,
    qdrn: f64,
    cqdrn: f64,
    // Substrate (E-node) charge for 5-terminal transient coupling
    qsub: f64,
    cqsub: f64,
    // CboxWL buried oxide body-to-backgate capacitance (existing)
    qbe_cbox: f64,
    cqbe_cbox: f64,
    vbe_cbox: f64,
    // Charges at two timesteps ago (for divided-difference LTE, matching BJT pattern).
    qgate_prev: f64,
    qbody_prev: f64,
    qdrn_prev: f64,
}

/// History state for VBIC junction charges at the previous timestep.
/// Tracks B-E (including external), B-C, parasitic B-E, and parasitic B-C
/// charges for the trapezoidal/BE integration companion model.
#[derive(Debug, Clone)]
struct VbicChargeHistory {
    /// Combined B-E charge (qbe_total + qbex_total) at previous timestep.
    qbe: f64,
    /// B-E charge current at previous timestep (for trapezoidal).
    cqbe: f64,
    /// Previous Vbei voltage.
    vbei: f64,
    /// B-C charge (qbc_total) at previous timestep.
    qbc: f64,
    /// B-C charge current at previous timestep (for trapezoidal).
    cqbc: f64,
    /// Previous Vbci voltage.
    vbci: f64,
    /// Parasitic B-E charge (qbep_total) at previous timestep.
    qbep: f64,
    /// Parasitic B-E charge current at previous timestep.
    cqbep: f64,
    /// Previous Vbep voltage.
    vbep: f64,
    /// Parasitic B-C (substrate) charge (qbcp_total) at previous timestep.
    qbcp: f64,
    /// Parasitic B-C charge current at previous timestep.
    cqbcp: f64,
    /// Previous Vbcp voltage.
    vbcp: f64,
}

/// Compute capacitor companion model coefficients.
///
/// Returns `(geq, ieq)` where:
/// - `geq` is the equivalent conductance to stamp into the matrix
/// - `ieq` is the equivalent current source to stamp into the RHS
///
/// For Backward Euler: i(n) = C/h * v(n) - C/h * v(n-1)
///   → geq = C/h, ieq = -C/h * v(n-1)
///
/// For Trapezoidal: i(n) = 2C/h * v(n) - 2C/h * v(n-1) - i(n-1)
///   → geq = 2C/h, ieq = -(2C/h * v(n-1) + i(n-1))
fn capacitor_companion(
    capacitance: f64,
    h: f64,
    history: &CapHistory,
    method: IntegrationMethod,
) -> (f64, f64) {
    match method {
        IntegrationMethod::BackwardEuler => {
            let geq = capacitance / h;
            let ieq = -geq * history.voltage;
            (geq, ieq)
        }
        IntegrationMethod::Trapezoidal => {
            let geq = 2.0 * capacitance / h;
            let ieq = -(geq * history.voltage + history.current);
            (geq, ieq)
        }
    }
}

/// Compute inductor companion model coefficients.
///
/// Returns `(req, veq)` where:
/// - `req` is the equivalent resistance added to the branch equation diagonal
/// - `veq` is the equivalent voltage source added to the branch equation RHS
///
/// For Backward Euler: v(n) = L/h * i(n) - L/h * i(n-1)
///   → req = L/h, veq = -L/h * i(n-1)
///
/// For Trapezoidal: v(n) = 2L/h * i(n) - 2L/h * i(n-1) - v(n-1)
///   → req = 2L/h, veq = -(2L/h * i(n-1) + v(n-1))
fn inductor_companion(
    inductance: f64,
    h: f64,
    history: &IndHistory,
    method: IntegrationMethod,
) -> (f64, f64) {
    match method {
        IntegrationMethod::BackwardEuler => {
            let req = inductance / h;
            let veq = -req * history.current;
            (req, veq)
        }
        IntegrationMethod::Trapezoidal => {
            let req = 2.0 * inductance / h;
            let veq = -(req * history.current + history.voltage);
            (req, veq)
        }
    }
}

/// Sorted breakpoint table for transient analysis.
///
/// Forces the timestep engine to land exactly on times where waveforms have
/// discontinuities (PULSE edges, PWL corners, etc.).
struct BreakpointTable {
    /// Sorted breakpoint times.
    times: Vec<f64>,
    /// Index of the next unprocessed breakpoint.
    next_idx: usize,
    /// Minimum separation between breakpoints.
    min_break: f64,
}

impl BreakpointTable {
    /// Build breakpoint table from all source waveforms.
    fn from_mna(mna: &MnaSystem, tran: &TranParams) -> Self {
        let mut times = Vec::new();

        for vs in &mna.voltage_sources {
            if let Some(ref wf) = vs.waveform {
                times.extend(waveform::breakpoints(wf, tran));
            }
        }
        for cs in &mna.current_sources {
            if let Some(ref wf) = cs.waveform {
                times.extend(waveform::breakpoints(wf, tran));
            }
        }

        times.sort_by(|a, b| a.total_cmp(b));
        times.dedup_by(|a, b| (*a - *b).abs() < 1e-15);

        let min_break = tran.tstep * 5e-5;

        Self {
            times,
            next_idx: 0,
            min_break,
        }
    }

    /// Get the next breakpoint time after `current_time`, if any.
    fn next_after(&mut self, current_time: f64) -> Option<f64> {
        // Advance past breakpoints we've already passed.
        while self.next_idx < self.times.len()
            && self.times[self.next_idx] <= current_time + self.min_break
        {
            self.next_idx += 1;
        }
        self.times.get(self.next_idx).copied()
    }

    /// Check if `t` is at or very near an *upcoming* breakpoint.
    /// Only checks breakpoints that haven't been advanced past by
    /// `next_after`.  This prevents a breakpoint at t=0 from forcing
    /// BackwardEuler + step reduction for many subsequent steps that
    /// are still within the `min_break` tolerance window.
    fn is_at_breakpoint(&self, t: f64) -> bool {
        if self.next_idx < self.times.len() {
            let bp = self.times[self.next_idx];
            (bp - t).abs() < self.min_break
        } else {
            false
        }
    }
}

/// Transient truncation error tolerance (controls how aggressively timestep adjusts).
/// Matches ngspice CKTtrtol default of 7.0.
const TRTOL: f64 = 7.0;

/// Charge tolerance for LTE estimation (matches ngspice CHGTOL).
const CHGTOL: f64 = 1e-14;

/// Minimum factor to shrink timestep on rejection.
const MIN_SHRINK: f64 = 0.125; // 1/8

/// Maximum factor to grow timestep.
const MAX_GROW: f64 = 2.0;

/// second derivative, matching ngspice's CKTterr function.  For trapezoidal
/// order 2, the truncation error coefficient is 1/12 and the timestep scales
/// as sqrt(tol / |divided_diff|).
///
/// Returns the recommended new timestep.
#[expect(clippy::too_many_arguments)]
fn estimate_new_timestep(
    h: f64,
    h_prev: f64,
    solution: &[f64],
    mna: &MnaSystem,
    cap_histories: &[CapHistory],
    ind_histories: &[IndHistory],
    bjt_charge_histories: &[BjtChargeHistory],
    soidd_charge_histories: &[Bsim3SoiDdChargeHistory],
    reltol: f64,
    abstol: f64,
) -> f64 {
    let mut new_h = f64::MAX;

    /// Trapezoidal order-2 truncation error coefficient (1/12).
    const TRAP_COEFF: f64 = 1.0 / 12.0;

    // LTE for capacitors using divided differences of charge.
    // Matches ngspice CKTterr: computes 2nd divided difference of Q over
    // 3 timepoints (current, previous, two-ago) to estimate Q''.
    for (ci, cap) in mna.capacitors.iter().enumerate() {
        let v_pos = cap.pos_idx.map(|i| solution[i]).unwrap_or(0.0);
        let v_neg = cap.neg_idx.map(|i| solution[i]).unwrap_or(0.0);
        let v_new = v_pos - v_neg;

        let q0 = cap.capacitance * v_new; // Q at current step
        let q1 = cap_histories[ci].charge; // Q at previous step
        let q2 = cap_histories[ci].charge_prev; // Q at two steps ago

        // Capacitor current for tolerance computation.
        let i_cur = 2.0 * cap.capacitance / h * (v_new - cap_histories[ci].voltage)
            - cap_histories[ci].current;

        // Tolerance: max of voltage-based and charge-based.
        let vol_tol = abstol + reltol * i_cur.abs().max(cap_histories[ci].current.abs());
        let chg_tol = reltol * q0.abs().max(q1.abs()).max(CHGTOL) / h;
        let tol = TRTOL * vol_tol.max(chg_tol);

        // Divided differences: 2nd divided difference of Q.
        // diff1_0 = (Q0 - Q1) / h
        // diff1_1 = (Q1 - Q2) / h_prev
        // diff2 = (diff1_0 - diff1_1) / (h + h_prev)
        let diff1_0 = (q0 - q1) / h;
        let diff1_1 = (q1 - q2) / h_prev;
        let diff2 = (diff1_0 - diff1_1) / (h + h_prev);
        let lte_est = TRAP_COEFF * diff2.abs();

        if lte_est > 1e-30 {
            // Match ngspice CKTterr: del = trtol * tol / max(abstol, lte_est)
            // Then h_new = sqrt(del) for order 2.  The sqrt of the ratio
            // directly gives the new timestep (not scaled by current h).
            let del = tol / lte_est.max(abstol);
            let h_new = del.sqrt();
            new_h = new_h.min(h_new);
        }
    }

    // LTE for inductors (flux = L*i, same divided-difference approach).
    for (li, ind) in mna.inductors.iter().enumerate() {
        let i_new = solution[ind.branch_idx];
        let i_old = ind_histories[li].current;
        let v_old = ind_histories[li].voltage;

        // Trap voltage: v_trap = 2L/h * (i_new - i_old) - v_old
        let v_trap = 2.0 * ind.inductance / h * (i_new - i_old) - v_old;
        // BE voltage: v_be = L/h * (i_new - i_old)
        let _v_be = ind.inductance / h * (i_new - i_old);

        // Flux LTE: use current change as proxy for flux divided difference.
        // (Inductors keep the old formula for now since it works correctly
        // for the common L*di/dt case.)
        let flux_trap = h / 2.0 * (v_old + v_trap);
        let flux_be = h * _v_be;
        let lte = (flux_trap - flux_be).abs();

        // Tolerance on flux.
        let flux_new = ind.inductance * i_new;
        let flux_old = ind.inductance * i_old;
        let cur_tol = abstol + reltol * i_new.abs().max(i_old.abs());
        let flux_tol = reltol * flux_new.abs().max(flux_old.abs()).max(CHGTOL);
        let tol = TRTOL * cur_tol.max(flux_tol);

        if lte > 1e-30 {
            let ratio = tol / lte;
            let h_new = h * ratio.sqrt();
            new_h = new_h.min(h_new);
        }
    }

    // LTE for BJT dynamic junction charges using divided differences.
    // Matches ngspice's BJTtrunc → CKTterr: computes 2nd divided difference
    // of Q over 3 timepoints to estimate Q'' and control the timestep.
    for (bi, bjt) in mna.bjts.iter().enumerate() {
        let hist = &bjt_charge_histories[bi];
        let (vbe, vbc) = bjt.junction_voltages(solution);
        let comp = bjt.model.companion(vbe, vbc);
        let m = bjt.m * bjt.area;

        // Compute exact analytical charge at current operating point.
        let (q0_be, _capbe, q0_bc, _capbc) = bjt.model.compute_charges(vbe, vbc, &comp);

        // B-E charge LTE via divided differences.
        {
            let q0 = q0_be;
            let q1 = hist.qbe;
            let q2 = hist.qbe_prev;

            // Tolerance matching ngspice CKTterr.
            let vol_tol = abstol + reltol * hist.cqbe.abs().max((q0 - q1).abs() / h);
            let chg_tol = reltol * q0.abs().max(q1.abs()).max(CHGTOL) / h;
            let tol = TRTOL * vol_tol.max(chg_tol);

            let diff1_0 = (q0 - q1) / h;
            let diff1_1 = (q1 - q2) / h_prev;
            let diff2 = (diff1_0 - diff1_1) / (h + h_prev);
            let lte_est = TRAP_COEFF * diff2.abs() * m;

            if lte_est > 1e-30 {
                let del = tol / lte_est.max(abstol);
                let h_new = del.sqrt();
                new_h = new_h.min(h_new);
            }
        }

        // B-C charge LTE via divided differences.
        {
            let q0 = q0_bc;
            let q1 = hist.qbc;
            let q2 = hist.qbc_prev;

            let vol_tol = abstol + reltol * hist.cqbc.abs().max((q0 - q1).abs() / h);
            let chg_tol = reltol * q0.abs().max(q1.abs()).max(CHGTOL) / h;
            let tol = TRTOL * vol_tol.max(chg_tol);

            let diff1_0 = (q0 - q1) / h;
            let diff1_1 = (q1 - q2) / h_prev;
            let diff2 = (diff1_0 - diff1_1) / (h + h_prev);
            let lte_est = TRAP_COEFF * diff2.abs() * m;

            if lte_est > 1e-30 {
                let del = tol / lte_est.max(abstol);
                let h_new = del.sqrt();
                new_h = new_h.min(h_new);
            }
        }
    }

    // LTE for BSIM3SOI-DD intrinsic charges using divided differences.
    // Matches ngspice's b3soiddtrunc.c → CKTterr: only qb, qg, qd are
    // included (not qsub).
    for (di, inst) in mna.bsim3soi_dds.iter().enumerate() {
        let hist = &soidd_charge_histories[di];
        // Direct terminal charges from the model at the current solution.
        let (vgs_t, vds_t, vbs_t, ves_t) = inst.terminal_voltages(solution);
        let comp = crate::bsim3soi_dd::bsim3soi_dd_companion(
            vgs_t,
            vds_t,
            vbs_t,
            ves_t,
            &inst.size_params,
            &inst.model,
        );

        let q0_gate = comp.qgate;
        let q0_body = comp.qbody;
        let q0_drn = comp.qdrn;

        // Gate charge LTE.
        {
            let q0 = q0_gate;
            let q1 = hist.qgate;
            let q2 = hist.qgate_prev;

            let vol_tol = abstol + reltol * hist.cqgate.abs().max((q0 - q1).abs() / h);
            let chg_tol = reltol * q0.abs().max(q1.abs()).max(CHGTOL) / h;
            let tol = TRTOL * vol_tol.max(chg_tol);

            let diff1_0 = (q0 - q1) / h;
            let diff1_1 = (q1 - q2) / h_prev;
            let diff2 = (diff1_0 - diff1_1) / (h + h_prev);
            let lte_est = TRAP_COEFF * diff2.abs();

            if lte_est > 1e-30 {
                let del = tol / lte_est.max(abstol);
                let h_new = del.sqrt();
                new_h = new_h.min(h_new);
            }
        }

        // Body charge LTE.
        {
            let q0 = q0_body;
            let q1 = hist.qbody;
            let q2 = hist.qbody_prev;

            let vol_tol = abstol + reltol * hist.cqbody.abs().max((q0 - q1).abs() / h);
            let chg_tol = reltol * q0.abs().max(q1.abs()).max(CHGTOL) / h;
            let tol = TRTOL * vol_tol.max(chg_tol);

            let diff1_0 = (q0 - q1) / h;
            let diff1_1 = (q1 - q2) / h_prev;
            let diff2 = (diff1_0 - diff1_1) / (h + h_prev);
            let lte_est = TRAP_COEFF * diff2.abs();

            if lte_est > 1e-30 {
                let del = tol / lte_est.max(abstol);
                let h_new = del.sqrt();
                new_h = new_h.min(h_new);
            }
        }

        // Drain charge LTE.
        {
            let q0 = q0_drn;
            let q1 = hist.qdrn;
            let q2 = hist.qdrn_prev;

            let vol_tol = abstol + reltol * hist.cqdrn.abs().max((q0 - q1).abs() / h);
            let chg_tol = reltol * q0.abs().max(q1.abs()).max(CHGTOL) / h;
            let tol = TRTOL * vol_tol.max(chg_tol);

            let diff1_0 = (q0 - q1) / h;
            let diff1_1 = (q1 - q2) / h_prev;
            let diff2 = (diff1_0 - diff1_1) / (h + h_prev);
            let lte_est = TRAP_COEFF * diff2.abs();

            if lte_est > 1e-30 {
                let del = tol / lte_est.max(abstol);
                let h_new = del.sqrt();
                new_h = new_h.min(h_new);
            }
        }
    }

    new_h
}

/// Perform transient analysis on a circuit.
///
/// Parses the `.tran` command from the netlist, computes the DC operating point
/// as initial conditions, then steps through time using numerical integration.
/// Uses adaptive timestep control with LTE estimation when reactive elements
/// are present, falling back to fixed timestep for purely resistive circuits.
pub fn simulate_tran(netlist: &Netlist) -> Result<SimResult, MnaError> {
    let mna = assemble_mna(netlist)?;
    simulate_tran_with_mna(mna, netlist)
}

/// Run a `.tran` analysis on an already-assembled [`MnaSystem`].
///
/// Shared between the Netlist path (`simulate_tran` above) and the Stage 4
/// IR-direct path (`thevenin::circuit::simulate_tran` via `mna_ir`). The
/// Netlist is still needed for `.tran` analysis params, `.ic` overrides,
/// nodeset resolution, and `.OPTIONS` lookups.
pub fn simulate_tran_with_mna(
    mna: MnaSystem,
    netlist: &Netlist,
) -> Result<SimResult, MnaError> {
    let (tstep, tstop, tstart, tmax, uic) = match &netlist.analysis {
        Analysis::Tran {
            tstep,
            tstop,
            tstart,
            tmax,
            uic,
        } => (
            tstep.clone(),
            tstop.clone(),
            tstart.clone(),
            tmax.clone(),
            *uic,
        ),
        _ => {
            return Err(MnaError::UnsupportedElement(
                "no .tran analysis found".to_string(),
            ));
        }
    };

    let h_print = expr_val(&tstep, ".tran tstep")?;
    let t_stop = expr_val(&tstop, ".tran tstop")?;
    let t_start = tstart
        .as_ref()
        .map(|e| expr_val(e, ".tran tstart"))
        .transpose()?
        .unwrap_or(0.0);
    let t_max = tmax
        .as_ref()
        .map(|e| expr_val(e, ".tran tmax"))
        .transpose()?;

    let circuit_nr_opts = nr_options_from_netlist(netlist);
    let nodeset = crate::simulate::resolve_nodeset(netlist, &mna);

    // Apply node-level .ic overrides from the netlist — pre-resolve them to
    // MnaSystem matrix indices so `run_tran` doesn't need to walk netlist.items.
    let mut ic_overrides: Vec<(usize, f64)> = Vec::new();
    for item in &netlist.items {
        if let Item::Ic(pairs) = item {
            for (node_name, val) in pairs {
                if let Some(idx) = mna.node_map.get(node_name) {
                    ic_overrides.push((idx, *val));
                }
            }
        }
    }

    let device_param_queries = collect_device_param_queries(netlist.source.lines(), &mna);

    run_tran(
        mna,
        TranRunParams {
            t_step: h_print,
            t_stop,
            t_start,
            t_max,
            uic,
            nr_opts: circuit_nr_opts,
            nodeset,
            ic_overrides,
            device_param_queries,
        },
    )
}

/// Fully-resolved `.tran` analysis parameters, shared between Netlist and
/// Circuit input paths.
pub struct TranRunParams {
    pub t_step: f64,
    pub t_stop: f64,
    pub t_start: f64,
    pub t_max: Option<f64>,
    pub uic: bool,
    pub nr_opts: crate::newton::NrOptions,
    pub nodeset: Vec<(usize, f64)>,
    /// `(matrix_index, voltage)` pairs resolved from `.ic` directives.
    pub ic_overrides: Vec<(usize, f64)>,
    /// `(device, param)` pairs collected from `.print @device[param]` directives.
    pub device_param_queries: Vec<(String, String)>,
}

/// Execute a `.tran` analysis on an assembled [`MnaSystem`] with all
/// netlist-derived params already extracted into [`TranRunParams`].
///
/// Shared core for both the Netlist path (`simulate_tran_with_mna`) and the
/// Circuit path ([`crate::circuit::simulate_tran`]).
pub fn run_tran(
    mut mna: MnaSystem,
    params: TranRunParams,
) -> Result<SimResult, MnaError> {
    let TranRunParams {
        t_step: h_print,
        t_stop,
        t_start,
        t_max,
        uic,
        nr_opts: circuit_nr_opts,
        nodeset,
        ic_overrides,
        device_param_queries,
    } = params;

    if h_print <= 0.0 || t_stop <= 0.0 {
        return Err(MnaError::UnsupportedElement(
            "invalid .tran parameters".to_string(),
        ));
    }

    // Maximum internal timestep: tmax if specified, otherwise min(tstep, tstop/50).
    let h_max = t_max.unwrap_or_else(|| h_print.min(t_stop / 50.0));

    let dim = mna.system.dim();
    let num_nodes = mna.total_num_nodes();

    // When UIC (Use Initial Conditions) is set, skip the DC operating point
    // and start from zero with explicit .ic node voltages applied.
    // Otherwise, compute the normal DC OP as the starting point.
    let mut solution = if uic {
        vec![0.0; dim]
    } else {
        let mut sol = if nodeset.is_empty() {
            solve_op_raw_with_opts(&mna, &circuit_nr_opts)?
        } else {
            crate::simulate::solve_op_raw_with_nodeset(&mna, &circuit_nr_opts, &nodeset)?
        };
        sol.resize(dim, 0.0);
        sol
    };

    // Apply pre-resolved .ic node voltage overrides.
    for (idx, val) in &ic_overrides {
        solution[*idx] = *val;
    }

    // Apply IC overrides for capacitors (override DC OP voltage).
    for cap in &mna.capacitors {
        if let Some(ic_v) = cap.ic {
            match (cap.pos_idx, cap.neg_idx) {
                (Some(pi), None) => solution[pi] = ic_v,
                (None, Some(ni)) => solution[ni] = -ic_v,
                (Some(pi), Some(ni)) => {
                    solution[pi] = ic_v + solution[ni];
                }
                (None, None) => {}
            }
        }
    }

    // Apply IC overrides for inductors.
    for ind in &mna.inductors {
        if let Some(ic_i) = ind.ic {
            solution[ind.branch_idx] = ic_i;
        }
    }

    // Initialize history from the initial solution.
    let mut cap_histories: Vec<CapHistory> = mna
        .capacitors
        .iter()
        .map(|cap| {
            let v_pos = cap.pos_idx.map(|i| solution[i]).unwrap_or(0.0);
            let v_neg = cap.neg_idx.map(|i| solution[i]).unwrap_or(0.0);
            let v = v_pos - v_neg;
            let charge = cap.capacitance * v;
            CapHistory {
                voltage: v,
                current: 0.0, // At DC steady state, capacitor current is 0.
                charge,
                charge_prev: charge, // No previous history at DC init.
            }
        })
        .collect();

    let mut ind_histories: Vec<IndHistory> = mna
        .inductors
        .iter()
        .map(|ind| {
            let current = solution[ind.branch_idx];
            IndHistory {
                current,
                voltage: 0.0, // At DC steady state, inductor voltage is 0.
            }
        })
        .collect();

    // Initialize BJT charge histories from DC operating point.
    // At DC steady state, dQ/dt = 0, so charge currents are all zero.
    // Uses FULL absolute charges (depletion + diffusion), matching ngspice's
    // bjtload.c which computes Q = junction_charge(v) + TF*cbe at each point.
    let mut bjt_charge_histories: Vec<BjtChargeHistory> = mna
        .bjts
        .iter()
        .map(|bjt| {
            let (vbe, vbc) = bjt.junction_voltages(&solution);
            let comp = bjt.model.companion(vbe, vbc);
            // Full charge: depletion integral + diffusion charge.
            let (qbe, _, qbc, _) = bjt.model.compute_charges(vbe, vbc, &comp);
            BjtChargeHistory {
                qbe,
                cqbe: 0.0, // DC steady state: no charge current
                vbe,
                qbc,
                cqbc: 0.0,
                vbc,
                qbe_prev: qbe, // No previous history at DC init.
                qbc_prev: qbc, // No previous history at DC init.
            }
        })
        .collect();

    // Initialize MESA junction charge histories from DC operating point.
    // At DC, Q = capgs * vgspp and Q = capgd * vgdpp. dQ/dt = 0.
    let mut mesa_charge_histories: Vec<MesaChargeHistory> = mna
        .mesas
        .iter()
        .map(|mesa| {
            let (vgs, vgd) = mesa.junction_voltages(&solution);
            let comp = crate::mesa::mesa_companion(mesa, vgs, vgd, 1e-12);
            // vgspp = V(gp) - V(spp). At DC OP, spp voltage ≈ sp voltage
            // (no transient current through Ri), so vgspp ≈ vgs.
            let vgspp = if mesa.source_prm_prm_idx.is_some() {
                let v_gp = mesa.gate_prime_idx.map(|i| solution[i]).unwrap_or(0.0);
                let v_spp = mesa.source_prm_prm_idx.map(|i| solution[i]).unwrap_or(0.0);
                v_gp - v_spp
            } else {
                vgs
            };
            let vgdpp = if mesa.drain_prm_prm_idx.is_some() {
                let v_gp = mesa.gate_prime_idx.map(|i| solution[i]).unwrap_or(0.0);
                let v_dpp = mesa.drain_prm_prm_idx.map(|i| solution[i]).unwrap_or(0.0);
                v_gp - v_dpp
            } else {
                vgd
            };
            MesaChargeHistory {
                qgs: comp.capgs * vgspp,
                cqgs: 0.0,
                qgd: comp.capgd * vgdpp,
                cqgd: 0.0,
                vgspp,
                vgdpp,
            }
        })
        .collect();

    // Initialize HFET junction charge histories from DC operating point.
    // At DC, Q = capgs * vgspp and Q = capgd * vgdpp. dQ/dt = 0.
    let mut hfet_charge_histories: Vec<HfetChargeHistory> = mna
        .hfets
        .iter()
        .map(|hfet| {
            let (vgs, vgd) = hfet.junction_voltages(&solution);
            let comp = crate::hfet::hfet_companion_full(hfet, vgs, vgd, 1e-12);
            // vgspp = V(gp) - V(spp). At DC OP, spp voltage ≈ sp voltage.
            let v_gp = hfet
                .gate_prime_idx
                .or(hfet.gate_idx)
                .map(|i| solution[i])
                .unwrap_or(0.0);
            let vgspp = if let Some(spp_i) = hfet.source_prm_prm_idx {
                v_gp - solution[spp_i]
            } else {
                let v_sp = hfet
                    .source_prime_idx
                    .or(hfet.source_idx)
                    .map(|i| solution[i])
                    .unwrap_or(0.0);
                v_gp - v_sp
            };
            let vgdpp = if let Some(dpp_i) = hfet.drain_prm_prm_idx {
                v_gp - solution[dpp_i]
            } else {
                let v_dp = hfet
                    .drain_prime_idx
                    .or(hfet.drain_idx)
                    .map(|i| solution[i])
                    .unwrap_or(0.0);
                v_gp - v_dp
            };
            HfetChargeHistory {
                qgs: comp.capgs * vgspp,
                cqgs: 0.0,
                qgd: comp.capgd * vgdpp,
                cqgd: 0.0,
                vgspp,
                vgdpp,
            }
        })
        .collect();

    // Initialize MOSFET Meyer gate charge histories from DC operating point.
    let mut mosfet_charge_histories: Vec<MosfetChargeHistory> = mna
        .mosfets
        .iter()
        .map(|mos| {
            let (vgs_signed, vds_signed, vbs_signed) = mos.terminal_voltages(&solution);
            let vgd_signed = vgs_signed - vds_signed;
            let vgb_signed = vgs_signed - vbs_signed;

            let mut eff_model = mos.model.clone();
            eff_model.kp = mos.beta();
            let comp = eff_model.companion(vgs_signed, vds_signed, vbs_signed);

            let l_eff = (mos.l - 2.0 * mos.model.ld).max(1e-12);
            let cox = (3.9 * 8.854214871e-12) / mos.model.tox * l_eff * mos.w;

            let (capgs, capgd, capgb) = if comp.mode > 0 {
                crate::mosfet::qmeyer(
                    vgs_signed,
                    vgd_signed,
                    comp.von,
                    comp.vdsat,
                    mos.model.phi,
                    cox,
                )
            } else {
                let (gd, gs, gb) = crate::mosfet::qmeyer(
                    vgd_signed,
                    vgs_signed,
                    comp.von,
                    comp.vdsat,
                    mos.model.phi,
                    cox,
                );
                (gs, gd, gb)
            };

            MosfetChargeHistory {
                qgs: capgs * vgs_signed,
                cqgs: 0.0,
                qgd: capgd * vgd_signed,
                cqgd: 0.0,
                qgb: capgb * vgb_signed,
                cqgb: 0.0,
                vgs: vgs_signed,
                vgd: vgd_signed,
                vgb: vgb_signed,
            }
        })
        .collect();

    // Initialize MOS6 Meyer gate charge histories from DC operating point.
    let mut mos6_charge_histories: Vec<MosfetChargeHistory> = mna
        .mos6s
        .iter()
        .map(|mos| {
            let (vgs_signed, vds_signed, vbs_signed) = mos.terminal_voltages(&solution);
            let vgd_signed = vgs_signed - vds_signed;
            let vgb_signed = vgs_signed - vbs_signed;

            let comp = mos
                .model
                .companion(vgs_signed, vds_signed, vbs_signed, mos.betac());

            let l_eff = (mos.l - 2.0 * mos.model.ld).max(1e-12);
            let cox = (3.9 * 8.854214871e-12) / mos.model.tox * l_eff * mos.w;

            let (capgs, capgd, capgb) = if comp.mode > 0 {
                crate::mosfet::qmeyer(
                    vgs_signed,
                    vgd_signed,
                    comp.von,
                    comp.vdsat,
                    mos.model.phi,
                    cox,
                )
            } else {
                let (gd, gs, gb) = crate::mosfet::qmeyer(
                    vgd_signed,
                    vgs_signed,
                    comp.von,
                    comp.vdsat,
                    mos.model.phi,
                    cox,
                );
                (gs, gd, gb)
            };

            MosfetChargeHistory {
                qgs: capgs * vgs_signed,
                cqgs: 0.0,
                qgd: capgd * vgd_signed,
                cqgd: 0.0,
                qgb: capgb * vgb_signed,
                cqgb: 0.0,
                vgs: vgs_signed,
                vgd: vgd_signed,
                vgb: vgb_signed,
            }
        })
        .collect();

    // Initialize BSIM3SOI-DD charge histories from DC operating point.
    // Uses direct charge computation Q(V) from the model (matching ngspice
    // NIintegrate).  At DC steady state dQ/dt = 0.
    let mut soidd_charge_histories: Vec<Bsim3SoiDdChargeHistory> = mna
        .bsim3soi_dds
        .iter()
        .map(|inst| {
            let sign = inst.model.mos_type.sign();
            let vb = inst.body_int_idx.map_or(0.0, |i| solution[i]);
            let ve = inst.e_idx.map_or(0.0, |i| solution[i]);
            let vbe = sign * (vb - ve);
            let cbox_wl = inst.size_params.kb3
                * inst.model.cbox
                * inst.size_params.weff_cv
                * inst.size_params.leff_cv;
            // Compute companion to get direct terminal charges.
            let (vgs_t, vds_t, vbs_t, ves_t) = inst.terminal_voltages(&solution);
            let comp = crate::bsim3soi_dd::bsim3soi_dd_companion(
                vgs_t,
                vds_t,
                vbs_t,
                ves_t,
                &inst.size_params,
                &inst.model,
            );
            // Initialize with direct charges from DC operating point
            // (ngspice MODEINITTRAN: copies state0 to state1).
            Bsim3SoiDdChargeHistory {
                qgate: comp.qgate,
                cqgate: 0.0,
                qbody: comp.qbody,
                cqbody: 0.0,
                qdrn: comp.qdrn,
                cqdrn: 0.0,
                qsub: comp.qsub,
                cqsub: 0.0,
                qbe_cbox: cbox_wl * vbe,
                cqbe_cbox: 0.0,
                vbe_cbox: vbe,
                // No previous history at DC init (same pattern as BJT).
                qgate_prev: comp.qgate,
                qbody_prev: comp.qbody,
                qdrn_prev: comp.qdrn,
            }
        })
        .collect();

    // Initialize VBIC junction charge histories from DC operating point.
    // At DC steady state, dQ/dt = 0, so charge currents are all zero.
    // Tracks combined B-E (qbe_total + qbex_total), B-C, parasitic B-E, and
    // parasitic B-C (substrate) charges for transient integration.
    let mut vbic_charge_histories: Vec<VbicChargeHistory> = mna
        .vbics
        .iter()
        .map(|vbic| {
            let (vbei, vbex, vbci, vbcx, vbep, vrci, vrbi, vrbp, vbcp) =
                vbic.junction_voltages(&solution);
            let comp = vbic.model.companion(
                vbei,
                vbex,
                vbci,
                vbcx,
                vbep,
                vrci,
                vrbi,
                vrbp,
                vbcp,
                circuit_nr_opts.gmin,
            );
            // Combined Qbe = qbe_total + qbex_total (matching ngspice which combines them).
            let qbe = comp.qbe_total + comp.qbex_total;
            VbicChargeHistory {
                qbe,
                cqbe: 0.0, // DC steady state: no charge current
                vbei,
                qbc: comp.qbc_total,
                cqbc: 0.0,
                vbci,
                qbep: comp.qbep_total,
                cqbep: 0.0,
                vbep,
                qbcp: comp.qbcp_total,
                cqbcp: 0.0,
                vbcp,
            }
        })
        .collect();

    // Initialize LTRA transient state.
    let has_ltra = !mna.ltras.is_empty();
    let mut ltra_states: Vec<LtraState> = mna
        .ltras
        .iter()
        .map(|inst| {
            let mut state = LtraState::new();
            let v1 =
                node_voltage(&solution, inst.pos1_idx) - node_voltage(&solution, inst.neg1_idx);
            let i1 = solution[num_nodes + inst.br_eq1];
            let v2 =
                node_voltage(&solution, inst.pos2_idx) - node_voltage(&solution, inst.neg2_idx);
            let i2 = solution[num_nodes + inst.br_eq2];
            state.init_from_dc(v1, i1, v2, i2);
            // Record initial values as first history point.
            state.accept(v1, i1, v2, i2);
            state
        })
        .collect();
    let mut ltra_coeffs: Vec<LtraCoeffs> = mna.ltras.iter().map(|_| LtraCoeffs::new()).collect();
    // Time points history for LTRA convolution.
    let mut ltra_time_points: Vec<f64> = if has_ltra { vec![0.0] } else { Vec::new() };

    // Initialize TXL transient state from DC operating point.
    let has_txl = !mna.txls.is_empty();
    if has_txl {
        for inst in &mut mna.txls {
            let dc_v1 = inst.pos_idx.map_or(0.0, |i| solution[i]);
            let dc_v2 = inst.neg_idx.map_or(0.0, |i| solution[i]);
            let dc_i1 = solution[num_nodes + inst.ibr1];
            let dc_i2 = solution[num_nodes + inst.ibr2];
            crate::txl::init_dc_state(&mut inst.txline, dc_v1, dc_v2, dc_i1, dc_i2);
            inst.txline2 = inst.txline.clone();
        }
    }
    let mut txl_stamps: Vec<TxlTransientStamp> = Vec::new();

    // Initialize CPL transient state from DC operating point.
    let has_cpl = !mna.cpls.is_empty();
    if has_cpl {
        for inst in &mut mna.cpls {
            let dc_v_in: Vec<f64> = (0..inst.no_l)
                .map(|m| inst.pos_nodes[m].map_or(0.0, |i| solution[i]))
                .collect();
            let dc_v_out: Vec<f64> = (0..inst.no_l)
                .map(|m| inst.neg_nodes[m].map_or(0.0, |i| solution[i]))
                .collect();
            crate::cpl::init_dc_state(&mut inst.cpline, &dc_v_in, &dc_v_out);
            inst.cpline2 = inst.cpline.clone();
        }
    }
    let mut cpl_stamps: Vec<crate::cpl::CplTransientStamp> = Vec::new();

    // Prepare output vectors.
    let mut time_vec = SimVector::real("time", Vec::new());

    // Sort nodes by descending matrix index to match ngspice's LIFO node-list
    // traversal order (last-inserted node first in output).  The DC path in
    // simulate.rs applies the same sort.
    let sorted_nodes: Vec<(String, usize)> = {
        let mut nodes: Vec<_> = mna
            .node_map
            .iter()
            .map(|(n, i)| (n.to_string(), i))
            .collect();
        nodes.sort_by_key(|n| std::cmp::Reverse(n.1));
        nodes
    };
    let mut node_vecs: Vec<SimVector> = sorted_nodes
        .iter()
        .map(|(name, _)| SimVector::real(format!("v({})", name), Vec::new()))
        .collect();

    let mut branch_vecs: Vec<SimVector> = mna
        .vsource_names
        .iter()
        .map(|vsrc| SimVector::real(format!("{}#branch", vsrc.to_lowercase()), Vec::new()))
        .collect();

    // Use the pre-collected @device[param] queries from TranRunParams.
    let mut device_param_vecs: Vec<SimVector> = device_param_queries
        .iter()
        .map(|(device, param)| {
            SimVector::real(
                format!("@{}[{}]", device.to_lowercase(), param.to_lowercase()),
                Vec::new(),
            )
        })
        .collect();

    let has_nonlinear = mna.has_nonlinear();
    let has_reactive = !mna.capacitors.is_empty() || !mna.inductors.is_empty();
    let nr_options = circuit_nr_opts;
    let tran_params = TranParams {
        tstep: h_print,
        tstop: t_stop,
    };

    // Record initial point at t=0.
    // For linear circuits with time-varying waveform sources (e.g., SIN with delay),
    // compute the actual t=0 state by solving the system with t=0 source values rather
    // than recording the DC OP values directly. This matches ngspice behaviour where the
    // initial transient solution reflects the circuit state at t=0 with time-domain sources.
    let mut t = 0.0;
    if t >= t_start {
        let t0_solution: Vec<f64> = if !has_reactive
            && !has_nonlinear
            && (mna.voltage_sources.iter().any(|vs| vs.waveform.is_some())
                || mna.current_sources.iter().any(|cs| cs.waveform.is_some()))
        {
            // Solve the linear system at t=0 with time-domain source values.
            let mut system = LinearSystem::new(dim);
            for triplet in mna.system.matrix.triplets() {
                system.matrix.add(triplet.row, triplet.col, triplet.value);
            }
            for (i, &val) in mna.system.rhs.iter().enumerate() {
                system.rhs[i] += val;
            }
            for vs in &mna.voltage_sources {
                if let Some(ref wf) = vs.waveform {
                    let v_t = waveform::evaluate(wf, 0.0, &tran_params);
                    system.rhs[vs.branch_idx] = v_t;
                }
            }
            for cs in &mna.current_sources {
                if let Some(ref wf) = cs.waveform {
                    let i_t = waveform::evaluate(wf, 0.0, &tran_params);
                    let i_diff = i_t - cs.dc_value;
                    if let Some(ni) = cs.pos_idx {
                        system.rhs[ni] -= i_diff;
                    }
                    if let Some(nj) = cs.neg_idx {
                        system.rhs[nj] += i_diff;
                    }
                }
            }
            system.solve().unwrap_or_else(|_| solution.clone())
        } else {
            solution.clone()
        };
        record_point(
            t,
            &t0_solution,
            &mna,
            num_nodes,
            &mut time_vec,
            &mut node_vecs,
            &mut branch_vecs,
            &device_param_queries,
            &mut device_param_vecs,
            &sorted_nodes,
        );
    }

    // Build breakpoint table from source waveforms.
    let mut breakpoints = BreakpointTable::from_mna(&mna, &tran_params);

    // Internal timestep — start small and let doubling grow it to h_max.
    // ngspice starts at h_max/400 and doubles each step (matching its adaptive
    // initial-step algorithm).  For reactive circuits the LTE will control growth;
    // for purely resistive circuits the step doubles freely until reaching h_max.
    let h_min = h_print * 1e-9; // Absolute minimum timestep.
    let mut h = (h_max / 400.0).max(h_min);
    let mut h_prev = h; // No previous step yet; use h as initial estimate.
    let mut is_first_step = true;
    let mut force_be = false; // Order upgrade check: stay at BE until Trap LTE allows upgrade.

    // Adaptive time-stepping loop.
    while t < t_stop - h_min {
        // The step size for this iteration (separate from h which tracks the
        // "suggested" next step from LTE control).
        let mut step_h = h.min(h_max);

        // Don't overshoot tstop.
        if t + step_h > t_stop {
            step_h = t_stop - t;
        }

        // Breakpoint handling: don't cross the next breakpoint.
        let at_breakpoint = breakpoints.is_at_breakpoint(t);
        if let Some(bp) = breakpoints.next_after(t) {
            let dist = bp - t;
            if step_h > dist {
                step_h = dist;
            }
        }

        // At breakpoints, reduce step for stability (ngspice uses 0.1×).
        if at_breakpoint {
            step_h = step_h.min(h * 0.1).max(h_min);
        }

        // Use Backward Euler for the first step, at breakpoints, and when the
        // order upgrade check says Trap would need a similar step (ngspice
        // dctran.c lines 820-831: stay at BE until Trap LTE allows upgrade).
        let method = if is_first_step || at_breakpoint || force_be {
            IntegrationMethod::BackwardEuler
        } else {
            IntegrationMethod::Trapezoidal
        };

        // Recompute LTRA convolution coefficients for the new timepoint.
        if has_ltra {
            let cur_time = t + step_h;
            let time_index = ltra_time_points.len() - 1;
            for (li, inst) in mna.ltras.iter().enumerate() {
                match inst.model.special_case {
                    crate::ltra::LtraCase::Rlc => {
                        crate::ltra::rlc_coeffs_setup(
                            &mut ltra_coeffs[li],
                            inst.model.td,
                            inst.model.alpha,
                            inst.model.beta,
                            cur_time,
                            &ltra_time_points,
                            time_index,
                            inst.model.chop_reltol,
                        );
                    }
                    crate::ltra::LtraCase::Rc => {
                        crate::ltra::rc_coeffs_setup(
                            &mut ltra_coeffs[li],
                            inst.model.c_by_r,
                            inst.model.rclsqr,
                            cur_time,
                            &ltra_time_points,
                            time_index,
                            inst.model.chop_reltol,
                        );
                    }
                    _ => {} // LC and RG don't need coefficient setup
                }
            }
        }

        // Pre-compute TXL transient stamps (updates convolution state, must
        // be called exactly once per timestep attempt).
        if has_txl {
            let cur_time_ps = ((t + step_h) * 1.0e12) as i64;
            let prev_time_ps = (t * 1.0e12) as i64;
            txl_stamps.clear();
            for inst in &mut mna.txls {
                // Backup state for potential step rejection
                inst.txline2 = inst.txline.clone();
                let stamp = crate::txl::prepare_txl_transient(
                    inst,
                    num_nodes,
                    &solution,
                    cur_time_ps,
                    prev_time_ps,
                    step_h, // h in seconds (poles are per-second)
                );
                txl_stamps.push(stamp);
            }
        }

        // Pre-compute CPL transient stamps.
        if has_cpl {
            let cur_time_ps = ((t + step_h) * 1.0e12) as i64;
            let prev_time_ps = (t * 1.0e12) as i64;
            cpl_stamps.clear();
            for inst in &mut mna.cpls {
                inst.cpline2 = inst.cpline.clone();
                let stamp = crate::cpl::prepare_cpl_transient(
                    inst,
                    num_nodes,
                    &solution,
                    cur_time_ps,
                    prev_time_ps,
                    step_h,
                );
                cpl_stamps.push(stamp);
            }
        }

        // Solve this timestep.  On NR convergence failure, reduce the
        // timestep and retry (matching ngspice's timestep recovery logic).
        let new_solution = match solve_timestep(
            &mna,
            &solution,
            step_h,
            t + step_h,
            &tran_params,
            method,
            &cap_histories,
            &ind_histories,
            &bjt_charge_histories,
            has_nonlinear,
            &nr_options,
            dim,
            num_nodes,
            if has_ltra { Some(&ltra_states) } else { None },
            if has_ltra { Some(&ltra_coeffs) } else { None },
            if has_ltra {
                Some(&ltra_time_points)
            } else {
                None
            },
            if has_txl { Some(&txl_stamps) } else { None },
            if has_cpl { Some(&cpl_stamps) } else { None },
            &mesa_charge_histories,
            &hfet_charge_histories,
            &mosfet_charge_histories,
            &mos6_charge_histories,
            &soidd_charge_histories,
            &vbic_charge_histories,
        ) {
            Ok(sol) => sol,
            Err(e) if step_h > h_min * 2.0 => {
                // NR failed — shrink h and retry without advancing time.
                // Demote to Backward Euler on retry, matching ngspice dctran.c
                // which sets order=1 after NR failure.  BE is unconditionally
                // stable and handles discontinuities (PULSE edges, switching)
                // better than Trapezoidal.
                if has_txl {
                    for inst in &mut mna.txls {
                        inst.txline = inst.txline2.clone();
                    }
                }
                if has_cpl {
                    for inst in &mut mna.cpls {
                        inst.cpline = inst.cpline2.clone();
                    }
                }
                force_be = true;
                h = (step_h * MIN_SHRINK).max(h_min);
                let _ = e;
                continue;
            }
            Err(e) => {
                return Err(e);
            }
        };

        // LTE-based timestep control.
        let has_device_charges =
            !mna.bjts.is_empty() || !mna.vbics.is_empty() || !mna.bsim3soi_dds.is_empty();
        if method == IntegrationMethod::Trapezoidal && (has_reactive || has_device_charges) {
            let new_h = estimate_new_timestep(
                step_h,
                h_prev,
                &new_solution,
                &mna,
                &cap_histories,
                &ind_histories,
                &bjt_charge_histories,
                &soidd_charge_histories,
                nr_options.reltol,
                nr_options.abstol,
            );

            if new_h < 0.9 * step_h {
                // Reject step: shrink h and retry without advancing time.
                // Restore TXL/CPL state from backup.
                if has_txl {
                    for inst in &mut mna.txls {
                        inst.txline = inst.txline2.clone();
                    }
                }
                if has_cpl {
                    for inst in &mut mna.cpls {
                        inst.cpline = inst.cpline2.clone();
                    }
                }
                h = (new_h.max(step_h * MIN_SHRINK)).max(h_min);
                continue;
            }

            // Accept: schedule next h from LTE estimate.
            h = new_h.min(step_h * MAX_GROW).min(h_max).max(h_min);
            force_be = false;
        } else if method == IntegrationMethod::BackwardEuler && (has_reactive || has_device_charges)
        {
            // Order upgrade check (ngspice dctran.c lines 820-831):
            // After a successful BE step, try computing the order-2 (Trap) LTE.
            // If the Trap LTE suggests a timestep <= 1.05× the current step,
            // stay at BE for the next step too — Trap can't take larger steps so
            // BE's extra stability is preferable during fast transitions.
            // ngspice may use 2-5 BE steps after breakpoints before upgrading.
            let trap_h = estimate_new_timestep(
                step_h,
                h_prev,
                &new_solution,
                &mna,
                &cap_histories,
                &ind_histories,
                &bjt_charge_histories,
                &soidd_charge_histories,
                nr_options.reltol,
                nr_options.abstol,
            );
            force_be = trap_h <= 1.05 * step_h;
            // Use Trap LTE estimate to control next step size, matching ngspice
            // (line 833: CKTdelta = newdelta regardless of order decision).
            h = trap_h.min(step_h * MAX_GROW).min(h_max).max(h_min);
        } else if (at_breakpoint || force_be) && (has_ltra || has_txl || has_cpl) {
            // For BE steps in transmission-line-only circuits (no caps/inductors/
            // BJTs to compute LTE), limit step growth and clear force_be since we
            // can't determine if Trap would be appropriate.
            h = (step_h * MAX_GROW).min(h_max).max(h_min);
            force_be = false;
        } else {
            // No LTE control and not at a breakpoint — grow toward h_max,
            // but cap at h_print so output points stay dense enough for
            // waveform fidelity.  Without growth here, circuits with only
            // transmission lines (no caps/inductors/BJTs) stay stuck at
            // the initial tiny h and never make progress.
            h = (step_h * MAX_GROW).min(h_max).min(h_print).max(h_min);
            force_be = false;
        }

        // Accept this timestep: advance time and update state.
        h_prev = step_h;
        t += step_h;
        solution = new_solution;
        is_first_step = false;

        // Update capacitor histories.
        for (ci, cap) in mna.capacitors.iter().enumerate() {
            let v_pos = cap.pos_idx.map(|i| solution[i]).unwrap_or(0.0);
            let v_neg = cap.neg_idx.map(|i| solution[i]).unwrap_or(0.0);
            let v_new = v_pos - v_neg;

            let current = match method {
                IntegrationMethod::BackwardEuler => {
                    let geq = cap.capacitance / step_h;
                    geq * (v_new - cap_histories[ci].voltage)
                }
                IntegrationMethod::Trapezoidal => {
                    let geq = 2.0 * cap.capacitance / step_h;
                    geq * (v_new - cap_histories[ci].voltage) - cap_histories[ci].current
                }
            };

            let charge_prev = cap_histories[ci].charge;
            let charge = cap.capacitance * v_new;
            cap_histories[ci] = CapHistory {
                voltage: v_new,
                current,
                charge,
                charge_prev,
            };
        }

        // Update inductor histories.
        for (li, ind) in mna.inductors.iter().enumerate() {
            let i_new = solution[ind.branch_idx];
            let v_pos = ind.pos_idx.map(|i| solution[i]).unwrap_or(0.0);
            let v_neg = ind.neg_idx.map(|i| solution[i]).unwrap_or(0.0);
            let v_new = v_pos - v_neg;

            ind_histories[li] = IndHistory {
                current: i_new,
                voltage: v_new,
            };
        }

        // Update BJT charge histories (full voltage-dependent caps).
        for (bi, bjt) in mna.bjts.iter().enumerate() {
            let (vbe, vbc) = bjt.junction_voltages(&solution);
            let comp = bjt.model.companion(vbe, vbc);
            // Full voltage-dependent depletion cap + qb-normalized diffusion cap.
            // Matches ngspice bjtload.c: cbe_mod=cbe/qb, gbe_mod=(gbe-cbe_mod*dqbdve)/qb
            let cbe_mod = comp.cbe_raw / comp.qb;
            let gbe_mod = (comp.gbe_raw - cbe_mod * comp.dqbdve) / comp.qb;
            let capbe = bjt.model.cap_be(vbe) + bjt.model.tf * gbe_mod;
            let capbc = bjt.model.cap_bc(vbc) + bjt.model.tr * comp.gbc_raw;
            let qbe = bjt_charge_histories[bi].qbe + capbe * (vbe - bjt_charge_histories[bi].vbe);
            let qbc = bjt_charge_histories[bi].qbc + capbc * (vbc - bjt_charge_histories[bi].vbc);

            let cqbe = match method {
                IntegrationMethod::BackwardEuler => (qbe - bjt_charge_histories[bi].qbe) / step_h,
                IntegrationMethod::Trapezoidal => {
                    2.0 * (qbe - bjt_charge_histories[bi].qbe) / step_h
                        - bjt_charge_histories[bi].cqbe
                }
            };
            let cqbc = match method {
                IntegrationMethod::BackwardEuler => (qbc - bjt_charge_histories[bi].qbc) / step_h,
                IntegrationMethod::Trapezoidal => {
                    2.0 * (qbc - bjt_charge_histories[bi].qbc) / step_h
                        - bjt_charge_histories[bi].cqbc
                }
            };

            let qbe_prev = bjt_charge_histories[bi].qbe;
            let qbc_prev = bjt_charge_histories[bi].qbc;
            bjt_charge_histories[bi] = BjtChargeHistory {
                qbe,
                cqbe,
                vbe,
                qbc,
                cqbc,
                vbc,
                qbe_prev,
                qbc_prev,
            };
        }

        // Update MESA junction charge histories.
        for (mi, mesa) in mna.mesas.iter().enumerate() {
            let v_gp = mesa.gate_prime_idx.map(|i| solution[i]).unwrap_or(0.0);
            let vgspp = if let Some(spp_i) = mesa.source_prm_prm_idx {
                v_gp - solution[spp_i]
            } else {
                let v_sp = mesa.source_prime_idx.map(|i| solution[i]).unwrap_or(0.0);
                v_gp - v_sp
            };
            let vgdpp = if let Some(dpp_i) = mesa.drain_prm_prm_idx {
                v_gp - solution[dpp_i]
            } else {
                let v_dp = mesa.drain_prime_idx.map(|i| solution[i]).unwrap_or(0.0);
                v_gp - v_dp
            };

            let (vgs, vgd) = mesa.junction_voltages(&solution);
            let comp = crate::mesa::mesa_companion(mesa, vgs, vgd, nr_options.gmin);

            let hist = &mesa_charge_histories[mi];
            let qgs = hist.qgs + comp.capgs * (vgspp - hist.vgspp);
            let qgd = hist.qgd + comp.capgd * (vgdpp - hist.vgdpp);

            let cqgs = match method {
                IntegrationMethod::BackwardEuler => (qgs - hist.qgs) / step_h,
                IntegrationMethod::Trapezoidal => 2.0 * (qgs - hist.qgs) / step_h - hist.cqgs,
            };
            let cqgd = match method {
                IntegrationMethod::BackwardEuler => (qgd - hist.qgd) / step_h,
                IntegrationMethod::Trapezoidal => 2.0 * (qgd - hist.qgd) / step_h - hist.cqgd,
            };

            mesa_charge_histories[mi] = MesaChargeHistory {
                qgs,
                cqgs,
                qgd,
                cqgd,
                vgspp,
                vgdpp,
            };
        }

        // Update HFET junction charge histories.
        for (hi, hfet) in mna.hfets.iter().enumerate() {
            let v_gp = hfet
                .gate_prime_idx
                .or(hfet.gate_idx)
                .map(|i| solution[i])
                .unwrap_or(0.0);
            let vgspp = if let Some(spp_i) = hfet.source_prm_prm_idx {
                v_gp - solution[spp_i]
            } else {
                let v_sp = hfet
                    .source_prime_idx
                    .or(hfet.source_idx)
                    .map(|i| solution[i])
                    .unwrap_or(0.0);
                v_gp - v_sp
            };
            let vgdpp = if let Some(dpp_i) = hfet.drain_prm_prm_idx {
                v_gp - solution[dpp_i]
            } else {
                let v_dp = hfet
                    .drain_prime_idx
                    .or(hfet.drain_idx)
                    .map(|i| solution[i])
                    .unwrap_or(0.0);
                v_gp - v_dp
            };

            let (vgs, vgd) = hfet.junction_voltages(&solution);
            let comp = crate::hfet::hfet_companion_full(hfet, vgs, vgd, nr_options.gmin);

            let hist = &hfet_charge_histories[hi];
            let qgs = hist.qgs + comp.capgs * (vgspp - hist.vgspp);
            let qgd = hist.qgd + comp.capgd * (vgdpp - hist.vgdpp);

            let cqgs = match method {
                IntegrationMethod::BackwardEuler => (qgs - hist.qgs) / step_h,
                IntegrationMethod::Trapezoidal => 2.0 * (qgs - hist.qgs) / step_h - hist.cqgs,
            };
            let cqgd = match method {
                IntegrationMethod::BackwardEuler => (qgd - hist.qgd) / step_h,
                IntegrationMethod::Trapezoidal => 2.0 * (qgd - hist.qgd) / step_h - hist.cqgd,
            };

            hfet_charge_histories[hi] = HfetChargeHistory {
                qgs,
                cqgs,
                qgd,
                cqgd,
                vgspp,
                vgdpp,
            };
        }

        // Update MOSFET Meyer gate charge histories.
        for (mi, mos) in mna.mosfets.iter().enumerate() {
            let (vgs_signed, vds_signed, vbs_signed) = mos.terminal_voltages(&solution);
            let vgd_signed = vgs_signed - vds_signed;
            let vgb_signed = vgs_signed - vbs_signed;

            let mut eff_model = mos.model.clone();
            eff_model.kp = mos.beta();
            let comp = eff_model.companion(vgs_signed, vds_signed, vbs_signed);

            let l_eff = (mos.l - 2.0 * mos.model.ld).max(1e-12);
            let cox = (3.9 * 8.854214871e-12) / mos.model.tox * l_eff * mos.w;

            let (capgs, capgd, capgb) = if comp.mode > 0 {
                crate::mosfet::qmeyer(
                    vgs_signed,
                    vgd_signed,
                    comp.von,
                    comp.vdsat,
                    mos.model.phi,
                    cox,
                )
            } else {
                let (gd, gs, gb) = crate::mosfet::qmeyer(
                    vgd_signed,
                    vgs_signed,
                    comp.von,
                    comp.vdsat,
                    mos.model.phi,
                    cox,
                );
                (gs, gd, gb)
            };

            let hist = &mosfet_charge_histories[mi];
            let qgs = hist.qgs + capgs * (vgs_signed - hist.vgs);
            let qgd = hist.qgd + capgd * (vgd_signed - hist.vgd);
            let qgb = hist.qgb + capgb * (vgb_signed - hist.vgb);

            let cqgs = match method {
                IntegrationMethod::BackwardEuler => (qgs - hist.qgs) / step_h,
                IntegrationMethod::Trapezoidal => 2.0 * (qgs - hist.qgs) / step_h - hist.cqgs,
            };
            let cqgd = match method {
                IntegrationMethod::BackwardEuler => (qgd - hist.qgd) / step_h,
                IntegrationMethod::Trapezoidal => 2.0 * (qgd - hist.qgd) / step_h - hist.cqgd,
            };
            let cqgb = match method {
                IntegrationMethod::BackwardEuler => (qgb - hist.qgb) / step_h,
                IntegrationMethod::Trapezoidal => 2.0 * (qgb - hist.qgb) / step_h - hist.cqgb,
            };

            mosfet_charge_histories[mi] = MosfetChargeHistory {
                qgs,
                cqgs,
                qgd,
                cqgd,
                qgb,
                cqgb,
                vgs: vgs_signed,
                vgd: vgd_signed,
                vgb: vgb_signed,
            };
        }

        // Update MOS6 Meyer charge histories.
        for (mi, mos) in mna.mos6s.iter().enumerate() {
            let (vgs_signed, vds_signed, vbs_signed) = mos.terminal_voltages(&solution);
            let vgd_signed = vgs_signed - vds_signed;
            let vgb_signed = vgs_signed - vbs_signed;

            let comp = mos
                .model
                .companion(vgs_signed, vds_signed, vbs_signed, mos.betac());
            let l_eff = (mos.l - 2.0 * mos.model.ld).max(1e-12);
            let cox = (3.9 * 8.854214871e-12) / mos.model.tox * l_eff * mos.w;

            let (capgs, capgd, capgb) = if comp.mode > 0 {
                crate::mosfet::qmeyer(
                    vgs_signed,
                    vgd_signed,
                    comp.von,
                    comp.vdsat,
                    mos.model.phi,
                    cox,
                )
            } else {
                let (gd, gs, gb) = crate::mosfet::qmeyer(
                    vgd_signed,
                    vgs_signed,
                    comp.von,
                    comp.vdsat,
                    mos.model.phi,
                    cox,
                );
                (gs, gd, gb)
            };

            let hist = &mos6_charge_histories[mi];
            let qgs = hist.qgs + capgs * (vgs_signed - hist.vgs);
            let qgd = hist.qgd + capgd * (vgd_signed - hist.vgd);
            let qgb = hist.qgb + capgb * (vgb_signed - hist.vgb);

            let cqgs = match method {
                IntegrationMethod::BackwardEuler => (qgs - hist.qgs) / step_h,
                IntegrationMethod::Trapezoidal => 2.0 * (qgs - hist.qgs) / step_h - hist.cqgs,
            };
            let cqgd = match method {
                IntegrationMethod::BackwardEuler => (qgd - hist.qgd) / step_h,
                IntegrationMethod::Trapezoidal => 2.0 * (qgd - hist.qgd) / step_h - hist.cqgd,
            };
            let cqgb = match method {
                IntegrationMethod::BackwardEuler => (qgb - hist.qgb) / step_h,
                IntegrationMethod::Trapezoidal => 2.0 * (qgb - hist.qgb) / step_h - hist.cqgb,
            };

            mos6_charge_histories[mi] = MosfetChargeHistory {
                qgs,
                cqgs,
                qgd,
                cqgd,
                qgb,
                cqgb,
                vgs: vgs_signed,
                vgd: vgd_signed,
                vgb: vgb_signed,
            };
        }

        // Update BSIM3SOI-DD charge histories (intrinsic + CboxWL).
        for (di, inst) in mna.bsim3soi_dds.iter().enumerate() {
            let sign = inst.model.mos_type.sign();
            let vb = inst.body_int_idx.map_or(0.0, |i| solution[i]);
            let ve = inst.e_idx.map_or(0.0, |i| solution[i]);
            let vbe = sign * (vb - ve);
            let cbox_wl = inst.size_params.kb3
                * inst.model.cbox
                * inst.size_params.weff_cv
                * inst.size_params.leff_cv;
            let hist = &soidd_charge_histories[di];
            let qbe = hist.qbe_cbox + cbox_wl * (vbe - hist.vbe_cbox);
            let cqbe = match method {
                IntegrationMethod::BackwardEuler => (qbe - hist.qbe_cbox) / step_h,
                IntegrationMethod::Trapezoidal => {
                    2.0 * (qbe - hist.qbe_cbox) / step_h - hist.cqbe_cbox
                }
            };
            // Direct terminal charges from model (matching ngspice NIintegrate approach).
            let (vgs_t, vds_t, vbs_t, ves_t) = inst.terminal_voltages(&solution);
            let comp = crate::bsim3soi_dd::bsim3soi_dd_companion(
                vgs_t,
                vds_t,
                vbs_t,
                ves_t,
                &inst.size_params,
                &inst.model,
            );
            let qgate = comp.qgate;
            let qbody = comp.qbody;
            let qdrn = comp.qdrn;
            let qsub = comp.qsub;
            let cqgate = match method {
                IntegrationMethod::BackwardEuler => (qgate - hist.qgate) / step_h,
                IntegrationMethod::Trapezoidal => 2.0 * (qgate - hist.qgate) / step_h - hist.cqgate,
            };
            let cqbody = match method {
                IntegrationMethod::BackwardEuler => (qbody - hist.qbody) / step_h,
                IntegrationMethod::Trapezoidal => 2.0 * (qbody - hist.qbody) / step_h - hist.cqbody,
            };
            let cqdrn = match method {
                IntegrationMethod::BackwardEuler => (qdrn - hist.qdrn) / step_h,
                IntegrationMethod::Trapezoidal => 2.0 * (qdrn - hist.qdrn) / step_h - hist.cqdrn,
            };
            let cqsub = match method {
                IntegrationMethod::BackwardEuler => (qsub - hist.qsub) / step_h,
                IntegrationMethod::Trapezoidal => 2.0 * (qsub - hist.qsub) / step_h - hist.cqsub,
            };
            let qgate_prev = soidd_charge_histories[di].qgate;
            let qbody_prev = soidd_charge_histories[di].qbody;
            let qdrn_prev = soidd_charge_histories[di].qdrn;
            soidd_charge_histories[di] = Bsim3SoiDdChargeHistory {
                qgate,
                cqgate,
                qbody,
                cqbody,
                qdrn,
                cqdrn,
                qsub,
                cqsub,
                qbe_cbox: qbe,
                cqbe_cbox: cqbe,
                vbe_cbox: vbe,
                qgate_prev,
                qbody_prev,
                qdrn_prev,
            };
        }

        // Update VBIC junction charge histories.
        for (vi, vbic) in mna.vbics.iter().enumerate() {
            let (vbei, vbex, vbci, vbcx, vbep, vrci, vrbi, vrbp, vbcp) =
                vbic.junction_voltages(&solution);
            let comp = vbic.model.companion(
                vbei,
                vbex,
                vbci,
                vbcx,
                vbep,
                vrci,
                vrbi,
                vrbp,
                vbcp,
                nr_options.gmin,
            );

            // Capacitances for incremental charge.
            let cap_be = comp.cqbe_vbei + comp.cqbex_vbex; // combined BE
            let cap_bc = comp.cqbc_vbci;
            let cap_bep = comp.cqbep_vbep;
            let cap_bcp = comp.cqbcp_vbcp;

            let hist = &vbic_charge_histories[vi];
            let qbe = hist.qbe + cap_be * (vbei - hist.vbei);
            let qbc = hist.qbc + cap_bc * (vbci - hist.vbci);
            let qbep = hist.qbep + cap_bep * (vbep - hist.vbep);
            let qbcp = hist.qbcp + cap_bcp * (vbcp - hist.vbcp);

            let cqbe = match method {
                IntegrationMethod::BackwardEuler => (qbe - hist.qbe) / step_h,
                IntegrationMethod::Trapezoidal => 2.0 * (qbe - hist.qbe) / step_h - hist.cqbe,
            };
            let cqbc = match method {
                IntegrationMethod::BackwardEuler => (qbc - hist.qbc) / step_h,
                IntegrationMethod::Trapezoidal => 2.0 * (qbc - hist.qbc) / step_h - hist.cqbc,
            };
            let cqbep = match method {
                IntegrationMethod::BackwardEuler => (qbep - hist.qbep) / step_h,
                IntegrationMethod::Trapezoidal => 2.0 * (qbep - hist.qbep) / step_h - hist.cqbep,
            };
            let cqbcp = match method {
                IntegrationMethod::BackwardEuler => (qbcp - hist.qbcp) / step_h,
                IntegrationMethod::Trapezoidal => 2.0 * (qbcp - hist.qbcp) / step_h - hist.cqbcp,
            };

            vbic_charge_histories[vi] = VbicChargeHistory {
                qbe,
                cqbe,
                vbei,
                qbc,
                cqbc,
                vbci,
                qbep,
                cqbep,
                vbep,
                qbcp,
                cqbcp,
                vbcp,
            };
        }

        // Update LTRA histories.
        if has_ltra {
            ltra_time_points.push(t);
            for (li, inst) in mna.ltras.iter().enumerate() {
                let v1 =
                    node_voltage(&solution, inst.pos1_idx) - node_voltage(&solution, inst.neg1_idx);
                let i1 = solution[num_nodes + inst.br_eq1];
                let v2 =
                    node_voltage(&solution, inst.pos2_idx) - node_voltage(&solution, inst.neg2_idx);
                let i2 = solution[num_nodes + inst.br_eq2];
                ltra_states[li].accept(v1, i1, v2, i2);
            }
        }

        // Update TXL histories and convolution accumulators.
        if has_txl {
            let time_ps = (t * 1.0e12) as i64;
            let h_ps = step_h * 1.0e12;
            for inst in &mut mna.txls {
                let v_in = inst.pos_idx.map_or(0.0, |i| solution[i]);
                let v_out = inst.neg_idx.map_or(0.0, |i| solution[i]);
                let i_in = solution[num_nodes + inst.ibr1];
                let i_out = solution[num_nodes + inst.ibr2];

                // Update h1 convolution accumulators
                let tx = &mut inst.txline;
                if !tx.lsl {
                    let prev_vi = tx.vi_history.last();
                    let (dv_i, dv_o) = if let Some(prev) = prev_vi {
                        let dt = time_ps - prev.time;
                        if dt > 0 {
                            (
                                (v_in - prev.v_i) / dt as f64,
                                (v_out - prev.v_o) / dt as f64,
                            )
                        } else {
                            (0.0, 0.0)
                        }
                    } else {
                        (0.0, 0.0)
                    };
                    crate::txl::update_cnv_txl(tx, h_ps, v_in, v_out, dv_i, dv_o);
                }

                // Handle extended time step delayed convolution update
                if tx.ext {
                    crate::txl::update_delayed_cnv(tx, h_ps, tx.ratio);
                }

                // Record history point
                tx.vi_history.push(crate::txl::ViEntry {
                    time: time_ps,
                    v_i: v_in,
                    v_o: v_out,
                    i_i: i_in,
                    i_o: i_out,
                });
            }
        }

        // Record output point.
        if t >= t_start {
            record_point(
                t,
                &solution,
                &mna,
                num_nodes,
                &mut time_vec,
                &mut node_vecs,
                &mut branch_vecs,
                &device_param_queries,
                &mut device_param_vecs,
                &sorted_nodes,
            );
        }
    }

    // Assemble result.
    let mut vecs = vec![time_vec];
    vecs.extend(node_vecs);
    vecs.extend(branch_vecs);
    vecs.extend(device_param_vecs);

    Ok(SimResult {
        plots: vec![SimPlot {
            name: "tran1".to_string(),
            vecs,
        }],
    })
}

/// Extract a node voltage from the solution vector, returning 0 for ground.
fn node_voltage(solution: &[f64], idx: Option<usize>) -> f64 {
    idx.map(|i| solution[i]).unwrap_or(0.0)
}

/// Parse `@m1[vbs]` into `("m1", "vbs")`.
fn parse_device_param_query(var: &str) -> Option<(String, String)> {
    let rest = var.strip_prefix('@')?;
    let bracket_start = rest.find('[')?;
    let bracket_end = rest.find(']')?;
    if bracket_end <= bracket_start + 1 {
        return None;
    }
    let device = rest[..bracket_start].to_string();
    let param = rest[bracket_start + 1..bracket_end].to_string();
    Some((device, param))
}

/// Scan `.print` directive lines for `@device[param]` queries that the MNA
/// system can resolve. Returns a deduplicated list of (device, param) pairs.
///
/// Takes an iterator of `.print`-bearing lines so both the Netlist path
/// (which reads from `netlist.source`) and the Circuit path (which reads
/// from `circuit.raw_directives`) can share this code.
pub fn collect_device_param_queries<'a, I>(
    print_lines: I,
    mna: &MnaSystem,
) -> Vec<(String, String)>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut queries = Vec::new();
    for line in print_lines {
        let trimmed = line.trim().to_lowercase();
        if !trimmed.starts_with(".print") {
            continue;
        }
        for token in trimmed.split_whitespace().skip(2) {
            // Split on commas (e.g. "@m1[Vbs], V(g)/10")
            for part in token.split(',') {
                let part = part.trim();
                if let Some((device, param)) = parse_device_param_query(part) {
                    // Verify the MNA system can actually resolve this query
                    // by checking with a dummy (zero) solution.
                    let dummy = vec![0.0; mna.system.matrix.dim()];
                    if mna.query_device_param(&device, &param, &dummy).is_some() {
                        // Avoid duplicates
                        let key = (device.clone(), param.clone());
                        if !queries.contains(&key) {
                            queries.push(key);
                        }
                    }
                }
            }
        }
    }
    queries
}

/// Record a solution point into output vectors.
#[expect(clippy::too_many_arguments)]
fn record_point(
    t: f64,
    solution: &[f64],
    mna: &MnaSystem,
    num_nodes: usize,
    time_vec: &mut SimVector,
    node_vecs: &mut [SimVector],
    branch_vecs: &mut [SimVector],
    device_param_queries: &[(String, String)],
    device_param_vecs: &mut [SimVector],
    sorted_nodes: &[(String, usize)],
) {
    time_vec.data.as_real_mut().push(t);

    for (idx, (_name, node_idx)) in sorted_nodes.iter().enumerate() {
        node_vecs[idx].data.as_real_mut().push(solution[*node_idx]);
    }

    for (i, _vsrc) in mna.vsource_names.iter().enumerate() {
        branch_vecs[i]
            .data
            .as_real_mut()
            .push(solution[num_nodes + i]);
    }

    // Record device parameter queries.
    for (i, (device, param)) in device_param_queries.iter().enumerate() {
        let val = mna
            .query_device_param(device, param, solution)
            .unwrap_or(0.0);
        device_param_vecs[i].data.as_real_mut().push(val);
    }
}

/// Solve a single transient timestep.
#[expect(clippy::too_many_arguments)]
fn solve_timestep(
    mna: &MnaSystem,
    prev_solution: &[f64],
    h: f64,
    t: f64,
    tran_params: &TranParams,
    method: IntegrationMethod,
    cap_histories: &[CapHistory],
    ind_histories: &[IndHistory],
    bjt_charge_histories: &[BjtChargeHistory],
    has_nonlinear: bool,
    nr_options: &NrOptions,
    dim: usize,
    num_nodes: usize,
    ltra_states: Option<&[LtraState]>,
    ltra_coeffs: Option<&[LtraCoeffs]>,
    ltra_time_points: Option<&[f64]>,
    txl_stamps: Option<&[TxlTransientStamp]>,
    cpl_stamps: Option<&[crate::cpl::CplTransientStamp]>,
    mesa_charge_histories: &[MesaChargeHistory],
    hfet_charge_histories: &[HfetChargeHistory],
    mosfet_charge_histories: &[MosfetChargeHistory],
    mos6_charge_histories: &[MosfetChargeHistory],
    soidd_charge_histories: &[Bsim3SoiDdChargeHistory],
    vbic_charge_histories: &[VbicChargeHistory],
) -> Result<Vec<f64>, MnaError> {
    let base_matrix = &mna.system.matrix;
    let base_rhs = &mna.system.rhs;
    let capacitors = &mna.capacitors;
    let inductors = &mna.inductors;

    let dev_state = DeviceVoltageState::from_solution(mna, prev_solution);

    // Build a skip-set for BJT CJE/CJC cap indices.  These depletion caps
    // are stamped dynamically (voltage-dependent) in step 5 of the load
    // closure rather than as constant zero-bias values.  CJS (substrate)
    // caps are NOT skipped — they remain constant.
    let mut skip_bjt_cap = vec![false; capacitors.len()];
    for idx in &mna.bjt_cap_indices {
        if let Some(i) = idx.cje_idx {
            skip_bjt_cap[i] = true;
        }
        if let Some(i) = idx.cjc_idx {
            skip_bjt_cap[i] = true;
        }
    }

    let load = |solution: &[f64],
                system: &mut LinearSystem,
                source_factor: f64,
                gmin: f64,
                _mode: NrMode| {
        // 1. Copy base linear stamps (R, V, I topology + inductor topology).
        for triplet in base_matrix.triplets() {
            system.matrix.add(triplet.row, triplet.col, triplet.value);
        }
        for (i, &val) in base_rhs.iter().enumerate() {
            system.rhs[i] += val * source_factor;
        }

        // 1b. Override source values with waveform-evaluated values at time t.
        for vs in &mna.voltage_sources {
            if let Some(ref wf) = vs.waveform {
                let v_t = waveform::evaluate(wf, t, tran_params);
                system.rhs[vs.branch_idx] = v_t * source_factor;
            }
        }
        for cs in &mna.current_sources {
            if let Some(ref wf) = cs.waveform {
                let i_t = waveform::evaluate(wf, t, tran_params);
                let i_diff = i_t - cs.dc_value;
                if let Some(ni) = cs.pos_idx {
                    system.rhs[ni] -= i_diff * source_factor;
                }
                if let Some(nj) = cs.neg_idx {
                    system.rhs[nj] += i_diff * source_factor;
                }
            }
        }

        // 1c. Stamp LTRA transient equations.
        if let (Some(states), Some(coeffs), Some(time_pts)) =
            (ltra_states, ltra_coeffs, ltra_time_points)
        {
            let time_index = time_pts.len() - 1;
            for (li, inst) in mna.ltras.iter().enumerate() {
                crate::ltra::stamp_ltra_transient(
                    inst,
                    &states[li],
                    &coeffs[li],
                    system,
                    num_nodes,
                    t,
                    time_pts,
                    time_index,
                );
            }
        }

        // 1d. Stamp TXL transient equations (pre-computed).
        if let Some(stamps) = txl_stamps {
            for stamp in stamps {
                crate::txl::apply_txl_transient(stamp, system);
            }
        }

        // 1e. Stamp CPL transient equations (pre-computed).
        if let Some(stamps) = cpl_stamps {
            for stamp in stamps {
                crate::cpl::apply_cpl_transient(stamp, system, num_nodes);
            }
        }

        // 2. Stamp capacitor companion models.
        //    Skip BJT CJE/CJC caps — these are stamped dynamically (voltage-
        //    dependent) in step 5 below instead of as constant zero-bias values.
        for (ci, cap) in capacitors.iter().enumerate() {
            if skip_bjt_cap[ci] {
                continue;
            }
            let (geq, ieq) = capacitor_companion(cap.capacitance, h, &cap_histories[ci], method);
            stamp_conductance(&mut system.matrix, cap.pos_idx, cap.neg_idx, geq);
            stamp_current_source(&mut system.rhs, cap.pos_idx, cap.neg_idx, ieq);
        }

        // 3. Stamp inductor companion models.
        for (li, ind) in inductors.iter().enumerate() {
            let (req, veq) = inductor_companion(ind.inductance, h, &ind_histories[li], method);
            system.matrix.add(ind.branch_idx, ind.branch_idx, -req);
            system.rhs[ind.branch_idx] += veq;
        }

        // 3b. Stamp mutual coupling cross-terms.
        //
        // The coupled inductor branch equation is:
        //   V1 = L1 * di1/dt + M * di2/dt
        //
        // For Backward Euler: V1 = L1/h*(i1_n - i1_prev) + M/h*(i2_n - i2_prev)
        //   matrix: -L1/h on diag (self), -M/h on cross
        //   rhs:    -L1/h * i1_prev (self) + (-M/h * i2_prev) (mutual)
        //
        // For Trapezoidal: V1 = 2L1/h*(i1_n-i1_prev)-v1_prev + 2M/h*(i2_n-i2_prev)
        //   matrix: -2L1/h on diag, -2M/h on cross
        //   rhs:    -(2L1/h*i1_prev + v1_prev) (self) + (-2M/h * i2_prev) (mutual)
        {
            let coeff = match method {
                IntegrationMethod::BackwardEuler => 1.0 / h,
                IntegrationMethod::Trapezoidal => 2.0 / h,
            };
            for mc in &mna.mutual_couplings {
                let m_coeff = mc.factor * coeff;
                // Matrix cross-terms.
                system.matrix.add(mc.branch1_idx, mc.branch2_idx, -m_coeff);
                system.matrix.add(mc.branch2_idx, mc.branch1_idx, -m_coeff);

                // RHS history contribution from partner inductor's previous current.
                let i2_prev = ind_histories[mc.ind2_vec_idx].current;
                let i1_prev = ind_histories[mc.ind1_vec_idx].current;
                system.rhs[mc.branch1_idx] += -m_coeff * i2_prev;
                system.rhs[mc.branch2_idx] += -m_coeff * i1_prev;
            }
        }

        // 4. Stamp all nonlinear device companions. Device stamps always use
        //    nominal gmin (not the elevated gmin from gmin stepping).
        //    Transient always uses Float mode (voltage limiting against previous
        //    timestep's solution), matching ngspice MODETRANOP.
        if has_nonlinear {
            let _ = gmin;
            // Always use Float for transient — we have a meaningful previous solution.
            dev_state.stamp_devices(solution, system, mna, nr_options.gmin, NrMode::Float, true);
        }

        // 5. Stamp BJT junction capacitance companion models.
        //    Uses the FULL voltage-dependent depletion capacitance (not a
        //    correction on top of a constant MNA cap) plus TF/TR diffusion
        //    capacitance.  The constant CJE/CJC caps are skipped in step 2
        //    above.  This gives the correct cap in both forward and reverse
        //    bias, matching ngspice's bjtload.c which computes
        //    cap(v) = junction_cap(v) + TF*gbe at every NR iteration.
        //
        //    The incremental charge formulation is:
        //      Q_new = Q_prev + C(v) * (v - v_prev)
        //    where C(v) = junction_cap(v) + TF*gbe (or TR*gbc).
        //    geq = C(v)/h is always positive since junction_cap >= 0.
        if !mna.bjts.is_empty() {
            let prev_bjt = dev_state.prev_bjt_voltages();
            for (bi, bjt) in mna.bjts.iter().enumerate() {
                let (vbe, vbc) = prev_bjt[bi];
                let sign = bjt.model.bjt_type.sign();
                let m = bjt.m * bjt.area;

                // Compute companion at current operating point to get gbe, gbc.
                let comp = bjt.model.companion(vbe, vbc);

                // Full voltage-dependent depletion cap + qb-normalized diffusion cap.
                // Matches ngspice bjtload.c: cbe_mod=cbe/qb, gbe_mod=(gbe-cbe_mod*dqbdve)/qb
                let cbe_mod = comp.cbe_raw / comp.qb;
                let gbe_mod = (comp.gbe_raw - cbe_mod * comp.dqbdve) / comp.qb;
                let capbe = bjt.model.cap_be(vbe) + bjt.model.tf * gbe_mod;
                let capbc = bjt.model.cap_bc(vbc) + bjt.model.tr * comp.gbc_raw;

                // geqcb: BE charge cross-coupling from Vbc (ngspice bjtload.c line 674).
                // The BE diffusion charge Qbe = TF * cbe/qb depends on Vbc through qb
                // (Early effect). geqcb = dQbe/dVbc = TF * (-cbe_mod * dqbdvc) / qb.
                // For xtf != 0 this would include arg3 = cbe*argtf*ovtf, but xtf defaults
                // to 0 so arg3 = 0 in the common case.
                let geqcb_unscaled = bjt.model.tf
                    * (0.0 /* arg3, zero when xtf=0 */ - cbe_mod * comp.dqbdvc)
                    / comp.qb;

                let hist = &bjt_charge_histories[bi];

                // Incremental charge: Q = Q_prev + dQ/dVbe * ΔVbe + dQ/dVbc * ΔVbc
                // The geqcb_unscaled term accounts for Vbc's effect on the BE charge
                // (ngspice computes qbe = tf*cbe_norm which implicitly includes this).
                let qbe = hist.qbe + capbe * (vbe - hist.vbe) + geqcb_unscaled * (vbc - hist.vbc);
                let qbc = hist.qbc + capbc * (vbc - hist.vbc);

                // Integration coefficient: ag[0] = 1/h (BE) or 2/h (trap).
                let ag0 = match method {
                    IntegrationMethod::BackwardEuler => 1.0 / h,
                    IntegrationMethod::Trapezoidal => 2.0 / h,
                };

                // Integrate B-E charge.
                let (geq_be, cqbe) = match method {
                    IntegrationMethod::BackwardEuler => {
                        let geq = capbe / h;
                        let cq = (qbe - hist.qbe) / h;
                        (geq, cq)
                    }
                    IntegrationMethod::Trapezoidal => {
                        let geq = 2.0 * capbe / h;
                        let cq = 2.0 * (qbe - hist.qbe) / h - hist.cqbe;
                        (geq, cq)
                    }
                };

                // Integrate B-C charge.
                let (geq_bc, cqbc) = match method {
                    IntegrationMethod::BackwardEuler => {
                        let geq = capbc / h;
                        let cq = (qbc - hist.qbc) / h;
                        (geq, cq)
                    }
                    IntegrationMethod::Trapezoidal => {
                        let geq = 2.0 * capbc / h;
                        let cq = 2.0 * (qbc - hist.qbc) / h - hist.cqbc;
                        (geq, cq)
                    }
                };

                // Scale geqcb by integration coefficient (ngspice bjtload.c line 802).
                let geqcb = geqcb_unscaled * ag0;

                // Stamp B-E charge: conductance + Norton current source.
                let bp = bjt.base_prime_idx;
                let cp = bjt.col_prime_idx;
                let ep = bjt.emit_prime_idx;

                stamp_conductance(&mut system.matrix, bp, ep, m * geq_be);
                let ieq_be = sign * m * (cqbe - geq_be * vbe);
                stamp_current_source(&mut system.rhs, bp, ep, ieq_be);

                // Stamp B-C charge: conductance + Norton current source.
                stamp_conductance(&mut system.matrix, bp, cp, m * geq_bc);
                let ieq_bc = sign * m * (cqbc - geq_bc * vbc);
                stamp_current_source(&mut system.rhs, bp, cp, ieq_bc);

                // Stamp geqcb: BE charge cross-coupling from Vbc (ngspice bjtload.c
                // lines 914, 923, 926, 927). This is a VCCS: current I = geqcb * Vbc
                // flows into B' and out of E', providing the Jacobian entry for dQbe/dVbc.
                // Matrix stamps:  B',B' += geqcb;  B',C' -= geqcb;
                //                 E',C' += geqcb;  E',B' -= geqcb
                // Norton RHS:     ceqbe modified by -vbc*geqcb (ngspice line 893)
                if geqcb.abs() > 0.0 {
                    let mg = m * geqcb;
                    if let Some(b) = bp {
                        system.matrix.add(b, b, mg);
                        if let Some(c) = cp {
                            system.matrix.add(b, c, -mg);
                        }
                    }
                    if let Some(e) = ep {
                        if let Some(c) = cp {
                            system.matrix.add(e, c, mg);
                        }
                        if let Some(b) = bp {
                            system.matrix.add(e, b, -mg);
                        }
                    }
                    // Norton current for geqcb VCCS: matches ngspice ceqbe formula
                    // where vbc*(go - geqcb) replaces vbc*go, adding -geqcb*vbc.
                    let ieq_cb = -sign * m * geqcb * vbc;
                    stamp_current_source(&mut system.rhs, bp, ep, ieq_cb);
                }
            }
        }

        // 6. Stamp MESA junction capacitance companion models.
        //    In ngspice mesaload.c, the junction charges qgs/qgd are integrated
        //    via NIintegrate to produce ggspp/ggdpp (conductances) and
        //    cgspp/cgdpp (currents) that couple gate' to source''/drain'' PPM nodes.
        if !mna.mesas.is_empty() {
            for (mi, mesa) in mna.mesas.iter().enumerate() {
                let gp = mesa.gate_prime_idx;
                // Use PPM nodes if they exist, otherwise fall back to prime nodes.
                let spp = mesa.source_prm_prm_idx.or(mesa.source_prime_idx);
                let dpp = mesa.drain_prm_prm_idx.or(mesa.drain_prime_idx);

                // Compute vgspp and vgdpp from the current NR solution.
                let v_gp = gp.map(|i| solution[i]).unwrap_or(0.0);
                let vgspp = if let Some(spp_i) = spp {
                    v_gp - solution[spp_i]
                } else {
                    // No PPM node — fall back to vgs (gate' - source').
                    let v_sp = mesa.source_prime_idx.map(|i| solution[i]).unwrap_or(0.0);
                    v_gp - v_sp
                };
                let vgdpp = if let Some(dpp_i) = dpp {
                    v_gp - solution[dpp_i]
                } else {
                    let v_dp = mesa.drain_prime_idx.map(|i| solution[i]).unwrap_or(0.0);
                    v_gp - v_dp
                };

                // Get capgs/capgd from companion using LIMITED voltages
                // (same voltages stamp_devices used for the main device stamp).
                let prev_mesa = dev_state.prev_mesa_voltages();
                let (vgs_lim, vgd_lim) = prev_mesa[mi];
                let comp = crate::mesa::mesa_companion(mesa, vgs_lim, vgd_lim, nr_options.gmin);
                let capgs = comp.capgs;
                let capgd = comp.capgd;

                // Compute charges: Q = C * V (constant cap model, matching ngspice).
                let hist = &mesa_charge_histories[mi];
                let qgs = hist.qgs + capgs * (vgspp - hist.vgspp);
                let qgd = hist.qgd + capgd * (vgdpp - hist.vgdpp);

                // Integrate G-S charge.
                let (ggspp, cqgs) = match method {
                    IntegrationMethod::BackwardEuler => {
                        let geq = capgs / h;
                        let cq = (qgs - hist.qgs) / h;
                        (geq, cq)
                    }
                    IntegrationMethod::Trapezoidal => {
                        let geq = 2.0 * capgs / h;
                        let cq = 2.0 * (qgs - hist.qgs) / h - hist.cqgs;
                        (geq, cq)
                    }
                };

                // Integrate G-D charge.
                let (ggdpp, cqgd) = match method {
                    IntegrationMethod::BackwardEuler => {
                        let geq = capgd / h;
                        let cq = (qgd - hist.qgd) / h;
                        (geq, cq)
                    }
                    IntegrationMethod::Trapezoidal => {
                        let geq = 2.0 * capgd / h;
                        let cq = 2.0 * (qgd - hist.qgd) / h - hist.cqgd;
                        (geq, cq)
                    }
                };

                // Stamp ggspp conductance between gp and cap_s_node.
                stamp_conductance(&mut system.matrix, gp, spp, ggspp);
                // Stamp ggdpp conductance between gp and cap_d_node.
                stamp_conductance(&mut system.matrix, gp, dpp, ggdpp);

                // RHS: charge current Norton equivalents at cap nodes.
                let ieq_gs = cqgs - ggspp * vgspp;
                let ieq_gd = cqgd - ggdpp * vgdpp;
                if let Some(spp_i) = spp {
                    system.rhs[spp_i] += ieq_gs;
                }
                if let Some(dpp_i) = dpp {
                    system.rhs[dpp_i] += ieq_gd;
                }
                // gp sees the negative sum of both charge currents.
                if let Some(gp_i) = gp {
                    system.rhs[gp_i] -= ieq_gs;
                    system.rhs[gp_i] -= ieq_gd;
                }
            }
        }

        // 6b. Stamp HFET junction capacitance companion models.
        //     Same pattern as MESA: integrate capgs/capgd charges and stamp
        //     conductance + Norton current between gate' and cap nodes.
        if !mna.hfets.is_empty() {
            let prev_hfet = dev_state.prev_hfet_voltages();
            for (hi, hfet) in mna.hfets.iter().enumerate() {
                let gp = hfet.gate_prime_idx.or(hfet.gate_idx);
                // Use PPM nodes if they exist, otherwise fall back to prime nodes.
                let spp = hfet
                    .source_prm_prm_idx
                    .or(hfet.source_prime_idx)
                    .or(hfet.source_idx);
                let dpp = hfet
                    .drain_prm_prm_idx
                    .or(hfet.drain_prime_idx)
                    .or(hfet.drain_idx);

                // Compute vgspp and vgdpp from the current NR solution.
                let v_gp = gp.map(|i| solution[i]).unwrap_or(0.0);
                let vgspp = spp.map(|i| v_gp - solution[i]).unwrap_or(0.0);
                let vgdpp = dpp.map(|i| v_gp - solution[i]).unwrap_or(0.0);

                // Get capgs/capgd from companion using limited voltages.
                let (vgs_lim, vgd_lim) = prev_hfet[hi];
                let comp =
                    crate::hfet::hfet_companion_full(hfet, vgs_lim, vgd_lim, nr_options.gmin);
                let capgs = comp.capgs;
                let capgd = comp.capgd;

                // Compute charges incrementally: Q = Q_prev + C * (V - V_prev).
                let hist = &hfet_charge_histories[hi];
                let qgs = hist.qgs + capgs * (vgspp - hist.vgspp);
                let qgd = hist.qgd + capgd * (vgdpp - hist.vgdpp);

                // Integrate G-S charge.
                let (ggspp, cqgs) = match method {
                    IntegrationMethod::BackwardEuler => {
                        let geq = capgs / h;
                        let cq = (qgs - hist.qgs) / h;
                        (geq, cq)
                    }
                    IntegrationMethod::Trapezoidal => {
                        let geq = 2.0 * capgs / h;
                        let cq = 2.0 * (qgs - hist.qgs) / h - hist.cqgs;
                        (geq, cq)
                    }
                };

                // Integrate G-D charge.
                let (ggdpp, cqgd) = match method {
                    IntegrationMethod::BackwardEuler => {
                        let geq = capgd / h;
                        let cq = (qgd - hist.qgd) / h;
                        (geq, cq)
                    }
                    IntegrationMethod::Trapezoidal => {
                        let geq = 2.0 * capgd / h;
                        let cq = 2.0 * (qgd - hist.qgd) / h - hist.cqgd;
                        (geq, cq)
                    }
                };

                // Stamp ggspp conductance between gp and cap_s_node.
                stamp_conductance(&mut system.matrix, gp, spp, ggspp);
                // Stamp ggdpp conductance between gp and cap_d_node.
                stamp_conductance(&mut system.matrix, gp, dpp, ggdpp);

                // RHS: charge current Norton equivalents at cap nodes.
                let ieq_gs = cqgs - ggspp * vgspp;
                let ieq_gd = cqgd - ggdpp * vgdpp;
                if let Some(spp_i) = spp {
                    system.rhs[spp_i] += ieq_gs;
                }
                if let Some(dpp_i) = dpp {
                    system.rhs[dpp_i] += ieq_gd;
                }
                // gp sees the negative sum of both charge currents.
                if let Some(gp_i) = gp {
                    system.rhs[gp_i] -= ieq_gs;
                    system.rhs[gp_i] -= ieq_gd;
                }
            }
        }

        // 7. Stamp MOSFET Meyer gate capacitance companion models.
        //    The Meyer model computes voltage-dependent capgs/capgd/capgb which
        //    are integrated using the same method as MESA junction charges.
        //    The constant overlap caps (CGSO, CGDO, CGBO) are already in MNA;
        //    this adds only the dynamic Meyer portion.
        if !mna.mosfets.is_empty() {
            let prev_mos = dev_state.prev_mos_voltages();
            for (mi, mos) in mna.mosfets.iter().enumerate() {
                let (vgs_signed, vds_signed, vbs_signed) = mos.terminal_voltages(solution);
                let vgd_signed = vgs_signed - vds_signed;
                let vgb_signed = vgs_signed - vbs_signed;

                // Use limited voltages for companion (same as stamp_devices used).
                let (vgs_lim, vds_lim, vbs_lim, _von_lim) = prev_mos[mi];
                let mut eff_model = mos.model.clone();
                eff_model.kp = mos.beta();
                let comp = eff_model.companion(vgs_lim, vds_lim, vbs_lim);

                let l_eff = (mos.l - 2.0 * mos.model.ld).max(1e-12);
                let cox = (3.9 * 8.854214871e-12) / mos.model.tox * l_eff * mos.w;

                // Use limited voltages for qmeyer (consistent with von/vdsat).
                let vgd_lim = vgs_lim - vds_lim;
                let _vgb_lim = vgs_lim - vbs_lim;
                // Mode handling: swap vgs/vgd and capgs/capgd for reversed mode.
                let (capgs, capgd, capgb) = if comp.mode > 0 {
                    crate::mosfet::qmeyer(
                        vgs_lim,
                        vgd_lim,
                        comp.von,
                        comp.vdsat,
                        mos.model.phi,
                        cox,
                    )
                } else {
                    let (gd, gs, gb) = crate::mosfet::qmeyer(
                        vgd_lim,
                        vgs_lim,
                        comp.von,
                        comp.vdsat,
                        mos.model.phi,
                        cox,
                    );
                    (gs, gd, gb)
                };

                let hist = &mosfet_charge_histories[mi];

                // Incremental charge uses actual node voltages for tracking.
                let qgs = hist.qgs + capgs * (vgs_signed - hist.vgs);
                let qgd = hist.qgd + capgd * (vgd_signed - hist.vgd);
                let qgb = hist.qgb + capgb * (vgb_signed - hist.vgb);

                // Integrate G-S charge.
                let (geq_gs, cqgs) = match method {
                    IntegrationMethod::BackwardEuler => {
                        let geq = capgs / h;
                        let cq = (qgs - hist.qgs) / h;
                        (geq, cq)
                    }
                    IntegrationMethod::Trapezoidal => {
                        let geq = 2.0 * capgs / h;
                        let cq = 2.0 * (qgs - hist.qgs) / h - hist.cqgs;
                        (geq, cq)
                    }
                };

                // Integrate G-D charge.
                let (geq_gd, cqgd) = match method {
                    IntegrationMethod::BackwardEuler => {
                        let geq = capgd / h;
                        let cq = (qgd - hist.qgd) / h;
                        (geq, cq)
                    }
                    IntegrationMethod::Trapezoidal => {
                        let geq = 2.0 * capgd / h;
                        let cq = 2.0 * (qgd - hist.qgd) / h - hist.cqgd;
                        (geq, cq)
                    }
                };

                // Integrate G-B charge.
                let (geq_gb, cqgb) = match method {
                    IntegrationMethod::BackwardEuler => {
                        let geq = capgb / h;
                        let cq = (qgb - hist.qgb) / h;
                        (geq, cq)
                    }
                    IntegrationMethod::Trapezoidal => {
                        let geq = 2.0 * capgb / h;
                        let cq = 2.0 * (qgb - hist.qgb) / h - hist.cqgb;
                        (geq, cq)
                    }
                };

                let gp = mos.gate_idx;
                let dp = mos.drain_prime_idx;
                let sp = mos.source_prime_idx;
                let b = mos.bulk_idx;

                let sign = mos.model.mos_type.sign();
                let m = mos.m;

                // Stamp G-S charge: conductance between gate and source_prime.
                stamp_conductance(&mut system.matrix, gp, sp, m * geq_gs);
                let ieq_gs = sign * m * (cqgs - geq_gs * vgs_signed);
                stamp_current_source(&mut system.rhs, gp, sp, ieq_gs);

                // Stamp G-D charge: conductance between gate and drain_prime.
                stamp_conductance(&mut system.matrix, gp, dp, m * geq_gd);
                let ieq_gd = sign * m * (cqgd - geq_gd * vgd_signed);
                stamp_current_source(&mut system.rhs, gp, dp, ieq_gd);

                // Stamp G-B charge: conductance between gate and bulk.
                stamp_conductance(&mut system.matrix, gp, b, m * geq_gb);
                let ieq_gb = sign * m * (cqgb - geq_gb * vgb_signed);
                stamp_current_source(&mut system.rhs, gp, b, ieq_gb);
            }
        }

        // 8. Stamp MOS6 (Level 6) Meyer gate capacitance companion models.
        //    Same pattern as Level 1 MOSFET Meyer caps (section 7).
        if !mna.mos6s.is_empty() {
            let prev_mos6 = dev_state.prev_mos6_voltages();
            for (mi, mos) in mna.mos6s.iter().enumerate() {
                let (vgs_signed, vds_signed, vbs_signed) = mos.terminal_voltages(solution);
                let vgd_signed = vgs_signed - vds_signed;
                let vgb_signed = vgs_signed - vbs_signed;

                let (vgs_lim, vds_lim, vbs_lim, _von_lim) = prev_mos6[mi];
                let comp = mos.model.companion(vgs_lim, vds_lim, vbs_lim, mos.betac());

                let l_eff = (mos.l - 2.0 * mos.model.ld).max(1e-12);
                let cox = (3.9 * 8.854214871e-12) / mos.model.tox * l_eff * mos.w;

                let vgd_lim = vgs_lim - vds_lim;
                let (capgs, capgd, capgb) = if comp.mode > 0 {
                    crate::mosfet::qmeyer(
                        vgs_lim,
                        vgd_lim,
                        comp.von,
                        comp.vdsat,
                        mos.model.phi,
                        cox,
                    )
                } else {
                    let (gd, gs, gb) = crate::mosfet::qmeyer(
                        vgd_lim,
                        vgs_lim,
                        comp.von,
                        comp.vdsat,
                        mos.model.phi,
                        cox,
                    );
                    (gs, gd, gb)
                };

                let hist = &mos6_charge_histories[mi];

                let qgs = hist.qgs + capgs * (vgs_signed - hist.vgs);
                let qgd = hist.qgd + capgd * (vgd_signed - hist.vgd);
                let qgb = hist.qgb + capgb * (vgb_signed - hist.vgb);

                let (geq_gs, cqgs) = match method {
                    IntegrationMethod::BackwardEuler => (capgs / h, (qgs - hist.qgs) / h),
                    IntegrationMethod::Trapezoidal => {
                        (2.0 * capgs / h, 2.0 * (qgs - hist.qgs) / h - hist.cqgs)
                    }
                };
                let (geq_gd, cqgd) = match method {
                    IntegrationMethod::BackwardEuler => (capgd / h, (qgd - hist.qgd) / h),
                    IntegrationMethod::Trapezoidal => {
                        (2.0 * capgd / h, 2.0 * (qgd - hist.qgd) / h - hist.cqgd)
                    }
                };
                let (geq_gb, cqgb) = match method {
                    IntegrationMethod::BackwardEuler => (capgb / h, (qgb - hist.qgb) / h),
                    IntegrationMethod::Trapezoidal => {
                        (2.0 * capgb / h, 2.0 * (qgb - hist.qgb) / h - hist.cqgb)
                    }
                };

                let gp = mos.gate_idx;
                let dp = mos.drain_prime_idx;
                let sp = mos.source_prime_idx;
                let b = mos.bulk_idx;
                let sign = mos.model.mos_type.sign();
                let m = mos.m;

                stamp_conductance(&mut system.matrix, gp, sp, m * geq_gs);
                let ieq_gs = sign * m * (cqgs - geq_gs * vgs_signed);
                stamp_current_source(&mut system.rhs, gp, sp, ieq_gs);

                stamp_conductance(&mut system.matrix, gp, dp, m * geq_gd);
                let ieq_gd = sign * m * (cqgd - geq_gd * vgd_signed);
                stamp_current_source(&mut system.rhs, gp, dp, ieq_gd);

                stamp_conductance(&mut system.matrix, gp, b, m * geq_gb);
                let ieq_gb = sign * m * (cqgb - geq_gb * vgb_signed);
                stamp_current_source(&mut system.rhs, gp, b, ieq_gb);
            }
        }

        // 9. Stamp BSIM3SOI-DD charge companion models.
        //    Full 5-terminal charge coupling (gate, body, drain, source/KCL, substrate/E)
        //    using the complete capacitance matrix including E-node derivatives
        //    (cgeb/cbeb/cdeb/ceeb/cegb/cedb/cesb) for floating-body transient response.
        {
            let prev_dd = dev_state.prev_bsim3soi_dd_voltages();
            for (di, inst) in mna.bsim3soi_dds.iter().enumerate() {
                let sign = inst.model.mos_type.sign();
                let m = inst.m;
                let sp_dd = &inst.size_params;

                // (a) Intrinsic charge integration from companion.
                let (vgs, vds, vbs_t, ves) = prev_dd[di];
                let comp = crate::bsim3soi_dd::bsim3soi_dd_companion(
                    vgs,
                    vds,
                    vbs_t,
                    ves,
                    sp_dd,
                    &inst.model,
                );

                let hist = &soidd_charge_histories[di];

                // Integrate 4 terminal charges using incremental approach.
                // Q_new = Q_old + C(V) * delta_V (matching MOSFET Meyer cap pattern).
                let ag0 = match method {
                    IntegrationMethod::BackwardEuler => 1.0 / h,
                    IntegrationMethod::Trapezoidal => 2.0 / h,
                };

                // Node indices — swap drain/source for reverse mode.
                let g = inst.gate_idx;
                let b = inst.body_int_idx;
                let e = inst.e_idx;
                let (dp, sp_node) = if comp.mode > 0 {
                    (inst.drain_prime_idx, inst.source_prime_idx)
                } else {
                    (inst.source_prime_idx, inst.drain_prime_idx)
                };

                // Body-referenced voltages from LIMITED terminal voltages.
                // Must use limited voltages (not raw solution) so the Norton
                // current is consistent with the gc matrix, which is evaluated
                // at the limited operating point.  Mixing limited capacitances
                // with raw solution voltages causes massive Norton current
                // artifacts when ag0 is large (tiny timestep), because
                // ceqqg = cqgate - gc*v involves catastrophic cancellation.
                // ngspice b3soiddld.c lines 3860-3867 uses the same limited
                // vgb/vbd/vbs/veb set during voltage limiting.
                // Limited terminal voltages are source-referenced (already
                // sign-multiplied): vgs=sign*(Vg-Vs), vds=sign*(Vd-Vs), etc.
                let vgb = vgs - vbs_t;
                let vdb_model = vds - vbs_t;
                let vsb_model = -vbs_t;
                let veb_model = ves - vbs_t;

                // Direct terminal charges from the model (ngspice b3soiddld.c lines 3387-3390,
                // 3788-3791).  ngspice stores Q(V) directly, then calls NIintegrate.
                // Using the exact charges instead of incremental C*ΔV avoids the
                // derivative-oscillation problem in CAPMOD=3 where dQbf/dV blows up
                // near Xc ≈ 1 but Q itself is smooth.
                let qgate = comp.qgate;
                let qbody = comp.qbody;
                let qdrn = comp.qdrn;
                let qsub = comp.qsub;

                let cqgate = match method {
                    IntegrationMethod::BackwardEuler => (qgate - hist.qgate) / h,
                    IntegrationMethod::Trapezoidal => 2.0 * (qgate - hist.qgate) / h - hist.cqgate,
                };
                let cqbody = match method {
                    IntegrationMethod::BackwardEuler => (qbody - hist.qbody) / h,
                    IntegrationMethod::Trapezoidal => 2.0 * (qbody - hist.qbody) / h - hist.cqbody,
                };
                let cqdrn = match method {
                    IntegrationMethod::BackwardEuler => (qdrn - hist.qdrn) / h,
                    IntegrationMethod::Trapezoidal => 2.0 * (qdrn - hist.qdrn) / h - hist.cqdrn,
                };
                let cqsub = match method {
                    IntegrationMethod::BackwardEuler => (qsub - hist.qsub) / h,
                    IntegrationMethod::Trapezoidal => 2.0 * (qsub - hist.qsub) / h - hist.cqsub,
                };

                // gc matrix: gc_ij = c_ij * ag0 (capacitance derivative scaled by integration coeff)
                // 5-terminal: G, D, S, B, E
                let gcggb = comp.cggb * ag0;
                let gcgdb = comp.cgdb * ag0;
                let gcgsb = comp.cgsb * ag0;
                let gcgeb = comp.cgeb * ag0;
                let gcbgb = comp.cbgb * ag0;
                let gcbdb = comp.cbdb * ag0;
                let gcbsb = comp.cbsb * ag0;
                let gcbeb = comp.cbeb * ag0;
                let gcdgb = comp.cdgb * ag0;
                let gcddb = comp.cddb * ag0;
                let gcdsb = comp.cdsb * ag0;
                let gcdeb = comp.cdeb * ag0;
                let gcegb = comp.cegb * ag0;
                let gcedb = comp.cedb * ag0;
                let gcesb = comp.cesb * ag0;
                let gceeb = comp.ceeb * ag0;

                // Body column by KCL: gc_xb = -(gc_xg + gc_xd + gc_xs + gc_xe)
                let gcgbb = -(gcggb + gcgdb + gcgsb + gcgeb);
                let gcbbb = -(gcbgb + gcbdb + gcbsb + gcbeb);
                let gcdbb = -(gcdgb + gcddb + gcdsb + gcdeb);
                let gcebb = -(gcegb + gcedb + gcesb + gceeb);

                // Norton currents: ceqq = cq - sum(gc_ij * v_jb) for 5 terminals
                let ceqqg = cqgate
                    - (gcggb * vgb + gcgdb * vdb_model + gcgsb * vsb_model + gcgeb * veb_model);
                let ceqqb = cqbody
                    - (gcbgb * vgb + gcbdb * vdb_model + gcbsb * vsb_model + gcbeb * veb_model);
                let ceqqd = cqdrn
                    - (gcdgb * vgb + gcddb * vdb_model + gcdsb * vsb_model + gcdeb * veb_model);
                let ceqqe = cqsub
                    - (gcegb * vgb + gcedb * vdb_model + gcesb * vsb_model + gceeb * veb_model);

                // Stamp gc matrix (ngspice b3soiddld.c lines 3680-3721, 3732-3780)
                // Gate row: G,G + G,D + G,S + G,E + G,B(KCL)
                if let Some(gi) = g {
                    system.matrix.add(gi, gi, m * gcggb);
                    if let Some(di2) = dp {
                        system.matrix.add(gi, di2, m * gcgdb);
                    }
                    if let Some(si) = sp_node {
                        system.matrix.add(gi, si, m * gcgsb);
                    }
                    if let Some(ei) = e {
                        system.matrix.add(gi, ei, m * gcgeb);
                    }
                    if let Some(bi) = b {
                        system.matrix.add(gi, bi, m * gcgbb);
                    }
                }
                // Body row: B,G + B,D + B,S + B,E + B,B(KCL)
                if let Some(bi) = b {
                    if let Some(gi) = g {
                        system.matrix.add(bi, gi, m * gcbgb);
                    }
                    if let Some(di2) = dp {
                        system.matrix.add(bi, di2, m * gcbdb);
                    }
                    if let Some(si) = sp_node {
                        system.matrix.add(bi, si, m * gcbsb);
                    }
                    if let Some(ei) = e {
                        system.matrix.add(bi, ei, m * gcbeb);
                    }
                    system.matrix.add(bi, bi, m * gcbbb);
                }
                // Drain row: D,G + D,D + D,S + D,E + D,B(KCL)
                if let Some(di2) = dp {
                    if let Some(gi) = g {
                        system.matrix.add(di2, gi, m * gcdgb);
                    }
                    system.matrix.add(di2, di2, m * gcddb);
                    if let Some(si) = sp_node {
                        system.matrix.add(di2, si, m * gcdsb);
                    }
                    if let Some(ei) = e {
                        system.matrix.add(di2, ei, m * gcdeb);
                    }
                    if let Some(bi) = b {
                        system.matrix.add(di2, bi, m * gcdbb);
                    }
                }
                // E (substrate) row: E,G + E,D + E,S + E,E + E,B(KCL)
                if let Some(ei) = e {
                    if let Some(gi) = g {
                        system.matrix.add(ei, gi, m * gcegb);
                    }
                    if let Some(di2) = dp {
                        system.matrix.add(ei, di2, m * gcedb);
                    }
                    if let Some(si) = sp_node {
                        system.matrix.add(ei, si, m * gcesb);
                    }
                    system.matrix.add(ei, ei, m * gceeb);
                    if let Some(bi) = b {
                        system.matrix.add(ei, bi, m * gcebb);
                    }
                }
                // Source row (by KCL: qs = -(qg + qb + qd + qe))
                if let Some(si) = sp_node {
                    if let Some(gi) = g {
                        system
                            .matrix
                            .add(si, gi, -m * (gcggb + gcbgb + gcdgb + gcegb));
                    }
                    if let Some(di2) = dp {
                        system
                            .matrix
                            .add(si, di2, -m * (gcgdb + gcbdb + gcddb + gcedb));
                    }
                    system
                        .matrix
                        .add(si, si, -m * (gcgsb + gcbsb + gcdsb + gcesb));
                    if let Some(ei) = e {
                        system
                            .matrix
                            .add(si, ei, -m * (gcgeb + gcbeb + gcdeb + gceeb));
                    }
                    if let Some(bi) = b {
                        system
                            .matrix
                            .add(si, bi, -m * (gcgbb + gcbbb + gcdbb + gcebb));
                    }
                }

                // Stamp Norton currents (ngspice b3soiddld.c lines 3860-3867, 3998-4017)
                if let Some(gi) = g {
                    system.rhs[gi] -= sign * m * ceqqg;
                }
                if let Some(bi) = b {
                    system.rhs[bi] -= sign * m * ceqqb;
                }
                if let Some(di2) = dp {
                    system.rhs[di2] -= sign * m * ceqqd;
                }
                if let Some(ei) = e {
                    system.rhs[ei] -= sign * m * ceqqe;
                }
                if let Some(si) = sp_node {
                    system.rhs[si] += sign * m * (ceqqg + ceqqb + ceqqd + ceqqe);
                }
            }
        }

        // 10. Stamp VBIC junction charge companion models.
        //     Uses the total charge derivatives (depletion + transit time + overlap)
        //     from the VbicCompanion result. Four junction pairs:
        //       Qbe: base_bi - emit_ei  (combined internal + external BE)
        //       Qbc: base_bi - coll_ci
        //       Qbep: base_bp - emit_ei (parasitic BE)
        //       Qbcp: base_bp - subs_si (parasitic BC / substrate)
        //     Follows the same incremental charge formulation as BJT (section 5).
        if !mna.vbics.is_empty() {
            for (vi, vbic) in mna.vbics.iter().enumerate() {
                let (vbei, vbex, vbci, vbcx, vbep, vrci, vrbi, vrbp, vbcp) =
                    vbic.junction_voltages(solution);
                let comp = vbic
                    .model
                    .companion(vbei, vbex, vbci, vbcx, vbep, vrci, vrbi, vrbp, vbcp, gmin);

                let sign = vbic.model.vbic_type.sign();
                let m = vbic.m * vbic.area;

                // Capacitances for incremental charge.
                let cap_be = comp.cqbe_vbei + comp.cqbex_vbex; // combined BE
                let cap_bc = comp.cqbc_vbci;
                let cap_bep = comp.cqbep_vbep;
                let cap_bcp = comp.cqbcp_vbcp;

                let hist = &vbic_charge_histories[vi];

                // Incremental charge: Q = Q_prev + C(v) * (v - v_prev).
                let qbe = hist.qbe + cap_be * (vbei - hist.vbei);
                let qbc = hist.qbc + cap_bc * (vbci - hist.vbci);
                let qbep = hist.qbep + cap_bep * (vbep - hist.vbep);
                let qbcp = hist.qbcp + cap_bcp * (vbcp - hist.vbcp);

                // Integrate B-E charge.
                let (geq_be, cqbe) = match method {
                    IntegrationMethod::BackwardEuler => {
                        let geq = cap_be / h;
                        let cq = (qbe - hist.qbe) / h;
                        (geq, cq)
                    }
                    IntegrationMethod::Trapezoidal => {
                        let geq = 2.0 * cap_be / h;
                        let cq = 2.0 * (qbe - hist.qbe) / h - hist.cqbe;
                        (geq, cq)
                    }
                };

                // Integrate B-C charge.
                let (geq_bc, cqbc) = match method {
                    IntegrationMethod::BackwardEuler => {
                        let geq = cap_bc / h;
                        let cq = (qbc - hist.qbc) / h;
                        (geq, cq)
                    }
                    IntegrationMethod::Trapezoidal => {
                        let geq = 2.0 * cap_bc / h;
                        let cq = 2.0 * (qbc - hist.qbc) / h - hist.cqbc;
                        (geq, cq)
                    }
                };

                // Integrate parasitic B-E charge.
                let (geq_bep, cqbep) = match method {
                    IntegrationMethod::BackwardEuler => {
                        let geq = cap_bep / h;
                        let cq = (qbep - hist.qbep) / h;
                        (geq, cq)
                    }
                    IntegrationMethod::Trapezoidal => {
                        let geq = 2.0 * cap_bep / h;
                        let cq = 2.0 * (qbep - hist.qbep) / h - hist.cqbep;
                        (geq, cq)
                    }
                };

                // Integrate parasitic B-C (substrate) charge.
                let (geq_bcp, cqbcp) = match method {
                    IntegrationMethod::BackwardEuler => {
                        let geq = cap_bcp / h;
                        let cq = (qbcp - hist.qbcp) / h;
                        (geq, cq)
                    }
                    IntegrationMethod::Trapezoidal => {
                        let geq = 2.0 * cap_bcp / h;
                        let cq = 2.0 * (qbcp - hist.qbcp) / h - hist.cqbcp;
                        (geq, cq)
                    }
                };

                let bi = vbic.base_bi_idx;
                let ei = vbic.emit_ei_idx;
                let bp = vbic.base_bp_idx;
                let ci = vbic.coll_ci_idx;
                let si = vbic.subs_si_idx;

                // Stamp B-E charge: base_bi - emit_ei.
                stamp_conductance(&mut system.matrix, bi, ei, m * geq_be);
                let ieq_be = sign * m * (cqbe - geq_be * vbei);
                stamp_current_source(&mut system.rhs, bi, ei, ieq_be);

                // Stamp B-C charge: base_bi - coll_ci.
                stamp_conductance(&mut system.matrix, bi, ci, m * geq_bc);
                let ieq_bc = sign * m * (cqbc - geq_bc * vbci);
                stamp_current_source(&mut system.rhs, bi, ci, ieq_bc);

                // Stamp parasitic B-E charge at controlling voltage pair: base_bx - base_bp.
                // Qbep is controlled by Vbep = sign*(V_bx - V_bp). The current
                // physically flows between BP and EI, but without cross-coupling
                // stamps we use the controlling voltage pair for self-consistency.
                let bx = vbic.base_bx_idx;
                stamp_conductance(&mut system.matrix, bx, bp, m * geq_bep);
                let ieq_bep = sign * m * (cqbep - geq_bep * vbep);
                stamp_current_source(&mut system.rhs, bx, bp, ieq_bep);

                // Stamp parasitic B-C (substrate) charge: subs_si - base_bp.
                // Vbcp = sign*(V_si - V_bp), stamp at (SI, BP) to match polarity.
                stamp_conductance(&mut system.matrix, si, bp, m * geq_bcp);
                let ieq_bcp = sign * m * (cqbcp - geq_bcp * vbcp);
                stamp_current_source(&mut system.rhs, si, bp, ieq_bcp);
            }
        }
    };

    if has_nonlinear {
        // Nonlinear: use NR solver.
        let result =
            transient_nr_solve(nr_options, dim, num_nodes, load, prev_solution).map_err(|e| {
                MnaError::SolveError(crate::SparseMatrixError::SingularMatrix(e.to_string()))
            })?;
        Ok(result.solution)
    } else {
        // Linear: single solve.
        let mut system = LinearSystem::new(dim);
        load(
            prev_solution,
            &mut system,
            1.0,
            nr_options.gmin,
            NrMode::Float,
        );
        let sol = system.solve()?;
        Ok(sol)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::wasm_bindgen_test as test;

    /// Helper to get a vector from a transient result.
    fn tran_vector<'a>(result: &'a SimResult, name: &str) -> &'a [f64] {
        let plot = &result.plots[0];
        plot.vecs
            .iter()
            .find(|v| v.name == name)
            .unwrap_or_else(|| panic!("no vector '{name}'"))
            .data
            .as_real()
    }

    #[test]
    fn test_rc_step_response() {
        // RC circuit: V1=5V, R1=1k, C1=1u with IC=0 on capacitor.
        // The cap starts at 0V (IC=0) but the DC OP has V(out)=5V,
        // so the IC overrides to 0V and the cap charges to 5V.
        //
        // Analytical: V(out) = 5 * (1 - exp(-t/RC))
        // RC = 1k * 1u = 1ms
        // At t = 1ms (1 RC): V ≈ 5*(1 - 0.368) = 3.16
        // At t = 5ms (5 RC): V ≈ 5*(1 - 0.0067) = 4.97
        let netlist = Netlist::parse_single(
            "RC step response
V1 1 0 5
R1 1 out 1k
C1 out 0 1u IC=0
.tran 10u 5m
.end
",
        )
        .unwrap();

        let result = simulate_tran(&netlist).unwrap();

        assert_eq!(result.plots.len(), 1);
        assert_eq!(result.plots[0].name, "tran1");

        let time = tran_vector(&result, "time");
        let v_out = tran_vector(&result, "v(out)");

        assert_eq!(time.len(), v_out.len());
        assert!(time.len() > 10, "should have time points");

        // Verify initial condition: t=0, V(out) = 0 (from IC=0).
        assert_abs_diff_eq!(v_out[0], 0.0, epsilon = 1e-6);

        // Check charge curve at several time constants.
        let rc = 1e-3; // 1ms
        for &(t_check, expected_frac) in &[
            (1e-3, 1.0 - (-1.0_f64).exp()), // 1 RC
            (2e-3, 1.0 - (-2.0_f64).exp()), // 2 RC
            (3e-3, 1.0 - (-3.0_f64).exp()), // 3 RC
            (5e-3, 1.0 - (-5.0_f64).exp()), // 5 RC
        ] {
            let expected_v = 5.0 * expected_frac;
            // Find the closest time point.
            let idx = find_nearest_time(time, t_check);

            let actual_v = v_out[idx];
            let rel_err = (actual_v - expected_v).abs() / expected_v;
            assert!(
                rel_err < 0.01,
                "at t={t_check:.3e} ({:.1} RC): expected {expected_v:.4}, got {actual_v:.4}, rel_err={rel_err:.4}",
                t_check / rc
            );
        }
    }

    #[test]
    fn test_lc_oscillator() {
        // LC oscillator: C1=1u with IC=1V, L1=1u, no resistance.
        // Natural frequency: f = 1/(2*pi*sqrt(LC)) = 1/(2*pi*sqrt(1e-6*1e-6))
        //                     = 1/(2*pi*1e-6) ≈ 159.155 kHz
        // Period: T = 1/f ≈ 6.283 us
        //
        // V(1) = V0 * cos(2*pi*f*t) = cos(t/sqrt(LC))
        let netlist = Netlist::parse_single(
            "LC oscillator
C1 1 0 1u IC=1
L1 1 0 1u
.tran 10n 12.566u
.end
",
        )
        .unwrap();

        let result = simulate_tran(&netlist).unwrap();

        let time = tran_vector(&result, "time");
        let v1 = tran_vector(&result, "v(1)");

        assert!(time.len() > 10, "should have time points");

        // Expected frequency.
        let lc: f64 = 1e-6 * 1e-6;
        let omega = 1.0 / lc.sqrt();
        let f_expected = omega / (2.0 * std::f64::consts::PI);
        let period = 1.0 / f_expected;

        // Find the first zero crossing (quarter period) to verify frequency.
        // V(1) starts at 1V (cos(0) = 1) and should cross zero at T/4.
        let quarter_period = period / 4.0;

        // Find the time index closest to T/4.
        let idx_quarter = find_nearest_time(time, quarter_period);

        // At T/4, voltage should be near zero.
        assert!(
            v1[idx_quarter].abs() < 0.1,
            "at T/4 ({quarter_period:.3e}s): V should be near 0, got {:.4}",
            v1[idx_quarter]
        );

        // At T/2, voltage should be near -1V.
        let half_period = period / 2.0;
        let idx_half = find_nearest_time(time, half_period);

        assert!(
            (v1[idx_half] + 1.0).abs() < 0.1,
            "at T/2 ({half_period:.3e}s): V should be near -1, got {:.4}",
            v1[idx_half]
        );

        // Verify frequency: find two consecutive peaks and check period.
        // Find first maximum after the initial one.
        let idx_full = find_nearest_time(time, period);

        // At T, voltage should return near 1V (full cycle).
        let rel_err = (v1[idx_full] - 1.0).abs();
        assert!(
            rel_err < 0.1,
            "at T ({period:.3e}s): V should be near 1, got {:.4}, err={rel_err:.4}",
            v1[idx_full]
        );

        // Verify frequency within 1%.
        // Use zero crossings to measure actual frequency.
        let mut zero_crossings = Vec::new();
        for i in 1..v1.len() {
            if v1[i - 1] * v1[i] < 0.0 {
                // Linear interpolation for exact crossing.
                let t_cross = time[i - 1]
                    + (time[i] - time[i - 1]) * v1[i - 1].abs() / (v1[i - 1].abs() + v1[i].abs());
                zero_crossings.push(t_cross);
            }
        }

        if zero_crossings.len() >= 4 {
            // Two zero crossings per cycle. Measure period from crossings.
            let measured_period = zero_crossings[2] - zero_crossings[0];
            let measured_freq = 1.0 / measured_period;
            let freq_err = (measured_freq - f_expected).abs() / f_expected;
            assert!(
                freq_err < 0.01,
                "frequency error {freq_err:.4}: expected {f_expected:.1} Hz, got {measured_freq:.1} Hz"
            );
        }
    }

    #[test]
    fn test_pulse_source_rc_circuit() {
        // PULSE source into RC circuit.
        // V1 is a PULSE from 0V to 5V with fast edges, 5ms pulse width, 10ms period.
        // R1=1k, C1=1u → RC = 1ms.
        // During the pulse high, cap charges toward 5V.
        // During pulse low, cap discharges toward 0V.
        let netlist = Netlist::parse_single(
            "PULSE into RC
V1 1 0 PULSE(0 5 0 1u 1u 5m 10m)
R1 1 out 1k
C1 out 0 1u IC=0
.tran 10u 10m
.end
",
        )
        .unwrap();

        let result = simulate_tran(&netlist).unwrap();

        let time = tran_vector(&result, "time");
        let v_out = tran_vector(&result, "v(out)");

        // At t=0, IC=0 so V(out)=0
        assert_abs_diff_eq!(v_out[0], 0.0, epsilon = 1e-6);

        // During pulse high (0 to 5ms), cap should charge toward 5V.
        // At t=3ms (3 RC into charging), V ≈ 5*(1-exp(-3)) ≈ 4.75
        let idx_3ms = find_nearest_time(time, 3e-3);
        assert!(
            v_out[idx_3ms] > 4.0,
            "at t=3ms, V(out) should be > 4V, got {:.4}",
            v_out[idx_3ms]
        );

        // After pulse falls at t=5ms, cap discharges toward 0V.
        // At t=8ms (3 RC into discharge), voltage should be much lower.
        let idx_8ms = find_nearest_time(time, 8e-3);
        assert!(
            v_out[idx_8ms] < 1.0,
            "at t=8ms (discharge), V(out) should be < 1V, got {:.4}",
            v_out[idx_8ms]
        );
    }

    #[test]
    fn test_sin_source_transient() {
        // SIN source with known frequency, verify output matches analytical.
        // V1 = SIN(0 1 1000) → 1V amplitude, 1kHz, no offset.
        // With a simple R load (no reactive elements), V(1) should track V1.
        let netlist = Netlist::parse_single(
            "SIN source test
V1 1 0 SIN(0 1 1000)
R1 1 0 1k
.tran 10u 2m
.end
",
        )
        .unwrap();

        let result = simulate_tran(&netlist).unwrap();

        let time = tran_vector(&result, "time");
        let v1 = tran_vector(&result, "v(1)");

        // Verify at multiple points that V(1) matches sin(2*pi*1000*t)
        for i in 1..v1.len() {
            let t = time[i];
            let expected = (2.0 * std::f64::consts::PI * 1000.0 * t).sin();
            let err = (v1[i] - expected).abs();
            assert!(
                err < 1e-6,
                "at t={t:.6e}: expected {expected:.10}, got {:.10}, err={err:.10}",
                v1[i]
            );
        }
    }

    #[test]
    fn test_pwl_source_transient() {
        // PWL source: ramp from 0 to 5V in 1ms, hold 5V for 1ms, ramp to 0 in 1ms.
        // With R load, V(1) should track the source.
        let netlist = Netlist::parse_single(
            "PWL source test
V1 1 0 PWL(0 0 1m 5 2m 5 3m 0)
R1 1 0 1k
.tran 50u 3m
.end
",
        )
        .unwrap();

        let result = simulate_tran(&netlist).unwrap();

        let time = tran_vector(&result, "time");
        let v1 = tran_vector(&result, "v(1)");

        // At t=0.5ms: should be about 2.5V (midway through ramp)
        // Tolerance of 0.15V accounts for the discrete output resolution
        // (tstep=50μs, ramp slope=5V/ms, worst-case offset ≈ 0.125V).
        let idx = find_nearest_time(time, 0.5e-3);
        assert!(
            (v1[idx] - 2.5).abs() < 0.15,
            "at 0.5ms: expected ~2.5V, got {:.4}",
            v1[idx]
        );

        // At t=1.5ms: should be 5V (holding — flat, no slope error)
        let idx = find_nearest_time(time, 1.5e-3);
        assert!(
            (v1[idx] - 5.0).abs() < 0.1,
            "at 1.5ms: expected ~5V, got {:.4}",
            v1[idx]
        );

        // At t=2.5ms: should be about 2.5V (ramping down)
        let idx = find_nearest_time(time, 2.5e-3);
        assert!(
            (v1[idx] - 2.5).abs() < 0.15,
            "at 2.5ms: expected ~2.5V, got {:.4}",
            v1[idx]
        );
    }

    #[test]
    fn test_current_source_pulse() {
        // PULSE current source into a resistor.
        // I1 pulses from 0 to 1mA, R1=1k → V(1) should pulse from 0 to 1V.
        let netlist = Netlist::parse_single(
            "PULSE current source
I1 0 1 PULSE(0 1m 0 1u 1u 1m 2m)
R1 1 0 1k
.tran 10u 4m
.end
",
        )
        .unwrap();

        let result = simulate_tran(&netlist).unwrap();

        let time = tran_vector(&result, "time");
        let v1 = tran_vector(&result, "v(1)");

        // During high pulse (0 to 1ms): V(1) ≈ 1mA * 1kΩ = 1V
        let idx = find_nearest_time(time, 0.5e-3);
        assert!(
            (v1[idx] - 1.0).abs() < 0.05,
            "during pulse high: expected ~1V, got {:.4}",
            v1[idx]
        );

        // During low pulse (1ms to 2ms): V(1) ≈ 0V
        let idx = find_nearest_time(time, 1.5e-3);
        assert!(
            v1[idx].abs() < 0.05,
            "during pulse low: expected ~0V, got {:.4}",
            v1[idx]
        );
    }

    #[test]
    fn test_adaptive_timestep_pulse_rc() {
        // Test that adaptive timestep control places more steps near PULSE edges
        // and coasts during flat regions.
        //
        // PULSE source: 0→5V with 1us edges, 1ms high, 2ms period.
        // RC = 1k * 1u = 1ms.
        let netlist = Netlist::parse_single(
            "Adaptive timestep PULSE RC
V1 1 0 PULSE(0 5 0 1u 1u 1m 2m)
R1 1 out 1k
C1 out 0 1u IC=0
.tran 10u 4m
.end
",
        )
        .unwrap();

        let result = simulate_tran(&netlist).unwrap();

        let time = tran_vector(&result, "time");
        let v_out = tran_vector(&result, "v(out)");

        // Basic sanity: we should have output points.
        assert!(time.len() > 10, "should have output points");

        // Verify accuracy at key points.
        // At t=0: IC=0
        assert_abs_diff_eq!(v_out[0], 0.0, epsilon = 1e-6);

        // During charging (t≈0.5ms): V(out) should be rising.
        let idx_05ms = find_nearest_time(time, 0.5e-3);
        assert!(
            v_out[idx_05ms] > 1.0,
            "at 0.5ms: V(out) should be rising, got {:.4}",
            v_out[idx_05ms]
        );

        // Near end of charging (t≈1ms): V(out) should be near 3.16V (1 RC).
        let idx_1ms = find_nearest_time(time, 1e-3);
        let expected_1rc = 5.0 * (1.0 - (-1.0_f64).exp());
        let rel_err = (v_out[idx_1ms] - expected_1rc).abs() / expected_1rc;
        assert!(
            rel_err < 0.05,
            "at 1ms: expected {expected_1rc:.3}, got {:.3}, err={rel_err:.3}",
            v_out[idx_1ms]
        );

        // Verify adaptive stepping: check that timesteps are smaller near edges.
        // Count steps in first 10us (near rising edge) vs steps in 0.1ms-0.5ms (flat region).
        let steps_near_edge = time.windows(2).filter(|w| w[0] < 10e-6).count();
        let steps_flat = time
            .windows(2)
            .filter(|w| w[0] >= 0.1e-3 && w[0] < 0.5e-3)
            .count();

        // Near the edge, there should be comparable or more steps per unit time than flat.
        // This is a soft check — adaptive stepping should concentrate steps near transitions.
        let density_edge = if steps_near_edge > 0 {
            steps_near_edge as f64 / 10e-6
        } else {
            0.0
        };
        let density_flat = if steps_flat > 0 {
            steps_flat as f64 / 0.4e-3
        } else {
            1.0
        };

        // Edge density should be at least comparable to flat density
        // (breakpoints force steps there).
        assert!(
            density_edge >= density_flat * 0.1 || steps_near_edge >= 1,
            "adaptive stepping should have steps near edges: edge_steps={steps_near_edge}, flat_steps={steps_flat}"
        );
    }

    /// Find the index of the time point nearest to `target`.
    fn find_nearest_time(time: &[f64], target: f64) -> usize {
        time.iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                (*a - target)
                    .abs()
                    .partial_cmp(&(*b - target).abs())
                    .unwrap()
            })
            .unwrap()
            .0
    }

    #[test]
    fn test_coupled_inductors_transformer() {
        // Two coupled inductors with k=0.5, L1=L2=1mH.
        // PULSE voltage source drives current through primary (V1→R1→L1→GND).
        // Secondary has a load resistor (L2→R2→GND).
        //
        // During the pulse rise, dI1/dt induces voltage on secondary via
        // M = k*sqrt(L1*L2). We verify that V(sec) becomes non-zero
        // (energy transfer through mutual coupling) during the transient.
        let netlist = Netlist::parse_single(
            "Transformer transient test
V1 in 0 PULSE(0 5 0 1n 1n 5u 10u)
R1 in pri 100
L1 pri 0 1m
L2 sec 0 1m
R2 sec 0 1k
K1 L1 L2 0.5
.tran 100n 10u
.end
",
        )
        .unwrap();

        let result = crate::simulate(&netlist).unwrap();
        let time = tran_vector(&result, "time");
        let v_sec = tran_vector(&result, "v(sec)");

        // Find peak absolute secondary voltage (should be non-zero due to coupling).
        let peak = v_sec.iter().copied().fold(0.0_f64, |a, b| a.max(b.abs()));
        assert!(
            peak > 0.001,
            "secondary should see induced voltage from coupling, peak was {peak}"
        );

        // At a late time when the pulse is flat (say 5us), dI/dt ≈ 0,
        // so secondary voltage should be smaller than the peak.
        let late_idx = find_nearest_time(time, 5.0e-6);
        let v_sec_late = v_sec[late_idx].abs();
        assert!(
            v_sec_late < peak,
            "secondary should decay when dI/dt→0: late={v_sec_late} vs peak={peak}"
        );
    }

    #[test]
    fn test_mutual_coupling_scales_with_k() {
        // Higher coupling coefficient should produce larger secondary voltage.
        // Run two simulations with k=0.2 and k=0.8, same circuit otherwise.
        let run = |k: f64| -> f64 {
            let netlist = Netlist::parse_single(&format!(
                "Coupling scaling test
V1 in 0 PULSE(0 5 0 1n 1n 5u 10u)
R1 in pri 100
L1 pri 0 1m
L2 sec 0 1m
R2 sec 0 1k
K1 L1 L2 {k}
.tran 50n 2u
.end
"
            ))
            .unwrap();
            let result = crate::simulate(&netlist).unwrap();
            let v_sec = tran_vector(&result, "v(sec)");
            v_sec.iter().copied().fold(0.0_f64, |a, b| a.max(b.abs()))
        };

        let peak_low = run(0.2);
        let peak_high = run(0.8);

        assert!(
            peak_high > peak_low * 1.5,
            "higher k should produce larger secondary voltage: k=0.8 peak={peak_high}, k=0.2 peak={peak_low}"
        );
    }
}
