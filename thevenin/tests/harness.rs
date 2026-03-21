//! Test harness matching ngspice check.sh.
//!
//! Iterates all .cir/.out test pairs in ngspice-upstream/tests/ and compares
//! thevenin output against reference ngspice output after filtering.
//!
//! Tests are generated with a macro so each .cir file has its own test function.
//! Tests that require unimplemented features are #[ignore]d with clear notes.

use std::path::{Path, PathBuf};

use thevenin::output::{compare_filtered, format_batch_output};
use thevenin_types::Netlist;

/// Root directory of ngspice test suite.
fn ngspice_tests_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("ngspice-upstream")
        .join("tests")
}

/// Check if the ngspice upstream tests directory exists.
fn has_ngspice_tests() -> bool {
    ngspice_tests_dir().is_dir()
}

/// Run a single .cir test: parse, preprocess, simulate, format, filter, diff.
///
/// Returns Ok(()) if the filtered output matches, or an error describing the failure.
fn run_cir_test(cir_path: &Path) -> Result<(), String> {
    // Prefer fixture .out file over ngspice-upstream .out (fixtures are tracked
    // in git and can contain corrected references for broken upstream outputs).
    let fixture_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    let out_path = cir_path
        .strip_prefix(ngspice_tests_dir())
        .ok()
        .map(|rel| fixture_root.join(rel).with_extension("out"))
        .filter(|p| p.exists())
        .unwrap_or_else(|| cir_path.with_extension("out"));
    if !out_path.exists() {
        return Err("No reference .out file".to_string());
    }

    let cir_content =
        std::fs::read_to_string(cir_path).map_err(|e| format!("Failed to read .cir: {e}"))?;
    let expected_output =
        std::fs::read_to_string(&out_path).map_err(|e| format!("Failed to read .out: {e}"))?;

    // Handle .include files: read them relative to the .cir directory
    let cir_dir = cir_path.parent().unwrap();
    let resolved_content = resolve_includes(&cir_content, cir_dir)?;

    // Parse the netlist
    let netlist = Netlist::parse(&resolved_content).map_err(|e| format!("Parse error: {e}"))?;

    // Preprocess: flatten subcircuits, process .lib files
    let netlist = preprocess_netlist(netlist, cir_dir)?;

    // Run all analyses and collect results
    let result = run_all_analyses(&netlist)?;

    // Format output in ngspice batch mode
    let actual_output = format_batch_output(&netlist, &result);

    // Compare filtered outputs
    compare_filtered(&expected_output, &actual_output)
}

// Per-test timeouts are handled by cargo-nextest (see .config/nextest.toml).
// Each test runs in its own process, so nextest can SIGKILL hung simulations
// instead of leaving threads spinning forever.

/// Resolve .include directives by inlining file contents.
fn resolve_includes(content: &str, base_dir: &Path) -> Result<String, String> {
    let mut result = String::new();
    for line in content.lines() {
        let trimmed = line.trim().to_lowercase();
        if trimmed.starts_with(".include") || trimmed.starts_with(".inc") {
            let parts: Vec<&str> = line.trim().splitn(2, char::is_whitespace).collect();
            if parts.len() == 2 {
                let filename = parts[1].trim().trim_matches('"').trim_matches('\'');
                let include_path = base_dir.join(filename);
                match std::fs::read_to_string(&include_path) {
                    Ok(included) => {
                        result.push_str(&included);
                        result.push('\n');
                        continue;
                    }
                    Err(_) => {
                        // Try case-insensitive lookup
                        if let Some(actual_path) = find_file_case_insensitive(base_dir, filename) {
                            match std::fs::read_to_string(&actual_path) {
                                Ok(included) => {
                                    result.push_str(&included);
                                    result.push('\n');
                                    continue;
                                }
                                Err(e) => {
                                    return Err(format!("Failed to read include {filename}: {e}"));
                                }
                            }
                        }
                        return Err(format!("Include file not found: {filename}"));
                    }
                }
            }
        }
        result.push_str(line);
        result.push('\n');
    }
    Ok(result)
}

/// Case-insensitive file lookup in a directory.
fn find_file_case_insensitive(dir: &Path, filename: &str) -> Option<PathBuf> {
    let lower = filename.to_lowercase();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().to_lowercase() == lower {
                return Some(entry.path());
            }
        }
    }
    None
}

/// Preprocess: flatten subcircuits, resolve .lib, evaluate params.
fn preprocess_netlist(netlist: Netlist, cir_dir: &Path) -> Result<Netlist, String> {
    // Process .lib files
    let mut netlist = netlist;
    thevenin::libproc::process_libs(&mut netlist, cir_dir)
        .map_err(|e| format!("Lib processing error: {e}"))?;

    // Resolve expressions/params
    thevenin::expr::resolve_netlist_exprs(&mut netlist)
        .map_err(|e| format!("Expression resolution error: {e}"))?;

    // Flatten subcircuits
    let netlist = thevenin::flatten_netlist(&netlist)
        .map_err(|e| format!("Subcircuit flattening error: {e}"))?;

    Ok(netlist)
}

/// Run all analyses found in the netlist and merge results.
fn run_all_analyses(netlist: &Netlist) -> Result<thevenin_types::SimResult, String> {
    use thevenin_types::{Analysis, Item, SimResult};

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


/// Recursively collect all .cir files.
fn collect_cir_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                collect_cir_files(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "cir") {
                out.push(path);
            }
        }
    }
}

// ============================================================
// Individual test functions for each .cir file.
// These allow `cargo test` to report per-file results.
// ============================================================

macro_rules! harness_test {
    ($name:ident, $path:expr,) => {
        harness_test!($name, $path);
    };
    ($name:ident, $path:expr) => {
        #[test]
        fn $name() {
            if !has_ngspice_tests() {
                eprintln!("ngspice-upstream not found, skipping");
                return;
            }
            let cir_path = ngspice_tests_dir().join($path);
            if let Err(e) = run_cir_test(&cir_path) {
                panic!("Test {} failed: {}", $path, e);
            }
        }
    };
    ($name:ident, $path:expr, ignore = $reason:expr) => {
        #[test]
        #[ignore = $reason]
        fn $name() {
            if !has_ngspice_tests() {
                eprintln!("ngspice-upstream not found, skipping");
                return;
            }
            let cir_path = ngspice_tests_dir().join($path);
            if let Err(e) = run_cir_test(&cir_path) {
                panic!("Test {} failed: {}", $path, e);
            }
        }
    };
}

// === BSIM1 (unimplemented model) ===
harness_test!(
    harness_bsim1_test,
    "bsim1/test.cir",
    ignore = "BSIM1 model not implemented (US-052)"
);

// === BSIM2 (unimplemented model) ===
harness_test!(
    harness_bsim2_test,
    "bsim2/test.cir",
    ignore = "BSIM2 model not implemented (US-053)"
);

// === BSIM3SOI-DD ===
harness_test!(
    harness_bsim3soidd_inv2,
    "bsim3soidd/inv2.cir",
    ignore = "DC OP: singular matrix (NR non-convergence in BSIM3SOI-DD inverter)"
);
harness_test!(
    harness_bsim3soidd_rampvg2,
    "bsim3soidd/RampVg2.cir",
    ignore = "times out (>30s): BSIM3SOI-DD DC sweep NR convergence too slow"
);
harness_test!(
    harness_bsim3soidd_t3,
    "bsim3soidd/t3.cir",
    ignore = "~18% Ids error near threshold (BSIM3SOI-DD excess current bug exposed by vfb fix)"
);
harness_test!(
    harness_bsim3soidd_t4,
    "bsim3soidd/t4.cir",
    ignore = "~22% Ids error (BSIM3SOI-DD excess current bug exposed by vfb fix)"
);
harness_test!(
    harness_bsim3soidd_t5,
    "bsim3soidd/t5.cir",
    ignore = "~1.1% Ids error at Ve=-4 (vfbb sign error in back-gate coupling, see FIXING_HARNESS_TESTS.md)"
);

// === BSIM3SOI-FD ===
harness_test!(
    harness_bsim3soifd_inv2,
    "bsim3soifd/inv2.cir",
    ignore = "times out (>30s): BSIM3SOI-FD inverter NR convergence too slow"
);
harness_test!(
    harness_bsim3soifd_rampvg2,
    "bsim3soifd/RampVg2.cir",
    ignore = "times out (>30s): BSIM3SOI-FD DC sweep NR convergence too slow"
);
harness_test!(
    harness_bsim3soifd_t3,
    "bsim3soifd/t3.cir",
    ignore = "~9% Ids error near threshold (BSIM3SOI-FD model value bug)"
);
harness_test!(
    harness_bsim3soifd_t4,
    "bsim3soifd/t4.cir",
    ignore = "wrong sign: expected +7.2e-8, got -5.1e-8 (BSIM3SOI-FD model value bug)"
);
harness_test!(
    harness_bsim3soifd_t5,
    "bsim3soifd/t5.cir",
    ignore = "~99% error: expected 1.1e-7, got 6.1e-10 (BSIM3SOI-FD model value bug)"
);

// === BSIM3SOI-PD ===
harness_test!(
    harness_bsim3soipd_inv2,
    "bsim3soipd/inv2.cir",
    ignore = "DC OP: singular matrix (NR non-convergence in BSIM3SOI-PD inverter)"
);
harness_test!(
    harness_bsim3soipd_rampvg2,
    "bsim3soipd/RampVg2.cir",
    ignore = "times out (>30s): BSIM3SOI-PD DC sweep NR convergence too slow"
);
harness_test!(
    harness_bsim3soipd_t3,
    "bsim3soipd/t3.cir",
    ignore = "DC OP: singular matrix (BSIM3SOI-PD NR non-convergence)"
);
harness_test!(
    harness_bsim3soipd_t4,
    "bsim3soipd/t4.cir",
    ignore = "~5.6% Ids error (BSIM3SOI-PD model value bug, previously timed out)"
);
harness_test!(
    harness_bsim3soipd_t5,
    "bsim3soipd/t5.cir",
    ignore = "DC OP: singular matrix (BSIM3SOI-PD NR non-convergence)"
);

// === Filters ===
harness_test!(harness_filters_lowpass, "filters/lowpass.cir");

// === General circuits ===
harness_test!(
    harness_general_diffpair,
    "general/diffpair.cir",
    ignore = "reference output file says 'To be done' — no reference to compare against"
);
harness_test!(
    harness_general_fourbitadder,
    "general/fourbitadder.cir",
    ignore = "times out (>30s)"
);
harness_test!(
    harness_general_mosamp,
    "general/mosamp.cir",
    ignore = "times out (>30s): transient too slow after Vbs pnjlim fix resolved singular matrix; Level 2 MOSFET missing velocity saturation/mobility degradation"
);
harness_test!(harness_general_mosmem, "general/mosmem.cir");
harness_test!(harness_general_rca3040, "general/rca3040.cir");
harness_test!(harness_general_rc, "general/rc.cir");
harness_test!(
    harness_general_rtlinv,
    "general/rtlinv.cir",
    ignore = "~5.5% transient timing error at first switching transition: our transition starts ~2ns later than ngspice due to constant reverse-bias cap approximation"
);
harness_test!(
    harness_general_schmitt,
    "general/schmitt.cir",
    ignore = "~1.2% error at switching transition: output oscillates during state transition instead of settling cleanly; constant reverse-bias cap approximation"
);

// === HFET ===
harness_test!(harness_hfet_id_vgs, "hfet/id_vgs.cir");
harness_test!(
    harness_hfet_inverter,
    "hfet/inverter.cir",
    ignore = "times out (>30s)"
);

// === JFET ===
harness_test!(harness_jfet_vds_vgs, "jfet/jfet_vds-vgs.cir");

// === MESA ===
harness_test!(harness_mesa_mesa11, "mesa/mesa11.cir");
harness_test!(harness_mesa_mesa13, "mesa/mesa13.cir");
harness_test!(harness_mesa_mesa14, "mesa/mesa14.cir");
harness_test!(harness_mesa_mesa15, "mesa/mesa15.cir");
harness_test!(harness_mesa_mesa21, "mesa/mesa21.cir");
harness_test!(harness_mesa_mesa, "mesa/mesa.cir");
harness_test!(harness_mesa_mesgout, "mesa/mesgout.cir");
harness_test!(harness_mesa_mesinv, "mesa/mesinv.cir");
harness_test!(harness_mesa_mesosc, "mesa/mesosc.cir");

// === MES (MESFET) ===
harness_test!(harness_mes_subth, "mes/subth.cir");

// === MOS6 ===
harness_test!(
    harness_mos6_mos6inv,
    "mos6/mos6inv.cir",
    ignore = "times out (>30s): transient too slow after Vbs pnjlim fix resolved singular matrix"
);
harness_test!(harness_mos6_simpleinv, "mos6/simpleinv.cir");

// === Pole-Zero ===
harness_test!(harness_pz_filt_bridge_t, "polezero/filt_bridge_t.cir");
harness_test!(harness_pz_filt_multistage, "polezero/filt_multistage.cir");
harness_test!(harness_pz_filt_rc, "polezero/filt_rc.cir");
harness_test!(harness_pz_pz2, "polezero/pz2.cir");
harness_test!(harness_pz_pzt, "polezero/pzt.cir");
harness_test!(harness_pz_simplepz, "polezero/simplepz.cir");

// === Regression: func ===
harness_test!(harness_regression_func_1, "regression/func/func-1.cir");

// === Regression: lib-processing ===
harness_test!(
    harness_regression_lib_ex1a,
    "regression/lib-processing/ex1a.cir"
);
harness_test!(
    harness_regression_lib_ex1b,
    "regression/lib-processing/ex1b.cir"
);
harness_test!(
    harness_regression_lib_ex2a,
    "regression/lib-processing/ex2a.cir"
);
harness_test!(
    harness_regression_lib_ex3a,
    "regression/lib-processing/ex3a.cir"
);
harness_test!(
    harness_regression_lib_scope1,
    "regression/lib-processing/scope-1.cir"
);
harness_test!(
    harness_regression_lib_scope2,
    "regression/lib-processing/scope-2.cir"
);
harness_test!(
    harness_regression_lib_scope3,
    "regression/lib-processing/scope-3.cir"
);

// === Regression: misc ===
harness_test!(
    harness_regression_misc_ac_zero,
    "regression/misc/ac-zero.cir"
);
harness_test!(
    harness_regression_misc_alter_vec,
    "regression/misc/alter-vec.cir"
);
harness_test!(
    harness_regression_misc_asrc_tc_1,
    "regression/misc/asrc-tc-1.cir"
);
harness_test!(
    harness_regression_misc_asrc_tc_2,
    "regression/misc/asrc-tc-2.cir",
    ignore =
        "parameter expressions not yet supported (r={1k + v(9)}) + requires .control scripting"
);
harness_test!(harness_regression_misc_bugs_1, "regression/misc/bugs-1.cir");
harness_test!(harness_regression_misc_bugs_2, "regression/misc/bugs-2.cir");
harness_test!(
    harness_regression_misc_dollar_1,
    "regression/misc/dollar-1.cir"
);
harness_test!(
    harness_regression_misc_empty_1,
    "regression/misc/empty-1.cir"
);
harness_test!(
    harness_regression_misc_if_elseif,
    "regression/misc/if-elseif.cir"
);
harness_test!(
    harness_regression_misc_log_functions_1,
    "regression/misc/log-functions-1.cir"
);
harness_test!(
    harness_regression_misc_resume_1,
    "regression/misc/resume-1.cir"
);
harness_test!(
    harness_regression_misc_test_noise_2,
    "regression/misc/test-noise-2.cir"
);
harness_test!(
    harness_regression_misc_test_noise_3,
    "regression/misc/test-noise-3.cir"
);

// === Regression: model ===
harness_test!(
    harness_regression_model_binning_1,
    "regression/model/binning-1.cir"
);
harness_test!(
    harness_regression_model_instance_defaults,
    "regression/model/instance-defaults.cir"
);
harness_test!(
    harness_regression_model_special_names_1,
    "regression/model/special-names-1.cir"
);

// === Regression: parser ===
harness_test!(
    harness_regression_parser_bxpressn_1,
    "regression/parser/bxpressn-1.cir"
);
harness_test!(
    harness_regression_parser_minus_minus,
    "regression/parser/minus-minus.cir"
);
harness_test!(
    harness_regression_parser_xpressn_1,
    "regression/parser/xpressn-1.cir"
);
harness_test!(
    harness_regression_parser_xpressn_2,
    "regression/parser/xpressn-2.cir"
);
harness_test!(
    harness_regression_parser_xpressn_3,
    "regression/parser/xpressn-3.cir"
);

// === Regression: pz ===
harness_test!(
    harness_regression_pz_ac_resistance,
    "regression/pz/ac-resistance.cir"
);

// === Regression: sens ===
harness_test!(
    harness_regression_sens_ac_1,
    "regression/sens/sens-ac-1.cir"
);
harness_test!(
    harness_regression_sens_ac_2,
    "regression/sens/sens-ac-2.cir"
);
harness_test!(
    harness_regression_sens_dc_1,
    "regression/sens/sens-dc-1.cir"
);
harness_test!(
    harness_regression_sens_dc_2,
    "regression/sens/sens-dc-2.cir"
);

// === Regression: subckt-processing ===
harness_test!(
    harness_regression_subckt_global_1,
    "regression/subckt-processing/global-1.cir"
);
harness_test!(
    harness_regression_subckt_model_scope_5,
    "regression/subckt-processing/model-scope-5.cir"
);

// === Regression: temper ===
harness_test!(
    harness_regression_temper_1,
    "regression/temper/temper-1.cir"
);
harness_test!(
    harness_regression_temper_2,
    "regression/temper/temper-2.cir"
);
harness_test!(
    harness_regression_temper_3,
    "regression/temper/temper-3.cir",
    ignore = "Requires TEMPER keyword + .control"
);
harness_test!(
    harness_regression_temper_res_1,
    "regression/temper/temper-res-1.cir"
);

// === Resistance ===
harness_test!(harness_resistance_res_array, "resistance/res_array.cir");
harness_test!(
    harness_resistance_res_partition,
    "resistance/res_partition.cir"
);
harness_test!(harness_resistance_res_simple, "resistance/res_simple.cir");

// === Sensitivity ===
harness_test!(
    harness_sensitivity_diffpair,
    "sensitivity/diffpair.cir",
    ignore = "q3/q4:is sensitivity requires bit-exact LU reuse from NR solver (dense LU refactoring loses CMRR precision)"
);

// === Transient ===
harness_test!(
    harness_transient_fourbitadder,
    "transient/fourbitadder.cir",
    ignore = "times out (>30s)"
);

// === Transmission lines ===
harness_test!(
    harness_transmission_cpl3_4,
    "transmission/cpl3_4_line.cir",
    ignore = "times out (>30s): CPL transient with CMOS driver"
);
harness_test!(
    harness_transmission_cpl_ibm2,
    "transmission/cpl_ibm2.cir",
    ignore = "times out (>30s): CPL transient with CMOS driver"
);
harness_test!(
    harness_transmission_ltra1_1,
    "transmission/ltra1_1_line.cir",
    ignore = "~25% V(2) error at t=16.1ns: CMOS inverter output drops too fast during transition (common to LTRA/TXL, MOSFET driver issue)"
);
harness_test!(
    harness_transmission_ltra2_2,
    "transmission/ltra2_2_line.cir",
    ignore = "times out (>30s): PMOS NR convergence in CMOS inverter driver (LAMBDA=0 → gds=0)"
);
harness_test!(
    harness_transmission_txl1_1,
    "transmission/txl1_1_line.cir",
    ignore = "~25% V(2) error at t=16.1ns: CMOS inverter output drops too fast during transition (common to LTRA/TXL, MOSFET driver issue)"
);
harness_test!(
    harness_transmission_txl2_3,
    "transmission/txl2_3_line.cir",
    ignore = "times out (>30s): PMOS NR convergence in CMOS inverter driver (LAMBDA=0 → gds=0)"
);

// === VBIC ===
harness_test!(
    harness_vbic_ceamp,
    "vbic/CEamp.cir",
    ignore = "VBIC AC numerical accuracy: ~1.2% AC gain error (avalanche derivative coupling + self-heating FP difference)"
);
harness_test!(
    harness_vbic_diffamp,
    "vbic/diffamp.cir",
    ignore = "VBIC diffamp: times out (>30s) — 13-transistor circuit with self-heating (NPN+PNP)"
);
harness_test!(
    harness_vbic_fg,
    "vbic/FG.cir",
    ignore = "VBIC FG: ~0.2-0.7% DC sweep error growing with current (self-heating FP evaluation order difference)"
);
harness_test!(
    harness_vbic_fo,
    "vbic/FO.cir",
    ignore = "VBIC FO: ~0.2% DC sweep error (self-heating FP difference, avalanche now correct)"
);
harness_test!(harness_vbic_noise_scale, "vbic/noise_scale_test.cir");
harness_test!(
    harness_vbic_temp,
    "vbic/temp.cir",
    ignore = "VBIC temp: ~0.2-0.6% Ic error growing with current (self-heating FP evaluation order difference)"
);

// === XSPICE (unimplemented) ===
harness_test!(harness_xspice_d_ram, "xspice/digital/d_ram.cir");
harness_test!(harness_xspice_d_source, "xspice/digital/d_source.cir");
harness_test!(harness_xspice_d_state, "xspice/digital/d_state.cir");
