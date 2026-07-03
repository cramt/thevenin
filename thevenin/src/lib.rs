//! Thevenin — a circuit simulator in Rust, compatible with
//! [ngspice](https://ngspice.sourceforge.io/).
//!
//! Thevenin runs SPICE-style analyses — DC operating point, DC sweep, AC
//! small-signal, transient, noise, sensitivity, transfer function, pole-zero,
//! and Fourier/FFT post-processing — over the same algorithms ngspice uses
//! (Modified Nodal Analysis, Newton–Raphson, sparse direct solve), with Rust's
//! type safety and first-class `wasm32` support.
//!
//! The canonical input is a [`cirq_ir::Circuit`] — a name-resolved,
//! parameter-evaluated circuit. The simulation entry points live in
//! [`circuit`]; [`circuit::simulate`] is the usual one.
//!
//! # Quick start
//!
//! ```
//! use thevenin::circuit::simulate;
//!
//! // Compile a Cirq-source circuit to IR, then simulate it.
//! let circuit = cirq_frontend::compile(
//!     "circuit divider {
//!          V1: vsource(in -> gnd, dc: 1.0)
//!          R1: resistor(in -> mid, 1k)
//!          R2: resistor(mid -> gnd, 2k)
//!          analysis op {}
//!      }",
//! )
//! .expect("compiles");
//!
//! let result = simulate(&circuit).expect("simulates");
//! let vmid = result["v(mid)"].data.as_real()[0];
//! assert!((vmid - 0.6667).abs() < 1e-3);
//! ```
//!
//! # Getting a `Circuit`
//!
//! - **From Cirq source** — [`cirq_frontend::compile`].
//! - **From a SPICE netlist** — [`cirq_spice_import`](https://docs.rs/cirq-spice-import),
//!   or use the [`thevenin-cirq`](https://docs.rs/thevenin-cirq) convenience
//!   crate's `simulate_spice_*` helpers to parse and simulate in one call.
//!
//! # What's supported
//!
//! Passive R/L/C/K; independent and dependent sources (V, I, E, G, H, F);
//! behavioural `B` sources; diodes; BJTs (Gummel-Poon, VBIC); a broad MOSFET
//! family (Levels 1/2/3/6, BSIM1–4, BSIM3SOI, HiSIM, VDMOS); JFET/MESFET/HFET;
//! transmission lines (LTRA/TXL/CPL/ideal); switches; XSPICE code models; and
//! nested subcircuits. See the [crate
//! README](https://github.com/cramt/thevenin#readme) for the full matrix and
//! known gaps.
//!
//! # Module map
//!
//! - [`circuit`] — the public simulation surface (start here).
//! - [`mna_ir`] — Modified Nodal Analysis assembly from the IR `Circuit`.
//!   [`mna`] holds the shared `MnaSystem` / device-instance types and stamping
//!   helpers it builds on.
//! - [`raw_output`] — ngspice raw-file / CSV result writers.

use thevenin_types::{Expr, Item, Netlist};

// ── Solver internals ────────────────────────────────────────────────────────
pub(crate) mod device_stamp;
pub mod mna;
pub mod mna_ir;
pub mod model_params;
pub(crate) mod newton;
pub(crate) mod physics;
pub(crate) mod simulate;
pub mod sparse;
#[cfg(test)]
mod test_support;
pub mod waveform;

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
pub mod bsim1;
pub mod bsim2;
pub mod bsim3;
pub mod bsim3soi_dd;
pub mod bsim3soi_fd;
pub mod bsim3soi_pd;
pub mod bsim4;
pub mod cpl;
pub mod diode;
pub mod hfet;
pub mod hisim;
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
// The simulator's only input is `cirq_ir::Circuit`; the public entry points
// live under [`thevenin::circuit`]. The legacy Netlist stamping path
// (`assemble_mna(&Netlist)` + the `simulate_*(&Netlist)` wrappers) has been
// removed — every analysis assembles MNA directly from the IR via
// [`mna_ir::assemble_mna_from_circuit`]. The `simulate_*_with_mna` helpers
// (which take a pre-assembled `MnaSystem`) remain accessible to sibling
// modules via their defining module (`crate::ac::simulate_ac_with_mna`, …).
pub use transient::{
    IntegrationMethod, TranOutcome, TranPauseSnapshot, TranRunParams, TranStartState,
    integration_method_from_netlist, parse_integration_method, run_tran,
};

// Utilities
pub use measure::evaluate_circuit_measures;
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
