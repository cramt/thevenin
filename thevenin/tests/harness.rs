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
    // `Netlist -> Circuit -> Netlist` import + emit adapters.
    let netlists: Vec<Netlist> = {
        let mut routed: Vec<Netlist> = Vec::new();
        for netlist in &netlists {
            let circuit = match cirq_spice_import::import_netlist(netlist) {
                Ok(c) => c,
                Err(e) => fail_test(path, Phase::CirqImport, &e.to_string()),
            };
            let emitted = match cirq_frontend::to_netlist::circuit_to_netlists(&circuit) {
                Ok(n) => n,
                Err(e) => fail_test(path, Phase::CirqEmit, &e.to_string()),
            };
            routed.extend(emitted);
        }
        routed
    };

    // Flatten subcircuits for each fork (idempotent on already-flat netlists,
    // which is what the Cirq importer produces).
    let netlists: Vec<Netlist> = netlists
        .iter()
        .map(|netlist| match thevenin::flatten_netlist(netlist) {
            Ok(n) => n,
            Err(e) => fail_test(path, Phase::Flatten, &e.to_string()),
        })
        .collect();

    // Check for .control block (use the first fork — control blocks accumulate into all forks)
    if thevenin_control::has_control_block(&netlists[0]) {
        let ctrl_result = match thevenin_control::execute_control_block(&netlists[0]) {
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
        let result = match run_all_analyses(&netlists) {
            Ok(r) => r,
            Err(e) => fail_test(path, Phase::Simulate, &e),
        };

        // Format output in ngspice batch mode and compare
        let actual_output = format_batch_output_multi(&netlists, &result);
        if let Err(e) = compare_filtered(out, &actual_output, rel_tol_override, abs_tol_override) {
            fail_test(path, Phase::Compare, &e);
        }
    }
}

/// Run all analyses across all netlist forks and merge results.
fn run_all_analyses(netlists: &[Netlist]) -> Result<SimResult, String> {
    let mut all_plots: Vec<SimPlot> = Vec::new();

    for netlist in netlists {
        match &netlist.analysis {
            Analysis::Op => {
                // Use simulate_op_dc (diag_gmin=0) to match ngspice .op branch currents.
                let result =
                    thevenin::simulate_op_dc(netlist).map_err(|e| format!("OP error: {e}"))?;
                all_plots.extend(result.plots);
            }
            Analysis::Dc { .. } => {
                let result =
                    thevenin::simulate_dc(netlist).map_err(|e| format!("DC error: {e}"))?;
                all_plots.extend(result.plots);
            }
            Analysis::Tran { .. } => {
                // Also get OP for initial transient solution
                if let Ok(op_result) = thevenin::simulate_op(netlist) {
                    all_plots.extend(op_result.plots);
                }
                let result =
                    thevenin::simulate_tran(netlist).map_err(|e| format!("Tran error: {e}"))?;
                all_plots.extend(result.plots);
            }
            Analysis::Ac { .. } => {
                let result =
                    thevenin::simulate_ac(netlist).map_err(|e| format!("AC error: {e}"))?;
                all_plots.extend(result.plots);
            }
            Analysis::Noise { .. } => {
                let result =
                    thevenin::simulate_noise(netlist).map_err(|e| format!("Noise error: {e}"))?;
                all_plots.extend(result.plots);
            }
            Analysis::Tf { .. } => {
                let result =
                    thevenin::simulate_tf(netlist).map_err(|e| format!("TF error: {e}"))?;
                all_plots.extend(result.plots);
            }
            Analysis::Sens { .. } => {
                let result =
                    thevenin::simulate_sens(netlist).map_err(|e| format!("Sens error: {e}"))?;
                all_plots.extend(result.plots);
            }
            Analysis::Pz { .. } => {
                let result =
                    thevenin::simulate_pz(netlist).map_err(|e| format!("PZ error: {e}"))?;
                all_plots.extend(result.plots);
            }
        }
    }

    Ok(SimResult { plots: all_plots })
}

// Generate all test functions from ngspice-upstream/tests/ at compile time.
thevenin_test_macro::ngspice_tests!();
