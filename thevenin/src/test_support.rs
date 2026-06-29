//! Crate-internal test helpers bridging SPICE `Netlist` fixtures to the
//! IR-native simulation path
//! (`cirq_spice_import::import_netlist` → `cirq_ir::Circuit` →
//! `mna_ir::assemble_mna_from_circuit` / `circuit::simulate_*`).
//!
//! This is the in-crate twin of `thevenin/tests/common/mod.rs`: unit tests in
//! `#[cfg(test)]` modules use it so they exercise the same Circuit-input path
//! the public API uses, instead of the legacy `assemble_mna(&Netlist)` /
//! `simulate_*(&Netlist)` stamping path that is being retired. Results and
//! errors are mapped back to `MnaError` so the migrated tests keep their shape.

use std::sync::Arc;

use cirq_ir::Analysis;
use thevenin_types::{Netlist, SimResult};
use thevenin_xspice::CodeModelRegistry;

use crate::circuit::CircuitSimError;
use crate::mna::{MnaError, MnaSystem};

fn circ_err_to_mna(e: CircuitSimError) -> MnaError {
    match e {
        CircuitSimError::Mna(m) => m,
        other => MnaError::UnsupportedElement(other.to_string()),
    }
}

/// Resolve `.param` / brace expressions, then import the Netlist to a Cirq IR
/// Circuit. Mirrors `tests/common/mod.rs::lift`; both halves are surfaced as
/// `MnaError` so import/resolve failures propagate like an assembly error.
fn lift(netlist: &Netlist) -> Result<cirq_ir::Circuit, MnaError> {
    let mut resolved = netlist.clone();
    crate::expr::resolve_netlist_exprs(&mut resolved)
        .map_err(|e| MnaError::UnsupportedElement(format!("resolve_netlist_exprs: {e}")))?;
    cirq_spice_import::import_netlist(&resolved)
        .map_err(|e| MnaError::UnsupportedElement(format!("import_netlist: {e:?}")))
}

fn lift_with(netlist: &Netlist, analysis: Option<Analysis>) -> Result<cirq_ir::Circuit, MnaError> {
    let mut c = lift(netlist)?;
    if let Some(a) = analysis {
        c.analyses = vec![a];
    }
    Ok(c)
}

/// Assemble an `MnaSystem` from a Netlist fixture via the IR path — the
/// drop-in replacement for `crate::mna::assemble_mna(&netlist)`.
pub(crate) fn assemble_ir(netlist: &Netlist) -> Result<MnaSystem, MnaError> {
    let circuit = lift(netlist)?;
    crate::mna_ir::assemble_mna_from_circuit(&circuit, false, None)?
        .ok_or_else(|| MnaError::UnsupportedElement("circuit not representable in mna_ir".into()))
}

/// Operating-point solve via the Circuit path (replaces
/// `simulate_op(&netlist)`). The declared analysis is overridden with `.op`,
/// matching the legacy `simulate_op`'s permissive behaviour.
pub(crate) fn op(netlist: &Netlist) -> Result<SimResult, MnaError> {
    let c = lift_with(netlist, Some(Analysis::Op))?;
    crate::circuit::simulate_op(&c).map_err(circ_err_to_mna)
}

/// OP solve with an XSPICE code-model registry (replaces
/// `simulate_op_with_xspice(&netlist, registry)`).
pub(crate) fn op_with_xspice(
    netlist: &Netlist,
    registry: Arc<CodeModelRegistry>,
) -> Result<SimResult, MnaError> {
    let c = lift_with(netlist, Some(Analysis::Op))?;
    crate::circuit::simulate_op_with_xspice(&c, registry).map_err(circ_err_to_mna)
}

/// DC sweep via the Circuit path (replaces `simulate_dc(&netlist)`).
pub(crate) fn dc(netlist: &Netlist) -> Result<SimResult, MnaError> {
    let c = lift(netlist)?;
    crate::circuit::simulate_dc(&c).map_err(circ_err_to_mna)
}

/// AC sweep via the Circuit path (replaces `simulate_ac(&netlist)`).
pub(crate) fn ac(netlist: &Netlist) -> Result<SimResult, MnaError> {
    let c = lift(netlist)?;
    crate::circuit::simulate_ac(&c).map_err(circ_err_to_mna)
}

/// Transient via the Circuit path (replaces `simulate_tran(&netlist)`). The
/// Circuit path prepends an OP plot (SPICE convention); the legacy
/// `simulate_tran` did not, so strip it to keep `plots[0]` the transient.
pub(crate) fn tran(netlist: &Netlist) -> Result<SimResult, MnaError> {
    let c = lift(netlist)?;
    let mut r = crate::circuit::simulate_tran(&c).map_err(circ_err_to_mna)?;
    r.plots.retain(|p| !p.name.starts_with("op"));
    Ok(r)
}

/// Noise analysis via the Circuit path (replaces `simulate_noise(&netlist)`).
pub(crate) fn noise(netlist: &Netlist) -> Result<SimResult, MnaError> {
    let c = lift(netlist)?;
    crate::circuit::simulate_noise(&c).map_err(circ_err_to_mna)
}

/// Pole-zero analysis via the Circuit path (replaces `simulate_pz(&netlist)`).
pub(crate) fn pz(netlist: &Netlist) -> Result<SimResult, MnaError> {
    let c = lift(netlist)?;
    crate::circuit::simulate_pz(&c).map_err(circ_err_to_mna)
}

/// Transfer-function analysis via the Circuit path (replaces
/// `simulate_tf(&netlist)`).
pub(crate) fn tf(netlist: &Netlist) -> Result<SimResult, MnaError> {
    let c = lift(netlist)?;
    crate::circuit::simulate_tf(&c).map_err(circ_err_to_mna)
}

/// Sensitivity analysis via the Circuit path (replaces
/// `simulate_sens(&netlist)`).
pub(crate) fn sens(netlist: &Netlist) -> Result<SimResult, MnaError> {
    let c = lift(netlist)?;
    crate::circuit::simulate_sens(&c).map_err(circ_err_to_mna)
}
