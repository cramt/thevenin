//! Cirq IR → Thevenin simulation entry points.
//!
//! This crate is the **Stage 4 surface** of the Cirq adoption plan
//! (`docs/migration/cirq-adoption-plan.md`). It lets callers run a
//! [`cirq_ir::Circuit`] directly through the simulator without first
//! constructing a [`thevenin_types::Netlist`] themselves.
//!
//! ```ignore
//! use thevenin_cirq::simulate_op;
//!
//! let circuit = cirq_spice_import::import_spice(src)?;
//! let result = simulate_op(&circuit)?;
//! ```
//!
//! ## Implementation notes
//!
//! For now, the entry points route internally through
//! [`cirq_frontend::to_netlist::circuit_to_netlists`] and the existing
//! [`thevenin`] simulator. As Stage 4 progresses, individual analyses will
//! gain direct IR → MNA paths that bypass the Netlist adapter entirely;
//! callers will see no behavioural change.
//!
//! The Netlist-shaped API in [`thevenin`] remains supported for the
//! foreseeable future (the simulator still consumes it internally), but
//! new code should prefer this crate as the entry point.

use cirq_frontend::to_netlist::{ConvertError, circuit_to_netlists};
use cirq_ir::Circuit;
use cirq_spice_import::ImportError;
use thevenin::MnaError;
use thevenin_types::{Analysis, Netlist, SimPlot, SimResult};

/// Errors that can occur when simulating a Cirq IR circuit.
#[derive(Debug, thiserror::Error)]
pub enum SimulateError {
    #[error("failed to parse SPICE source: {0}")]
    SpiceParse(String),

    #[error("failed to import SPICE into Cirq IR: {0}")]
    SpiceImport(#[from] ImportError),

    #[error("failed to lower Cirq IR to Netlist: {0}")]
    Convert(#[from] ConvertError),

    #[error("simulation failed: {0}")]
    Simulate(#[from] MnaError),

    #[error("subcircuit flattening failed: {0}")]
    Flatten(String),

    #[error(
        "circuit has no `{expected}` analysis (it has {found} analyses); call \
         the matching `simulate_*` for one of the declared analyses, or add the \
         right `Analysis` variant to the circuit"
    )]
    WrongAnalysis {
        expected: &'static str,
        found: usize,
    },
}

/// Convert a [`Circuit`] into the per-analysis [`Netlist`] forms used by the
/// existing simulator. Flattens subcircuits (idempotent on already-flat
/// netlists, which is what `circuit_to_netlists` produces).
fn lower(circuit: &Circuit) -> Result<Vec<Netlist>, SimulateError> {
    let nls = circuit_to_netlists(circuit)?;
    nls.into_iter()
        .map(|nl| thevenin::flatten_netlist(&nl).map_err(|e| SimulateError::Flatten(e.to_string())))
        .collect()
}

/// Pick the first netlist whose analysis matches `expected` discriminant.
fn pick_for<'a>(
    nls: &'a [Netlist],
    expected: &'static str,
    matches: impl Fn(&Analysis) -> bool,
) -> Result<&'a Netlist, SimulateError> {
    nls.iter()
        .find(|nl| matches(&nl.analysis))
        .ok_or(SimulateError::WrongAnalysis {
            expected,
            found: nls.len(),
        })
}

/// Run a DC operating-point analysis on the circuit.
///
/// If the circuit declares no analyses, an implicit `.op` is used. If it
/// declares multiple analyses, the first `Analysis::Op` (or the implicit
/// default) is chosen — callers wanting fine control should select the
/// netlist explicitly via [`cirq_frontend::to_netlist::circuit_to_netlists`].
pub fn simulate_op(circuit: &Circuit) -> Result<SimResult, SimulateError> {
    let nls = lower(circuit)?;
    let nl = pick_for(&nls, "op", |a| matches!(a, Analysis::Op))?;
    Ok(thevenin::simulate_op(nl)?)
}

/// Run a DC sweep on the circuit's first declared `.dc` analysis.
pub fn simulate_dc(circuit: &Circuit) -> Result<SimResult, SimulateError> {
    let nls = lower(circuit)?;
    let nl = pick_for(&nls, "dc", |a| matches!(a, Analysis::Dc { .. }))?;
    Ok(thevenin::simulate_dc(nl)?)
}

/// Run a transient analysis on the circuit's first declared `.tran` analysis.
///
/// As with the harness, an operating-point solve is prepended so the
/// transient starts from a valid steady state.
pub fn simulate_tran(circuit: &Circuit) -> Result<SimResult, SimulateError> {
    let nls = lower(circuit)?;
    let nl = pick_for(&nls, "tran", |a| matches!(a, Analysis::Tran { .. }))?;
    let mut plots: Vec<SimPlot> = Vec::new();
    if let Ok(op) = thevenin::simulate_op(nl) {
        plots.extend(op.plots);
    }
    plots.extend(thevenin::simulate_tran(nl)?.plots);
    Ok(SimResult { plots })
}

/// Run an AC small-signal analysis on the circuit's first declared `.ac`.
pub fn simulate_ac(circuit: &Circuit) -> Result<SimResult, SimulateError> {
    let nls = lower(circuit)?;
    let nl = pick_for(&nls, "ac", |a| matches!(a, Analysis::Ac { .. }))?;
    Ok(thevenin::simulate_ac(nl)?)
}

// ---------------------------------------------------------------------------
// SPICE source → SimResult convenience entry points
// ---------------------------------------------------------------------------
//
// These let callers drive a simulation straight from SPICE text without
// constructing a Circuit manually. The path is:
//   SPICE source → Netlist (parse) → Circuit (cirq_spice_import) → simulate_*
// which mirrors the canonical Stage 3 pipeline.
//
// `cirq_spice_import::import_spice` returns a `Vec<Circuit>` because a SPICE
// file may declare multiple `.tran`/`.dc`/etc. analyses via control blocks,
// producing one fork per analysis. The convenience helpers below pick the
// first circuit; callers wanting all forks should call `import_spice`
// directly and dispatch each circuit.

fn import_first(source: &str) -> Result<Circuit, SimulateError> {
    let mut circuits = cirq_spice_import::import_spice(source)?;
    circuits
        .drain(..)
        .next()
        .ok_or_else(|| SimulateError::SpiceParse("SPICE source produced no circuits".into()))
}

/// Parse SPICE source and run a DC operating-point analysis.
pub fn simulate_spice_op(source: &str) -> Result<SimResult, SimulateError> {
    simulate_op(&import_first(source)?)
}

/// Parse SPICE source and run a DC sweep.
pub fn simulate_spice_dc(source: &str) -> Result<SimResult, SimulateError> {
    simulate_dc(&import_first(source)?)
}

/// Parse SPICE source and run a transient analysis.
pub fn simulate_spice_tran(source: &str) -> Result<SimResult, SimulateError> {
    simulate_tran(&import_first(source)?)
}

/// Parse SPICE source and run an AC small-signal analysis.
pub fn simulate_spice_ac(source: &str) -> Result<SimResult, SimulateError> {
    simulate_ac(&import_first(source)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cirq_ir::{
        Analysis as IrAnalysis, Circuit, Connection, DcAnalysis, DcSweep, Element, ElementKind,
        FrequencyScale, Id, Net, ResolvedParam, SourceSpec, TranAnalysis, Value,
    };

    fn voltage_divider() -> Circuit {
        let nets = vec![
            Net {
                id: Id(0),
                name: "gnd".into(),
                is_global: true,
            },
            Net {
                id: Id(1),
                name: "in".into(),
                is_global: false,
            },
            Net {
                id: Id(2),
                name: "mid".into(),
                is_global: false,
            },
        ];
        let elements = vec![
            Element {
                id: Id(0),
                name: "V1".into(),
                kind: ElementKind::VoltageSource,
                connections: vec![
                    Connection {
                        terminal: "pos".into(),
                        net: Id(1),
                    },
                    Connection {
                        terminal: "neg".into(),
                        net: Id(0),
                    },
                ],
                params: vec![],
                model: None,
                source_spec: Some(SourceSpec {
                    dc: Some(1.0),
                    ac: None,
                    waveform: None,
                }),
            },
            Element {
                id: Id(1),
                name: "R1".into(),
                kind: ElementKind::Resistor,
                connections: vec![
                    Connection {
                        terminal: "pos".into(),
                        net: Id(1),
                    },
                    Connection {
                        terminal: "neg".into(),
                        net: Id(2),
                    },
                ],
                params: vec![("value".into(), Value::Real(1_000.0))],
                model: None,
                source_spec: None,
            },
            Element {
                id: Id(2),
                name: "R2".into(),
                kind: ElementKind::Resistor,
                connections: vec![
                    Connection {
                        terminal: "pos".into(),
                        net: Id(2),
                    },
                    Connection {
                        terminal: "neg".into(),
                        net: Id(0),
                    },
                ],
                params: vec![("value".into(), Value::Real(2_000.0))],
                model: None,
                source_spec: None,
            },
        ];
        Circuit {
            name: "voltage_divider".into(),
            nets,
            elements,
            models: vec![],
            analyses: vec![IrAnalysis::Op],
            params: Vec::<ResolvedParam>::new(),
            options: vec![],
            temps: vec![],
            save: vec![],
            funcs: vec![],
            initial_conditions: vec![],
            nodeset: vec![],
            measures: vec![],
            code_blocks: vec![],
            raw_directives: vec![],
        }
    }

    #[test]
    fn op_voltage_divider() {
        let circuit = voltage_divider();
        let result = simulate_op(&circuit).expect("simulate");
        let vecs = &result.plots[0].vecs;
        let v_mid = vecs.iter().find(|v| v.name == "v(mid)").expect("v(mid)");
        let v = match &v_mid.data {
            thevenin_types::VectorData::Real(r) => r[0],
            _ => panic!("expected real"),
        };
        // Ideal divider: V_mid = 1V * R2/(R1+R2) = 2/3 V.
        assert!((v - 2.0 / 3.0).abs() < 1e-6, "v(mid) = {v}");
    }

    #[test]
    fn op_matches_netlist_path() {
        // Sanity check: running through the new Circuit entry point produces
        // bitwise-identical results to going through the Netlist path
        // manually. This is the equivalence contract the harness already
        // enforces in the large; the unit test pins it for the small case.
        let circuit = voltage_divider();
        let via_circuit = simulate_op(&circuit).unwrap();
        let nl = circuit_to_netlists(&circuit)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let nl = thevenin::flatten_netlist(&nl).unwrap();
        let via_netlist = thevenin::simulate_op(&nl).unwrap();
        // Compare every vector's first sample.
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
            assert_eq!(av, bv, "vec {} drift", a.name);
        }
    }

    #[test]
    fn wrong_analysis_returns_error() {
        let mut c = voltage_divider();
        c.analyses = vec![IrAnalysis::Tran(TranAnalysis {
            step: 1e-9,
            stop: 1e-6,
            start: 0.0,
            uic: false,
            tmax: None,
        })];
        let err = simulate_op(&c).unwrap_err();
        assert!(matches!(
            err,
            SimulateError::WrongAnalysis { expected: "op", .. }
        ));
    }

    // Smoke test: DC sweep + AC entry points compile and dispatch correctly.
    // (Detailed numerical behaviour is covered by the regression harness.)
    #[test]
    fn dc_entry_point_compiles() {
        let mut c = voltage_divider();
        c.analyses = vec![IrAnalysis::Dc(DcAnalysis {
            sweeps: vec![DcSweep {
                source: Id(0),
                start: 0.0,
                stop: 1.0,
                step: 0.1,
            }],
        })];
        simulate_dc(&c).expect("dc");
    }

    #[test]
    fn ac_entry_point_compiles() {
        use cirq_ir::AcAnalysis;
        let mut c = voltage_divider();
        c.analyses = vec![IrAnalysis::Ac(AcAnalysis {
            start: 1.0,
            stop: 1e3,
            points: 10,
            scale: FrequencyScale::Decade,
        })];
        simulate_ac(&c).expect("ac");
    }

    // -----------------------------------------------------------------------
    // SPICE-source convenience entry points
    // -----------------------------------------------------------------------

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
        // Sweep from 0V to 5V in 0.1V steps -> 51 samples; final V(out) = V_in/2.
        assert!(pts.len() >= 50);
        let last = *pts.last().unwrap();
        assert!((last - 2.5).abs() < 1e-6, "last v(out) = {last}");
    }
}
