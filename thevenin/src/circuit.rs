//! Cirq IR — direct simulation entry points.
//!
//! These are the **Stage 4 surface** of the Cirq adoption plan
//! (`docs/migration/cirq-adoption-plan.md`). Callers pass a
//! [`cirq_ir::Circuit`] directly instead of constructing a
//! [`thevenin_types::Netlist`] themselves.
//!
//! For now, the entry points lower the circuit to one or more Netlists
//! internally via [`cirq_frontend::to_netlist::circuit_to_netlists`] and then
//! dispatch to the existing Netlist-shaped simulator. As Stage 4 progresses,
//! individual analyses will gain direct IR → MNA paths that bypass the
//! Netlist adapter entirely; callers see no behavioural change.
//!
//! ## Picking the right analysis
//!
//! A [`cirq_ir::Circuit`] can declare any number of analyses
//! (`circuit.analyses`); each [`Self::simulate_op`]-style entry point picks
//! the first analysis matching its discriminant. Callers wanting fine
//! control over multi-analysis circuits should call
//! [`cirq_frontend::to_netlist::circuit_to_netlists`] themselves and dispatch
//! each resulting netlist with [`crate::simulate_op`] / etc.

use cirq_frontend::to_netlist::{ConvertError, circuit_to_netlists};
use cirq_ir::Circuit;
use thevenin_types::{Analysis, Netlist, SimPlot, SimResult};

use crate::MnaError;
use crate::mna_ir;

/// Errors that can occur when simulating a [`Circuit`] directly.
#[derive(Debug, thiserror::Error)]
pub enum CircuitSimError {
    #[error("failed to lower Cirq IR to Netlist: {0}")]
    Convert(#[from] ConvertError),

    #[error("simulation failed: {0}")]
    Mna(#[from] MnaError),

    #[error("subcircuit flattening failed: {0}")]
    Flatten(String),

    #[error(
        "circuit has no `{expected}` analysis (it has {found} declared); call the \
         matching simulate_* for one of the declared analyses, or add the right \
         Analysis variant to the circuit"
    )]
    WrongAnalysis {
        expected: &'static str,
        found: usize,
    },
}

/// Lower a [`Circuit`] into per-analysis [`Netlist`]s, flattening subcircuits.
/// The flatten step is idempotent on the netlists produced by
/// [`circuit_to_netlists`] (which emits already-flat netlists), so this is
/// cheap when there's nothing to flatten.
fn lower(circuit: &Circuit) -> Result<Vec<Netlist>, CircuitSimError> {
    let nls = circuit_to_netlists(circuit)?;
    nls.into_iter()
        .map(|nl| crate::flatten_netlist(&nl).map_err(|e| CircuitSimError::Flatten(e.to_string())))
        .collect()
}

/// Pick the first netlist whose analysis matches the predicate.
fn pick<'a>(
    nls: &'a [Netlist],
    expected: &'static str,
    matches: impl Fn(&Analysis) -> bool,
) -> Result<&'a Netlist, CircuitSimError> {
    nls.iter()
        .find(|nl| matches(&nl.analysis))
        .ok_or(CircuitSimError::WrongAnalysis {
            expected,
            found: nls.len(),
        })
}

/// Run a DC operating-point analysis on the circuit.
///
/// If the circuit contains only linear elements (R, V, I, C, L), the assembly
/// is performed directly from the IR without going through the Netlist
/// adapter — this is the first Stage 4 direct-stamping case. For circuits
/// with any nonlinear / unsupported element, the implementation falls back
/// to lowering via `circuit_to_netlists`. Either path is observably
/// equivalent (the direct path matches the Netlist path bit-for-bit by
/// construction — it uses the same `LinearSystem::solve`).
pub fn simulate_op(circuit: &Circuit) -> Result<SimResult, CircuitSimError> {
    if has_op_analysis(circuit)
        && let Some(result) = simulate_op_direct(circuit)
    {
        return Ok(result);
    }
    let nls = lower(circuit)?;
    let nl = pick(&nls, "op", |a| matches!(a, Analysis::Op))?;
    Ok(crate::simulate_op(nl)?)
}

/// Whether the circuit either declares an `.op` analysis or has no analyses
/// (in which case the simulator defaults to OP).
fn has_op_analysis(circuit: &Circuit) -> bool {
    circuit.analyses.is_empty()
        || circuit
            .analyses
            .iter()
            .any(|a| matches!(a, cirq_ir::Analysis::Op))
}

/// Direct IR → MNA path for the DC operating point.
///
/// Returns `Some(result)` if [`mna_ir::assemble_mna_from_circuit`] accepts
/// the circuit (currently the linear subset R / V / I / C / L / E / G / H /
/// F; future sessions extend to nonlinear devices per
/// `docs/migration/mna-ir-pivot-plan.md`). Otherwise returns `None` and the
/// caller falls back to the lowering path.
///
/// The solve and SimResult formatting route through
/// [`crate::simulate::simulate_op_with_mna`] so output shape is identical
/// to the Netlist path regardless of how the MNA was assembled.
fn simulate_op_direct(circuit: &Circuit) -> Option<SimResult> {
    let mna = mna_ir::assemble_mna_from_circuit(circuit, false, None).ok()??;
    let opts = mna_ir::nr_options_from_circuit(circuit);
    let nodeset = mna_ir::resolve_nodeset_from_circuit(circuit, &mna);
    crate::simulate::simulate_op_with_mna(&mna, &opts, &nodeset).ok()
}

/// Run a DC sweep on the circuit's first declared `.dc` analysis.
///
/// When [`mna_ir::assemble_mna_from_circuit`] accepts the circuit (always
/// — every existing `IrElementKind` is supported), this is fully
/// Circuit-driven: NR options come from `circuit.options`, sweep params
/// from `circuit.analyses`, and the MnaSystem is built directly from IR.
/// No lowered Netlist is constructed at all on the happy path.
///
/// For the fallback (a future `IrElementKind` not yet handled by mna_ir
/// would land here), the lowered Netlist + the existing
/// `crate::simulate_dc` Netlist path is used.
pub fn simulate_dc(circuit: &Circuit) -> Result<SimResult, CircuitSimError> {
    if let Some(mna) = mna_ir::assemble_mna_from_circuit(circuit, false, None)? {
        let mut nr_opts = mna_ir::nr_options_from_circuit(circuit);
        nr_opts.diag_gmin = 0.0;
        let params = mna_ir::dc_sweep_params_from_circuit(circuit)?;
        return Ok(crate::simulate::run_dc_sweep(mna, nr_opts, params)?);
    }
    let nls = lower(circuit)?;
    let nl = pick(&nls, "dc", |a| matches!(a, Analysis::Dc { .. }))?;
    Ok(crate::simulate_dc(nl)?)
}

/// Run a transient analysis on the circuit's first declared `.tran` analysis.
///
/// As with the harness, an operating-point solve is prepended so the
/// transient starts from a valid steady state. Fully Circuit-driven on
/// the happy path: NR options, nodeset, `.ic` overrides, `.print`
/// queries, and `.tran` analysis params all come from the IR Circuit.
/// The lowered Netlist is only used for the fallback path.
pub fn simulate_tran(circuit: &Circuit) -> Result<SimResult, CircuitSimError> {
    if let Some(mna_op) = mna_ir::assemble_mna_from_circuit(circuit, false, None)? {
        let opts = mna_ir::nr_options_from_circuit(circuit);
        let nodeset = mna_ir::resolve_nodeset_from_circuit(circuit, &mna_op);
        let mut plots: Vec<SimPlot> = Vec::new();
        if let Ok(op) = crate::simulate::simulate_op_with_mna(&mna_op, &opts, &nodeset) {
            plots.extend(op.plots);
        }
        let mna_tran = mna_ir::assemble_mna_from_circuit(circuit, false, None)?
            .expect("mna_ir already accepted this circuit");
        let params = mna_ir::tran_params_from_circuit(circuit, &mna_tran)?;
        plots.extend(crate::transient::run_tran(mna_tran, params)?.plots);
        return Ok(SimResult { plots });
    }
    let nls = lower(circuit)?;
    let nl = pick(&nls, "tran", |a| matches!(a, Analysis::Tran { .. }))?;
    let mut plots: Vec<SimPlot> = Vec::new();
    if let Ok(op) = crate::simulate_op(nl) {
        plots.extend(op.plots);
    }
    plots.extend(crate::simulate_tran(nl)?.plots);
    Ok(SimResult { plots })
}

/// Run an AC small-signal analysis on the circuit's first declared `.ac`.
///
/// Fully Circuit-driven on the happy path: NR options, nodeset, AC source
/// excitations, and `.ac` sweep params all come from the IR Circuit. The
/// lowered Netlist is only constructed for the fallback (a future
/// IrElementKind not yet handled by mna_ir).
pub fn simulate_ac(circuit: &Circuit) -> Result<SimResult, CircuitSimError> {
    if let Some(mna) = mna_ir::assemble_mna_from_circuit(circuit, false, None)? {
        let params = mna_ir::ac_sweep_params_from_circuit(circuit, &mna)?;
        return Ok(crate::ac::run_ac_sweep(mna, params)?);
    }
    let nls = lower(circuit)?;
    let nl = pick(&nls, "ac", |a| matches!(a, Analysis::Ac { .. }))?;
    Ok(crate::simulate_ac(nl)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cirq_ir::{
        Analysis as IrAnalysis, Connection, Element, ElementKind, Id, Net, ResolvedParam,
        SourceSpec, TranAnalysis, Value,
    };

    fn voltage_divider() -> Circuit {
        Circuit {
            name: "voltage_divider".into(),
            nets: vec![
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
            ],
            elements: vec![
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
            ],
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
        let result = simulate_op(&voltage_divider()).expect("op");
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
            CircuitSimError::WrongAnalysis { expected: "op", .. }
        ));
    }

    /// The direct path must produce a `SimResult` whose v(node) and branch
    /// vectors match the Netlist-routed path bit-for-bit. This is the
    /// equivalence contract for Stage 4 incremental direct stamping.
    #[test]
    fn direct_path_matches_lowered_path_voltage_divider() {
        let c = voltage_divider();
        let direct = simulate_op_direct(&c).expect("direct path accepts linear circuit");

        let nls = lower(&c).unwrap();
        let nl = pick(&nls, "op", |a| matches!(a, Analysis::Op)).unwrap();
        let lowered = crate::simulate_op(nl).unwrap();

        assert_eq!(direct.plots.len(), lowered.plots.len());
        for (a, b) in direct.plots[0]
            .vecs
            .iter()
            .zip(lowered.plots[0].vecs.iter())
        {
            assert_eq!(a.name, b.name, "vec name mismatch");
            let av = match &a.data {
                thevenin_types::VectorData::Real(r) => r[0],
                _ => panic!(),
            };
            let bv = match &b.data {
                thevenin_types::VectorData::Real(r) => r[0],
                _ => panic!(),
            };
            assert_eq!(av, bv, "drift in {}: direct={av} lowered={bv}", a.name);
        }
    }

}
