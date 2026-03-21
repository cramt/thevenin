//! Test harness matching ngspice check.sh.
//!
//! All `.cir`/`.out` test pairs from `ngspice-upstream/tests/` are discovered
//! at compile time by the `ngspice_tests!()` proc macro, with file contents
//! embedded as string literals (no filesystem access at runtime — works on WASM).
//!
//! Ignore reasons are maintained in `tests/ignore.toml`.

use thevenin::output::{compare_filtered, format_batch_output};
use thevenin_types::{Analysis, Item, Netlist, SimResult};

/// Run a single embedded test: parse, preprocess, simulate, format, filter, diff.
///
/// Called by the generated test functions. All file contents are passed in
/// as string literals (resolved at compile time by the proc macro).
///
/// Requires `cargo nextest` (each test gets its own process for timeout handling).
/// Under plain `cargo test`, skips gracefully with a warning.
fn run_embedded_test(path: &str, cir: &str, out: &str, aux_files: &[(&str, &str)]) {
    if std::env::var_os("NEXTEST").is_none() {
        eprintln!("skipping {path}: harness tests require `cargo nextest run`");
        return;
    }

    // Parse the netlist (includes already resolved at compile time)
    let mut netlist =
        Netlist::parse(cir).unwrap_or_else(|e| panic!("Test {path} parse error: {e}"));

    // Process .lib directives using the embedded file map
    thevenin::libproc::process_libs_embedded(&mut netlist, aux_files)
        .unwrap_or_else(|e| panic!("Test {path} lib processing error: {e}"));

    // Resolve expressions/params
    thevenin::expr::resolve_netlist_exprs(&mut netlist)
        .unwrap_or_else(|e| panic!("Test {path} expression resolution error: {e}"));

    // Flatten subcircuits
    let netlist = thevenin::flatten_netlist(&netlist)
        .unwrap_or_else(|e| panic!("Test {path} subcircuit flattening error: {e}"));

    // Run all analyses and collect results
    let result =
        run_all_analyses(&netlist).unwrap_or_else(|e| panic!("Test {path} simulation error: {e}"));

    // Format output in ngspice batch mode and compare
    let actual_output = format_batch_output(&netlist, &result);
    if let Err(e) = compare_filtered(out, &actual_output) {
        panic!("Test {path} failed: {e}");
    }
}

/// Run all analyses found in the netlist and merge results.
fn run_all_analyses(netlist: &Netlist) -> Result<SimResult, String> {
    let analyses: Vec<&Analysis> = netlist
        .items
        .iter()
        .filter_map(|item| {
            if let Item::Analysis(a) = item {
                Some(a)
            } else {
                None
            }
        })
        .collect();

    let mut all_plots = Vec::new();

    // If there are no explicit analyses, try OP
    if analyses.is_empty() {
        let result =
            thevenin::simulate_op(netlist).map_err(|e| format!("OP simulation error: {e}"))?;
        return Ok(result);
    }

    // Track which multi-analysis simulation types have already run.
    // simulate_tf / simulate_sens process ALL matching analyses in the netlist,
    // so calling them once per directive would produce duplicates.
    let mut tf_done = false;
    let mut sens_done = false;

    for analysis in &analyses {
        match analysis {
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
                if !tf_done {
                    let result =
                        thevenin::simulate_tf(netlist).map_err(|e| format!("TF error: {e}"))?;
                    all_plots.extend(result.plots);
                    tf_done = true;
                }
            }
            Analysis::Sens { .. } => {
                if !sens_done {
                    let result =
                        thevenin::simulate_sens(netlist).map_err(|e| format!("Sens error: {e}"))?;
                    all_plots.extend(result.plots);
                    sens_done = true;
                }
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
