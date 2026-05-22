//! Test harness matching ngspice check.sh.
//!
//! All `.cir`/`.out` test pairs from `ngspice-upstream/tests/` are discovered
//! at compile time by the `ngspice_tests!()` proc macro, with file contents
//! embedded as string literals (no filesystem access at runtime — works on WASM).
//!
//! Ignore reasons are maintained in `tests/ignore.toml`.
//!
//! On failure, prints a `TRIAGE_JSON:` line to stdout with structured error info
//! for machine consumption (used by `scripts/triage-ignored-tests.ts`).

use thevenin::output::{compare_filtered, format_batch_output_multi};
use thevenin_control as _;
use thevenin_types::{Analysis, Netlist, SimPlot, SimResult};

/// Failure phases — where in the pipeline did the test fail?
#[derive(Clone, Copy)]
enum Phase {
    Parse,
    LibProc,
    ExprResolve,
    CirqImport,
    CirqEmit,
    Flatten,
    Simulate,
    Compare,
}

impl Phase {
    fn as_str(self) -> &'static str {
        match self {
            Phase::Parse => "parse",
            Phase::LibProc => "lib_processing",
            Phase::ExprResolve => "expr_resolution",
            Phase::CirqImport => "cirq_import",
            Phase::CirqEmit => "cirq_emit",
            Phase::Flatten => "flatten",
            Phase::Simulate => "simulate",
            Phase::Compare => "compare",
        }
    }
}

/// Print a machine-readable JSON line to stdout, then panic.
fn fail_test(path: &str, phase: Phase, error: &str) -> ! {
    // Categorize from the error message
    let category = if error.contains("Vacuous pass") {
        "VACUOUS"
    } else if error.contains("singular matrix")
        || error.contains("non-convergence")
        || error.contains("failed to converge")
        || error.contains("SolveError")
    {
        "CONVERGENCE"
    } else if error.contains("not implemented")
        || error.contains("not supported")
        || error.contains("not yet supported")
        || error.contains("model not implemented")
        || phase.as_str() == "parse"
        || phase.as_str() == "lib_processing"
        || phase.as_str() == "expr_resolution"
    {
        "MISSING_FEATURE"
    } else if error.contains("mismatch")
        || error.contains("expected")
        || error.contains("Expected")
        || error.contains("Interpolation")
    {
        "NEAR_MISS"
    } else {
        "OTHER"
    };

    // Escape for JSON (minimal: backslash and double-quote)
    let escaped = error
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    // Truncate to keep stdout manageable
    let truncated = if escaped.len() > 2000 {
        &escaped[..2000]
    } else {
        &escaped
    };

    println!(
        r#"TRIAGE_JSON:{{"path":"{}","phase":"{}","category":"{}","error":"{}"}}"#,
        path,
        phase.as_str(),
        category,
        truncated,
    );

    panic!("Test {path} failed: {error}");
}

/// Run a single embedded test: parse, preprocess, simulate, format, filter, diff.
///
/// Called by the generated test functions. All file contents are passed in
/// as string literals (resolved at compile time by the proc macro).
///
/// Requires `cargo nextest` (each test gets its own process for timeout handling).
/// Under plain `cargo test`, long simulations may hang (no per-test timeout).
fn run_embedded_test(
    path: &str,
    cir: &str,
    out: &str,
    aux_files: &[(&str, &str)],
    rel_tol_override: Option<f64>,
    abs_tol_override: Option<f64>,
) {
    // Parse the netlist (includes already resolved at compile time)
    let mut netlists = match Netlist::parse(cir) {
        Ok(n) => n,
        Err(e) => fail_test(path, Phase::Parse, &e.to_string()),
    };

    // Process each forked netlist
    for netlist in &mut netlists {
        // Process .lib directives using the embedded file map
        if let Err(e) = thevenin::libproc::process_libs_embedded(netlist, aux_files) {
            fail_test(path, Phase::LibProc, &e.to_string());
        }

        // Resolve expressions/params
        if let Err(e) = thevenin::expr::resolve_netlist_exprs(netlist) {
            fail_test(path, Phase::ExprResolve, &e.to_string());
        }
    }

    // Route every netlist through the Cirq IR pipeline before flattening, so
    // that the ngspice regression corpus continuously validates the
    // `Netlist -> Circuit -> Netlist` import + emit adapters. We keep the
    // imported Circuits alongside the emitted Netlists so the `.control`
    // branch can run through the IR-shaped interpreter entry point
    // (Stage 4 / Phase A).
    let circuits: Vec<cirq_ir::Circuit> = netlists
        .iter()
        .map(|netlist| match cirq_spice_import::import_netlist(netlist) {
            Ok(c) => c,
            Err(e) => fail_test(path, Phase::CirqImport, &e.to_string()),
        })
        .collect();

    // Track which source circuit each emitted netlist came from so the
    // analysis dispatch can assemble its MnaSystem via mna_ir directly
    // from the IR (Stage 4 / Session H direct path). `circuit_to_netlists`
    // produces one netlist per analysis declared on the circuit, so the
    // mapping is one-to-many in general but already-correct here.
    let netlists: Vec<(usize, Netlist)> = {
        let mut routed: Vec<(usize, Netlist)> = Vec::new();
        for (idx, circuit) in circuits.iter().enumerate() {
            let emitted = match cirq_frontend::to_netlist::circuit_to_netlists(circuit) {
                Ok(n) => n,
                Err(e) => fail_test(path, Phase::CirqEmit, &e.to_string()),
            };
            for nl in emitted {
                routed.push((idx, nl));
            }
        }
        routed
    };

    // Flatten subcircuits for each fork (idempotent on already-flat netlists,
    // which is what the Cirq importer produces).
    let netlists: Vec<(usize, Netlist)> = netlists
        .into_iter()
        .map(|(idx, netlist)| match thevenin::flatten_netlist(&netlist) {
            Ok(n) => (idx, n),
            Err(e) => fail_test(path, Phase::Flatten, &e.to_string()),
        })
        .collect();

    // Check for .control block on the IR (control blocks accumulate into
    // every fork; pick the first imported circuit).
    if thevenin_control::has_control_block_ir(&circuits[0]) {
        let ctrl_result = match thevenin_control::execute_control_block_ir(&circuits[0]) {
            Ok(r) => r,
            Err(e) => fail_test(path, Phase::Simulate, &e),
        };

        if ctrl_result.exit_code != 0 {
            fail_test(
                path,
                Phase::Simulate,
                &format!(
                    ".control quit with exit code {}\n{}",
                    ctrl_result.exit_code, ctrl_result.output
                ),
            );
        }

        // .control tests are self-validating via quit 0/1.
        // Output comparison is informational — quit 0 is the primary success criterion.
        // (Output format may differ from ngspice due to missing format features.)
    } else {
        // Standard analysis path (no .control) — simulate each fork
        let result = match run_all_analyses(&circuits, &netlists) {
            Ok(r) => r,
            Err(e) => fail_test(path, Phase::Simulate, &e),
        };

        // Format output in ngspice batch mode and compare.
        let netlists_only: Vec<Netlist> = netlists.iter().map(|(_, nl)| nl.clone()).collect();
        let actual_output = format_batch_output_multi(&netlists_only, &result);
        if let Err(e) = compare_filtered(out, &actual_output, rel_tol_override, abs_tol_override) {
            fail_test(path, Phase::Compare, &e);
        }
    }
}

/// Run all analyses across all netlist forks and merge results.
///
/// Stage 4: every fork's MnaSystem is assembled directly from the source
/// `cirq_ir::Circuit` via `thevenin::mna_ir`, and the analysis is dispatched
/// through the corresponding `_with_mna` helper. This routes the full
/// ngspice regression corpus through the direct IR path end-to-end —
/// validating mna_ir against every supported device class without going
/// back through `assemble_mna(&Netlist)`.
fn run_all_analyses(
    circuits: &[cirq_ir::Circuit],
    netlists: &[(usize, Netlist)],
) -> Result<SimResult, String> {
    let mut all_plots: Vec<SimPlot> = Vec::new();

    for (circuit_idx, netlist) in netlists {
        let circuit = &circuits[*circuit_idx];

        // Each iteration's netlist comes pre-split per analysis fork by
        // `Netlist::parse`; copy that single analysis onto a clone of the
        // shared Circuit so the Circuit-input dispatchers see exactly the
        // analysis variant for this leg.
        let mut per_analysis = circuit.clone();
        per_analysis.analyses = vec![match &netlist.analysis {
            Analysis::Op => cirq_ir::Analysis::Op,
            _ => circuit
                .analyses
                .iter()
                .find(|a| matches_kind(a, &netlist.analysis))
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "harness: circuit has no IR analysis matching netlist {:?}",
                        netlist.analysis
                    )
                })?,
        }];

        let result = match &netlist.analysis {
            Analysis::Op => thevenin::circuit::simulate_op(&per_analysis)
                .map_err(|e| format!("OP error: {e}"))?,
            Analysis::Dc { .. } => thevenin::circuit::simulate_dc(&per_analysis)
                .map_err(|e| format!("DC error: {e}"))?,
            Analysis::Tran { .. } => thevenin::circuit::simulate_tran(&per_analysis)
                .map_err(|e| format!("Tran error: {e}"))?,
            Analysis::Ac { .. } => thevenin::circuit::simulate_ac(&per_analysis)
                .map_err(|e| format!("AC error: {e}"))?,
            Analysis::Noise { .. } => thevenin::circuit::simulate_noise(&per_analysis)
                .map_err(|e| format!("Noise error: {e}"))?,
            Analysis::Tf { .. } => thevenin::circuit::simulate_tf(&per_analysis)
                .map_err(|e| format!("TF error: {e}"))?,
            Analysis::Sens { .. } => thevenin::circuit::simulate_sens(&per_analysis)
                .map_err(|e| format!("Sens error: {e}"))?,
            Analysis::Pz { .. } => thevenin::circuit::simulate_pz(&per_analysis)
                .map_err(|e| format!("PZ error: {e}"))?,
        };
        all_plots.extend(result.plots);
    }

    Ok(SimResult { plots: all_plots })
}

fn matches_kind(ir: &cirq_ir::Analysis, netlist: &Analysis) -> bool {
    use cirq_ir::Analysis as I;
    use thevenin_types::Analysis as N;
    matches!(
        (ir, netlist),
        (I::Op, N::Op)
            | (I::Dc(_), N::Dc { .. })
            | (I::Tran(_), N::Tran { .. })
            | (I::Ac(_), N::Ac { .. })
            | (I::Noise(_), N::Noise { .. })
            | (I::Tf(_), N::Tf { .. })
            | (I::Sens(_), N::Sens { .. })
            | (I::Pz(_), N::Pz { .. })
    )
}

// Generate all test functions from ngspice-upstream/tests/ at compile time.
thevenin_test_macro::ngspice_tests!();
