//! Shared test helpers that bridge the SPICE-shaped `Netlist` test fixtures
//! to the public `thevenin::circuit::simulate_*(&Circuit)` surface.
//!
//! The Netlist-shaped `thevenin::simulate_*(&Netlist)` entry points are
//! `pub(crate)` after the Stage 4 IR migration; integration tests in
//! `thevenin/tests/` use this module to keep their existing
//! `Netlist::parse_single(spice) → simulate` flow working without
//! depending on the demoted internal APIs.
//!
//! Pattern: each helper imports the parsed Netlist into a Cirq IR Circuit,
//! sets the appropriate analysis (overriding any parsed analysis when the
//! caller selected one explicitly), and dispatches through the public
//! Circuit-input surface.
//!
//! Note: integration tests live in separate binaries, so this module is
//! included via `mod common;` in each test file rather than re-exported
//! from a library.

#![allow(dead_code)]

use cirq_ir::Analysis;
use cirq_spice_import::import_netlist;
use thevenin_types::{Netlist, SimResult};

fn lift(netlist: &Netlist, override_analysis: Option<Analysis>) -> cirq_ir::Circuit {
    // The IR importer's mini-evaluator doesn't cover the full SPICE
    // expression grammar (no `**`, no ternaries, no TEMPER), so resolve
    // params and brace expressions via thevenin's evaluator first. This
    // matches what the Netlist-shape simulator entry points used to do
    // internally.
    let mut resolved = netlist.clone();
    thevenin::expr::resolve_netlist_exprs(&mut resolved).expect("resolve_netlist_exprs");
    let mut circuit = import_netlist(&resolved).expect("import_netlist");
    if let Some(a) = override_analysis {
        circuit.analyses = vec![a];
    }
    circuit
}

/// Run a DC operating-point solve. Overrides any declared `.tran` / `.ac`
/// analysis with `.op` to match the Netlist-shape's permissive `simulate_op`,
/// which ignored the netlist's declared analysis.
pub fn simulate_op(netlist: &Netlist) -> SimResult {
    let c = lift(netlist, Some(Analysis::Op));
    thevenin::circuit::simulate_op(&c).expect("circuit::simulate_op")
}

/// Run a DC sweep. Requires the netlist to declare `.dc`.
pub fn simulate_dc(netlist: &Netlist) -> SimResult {
    let c = lift(netlist, None);
    thevenin::circuit::simulate_dc(&c).expect("circuit::simulate_dc")
}

/// `simulate_dc` variant that returns the underlying error instead of
/// panicking. Used by exploratory tests that probe the convergence
/// boundary of a sweep.
pub fn try_simulate_dc(netlist: &Netlist) -> Result<SimResult, String> {
    let c = lift(netlist, None);
    thevenin::circuit::simulate_dc(&c).map_err(|e| e.to_string())
}

/// `simulate_op` variant that returns the underlying error instead of
/// panicking. Used by tests that assert convergence behaviour explicitly.
pub fn try_simulate_op(netlist: &Netlist) -> Result<SimResult, String> {
    let c = lift(netlist, Some(Analysis::Op));
    thevenin::circuit::simulate_op(&c).map_err(|e| e.to_string())
}

/// Run a transient analysis. Requires the netlist to declare `.tran`.
///
/// `thevenin::circuit::simulate_tran` prepends an OP solve to the result
/// plots (matching the SPICE convention of seeding the transient with the
/// DC operating point). The Netlist-shape `thevenin::simulate_tran` used
/// to *not* emit that OP plot; we strip it here so existing tests that
/// expect `plots[0].name == "tran1"` keep working.
pub fn simulate_tran(netlist: &Netlist) -> SimResult {
    let c = lift(netlist, None);
    let mut result = thevenin::circuit::simulate_tran(&c).expect("circuit::simulate_tran");
    result.plots.retain(|p| !p.name.starts_with("op"));
    result
}

/// Run an AC small-signal sweep. Requires the netlist to declare `.ac`.
pub fn simulate_ac(netlist: &Netlist) -> SimResult {
    let c = lift(netlist, None);
    thevenin::circuit::simulate_ac(&c).expect("circuit::simulate_ac")
}

/// Run a sensitivity analysis. Requires the netlist to declare `.sens`.
pub fn simulate_sens(netlist: &Netlist) -> SimResult {
    let c = lift(netlist, None);
    thevenin::circuit::simulate_sens(&c).expect("circuit::simulate_sens")
}

/// Run a noise analysis. Requires the netlist to declare `.noise`.
pub fn simulate_noise(netlist: &Netlist) -> SimResult {
    let c = lift(netlist, None);
    thevenin::circuit::simulate_noise(&c).expect("circuit::simulate_noise")
}

/// Run a pole-zero analysis. Requires the netlist to declare `.pz`.
pub fn simulate_pz(netlist: &Netlist) -> SimResult {
    let c = lift(netlist, None);
    thevenin::circuit::simulate_pz(&c).expect("circuit::simulate_pz")
}

/// Run a transfer function analysis. Requires the netlist to declare `.tf`.
pub fn simulate_tf(netlist: &Netlist) -> SimResult {
    let c = lift(netlist, None);
    thevenin::circuit::simulate_tf(&c).expect("circuit::simulate_tf")
}

/// Run every declared analysis on the netlist via the Circuit-input
/// top-level dispatcher. Equivalent to the Netlist-shape's
/// `thevenin::simulate(&Netlist)`.
pub fn simulate(netlist: &Netlist) -> SimResult {
    let c = lift(netlist, None);
    thevenin::circuit::simulate(&c).expect("circuit::simulate")
}
