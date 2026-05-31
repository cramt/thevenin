//! End-to-end tests for native Cirq `analysis four` / `analysis fft` blocks.
//!
//! Drives the full pipeline: Cirq source -> `cirq_frontend::compile` -> IR ->
//! `thevenin_cirq::simulate` -> the `fourier1` / `fft1` result plots. Proves
//! the native analysis kinds reach the Fourier post-processors.

use thevenin_types::VectorData;

fn compile(source: &str) -> cirq_ir::Circuit {
    cirq_frontend::compile(source).unwrap_or_else(|diags| {
        for d in &diags {
            eprintln!("{:?}: {}", d.severity, d.message);
        }
        panic!("compile failed with {} diagnostics", diags.len());
    })
}

/// A 1 kHz sine driving an RC low-pass. The fundamental dominates the output
/// spectrum, so the first harmonic magnitude is the largest non-DC term.
const SINE_RC: &str = r#"
circuit sine_rc {
    V1: vsource(in -> gnd, sin: { v0: 0, va: 1, freq: 1k })
    R1: resistor(in -> out, 1k)
    C1: capacitor(out -> gnd, 100n)

    analysis tran {
        step: 5u
        stop: 5m
    }

    analysis four {
        fundamental: 1k
        output: v(out)
        harmonics: 5
    }

    analysis fft {
        output: v(out)
        npoints: 1024
        window: hann
    }
}
"#;

fn plot_vec(result: &thevenin_types::SimResult, plot: &str, vec: &str) -> Vec<f64> {
    let p = result
        .plots
        .iter()
        .find(|p| p.name == plot)
        .unwrap_or_else(|| panic!("no `{plot}` plot"));
    let v = p
        .vecs
        .iter()
        .find(|v| v.name == vec)
        .unwrap_or_else(|| panic!("no `{vec}` in `{plot}`"));
    match &v.data {
        VectorData::Real(r) => r.clone(),
        other => panic!("`{vec}` is not real: {other:?}"),
    }
}

#[test]
fn native_four_block_runs_fourier() {
    let result = thevenin_cirq::simulate(&compile(SINE_RC)).expect("simulate");

    // `analysis four` emits a `fourier1` plot with magnitude columns.
    let mags = plot_vec(&result, "fourier1", "v_out__mag");
    // index 0 is DC; index 1 is the fundamental. The fundamental should carry
    // real signal energy.
    assert!(
        mags.len() >= 2,
        "expected DC + harmonics, got {}",
        mags.len()
    );
    assert!(
        mags[1] > 0.1,
        "fundamental magnitude should be significant, got {}",
        mags[1]
    );
}

#[test]
fn native_fft_block_runs_fft() {
    let result = thevenin_cirq::simulate(&compile(SINE_RC)).expect("simulate");

    // `analysis fft` emits an `fft1` plot; the `<vec>_fft` column is the
    // complex spectrum.
    let fft1 = result
        .plots
        .iter()
        .find(|p| p.name == "fft1")
        .expect("no `fft1` plot");
    let spectrum = fft1
        .vecs
        .iter()
        .find(|v| v.name == "v_out__fft")
        .expect("no `v_out__fft` column");
    let mags: Vec<f64> = match &spectrum.data {
        VectorData::Complex(c) => c.iter().map(|z| z.magnitude()).collect(),
        VectorData::Real(r) => r.clone(),
    };
    assert!(!mags.is_empty(), "fft spectrum should be non-empty");
    assert!(
        mags.iter().any(|m| *m > 0.0),
        "fft spectrum should contain energy"
    );
}
