//! `.control` block interpreter for the thevenin circuit simulator.
//!
//! Parses and executes the ngspice `.control` / `.endc` scripting language,
//! supporting simulation commands, vector expressions, control flow, and
//! variable management.

pub mod ast;
pub mod context;
pub mod exec;
pub mod parse;
pub mod vecexpr;

use cirq_ir::Circuit;
use context::SimContext;
use exec::ControlResult;
use thevenin_types::{Netlist, SimResult};

/// Execute a `.control` block from a netlist.
///
/// Finds the first `Item::Control` in the netlist, parses it, and executes
/// all commands. Returns the merged simulation results and exit code.
pub fn execute_control_block(netlist: &Netlist) -> Result<ControlResult, String> {
    // Find .control block(s)
    let control_lines: Vec<&Vec<String>> = netlist
        .items
        .iter()
        .filter_map(|item| {
            if let thevenin_types::Item::Control(lines) = item {
                Some(lines)
            } else {
                None
            }
        })
        .collect();

    if control_lines.is_empty() {
        return Err("no .control block found".to_string());
    }

    // Create execution context with the netlist (minus .control blocks)
    let mut ctx = SimContext::new(netlist.clone());

    // Execute each .control block
    for lines in control_lines {
        let stmts = parse::parse_control_block(lines)?;
        exec::execute(&stmts, &mut ctx)?;
        if ctx.exit_code.is_some() {
            break;
        }
    }

    let exit_code = ctx.exit_code.unwrap_or(0);

    // Merge all plots into a SimResult
    let sim_result = SimResult { plots: ctx.plots };

    Ok(ControlResult {
        sim_result,
        exit_code,
        output: ctx.output,
    })
}

/// Check if a netlist contains a `.control` block.
pub fn has_control_block(netlist: &Netlist) -> bool {
    netlist
        .items
        .iter()
        .any(|item| matches!(item, thevenin_types::Item::Control(_)))
}

/// Check if a Cirq IR circuit contains a `.control` code block.
///
/// The Cirq IR stores `.control` source verbatim as a [`cirq_ir::CodeBlock`]
/// with `language == "control"`; this is the IR-shaped equivalent of
/// [`has_control_block`].
pub fn has_control_block_ir(circuit: &Circuit) -> bool {
    circuit.code_blocks.iter().any(|b| b.language == "control")
}

/// Execute a `.control` block from a Cirq IR circuit.
///
/// **Stage 4 / Phase B.** The canonical IR-shaped entry point for driving
/// `.control` from a [`Circuit`]. Builds a [`SimContext`] via
/// [`SimContext::from_circuit`] so the analysis dispatcher in `exec.rs` can
/// route Op / Dc / Tran / Ac runs through [`thevenin::circuit::simulate_*`]
/// while keeping helpers that still operate on the SPICE Netlist shape
/// (TEMPER eval, `@device[param]` lookups, alter) working unchanged.
///
/// `.control` lines come from the circuit's [`cirq_ir::CodeBlock`] entries
/// directly — control blocks accumulate to every netlist fork produced
/// from a circuit, so picking from the IR is equivalent to picking the
/// first fork on the lowered side.
pub fn execute_control_block_ir(circuit: &Circuit) -> Result<ControlResult, String> {
    let control_lines: Vec<&Vec<String>> = circuit
        .code_blocks
        .iter()
        .filter(|b| b.language == "control")
        .map(|b| &b.lines)
        .collect();

    if control_lines.is_empty() {
        return Err("no .control block found".to_string());
    }

    let mut ctx = SimContext::from_circuit(circuit.clone())?;

    for lines in control_lines {
        let stmts = parse::parse_control_block(lines)?;
        exec::execute(&stmts, &mut ctx)?;
        if ctx.exit_code.is_some() {
            break;
        }
    }

    let exit_code = ctx.exit_code.unwrap_or(0);
    let sim_result = SimResult { plots: ctx.plots };

    Ok(ControlResult {
        sim_result,
        exit_code,
        output: ctx.output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cirq_ir::{
        Analysis as IrAnalysis, CodeBlock, Connection, Element, ElementKind, Id, Net,
        ResolvedParam, SourceSpec, Value,
    };

    /// Build a minimal divider Circuit carrying a `.control` block. Picked so
    /// every step the interpreter touches has a deterministic, easy-to-check
    /// answer (V(mid) = 2/3 V at the OP).
    fn divider_with_control(control: Vec<String>) -> Circuit {
        Circuit {
            name: "divider".into(),
            nets: vec![
                Net {
                    id: Id(0),
                    name: "0".into(),
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
            code_blocks: vec![CodeBlock {
                language: "control".into(),
                lines: control,
            }],
            raw_directives: vec![],
        }
    }

    #[test]
    fn has_control_block_ir_detects_control_code_block() {
        let with_control = divider_with_control(vec!["op".into(), "quit 0".into()]);
        assert!(has_control_block_ir(&with_control));

        let mut without = with_control.clone();
        without.code_blocks.clear();
        assert!(!has_control_block_ir(&without));

        // Other languages don't count.
        let other = Circuit {
            code_blocks: vec![CodeBlock {
                language: "scheme".into(),
                lines: vec!["(display 42)".into()],
            }],
            ..without
        };
        assert!(!has_control_block_ir(&other));
    }

    /// The IR-shaped entry point must produce the same `ControlResult` as
    /// lowering the circuit and running the legacy Netlist-shaped entry
    /// point — they share the interpreter, so any drift means the IR
    /// lowering or the entry-point wrapper introduced a difference. This is
    /// the Phase A equivalence contract.
    #[test]
    fn ir_entry_point_matches_netlist_entry_point() {
        let circuit = divider_with_control(vec![
            "op".into(),
            "let half = v(mid) * 2".into(),
            "echo result: $&half".into(),
            "quit 0".into(),
        ]);

        let via_ir = execute_control_block_ir(&circuit).expect("IR path");

        let nl = cirq_frontend::to_netlist::circuit_to_netlists(&circuit)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let nl = thevenin::flatten_netlist(&nl).unwrap();
        let via_netlist = execute_control_block(&nl).expect("Netlist path");

        assert_eq!(via_ir.exit_code, via_netlist.exit_code);
        assert_eq!(via_ir.output, via_netlist.output);
        assert_eq!(
            via_ir.sim_result.plots.len(),
            via_netlist.sim_result.plots.len()
        );
        for (a, b) in via_ir
            .sim_result
            .plots
            .iter()
            .zip(via_netlist.sim_result.plots.iter())
        {
            assert_eq!(a.name, b.name);
            assert_eq!(a.vecs.len(), b.vecs.len());
        }
    }

    /// An empty `.control` block is a parse error in the legacy path; the IR
    /// path should surface the same error rather than swallowing it.
    #[test]
    fn ir_entry_point_errors_when_no_control_block() {
        let mut circuit = divider_with_control(vec!["op".into(), "quit 0".into()]);
        circuit.code_blocks.clear();
        let err = match execute_control_block_ir(&circuit) {
            Ok(_) => panic!("expected error for missing .control block"),
            Err(e) => e,
        };
        assert!(
            err.contains("no .control block"),
            "expected missing-control error, got: {err}"
        );
    }

    /// Helper: pull the (last) value of a real vector out of a control
    /// result by name, panicking if it isn't there or isn't real.
    fn vec_value(result: &ControlResult, plot_idx: usize, name: &str) -> f64 {
        let plot = &result.sim_result.plots[plot_idx];
        let v = plot
            .vecs
            .iter()
            .find(|v| v.name.to_lowercase() == name.to_lowercase())
            .unwrap_or_else(|| panic!("missing vec {name} in plot {}", plot.name));
        match &v.data {
            thevenin_types::VectorData::Real(r) => *r.last().unwrap(),
            _ => panic!("expected real vec for {name}"),
        }
    }

    /// Plain-form `alter v1 = -5` must mutate V1's DC value in the
    /// driving Circuit, so the subsequent OP sees -5 V instead of the
    /// original 1 V. Without IR-side mutation, V(mid) would still be
    /// 0.667 V; with mutation, it should be -5 * 2/3 ≈ -3.333 V.
    #[test]
    fn alter_plain_form_mutates_voltage_source() {
        let circuit =
            divider_with_control(vec!["alter v1 = -5".into(), "op".into(), "quit 0".into()]);
        let result = execute_control_block_ir(&circuit).expect("alter+op");
        assert_eq!(result.exit_code, 0);
        let v_mid = vec_value(&result, 0, "v(mid)");
        assert!(
            (v_mid - (-5.0 * 2.0 / 3.0)).abs() < 1e-6,
            "expected v(mid) ≈ -3.333 (from V1=-5), got {v_mid}"
        );
    }

    /// `alter @r1[resistance] = 4k` mutates R1's `value` param via the
    /// bracketed form. The divider becomes 4k:2k, so V(mid) = 1V * 2/(4+2) = 0.333 V.
    #[test]
    fn alter_bracketed_form_mutates_resistor_value() {
        let circuit = divider_with_control(vec![
            "alter @r1[value] = 4k".into(),
            "op".into(),
            "quit 0".into(),
        ]);
        let result = execute_control_block_ir(&circuit).expect("alter+op");
        assert_eq!(result.exit_code, 0);
        let v_mid = vec_value(&result, 0, "v(mid)");
        assert!(
            (v_mid - (2.0 / 6.0)).abs() < 1e-6,
            "expected v(mid) ≈ 0.333 (R1=4k, R2=2k), got {v_mid}"
        );
    }

    /// Without a Circuit on the SimContext (legacy `--legacy` SPICE
    /// callers), `alter` keeps its historical stored-vector behavior so
    /// callers don't observe a regression. We verify by constructing a
    /// SimContext via `new(netlist)` and inspecting the stashed vector
    /// directly — the `$&@v1[dc]` echo syntax only reads alphanumerics,
    /// so the bracketed key has to be looked up directly.
    #[test]
    fn alter_legacy_path_stashes_named_vector() {
        use crate::context::SimContext;
        use crate::{exec, parse};
        use thevenin_types::{Analysis, Netlist};

        let nl = Netlist {
            title: String::new(),
            items: vec![],
            analysis: Analysis::Op,
            source: String::new(),
        };
        let lines = vec!["alter @v1[dc] = 2.5".into(), "quit 0".into()];
        let stmts = parse::parse_control_block(&lines).unwrap();
        let mut ctx = SimContext::new(nl);
        exec::execute(&stmts, &mut ctx).unwrap();

        // Legacy path: no Circuit, so nothing was mutated; the value is
        // only available as a named vector.
        assert!(
            ctx.circuit.is_none(),
            "legacy ctor should leave circuit None"
        );
        let stash = ctx
            .find_vector("@v1[dc]")
            .expect("legacy stash for @v1[dc]");
        let v = match &stash.data {
            thevenin_types::VectorData::Real(r) => r[0],
            _ => panic!("expected real"),
        };
        assert!(
            (v - 2.5).abs() < 1e-12,
            "expected stashed v1[dc] = 2.5, got {v}"
        );
    }
}
