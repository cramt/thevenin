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

use std::collections::HashMap;

use cirq_frontend::to_netlist::{ConvertError, circuit_to_netlists};
use cirq_ir::{Circuit, ElementKind as IrElementKind, Id, Value};
use thevenin_types::{Analysis, Netlist, SimPlot, SimResult, SimVector};

use crate::{LinearSystem, MnaError};

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

/// Direct IR → MNA path for the linear-only DC operating point.
///
/// Returns `Some(result)` if every element in the circuit is one of
/// R / V / I / C / L (treating C as DC-open, L as DC-short). Otherwise
/// returns `None` and the caller should fall back to the lowering path.
///
/// The assembled `LinearSystem` matches what `assemble_mna_flat` would
/// produce for the same circuit on the linear path: node indices are
/// assigned in element-traversal order (pos before neg), voltage source
/// branch rows follow the node rows in element order, and the solver is
/// the same `LinearSystem::solve` used by `solve_op_raw`. Output vectors
/// follow the same `v(node)` / `name#branch` naming and the same
/// descending-matrix-index node ordering as `simulate::simulate_op`.
fn simulate_op_direct(circuit: &Circuit) -> Option<SimResult> {
    // Fast-fail if any element kind isn't part of the linear subset.
    for elem in &circuit.elements {
        match elem.kind {
            IrElementKind::Resistor
            | IrElementKind::VoltageSource
            | IrElementKind::CurrentSource
            | IrElementKind::Capacitor
            | IrElementKind::Inductor => {}
            _ => return None,
        }
    }

    // Treat the net named "0" or "gnd" as ground (no matrix row).
    // SPICE-imported circuits use "0"; Cirq-source-compiled circuits use
    // "gnd" — both must work because the harness routes through Cirq IR.
    let gnd_id: Option<Id> = circuit
        .nets
        .iter()
        .find(|n| n.name == "0" || n.name == "gnd")
        .map(|n| n.id);

    let net_name = |id: Id| -> &str {
        circuit
            .nets
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.name.as_str())
            .unwrap_or("?")
    };

    // First pass: collect node indices in element-traversal order and count
    // voltage sources (V and L both add a branch row).
    let mut node_idx: HashMap<Id, usize> = HashMap::new();
    let mut node_order: Vec<Id> = Vec::new();
    let intern = |id: Id, node_idx: &mut HashMap<Id, usize>, node_order: &mut Vec<Id>| {
        if Some(id) == gnd_id {
            return;
        }
        if !node_idx.contains_key(&id) {
            let i = node_idx.len();
            node_idx.insert(id, i);
            node_order.push(id);
        }
    };

    let mut vsource_names: Vec<String> = Vec::new();
    for elem in &circuit.elements {
        let pos = terminal_net(elem, "pos")?;
        let neg = terminal_net(elem, "neg")?;
        intern(pos, &mut node_idx, &mut node_order);
        intern(neg, &mut node_idx, &mut node_order);
        if matches!(
            elem.kind,
            IrElementKind::VoltageSource | IrElementKind::Inductor
        ) {
            vsource_names.push(elem.name.clone());
        }
    }

    let n_nodes = node_idx.len();
    let dim = n_nodes + vsource_names.len();
    if dim == 0 {
        // Empty circuit — fall back so the lowering path can produce the
        // standard empty SimResult shape.
        return None;
    }
    let mut system = LinearSystem::new(dim);

    // Second pass: stamp every element.
    let mut vsi = 0usize;
    for elem in &circuit.elements {
        let pos_id = terminal_net(elem, "pos")?;
        let neg_id = terminal_net(elem, "neg")?;
        let p = if Some(pos_id) == gnd_id {
            None
        } else {
            node_idx.get(&pos_id).copied()
        };
        let n = if Some(neg_id) == gnd_id {
            None
        } else {
            node_idx.get(&neg_id).copied()
        };

        match &elem.kind {
            IrElementKind::Resistor => {
                let value = param_real(elem, "value")?;
                if value == 0.0 {
                    return None; // 0-ohm resistor: fall back (handled as short elsewhere).
                }
                let g = 1.0 / value;
                if let Some(p) = p {
                    system.matrix.add(p, p, g);
                }
                if let Some(n) = n {
                    system.matrix.add(n, n, g);
                }
                if let (Some(p), Some(n)) = (p, n) {
                    system.matrix.add(p, n, -g);
                    system.matrix.add(n, p, -g);
                }
            }
            IrElementKind::VoltageSource => {
                let dc = elem.source_spec.as_ref().and_then(|s| s.dc).unwrap_or(0.0);
                let branch = n_nodes + vsi;
                vsi += 1;
                if let Some(p) = p {
                    system.matrix.add(p, branch, 1.0);
                    system.matrix.add(branch, p, 1.0);
                }
                if let Some(n) = n {
                    system.matrix.add(n, branch, -1.0);
                    system.matrix.add(branch, n, -1.0);
                }
                system.rhs[branch] = dc;
            }
            IrElementKind::Inductor => {
                // DC: inductor is a short — same stamping as 0V vsource.
                let branch = n_nodes + vsi;
                vsi += 1;
                if let Some(p) = p {
                    system.matrix.add(p, branch, 1.0);
                    system.matrix.add(branch, p, 1.0);
                }
                if let Some(n) = n {
                    system.matrix.add(n, branch, -1.0);
                    system.matrix.add(branch, n, -1.0);
                }
            }
            IrElementKind::CurrentSource => {
                let dc = elem.source_spec.as_ref().and_then(|s| s.dc).unwrap_or(0.0);
                if let Some(p) = p {
                    system.rhs[p] -= dc;
                }
                if let Some(n) = n {
                    system.rhs[n] += dc;
                }
            }
            IrElementKind::Capacitor => {
                // DC: open circuit — no stamp.
            }
            _ => unreachable!("filtered above"),
        }
    }

    let solution = system.solve().ok()?;

    // Build SimResult to match simulate::simulate_op:
    //   - Node voltages in descending matrix-index order (LIFO).
    //   - Voltage source branch currents in element-insertion order.
    let mut vecs: Vec<SimVector> = Vec::new();
    let mut node_list: Vec<(Id, usize)> = node_order
        .iter()
        .map(|id| (*id, *node_idx.get(id).unwrap()))
        .collect();
    node_list.sort_by_key(|(_, i)| std::cmp::Reverse(*i));
    for (id, idx) in &node_list {
        let v = solution.get(*idx).copied().unwrap_or(0.0);
        vecs.push(SimVector::real(format!("v({})", net_name(*id)), vec![v]));
    }
    for (i, vsrc) in vsource_names.iter().enumerate() {
        let idx = n_nodes + i;
        let current = solution.get(idx).copied().unwrap_or(0.0);
        vecs.push(SimVector::real(
            format!("{}#branch", vsrc.to_lowercase()),
            vec![current],
        ));
    }

    Some(SimResult {
        plots: vec![SimPlot {
            name: "op1".to_string(),
            vecs,
        }],
    })
}

/// Look up the Id of an element's terminal connection. Returns `None` if the
/// element doesn't declare the named terminal (which should be impossible
/// for the linear element kinds this module handles, but is reported back
/// so the caller falls back to the lowering path rather than panicking).
fn terminal_net(elem: &cirq_ir::Element, terminal: &str) -> Option<Id> {
    elem.connections
        .iter()
        .find(|c| c.terminal == terminal)
        .map(|c| c.net)
}

/// Read a numeric element parameter by name.
fn param_real(elem: &cirq_ir::Element, name: &str) -> Option<f64> {
    elem.params
        .iter()
        .find(|(k, _)| k == name)
        .and_then(|(_, v)| match v {
            Value::Real(f) => Some(*f),
            Value::Integer(i) => Some(*i as f64),
            _ => None,
        })
}

/// Run a DC sweep on the circuit's first declared `.dc` analysis.
pub fn simulate_dc(circuit: &Circuit) -> Result<SimResult, CircuitSimError> {
    let nls = lower(circuit)?;
    let nl = pick(&nls, "dc", |a| matches!(a, Analysis::Dc { .. }))?;
    Ok(crate::simulate_dc(nl)?)
}

/// Run a transient analysis on the circuit's first declared `.tran` analysis.
///
/// As with the harness, an operating-point solve is prepended so the
/// transient starts from a valid steady state.
pub fn simulate_tran(circuit: &Circuit) -> Result<SimResult, CircuitSimError> {
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
pub fn simulate_ac(circuit: &Circuit) -> Result<SimResult, CircuitSimError> {
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

    /// A circuit with any non-linear element must fall back to the lowering
    /// path. `simulate_op_direct` returns `None`.
    #[test]
    fn direct_path_rejects_nonlinear_circuits() {
        let mut c = voltage_divider();
        c.elements.push(cirq_ir::Element {
            id: Id(3),
            name: "D1".into(),
            kind: IrElementKind::Diode,
            connections: vec![
                cirq_ir::Connection {
                    terminal: "anode".into(),
                    net: Id(2),
                },
                cirq_ir::Connection {
                    terminal: "cathode".into(),
                    net: Id(0),
                },
            ],
            params: vec![],
            model: None,
            source_spec: None,
        });
        assert!(simulate_op_direct(&c).is_none());
    }
}
