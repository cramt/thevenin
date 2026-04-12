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

use thevenin_types::{Analysis, Expr, Item, Netlist, SimResult};

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
pub(crate) mod mna;
pub(crate) mod newton;
pub(crate) mod physics;
pub(crate) mod simulate;
pub(crate) mod sparse;
pub(crate) mod waveform;

// ── Analysis modules ────────────────────────────────────────────────────────
pub(crate) mod ac;
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
pub mod mos6;
pub mod mosfet;
pub mod txl;
pub mod vbic;

// ── Public modules (used by thevenin-control and test harness) ──────────────
pub mod expr;
pub mod libproc;
pub mod output;
pub mod subckt;

// ── Crate-internal re-exports (used across internal modules via `crate::`) ───
pub(crate) use mna::stamp_conductance;
pub(crate) use sparse::{LinearSystem, SparseMatrix, SparseMatrixError};

// ── Public API ──────────────────────────────────────────────────────────────

// Analysis functions
pub use ac::simulate_ac;
pub use noise::simulate_noise;
pub use pz::simulate_pz;
pub use sens::simulate_sens;
pub use simulate::{simulate_dc, simulate_op, simulate_op_dc, simulate_op_with_xspice};
pub use tf::simulate_tf;
pub use transient::simulate_tran;

// Utilities
pub use mna::MnaError;
pub use simulate::nr_options_from_netlist;
pub use subckt::flatten_netlist;

/// Extract simulation temperature from a netlist (from `.temp` directive or `.options temp=`).
/// Returns 27.0°C (room temperature) as default.
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
pub fn simulate(netlist: &Netlist) -> Result<SimResult, MnaError> {
    match &netlist.analysis {
        Analysis::Op => simulate_op(netlist),
        Analysis::Dc { .. } => simulate_dc(netlist),
        Analysis::Ac { .. } => simulate_ac(netlist),
        Analysis::Tran { .. } => simulate_tran(netlist),
        Analysis::Noise { .. } => simulate_noise(netlist),
        Analysis::Sens { .. } => simulate_sens(netlist),
        Analysis::Tf { .. } => simulate_tf(netlist),
        Analysis::Pz { .. } => simulate_pz(netlist),
    }
}
