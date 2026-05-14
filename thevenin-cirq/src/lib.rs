//! Cirq IR → Thevenin simulation entry points (SPICE-source convenience).
//!
//! The **Circuit-shaped API** for Stage 4 now lives in [`thevenin::circuit`]
//! directly — `thevenin` depends on `cirq-frontend` so it can lower a
//! [`cirq_ir::Circuit`] internally. This crate re-exports those Circuit
//! entry points for callers already using `thevenin-cirq`, and adds
//! SPICE-source convenience helpers that pair [`cirq_spice_import`] with the
//! simulator in one call.
//!
//! ```ignore
//! use thevenin_cirq::simulate_spice_op;
//!
//! let result = simulate_spice_op("V1 in 0 1\nR1 in 0 1k\n.op\n.end\n")?;
//! ```
//!
//! ## Why a separate crate?
//!
//! The SPICE-source helpers depend on [`cirq_spice_import`], which already
//! depends on `thevenin` for subcircuit flattening — promoting it to a
//! production dep of `thevenin` would close a cycle. So the SPICE-source
//! conveniences live here, while the Circuit-shaped API graduated into
//! `thevenin` itself.

use cirq_ir::Circuit;
use cirq_spice_import::ImportError;
use thevenin::circuit::CircuitSimError;
use thevenin_types::SimResult;

// Re-export the Circuit-shaped API from thevenin so existing callers writing
// `thevenin_cirq::simulate_op(&circuit)` continue to work.
pub use thevenin::circuit::{simulate_ac, simulate_dc, simulate_op, simulate_tran};

/// Errors that can arise when driving the simulator from raw SPICE source.
#[derive(Debug, thiserror::Error)]
pub enum SimulateError {
    #[error("failed to parse SPICE source: {0}")]
    SpiceParse(String),

    #[error("failed to import SPICE into Cirq IR: {0}")]
    SpiceImport(#[from] ImportError),

    #[error(transparent)]
    Circuit(#[from] CircuitSimError),
}

fn import_first(source: &str) -> Result<Circuit, SimulateError> {
    let mut circuits = cirq_spice_import::import_spice(source)?;
    circuits
        .drain(..)
        .next()
        .ok_or_else(|| SimulateError::SpiceParse("SPICE source produced no circuits".into()))
}

/// Parse SPICE source and run a DC operating-point analysis.
pub fn simulate_spice_op(source: &str) -> Result<SimResult, SimulateError> {
    Ok(simulate_op(&import_first(source)?)?)
}

/// Parse SPICE source and run a DC sweep.
pub fn simulate_spice_dc(source: &str) -> Result<SimResult, SimulateError> {
    Ok(simulate_dc(&import_first(source)?)?)
}

/// Parse SPICE source and run a transient analysis.
pub fn simulate_spice_tran(source: &str) -> Result<SimResult, SimulateError> {
    Ok(simulate_tran(&import_first(source)?)?)
}

/// Parse SPICE source and run an AC small-signal analysis.
pub fn simulate_spice_ac(source: &str) -> Result<SimResult, SimulateError> {
    Ok(simulate_ac(&import_first(source)?)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spice_op_voltage_divider() {
        let src = "Voltage Divider\n\
                   V1 in 0 1.0\n\
                   R1 in mid 1k\n\
                   R2 mid 0 2k\n\
                   .op\n\
                   .end\n";
        let result = simulate_spice_op(src).expect("simulate spice op");
        let v_mid = result.plots[0]
            .vecs
            .iter()
            .find(|v| v.name == "v(mid)")
            .expect("v(mid)");
        let v = match &v_mid.data {
            thevenin_types::VectorData::Real(r) => r[0],
            _ => panic!(),
        };
        assert!((v - 2.0 / 3.0).abs() < 1e-6, "v(mid) = {v}");
    }

    #[test]
    fn spice_dc_sweep() {
        let src = "DC Sweep\n\
                   V1 in 0 1.0\n\
                   R1 in out 1k\n\
                   R2 out 0 1k\n\
                   .dc V1 0 5 0.1\n\
                   .end\n";
        let result = simulate_spice_dc(src).expect("simulate spice dc");
        let v_out = result.plots[0]
            .vecs
            .iter()
            .find(|v| v.name == "v(out)")
            .expect("v(out)");
        let pts = match &v_out.data {
            thevenin_types::VectorData::Real(r) => r,
            _ => panic!(),
        };
        assert!(pts.len() >= 50);
        let last = *pts.last().unwrap();
        assert!((last - 2.5).abs() < 1e-6, "last v(out) = {last}");
    }

    /// Sanity-check that the re-exported `simulate_op` from `thevenin::circuit`
    /// produces results equivalent to the legacy Netlist path.
    #[test]
    fn op_matches_netlist_path() {
        use cirq_frontend::to_netlist::circuit_to_netlists;

        let src = "Voltage Divider\n\
                   V1 in 0 1.0\n\
                   R1 in mid 1k\n\
                   R2 mid 0 2k\n\
                   .op\n\
                   .end\n";
        let circuits = cirq_spice_import::import_spice(src).unwrap();
        let circuit = &circuits[0];

        let via_circuit = simulate_op(circuit).unwrap();
        let nl = circuit_to_netlists(circuit)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let nl = thevenin::flatten_netlist(&nl).unwrap();
        let via_netlist = thevenin::simulate_op(&nl).unwrap();

        assert_eq!(
            via_circuit.plots[0].vecs.len(),
            via_netlist.plots[0].vecs.len()
        );
        for (a, b) in via_circuit.plots[0]
            .vecs
            .iter()
            .zip(via_netlist.plots[0].vecs.iter())
        {
            assert_eq!(a.name, b.name);
            let av = match &a.data {
                thevenin_types::VectorData::Real(r) => r[0],
                _ => continue,
            };
            let bv = match &b.data {
                thevenin_types::VectorData::Real(r) => r[0],
                _ => continue,
            };
            assert_eq!(av, bv, "drift in {}", a.name);
        }
    }
}
