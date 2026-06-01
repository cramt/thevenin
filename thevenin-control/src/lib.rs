//! `.control` block interpreter for the thevenin circuit simulator.
//!
//! Parses and executes the ngspice `.control` / `.endc` scripting language
//! against a [`cirq_ir::Circuit`]: simulation commands (`op`, `dc`, `ac`,
//! `tran`, `run`, …), vector expressions and `print` / `let`, control flow
//! (`if` / `while` / `foreach` / `repeat`), `alter` of element and model
//! parameters, multi-run state (`resume`, `reset`), and result output
//! (`write` to ngspice raw / CSV).
//!
//! The interpreter is IR-native on the input side: simulation runs through
//! [`thevenin::circuit`](https://docs.rs/thevenin), `alter` mutates the
//! [`cirq_ir::Circuit`] directly, and analysis commands parse straight to
//! [`cirq_ir::Analysis`]. It depends on [`thevenin_types`] only for the
//! simulator's *result* types ([`SimResult`](thevenin_types::SimResult) and
//! friends).
//!
//! The typed `.control` statement AST and its parser live in
//! [`cirq_ir::control`]; this crate is the executor on top of them.

pub mod ast;
pub mod context;
pub mod exec;
pub mod language;
pub mod parse;
pub mod vecexpr;

use cirq_ir::Circuit;
use context::SimContext;
use exec::ControlResult;
use language::LanguageRegistry;
use thevenin_types::SimResult;

/// Check if a Cirq IR circuit contains a `.control` code block.
///
/// The Cirq IR stores `.control` source verbatim as a [`cirq_ir::CodeBlock`]
/// with `language == "control"`. This is the back-compat convenience over
/// [`has_code_block_ir`] with the [default](LanguageRegistry::default)
/// registry.
pub fn has_control_block_ir(circuit: &Circuit) -> bool {
    has_code_block_ir(circuit, &LanguageRegistry::default())
}

/// Whether `circuit` has any code block whose language `registry` can execute.
pub fn has_code_block_ir(circuit: &Circuit, registry: &LanguageRegistry) -> bool {
    circuit
        .code_blocks
        .iter()
        .any(|b| registry.contains(&b.language))
}

/// Execute a `.control` block from a Cirq IR circuit.
///
/// Back-compat convenience over [`execute_code_blocks_ir`] with the
/// [default](LanguageRegistry::default) registry (which handles only
/// `"control"`).
pub fn execute_control_block_ir(circuit: &Circuit) -> Result<ControlResult, String> {
    execute_code_blocks_ir(circuit, &LanguageRegistry::default())
}

/// Execute every embedded code block in `circuit`, routing each through the
/// handler `registry` registers for its language tag.
///
/// Builds one [`SimContext`] via [`SimContext::from_circuit`] (so the analysis
/// dispatcher in `exec.rs` routes Op / Dc / Tran / Ac through
/// [`thevenin::circuit`]) and runs all blocks against it in declaration order,
/// stopping early if a handler sets [`SimContext::exit_code`]. Each handler
/// receives the IR's pre-parsed AST when present (see [`language`]).
///
/// Returns `Err` if the circuit has no executable code block, or if a block's
/// language has no registered handler — the latter should not happen when the
/// circuit was compiled with a matching `cirq_frontend::LanguageRegistry`.
pub fn execute_code_blocks_ir(
    circuit: &Circuit,
    registry: &LanguageRegistry,
) -> Result<ControlResult, String> {
    let blocks: Vec<&cirq_ir::CodeBlock> = circuit
        .code_blocks
        .iter()
        .filter(|b| registry.contains(&b.language))
        .collect();

    if blocks.is_empty() {
        return Err("no executable code block found".to_string());
    }

    let mut ctx = SimContext::from_circuit(circuit.clone())?;

    for block in blocks {
        let handler = registry.handler(&block.language).ok_or_else(|| {
            format!(
                "no handler registered for code block language {:?}",
                block.language
            )
        })?;
        handler.execute(&block.lines, block.parsed.as_deref(), &mut ctx)?;
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
            csparams: Vec::<ResolvedParam>::new(),
            options: vec![],
            temps: vec![],
            save: vec![],
            funcs: vec![],
            initial_conditions: vec![],
            nodeset: vec![],
            measures: vec![],
            code_blocks: vec![CodeBlock::from_lines("control", control)],
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
            code_blocks: vec![CodeBlock::from_lines("scheme", vec!["(display 42)".into()])],
            ..without
        };
        assert!(!has_control_block_ir(&other));
    }

    /// An empty `.control` block must surface a clear error rather than
    /// being silently treated as a no-op.
    #[test]
    fn ir_entry_point_errors_when_no_control_block() {
        let mut circuit = divider_with_control(vec!["op".into(), "quit 0".into()]);
        circuit.code_blocks.clear();
        let err = match execute_control_block_ir(&circuit) {
            Ok(_) => panic!("expected error for missing .control block"),
            Err(e) => e,
        };
        assert!(
            err.contains("no executable code block"),
            "expected missing-code-block error, got: {err}"
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

    /// When `alter` targets a device the driving Circuit doesn't have, it
    /// falls back to stashing the value as a named vector so subsequent
    /// `find_vector("@device[param]")` lookups still resolve. We exercise
    /// the no-Circuit branch by constructing a [`SimContext`] without one,
    /// which is the lightest way to reach the fallback path.
    #[test]
    fn alter_fallback_stashes_named_vector() {
        use crate::context::SimContext;
        use crate::{exec, parse};

        let lines = vec!["alter @v1[dc] = 2.5".into(), "quit 0".into()];
        let stmts = parse::parse_control_block(&lines).unwrap();
        let mut ctx = SimContext::new();
        exec::execute(&stmts, &mut ctx).unwrap();

        let stash = ctx
            .find_vector("@v1[dc]")
            .expect("alter fallback stashed value as named vector");
        let v = match &stash.data {
            thevenin_types::VectorData::Real(r) => r[0],
            _ => panic!("expected real"),
        };
        assert!(
            (v - 2.5).abs() < 1e-12,
            "expected stashed v1[dc] = 2.5, got {v}"
        );
    }

    // -----------------------------------------------------------------------
    // Typed-control-AST plumbing
    // -----------------------------------------------------------------------

    /// `CodeBlock::from_lines` populates `parsed` for control blocks. Other
    /// languages stay unparsed.
    #[test]
    fn code_block_from_lines_parses_control_only() {
        let control = CodeBlock::from_lines(
            "control",
            vec!["op".into(), "let gain = v(out)".into(), "quit 0".into()],
        );
        assert!(control.parsed.is_some(), "control block must be parsed");
        let stmts = control.parsed.as_ref().unwrap();
        assert_eq!(stmts.len(), 3);
        assert!(matches!(
            stmts[0],
            cirq_ir::control::Statement::RunAnalysis(_)
        ));
        assert!(matches!(stmts[1], cirq_ir::control::Statement::Let { .. }));
        assert!(matches!(
            stmts[2],
            cirq_ir::control::Statement::Quit(Some(0))
        ));

        let other = CodeBlock::from_lines("scheme", vec!["(display 42)".into()]);
        assert!(other.parsed.is_none(), "non-control blocks stay unparsed");
    }

    /// The executor honors the IR's pre-parsed AST: if `parsed` is `Some`
    /// but `lines` is garbage, execution still succeeds — and conversely,
    /// the fallback re-parser fires when `parsed` is `None`.
    #[test]
    fn executor_prefers_parsed_form_over_lines() {
        // Build a circuit whose CodeBlock has a real parsed AST but
        // `lines` that would fail to parse if re-tokenized.
        let mut circuit = divider_with_control(vec!["op".into(), "quit 0".into()]);
        let parsed = circuit.code_blocks[0].parsed.clone();
        circuit.code_blocks[0].lines = vec!["@@ not valid control @@".into()];
        circuit.code_blocks[0].parsed = parsed;

        let result = execute_control_block_ir(&circuit).expect("typed path runs");
        assert_eq!(result.exit_code, 0);
        let v_mid = vec_value(&result, 0, "v(mid)");
        assert!((v_mid - 2.0 / 3.0).abs() < 1e-6, "got {v_mid}");
    }

    /// When `parsed` is `None`, the executor falls back to parsing `lines`
    /// — so the same circuit still runs even if `from_lines` was bypassed.
    #[test]
    fn executor_falls_back_to_lines_when_parsed_is_none() {
        let mut circuit = divider_with_control(vec!["op".into(), "quit 0".into()]);
        circuit.code_blocks[0].parsed = None;
        let result = execute_control_block_ir(&circuit).expect("fallback path runs");
        assert_eq!(result.exit_code, 0);
        let v_mid = vec_value(&result, 0, "v(mid)");
        assert!((v_mid - 2.0 / 3.0).abs() < 1e-6, "got {v_mid}");
    }

    /// `.csparam` entries land in the `.control` block's variable scope:
    /// `echo $x` prints the seeded value.
    #[test]
    fn csparam_seeded_into_control_scope() {
        let mut circuit = divider_with_control(vec!["echo $x".into(), "quit 0".into()]);
        circuit.csparams = vec![ResolvedParam {
            name: "x".into(),
            value: Value::Real(42.0),
        }];
        let result = execute_control_block_ir(&circuit).expect("control runs");
        assert_eq!(result.exit_code, 0);
        assert!(
            result.output.contains("42"),
            "expected echo of csparam x=42, got: {:?}",
            result.output
        );
    }

    /// When `.csparam` and `.param` collide on a name, `.csparam` wins in
    /// the control scope (ngspice behaviour).
    #[test]
    fn csparam_overrides_param_on_name_collision() {
        let mut circuit = divider_with_control(vec!["echo $x".into(), "quit 0".into()]);
        circuit.params = vec![ResolvedParam {
            name: "x".into(),
            value: Value::Real(1.0),
        }];
        circuit.csparams = vec![ResolvedParam {
            name: "x".into(),
            value: Value::Real(99.0),
        }];
        let result = execute_control_block_ir(&circuit).expect("control runs");
        // `.param` must not leak into the variable scope; only `.csparam`
        // is seeded. The output must reflect the csparam value.
        assert!(
            result.output.contains("99"),
            "expected csparam to win, got: {:?}",
            result.output
        );
    }

    // -----------------------------------------------------------------------
    // while / repeat / save (A4 checklist)
    // -----------------------------------------------------------------------

    /// `while` evaluates its condition before each iteration and stops once
    /// it becomes false. Counter starts at 5; loop decrements by 1; final
    /// value is 0 after exactly 5 iterations.
    ///
    /// Use `k` as the counter name because `i` collides with the built-in
    /// imaginary-unit constant in the expression evaluator.
    #[test]
    fn while_decrements_counter() {
        use crate::context::SimContext;
        use crate::{exec, parse};

        let lines = vec![
            "let k = 5".into(),
            "let count = 0".into(),
            "while k > 0".into(),
            "  let k = k - 1".into(),
            "  let count = count + 1".into(),
            "end".into(),
        ];
        let stmts = parse::parse_control_block(&lines).unwrap();
        let mut ctx = SimContext::new();
        exec::execute(&stmts, &mut ctx).unwrap();

        let k = ctx
            .find_vector("k")
            .expect("loop variable k present after while");
        let count = ctx
            .find_vector("count")
            .expect("iteration counter present after while");
        assert_eq!(k.data.as_real(), &[0.0], "counter ran to zero");
        assert_eq!(count.data.as_real(), &[5.0], "ran exactly 5 iterations");
    }

    /// A condition that's false from the start runs the body zero times.
    #[test]
    fn while_condition_false_from_start_runs_zero_times() {
        use crate::context::SimContext;
        use crate::{exec, parse};

        let lines = vec![
            "let count = 0".into(),
            "while 0".into(),
            "  let count = count + 1".into(),
            "end".into(),
        ];
        let stmts = parse::parse_control_block(&lines).unwrap();
        let mut ctx = SimContext::new();
        exec::execute(&stmts, &mut ctx).unwrap();

        let count = ctx.find_vector("count").unwrap();
        assert_eq!(count.data.as_real(), &[0.0]);
    }

    /// A `while` whose condition never goes false must hit the iteration cap
    /// and surface a clear error rather than hang.
    #[test]
    fn while_runaway_loop_caps_at_max_iters() {
        use crate::context::SimContext;
        use crate::{exec, parse};

        let lines = vec![
            "let k = 0".into(),
            "while 1".into(),
            "  let k = k + 1".into(),
            "end".into(),
        ];
        let stmts = parse::parse_control_block(&lines).unwrap();
        let mut ctx = SimContext::new();
        let result = exec::execute(&stmts, &mut ctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("MAX_LOOP_ITERS"),
            "expected MAX_LOOP_ITERS in error, got: {err}"
        );
    }

    /// A body that references an undefined vector errors out, and the error
    /// propagates from `execute` rather than being silently swallowed.
    #[test]
    fn while_body_error_propagates() {
        use crate::context::SimContext;
        use crate::{exec, parse};

        let lines = vec![
            "let k = 2".into(),
            "while k > 0".into(),
            "  let bogus = no_such_vec + 1".into(),
            "  let k = k - 1".into(),
            "end".into(),
        ];
        let stmts = parse::parse_control_block(&lines).unwrap();
        let mut ctx = SimContext::new();
        let result = exec::execute(&stmts, &mut ctx);
        assert!(result.is_err(), "expected body error to propagate");
        let err = result.unwrap_err();
        assert!(
            err.contains("no_such_vec") || err.contains("undefined"),
            "expected undefined-vector error, got: {err}"
        );
    }

    /// `repeat N` runs the body exactly N times. Counter starts at 0;
    /// after `repeat 3` of `count = count + 1`, count is 3.
    #[test]
    fn repeat_runs_body_n_times() {
        use crate::context::SimContext;
        use crate::{exec, parse};

        let lines = vec![
            "let count = 0".into(),
            "repeat 3".into(),
            "  let count = count + 1".into(),
            "end".into(),
        ];
        let stmts = parse::parse_control_block(&lines).unwrap();
        let mut ctx = SimContext::new();
        exec::execute(&stmts, &mut ctx).unwrap();

        let count = ctx.find_vector("count").unwrap();
        assert_eq!(count.data.as_real(), &[3.0]);
    }

    /// `repeat 0` and `repeat -5` both run the body zero times — matches
    /// ngspice's permissive behaviour.
    #[test]
    fn repeat_non_positive_count_runs_zero_times() {
        use crate::context::SimContext;
        use crate::{exec, parse};

        for count_expr in ["0", "-5"] {
            let lines = vec![
                "let count = 0".into(),
                format!("repeat {count_expr}"),
                "  let count = count + 1".into(),
                "end".into(),
            ];
            let stmts = parse::parse_control_block(&lines).unwrap();
            let mut ctx = SimContext::new();
            exec::execute(&stmts, &mut ctx).unwrap();

            let count = ctx.find_vector("count").unwrap();
            assert_eq!(
                count.data.as_real(),
                &[0.0],
                "repeat {count_expr} must skip body entirely"
            );
        }
    }

    /// `repeat` re-evaluates `count` only once at entry; mutating the
    /// referenced variable inside the body must not change the loop count.
    #[test]
    fn repeat_count_evaluated_once_at_entry() {
        use crate::context::SimContext;
        use crate::{exec, parse};

        // Inside the body we mutate `n`, but `repeat n` should run exactly
        // 3 times because `n` was 3 at entry.
        let lines = vec![
            "let n = 3".into(),
            "let count = 0".into(),
            "repeat n".into(),
            "  let count = count + 1".into(),
            "  let n = 0".into(),
            "end".into(),
        ];
        let stmts = parse::parse_control_block(&lines).unwrap();
        let mut ctx = SimContext::new();
        exec::execute(&stmts, &mut ctx).unwrap();

        let count = ctx.find_vector("count").unwrap();
        assert_eq!(count.data.as_real(), &[3.0]);
    }

    /// `save v(out)` inside `.control` appends to the driving circuit's
    /// save list so the next `op` (and any future analysis) honours it.
    #[test]
    fn save_appends_to_circuit_save_list() {
        let circuit = divider_with_control(vec![
            "save v(mid)".into(),
            "save i(v1)".into(),
            "op".into(),
            "quit 0".into(),
        ]);
        let result = execute_control_block_ir(&circuit).expect("op runs");
        assert_eq!(result.exit_code, 0);
        // The op plot still contains v(mid) — save doesn't prune what the
        // analysis would otherwise produce. The important assertion is the
        // circuit's save list is populated. We can re-execute against a
        // fresh context to inspect the circuit, but the ctx consumed the
        // circuit by value. Easier: directly drive the executor with a
        // fresh SimContext and inspect the mutated circuit through it.
        use crate::context::SimContext;
        use crate::{exec, parse};

        let lines = vec!["save v(mid) i(v1)".into(), "save v(out)".into()];
        let stmts = parse::parse_control_block(&lines).unwrap();
        let mut ctx = SimContext::from_circuit(circuit).unwrap();
        exec::execute(&stmts, &mut ctx).unwrap();

        let circuit_after = ctx.circuit().expect("circuit still attached");
        assert!(
            circuit_after.save.iter().any(|s| s == "v(mid)"),
            "v(mid) recorded, got {:?}",
            circuit_after.save
        );
        assert!(
            circuit_after.save.iter().any(|s| s == "i(v1)"),
            "i(v1) recorded, got {:?}",
            circuit_after.save
        );
        assert!(
            circuit_after.save.iter().any(|s| s == "v(out)"),
            "v(out) recorded, got {:?}",
            circuit_after.save
        );
    }

    /// Repeated `save` of the same spec is deduplicated; the list grows
    /// monotonically without redundant entries.
    #[test]
    fn save_dedupes_repeated_specs() {
        use crate::context::SimContext;
        use crate::{exec, parse};

        let circuit = divider_with_control(vec![]);
        let mut ctx = SimContext::from_circuit(circuit).unwrap();
        let lines = vec!["save v(mid)".into(), "save v(mid)".into()];
        let stmts = parse::parse_control_block(&lines).unwrap();
        exec::execute(&stmts, &mut ctx).unwrap();

        let circuit_after = ctx.circuit().unwrap();
        let count = circuit_after
            .save
            .iter()
            .filter(|s| s.as_str() == "v(mid)")
            .count();
        assert_eq!(count, 1, "duplicate save specs deduped");
    }

    /// End-to-end: `.csparam` parsed from SPICE text survives all the way
    /// into the control-block executor's variable scope.
    #[test]
    fn csparam_round_trip_from_spice_to_control() {
        let spice = "\
csparam round trip
.csparam gain=2.5
V1 in 0 1
R1 in mid 1k
R2 mid 0 1k
.control
echo $gain
quit 0
.endc
.op
.end
";
        let circuits = cirq_spice_import::import_spice(spice).expect("import");
        let result = execute_control_block_ir(&circuits[0]).expect("control runs");
        assert_eq!(result.exit_code, 0);
        assert!(
            result.output.contains("2.5"),
            "expected gain echoed as 2.5, got: {:?}",
            result.output
        );
    }

    /// The `write` control command should drop a real file on disk. Using a
    /// unique temp filename so parallel test runs don't collide.
    #[test]
    fn write_command_emits_raw_file() {
        let path =
            std::env::temp_dir().join(format!("thevenin_write_test_{}.raw", std::process::id()));
        let path_str = path.to_string_lossy().into_owned();
        // Clean up any stale fixture from a previous failed run.
        let _ = std::fs::remove_file(&path);

        let circuit = divider_with_control(vec![
            "op".into(),
            format!("write {path_str}"),
            "quit 0".into(),
        ]);
        let result = execute_control_block_ir(&circuit).expect("control runs");
        assert_eq!(result.exit_code, 0);

        let bytes = std::fs::read(&path).expect("raw file written");
        // Binary raw by default — header is text, payload is bytes.
        let header_end = bytes
            .windows(b"Binary:\n".len())
            .position(|w| w == b"Binary:\n")
            .expect("Binary: marker present");
        let header = std::str::from_utf8(&bytes[..header_end]).expect("header is utf-8");
        assert!(header.contains("Plotname: Operating Point"));
        assert!(header.contains("v(mid)"));
        let _ = std::fs::remove_file(&path);
    }

    // -----------------------------------------------------------------------
    // source / measure / vector-arithmetic (A4 polish, R6)
    // -----------------------------------------------------------------------

    /// `source <path>` opens the named file, parses it as a `.control` block,
    /// and runs its statements in the current context. State produced by the
    /// sub-script (variables, vectors) is visible to the calling script
    /// after `source` returns.
    #[test]
    fn source_command_executes_sub_script() {
        use std::io::Write;
        let dir = std::env::temp_dir();
        let path = dir.join(format!("thevenin_source_basic_{}.cs", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "let nested = 7").unwrap();
        writeln!(f, "echo nested-was-here").unwrap();
        drop(f);

        let path_str = path.to_string_lossy().into_owned();
        let circuit = divider_with_control(vec![format!("source {path_str}"), "quit 0".into()]);
        let result = execute_control_block_ir(&circuit).expect("source runs");
        assert_eq!(result.exit_code, 0);
        assert!(
            result.output.contains("nested-was-here"),
            "echo from sourced script must appear in output, got: {:?}",
            result.output
        );
        let _ = std::fs::remove_file(&path);
    }

    /// `source` on a nonexistent file surfaces a clear error rather than
    /// being silently swallowed.
    #[test]
    fn source_command_missing_file_errors() {
        let circuit = divider_with_control(vec![
            "source /nonexistent/path/that/does/not/exist.cs".into(),
            "quit 0".into(),
        ]);
        let err = execute_control_block_ir(&circuit).err().unwrap_or_default();
        assert!(
            err.contains("cannot read") || err.contains("source"),
            "expected missing-file error, got: {err}"
        );
    }

    /// A sub-script that sources itself is rejected by the recursion guard
    /// before the OS stack overflows.
    #[test]
    fn source_command_recursion_guard() {
        use std::io::Write;
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "thevenin_source_recursive_{}.cs",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "source {}", path.display()).unwrap();
        drop(f);

        let path_str = path.to_string_lossy().into_owned();
        let circuit = divider_with_control(vec![format!("source {path_str}"), "quit 0".into()]);
        let err = execute_control_block_ir(&circuit).err().unwrap_or_default();
        assert!(
            err.contains("recursive"),
            "expected recursion guard error, got: {err}"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// `measure tran vmax MAX v(out)` evaluates the measurement against the
    /// current plot and records the result. We use a divider OP and a custom
    /// "tran" plot since op is the only ready-to-run analysis on the fixture.
    #[test]
    fn measure_command_records_result_in_measurements_plot() {
        use crate::context::SimContext;
        use crate::{exec, parse};
        use thevenin_types::{SimPlot, SimVector, VectorData};

        let mut ctx = SimContext::new();
        // Synthesise a tran plot so the measure command has something to bind to.
        ctx.add_plot(SimPlot {
            name: "tran1".to_string(),
            vecs: vec![
                SimVector {
                    name: "time".to_string(),
                    data: VectorData::Real(vec![0.0, 1.0, 2.0, 3.0]),
                },
                SimVector {
                    name: "v(out)".to_string(),
                    data: VectorData::Real(vec![0.0, 1.5, 3.0, 1.0]),
                },
            ],
        });

        let lines = vec![
            "measure tran vmax MAX v(out)".to_string(),
            "echo vmax=$vmax".to_string(),
        ];
        let stmts = parse::parse_control_block(&lines).unwrap();
        exec::execute(&stmts, &mut ctx).expect("measure runs");

        let meas_plot = ctx
            .plots
            .iter()
            .find(|p| p.name == "measurements")
            .expect("measurements plot created");
        let vmax = meas_plot
            .vecs
            .iter()
            .find(|v| v.name == "vmax")
            .expect("vmax recorded");
        let real = vmax.data.as_real();
        assert_eq!(real.len(), 1, "vmax is scalar");
        assert!(
            (real[0] - 3.0).abs() < 1e-12,
            "expected MAX v(out) = 3.0, got {}",
            real[0]
        );
        assert!(
            ctx.output.contains("vmax=3"),
            "echo $vmax should reflect the recorded measurement, got: {:?}",
            ctx.output
        );
    }

    /// `print v(out)*2` produces a vector with each element doubled.
    #[test]
    fn print_supports_scalar_broadcast_vector_arithmetic() {
        use crate::context::SimContext;
        use crate::{exec, parse};
        use thevenin_types::{SimPlot, SimVector, VectorData};

        let mut ctx = SimContext::new();
        ctx.add_plot(SimPlot {
            name: "tran1".to_string(),
            vecs: vec![SimVector {
                name: "v(out)".to_string(),
                data: VectorData::Real(vec![1.0, 2.0, 3.0]),
            }],
        });

        let lines = vec!["print v(out)*2".to_string()];
        let stmts = parse::parse_control_block(&lines).unwrap();
        exec::execute(&stmts, &mut ctx).expect("print runs");
        // Each formatted entry should be twice the original: 2, 4, 6.
        // The exec formatter uses "[i] = <value>" per element; just check
        // for "2.000000e0", "4.000000e0", "6.000000e0".
        assert!(
            ctx.output.contains("2.000000e0"),
            "doubled v(out)[0]=2 missing: {:?}",
            ctx.output
        );
        assert!(
            ctx.output.contains("4.000000e0"),
            "doubled v(out)[1]=4 missing: {:?}",
            ctx.output
        );
        assert!(
            ctx.output.contains("6.000000e0"),
            "doubled v(out)[2]=6 missing: {:?}",
            ctx.output
        );
    }

    /// `print v(out) .* v(out)` uses the dotted element-wise form and is
    /// equivalent to the bare `*` in this evaluator. Squaring [1, 2, 3]
    /// gives [1, 4, 9].
    #[test]
    fn print_supports_dotted_elementwise_operators() {
        use crate::context::SimContext;
        use crate::{exec, parse};
        use thevenin_types::{SimPlot, SimVector, VectorData};

        let mut ctx = SimContext::new();
        ctx.add_plot(SimPlot {
            name: "tran1".to_string(),
            vecs: vec![SimVector {
                name: "v(out)".to_string(),
                data: VectorData::Real(vec![1.0, 2.0, 3.0]),
            }],
        });

        let lines = vec!["let sq = v(out) .* v(out)".to_string()];
        let stmts = parse::parse_control_block(&lines).unwrap();
        exec::execute(&stmts, &mut ctx).expect("let with dotted op runs");

        let sq = ctx.find_vector("sq").expect("sq stored");
        let real = sq.data.as_real();
        assert_eq!(real, &[1.0, 4.0, 9.0], "got {real:?}");
    }

    /// Range indexing `v[1:3]` selects the half-open slice [1, 3).
    #[test]
    fn print_supports_range_indexing() {
        use crate::context::SimContext;
        use crate::{exec, parse};
        use thevenin_types::{SimPlot, SimVector, VectorData};

        let mut ctx = SimContext::new();
        ctx.add_plot(SimPlot {
            name: "tran1".to_string(),
            vecs: vec![SimVector {
                name: "v(out)".to_string(),
                data: VectorData::Real(vec![10.0, 20.0, 30.0, 40.0, 50.0]),
            }],
        });

        let lines = vec!["let slice = v(out)[1:3]".to_string()];
        let stmts = parse::parse_control_block(&lines).unwrap();
        exec::execute(&stmts, &mut ctx).expect("range index runs");

        let slice = ctx.find_vector("slice").expect("slice stored");
        let real = slice.data.as_real();
        assert_eq!(real, &[20.0, 30.0], "got {real:?}");
    }

    /// `.csv` extension switches the writer to CSV regardless of `filetype`.
    #[test]
    fn write_command_csv_extension() {
        let path =
            std::env::temp_dir().join(format!("thevenin_write_test_{}.csv", std::process::id()));
        let path_str = path.to_string_lossy().into_owned();
        let _ = std::fs::remove_file(&path);

        let circuit = divider_with_control(vec![
            "op".into(),
            format!("write {path_str}"),
            "quit 0".into(),
        ]);
        let result = execute_control_block_ir(&circuit).expect("control runs");
        assert_eq!(result.exit_code, 0);

        let text = std::fs::read_to_string(&path).expect("csv written");
        let mut lines = text.lines();
        let header = lines.next().expect("header row");
        assert!(
            header.contains("v(mid)"),
            "csv header lists v(mid): {header:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    // -----------------------------------------------------------------------
    // Language registry (B4 — execution half)
    // -----------------------------------------------------------------------

    use crate::language::{LanguageHandler, LanguageRegistry};

    /// A stub handler that records that it ran and emits a marker into the
    /// context's output, proving custom languages route through the registry.
    struct MarkerHandler;
    impl LanguageHandler for MarkerHandler {
        fn execute(
            &self,
            lines: &[String],
            _parsed: Option<&[cirq_ir::control::Statement]>,
            ctx: &mut SimContext,
        ) -> Result<(), String> {
            ctx.output.push_str(&format!("marker:{}", lines.join("|")));
            ctx.exit_code = Some(7);
            Ok(())
        }
    }

    /// A custom handler registered for a non-`control` tag is invoked, and its
    /// side effects (output, exit code) land on the shared context.
    #[test]
    fn custom_handler_routes_and_mutates_context() {
        let mut circuit = divider_with_control(vec!["op".into(), "quit 0".into()]);
        // Replace the control block with a custom-language block.
        circuit.code_blocks = vec![CodeBlock::from_lines("marker", vec!["hello".into()])];

        let mut registry = LanguageRegistry::empty();
        registry.register("marker", Box::new(MarkerHandler));

        let result = execute_code_blocks_ir(&circuit, &registry).expect("custom handler runs");
        assert_eq!(result.exit_code, 7);
        assert!(
            result.output.contains("marker:hello"),
            "expected marker output, got: {:?}",
            result.output
        );
    }

    /// `has_code_block_ir` reports whether the registry can execute any block.
    #[test]
    fn has_code_block_ir_respects_registry() {
        let circuit = Circuit {
            code_blocks: vec![CodeBlock::from_lines("marker", vec!["x".into()])],
            ..divider_with_control(vec![])
        };
        // Default registry only knows "control" → no executable block.
        assert!(!has_code_block_ir(&circuit, &LanguageRegistry::default()));
        // A registry with the marker handler sees it.
        let mut registry = LanguageRegistry::empty();
        registry.register("marker", Box::new(MarkerHandler));
        assert!(has_code_block_ir(&circuit, &registry));
    }

    /// `tags()` exposes the registered languages so a host can mirror them into
    /// the frontend's compile-time registry.
    #[test]
    fn registry_tags_lists_registered_languages() {
        let mut registry = LanguageRegistry::with_control();
        registry.register("scheme", Box::new(MarkerHandler));
        let mut tags: Vec<&str> = registry.tags().collect();
        tags.sort_unstable();
        assert_eq!(tags, vec!["control", "scheme"]);
    }
}
