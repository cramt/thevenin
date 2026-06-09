//! End-to-end tests for native Cirq `measure <kind> <name> = <expr>` syntax.
//!
//! Drives the full pipeline: Cirq source → `cirq_frontend::compile` → IR →
//! `thevenin_cirq::simulate_tran` → the `measurements` result plot. This is
//! the capstone proving probe functions, derived arithmetic, and the
//! conditional pass/fail form all reach the simulator and evaluate.

use thevenin_types::VectorData;

/// Compile Cirq source to IR, run a transient, and return the named scalar
/// from the `measurements` plot.
fn measure_value(source: &str, name: &str) -> f64 {
    let circuit = cirq_frontend::compile(source).unwrap_or_else(|diags| {
        for d in &diags {
            eprintln!("{:?}: {}", d.severity, d.message);
        }
        panic!("compile failed with {} diagnostics", diags.len());
    });

    // Measurements run after the full analysis deck, so drive the top-level
    // `simulate` dispatcher rather than a single-analysis entry point.
    let result = thevenin_cirq::simulate(&circuit).expect("simulate");

    let plot = result
        .plots
        .iter()
        .find(|p| p.name == "measurements")
        .expect("a `measurements` plot");
    let vec = plot
        .vecs
        .iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("no measurement named `{name}`"));
    match &vec.data {
        VectorData::Real(r) => r[0],
        other => panic!("measurement `{name}` is not real: {other:?}"),
    }
}

/// A resistor charging a capacitor from a 1 V step. v(out) rises monotonically
/// from 0 toward 1 V, so MAX≈1, MIN≈0, and the swing clears 100 mV.
const RC_STEP: &str = r#"
circuit rc_step {
    V1: vsource(in -> gnd, pulse: { v1: 0, v2: 1, td: 0, tr: 1n, tf: 1n, pw: 1m, per: 2m })
    R1: resistor(in -> out, 1k)
    C1: capacitor(out -> gnd, 100n)

    analysis tran {
        step: 1u
        stop: 500u
    }

    measure tran vout_max = max(v(out))
    measure tran vout_min = min(v(out))
    measure tran swing = vout_max - vout_min
    measure tran ok = (swing > 100m) ? 1 : 0
}
"#;

// r[verify analysis.measure]
// r[verify analysis.measure.expr]
// r[verify analysis.tran]
// r[verify elem.vsource]
// r[verify elem.waveform]
// r[verify elem.resistor]
// r[verify elem.capacitor]
#[test]
fn aggregate_probes_reach_simulator() {
    // After ~5 time-constants (RC = 100 µs) the cap is nearly fully charged.
    let vmax = measure_value(RC_STEP, "vout_max");
    assert!(vmax > 0.9, "vout_max should approach 1 V, got {vmax}");

    let vmin = measure_value(RC_STEP, "vout_min");
    assert!(vmin.abs() < 1e-3, "vout_min should be ~0 V, got {vmin}");
}

// r[verify analysis.measure]
// r[verify analysis.measure.expr]
// r[verify analysis.tran]
// r[verify param.conditional]
// r[verify expr.arithmetic]
#[test]
fn derived_and_conditional_measurements_evaluate() {
    let swing = measure_value(RC_STEP, "swing");
    assert!(swing > 0.9, "swing should be the full charge, got {swing}");

    // The pass/fail ternary over a comparison — the headline feature, end to end.
    let ok = measure_value(RC_STEP, "ok");
    assert_eq!(ok, 1.0, "swing exceeds 100 mV so `ok` must be 1");
}
