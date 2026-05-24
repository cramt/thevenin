//! Thevenin — a Rust circuit simulator (ngspice-compatible).
//!
//! # Quick start
//!
//! ```rust
//! use thevenin::simulate;
//! use thevenin_types::Netlist;
//!
//! let netlist = Netlist::parse_single("
//! Voltage Divider
//! V1 in 0 1.0
//! R1 in mid 1k
//! R2 mid 0 2k
//! .op
//! .end
//! ").unwrap();
//!
//! let result = simulate(&netlist).unwrap();
//! let vmid = result["v(mid)"].data.as_real()[0];
//! assert!((vmid - 0.6667).abs() < 0.001);
//! ```

use thevenin_types::{Expr, Item, Netlist};

/// Parse a numeric expression value, returning an error if it's not a literal number.
pub(crate) fn expr_val(expr: &Expr, context: &str) -> Result<f64, mna::MnaError> {
    match expr {
        Expr::Num(v) => Ok(*v),
        _ => Err(MnaError::NonNumericValue {
            element: context.to_string(),
        }),
    }
}

/// Parse a numeric expression value, returning a default if it's not a literal number.
pub(crate) fn expr_val_or(expr: &Expr, default: f64) -> f64 {
    match expr {
        Expr::Num(v) => *v,
        _ => default,
    }
}

// ── Solver internals ────────────────────────────────────────────────────────
pub(crate) mod device_stamp;
pub mod mna;
pub mod mna_ir;
pub(crate) mod newton;
pub(crate) mod physics;
pub(crate) mod simulate;
pub mod sparse;
pub(crate) mod waveform;

// ── Analysis modules ────────────────────────────────────────────────────────
pub(crate) mod ac;
pub mod fourier;
pub(crate) mod measure;
pub(crate) mod noise;
pub(crate) mod pz;
pub(crate) mod sens;
pub(crate) mod tf;
pub(crate) mod transient;

// ── Device models (pub for ongoing development) ─────────────────────────────
pub mod bjt;
pub mod bsim3;
pub mod bsim3soi_dd;
pub mod bsim3soi_fd;
pub mod bsim3soi_pd;
pub mod bsim4;
pub mod cpl;
pub mod diode;
pub mod hfet;
pub mod jfet;
pub mod ltra;
pub mod mesa;
pub mod mesfet;
pub mod mos2;
pub mod mos3;
pub mod mos6;
pub mod mosfet;
pub mod switch;
pub mod tline;
pub mod txl;
pub mod vbic;
pub mod vdmos;

// ── Public modules (used by thevenin-control and test harness) ──────────────
pub mod circuit;
pub mod expr;
pub mod libproc;
pub mod output;
pub mod raw_output;
pub mod subckt;

// ── Crate-internal re-exports (used across internal modules via `crate::`) ───
pub(crate) use mna::stamp_conductance;
pub(crate) use sparse::{LinearSystem, SparseMatrix, SparseMatrixError};

// ── Public API ──────────────────────────────────────────────────────────────
//
// Stage 4 of the Cirq adoption plan retired the Netlist-shaped simulator
// surface; the public entry points now live under [`thevenin::circuit`]
// and operate on `cirq_ir::Circuit`. The Netlist-shaped `simulate_*` and
// `simulate_*_with_mna` helpers stay `pub(crate)` so internal modules
// (notably `mna_ir`, `noise`, and `circuit`) can still dispatch through
// them. New callers should use `thevenin::circuit::simulate_*` directly.

// Analysis functions — Netlist-shaped, crate-internal.
//
// `_with_mna` variants stay accessible to their sibling modules via
// `crate::ac::simulate_ac_with_mna` etc., so we don't re-export them here.
// Only the bare `simulate_*` Netlist entrypoints are re-exported because
// they're called from within the crate (notably by `circuit::*` as the
// post-IR-stamp fallback path).
pub(crate) use ac::simulate_ac;
pub(crate) use noise::simulate_noise;
pub(crate) use pz::simulate_pz;
pub(crate) use sens::simulate_sens;
pub(crate) use simulate::{simulate_dc, simulate_op};
pub(crate) use tf::simulate_tf;
pub(crate) use transient::simulate_tran;
pub use transient::{TranOutcome, TranPauseSnapshot, TranRunParams, TranStartState, run_tran};

// Utilities
pub use mna::MnaError;
pub use subckt::flatten_netlist;

/// Extract simulation temperature from a netlist (from `.temp` directive or `.options temp=`).
/// Returns 27.0°C (room temperature) as default.
///
/// When multiple `.temp` directives exist, returns the last one (last-wins
/// semantics for single-temperature queries — use [`netlist_temps`] for
/// multi-temperature sweeps).
pub fn netlist_temp(netlist: &Netlist) -> f64 {
    let mut temp_c = 27.0_f64;
    for item in &netlist.items {
        match item {
            Item::Temp(t) => temp_c = *t,
            Item::Options(params) => {
                for p in params {
                    if let Expr::Num(v) = &p.value
                        && p.name.eq_ignore_ascii_case("TEMP")
                    {
                        temp_c = *v;
                    }
                }
            }
            _ => {}
        }
    }
    temp_c
}

/// Extract all simulation temperatures from a netlist.
///
/// Returns every `.temp` value found. When multiple temperatures are
/// specified (e.g. `.temp 25 50 100`), the simulation should be run at each.
/// Returns an empty vec if no `.temp` directive exists (caller should use
/// the default 27°C).
pub fn netlist_temps(netlist: &Netlist) -> Vec<f64> {
    let mut temps = Vec::new();
    for item in &netlist.items {
        if let Item::Temp(t) = item {
            temps.push(*t);
        }
    }
    temps
}

/// Extract nominal temperature (TNOM) from `.options` in Kelvin.
/// Defaults to 300.15K (27°C).
pub fn netlist_tnom(netlist: &Netlist) -> f64 {
    let mut tnom_c = 27.0_f64;
    for item in &netlist.items {
        if let Item::Options(params) = item {
            for p in params {
                if let Expr::Num(v) = &p.value
                    && p.name.eq_ignore_ascii_case("TNOM")
                {
                    tnom_c = *v;
                }
            }
        }
    }
    tnom_c + 273.15
}

/// Run the single analysis in the netlist and return the result.
///
/// Each `Netlist` contains exactly one analysis command. This function
/// dispatches to the appropriate simulator based on that analysis.
///
/// When multiple `.temp` directives are present, the simulation is run once
/// per temperature and results are collected into a single `SimResult` with
/// a `"temperature"` sweep variable.
///
/// Test-only post Stage 4 — kept around so internal unit tests in `ac.rs`
/// and `transient.rs` can keep calling the Netlist-shaped flow. Public
/// callers should use [`thevenin::circuit::simulate`].
#[cfg(test)]
pub(crate) fn simulate(netlist: &Netlist) -> Result<thevenin_types::SimResult, MnaError> {
    let temps = netlist_temps(netlist);
    let mut result = if temps.len() > 1 {
        simulate_multi_temp(netlist, &temps)?
    } else {
        simulate_single(netlist)?
    };

    // Evaluate .meas directives against the simulation results.
    measure::evaluate_measurements(netlist, &mut result);

    Ok(result)
}

/// Dispatch to the appropriate simulator for a single analysis run.
#[cfg(test)]
fn simulate_single(netlist: &Netlist) -> Result<thevenin_types::SimResult, MnaError> {
    use thevenin_types::Analysis;
    match &netlist.analysis {
        Analysis::Op => simulate_op(netlist),
        Analysis::Dc { .. } => simulate_dc(netlist),
        Analysis::Ac { .. } => simulate_ac(netlist),
        Analysis::Tran { .. } => simulate_tran(netlist),
        Analysis::Noise { .. } => simulate_noise(netlist),
        Analysis::Sens { .. } => simulate_sens(netlist),
        Analysis::Tf { .. } => simulate_tf(netlist),
        Analysis::Pz { .. } => simulate_pz(netlist),
        // Fourier post-processing has no Netlist-shape simulator entry —
        // the Netlist-side dispatch is test-only and dropped after Stage 4.
        // Callers exercising .four/.fft should use the Circuit-based API.
        Analysis::Four { .. } | Analysis::Fft { .. } => Err(MnaError::UnsupportedElement(
            ".four/.fft are only supported via thevenin::circuit::simulate".to_string(),
        )),
    }
}

/// Run the analysis at each temperature, collecting results.
///
/// Creates a modified netlist for each temperature (stripping all `.temp`
/// items and inserting a single one), runs the simulation, and produces one
/// plot per temperature in the result.
#[cfg(test)]
fn simulate_multi_temp(
    netlist: &Netlist,
    temps: &[f64],
) -> Result<thevenin_types::SimResult, MnaError> {
    use thevenin_types::SimResult;
    let mut plots = Vec::with_capacity(temps.len());

    for (i, &temp) in temps.iter().enumerate() {
        // Build a netlist with only this temperature.
        let mut single_temp_netlist = netlist.clone();
        single_temp_netlist
            .items
            .retain(|item| !matches!(item, Item::Temp(_)));
        single_temp_netlist.items.push(Item::Temp(temp));

        let result = simulate_single(&single_temp_netlist)?;

        // Rename each plot to include the temperature and index.
        for mut plot in result.plots {
            plot.name = format!("{}_temp{}_{}", plot.name, i + 1, temp);
            plots.push(plot);
        }
    }

    Ok(SimResult { plots })
}
