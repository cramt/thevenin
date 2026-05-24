//! Integration tests for `.four` / `.fft` Fourier post-processing.
//!
//! These tests drive a transient simulation through the public Circuit
//! surface, then apply [`thevenin::fourier::four_analysis`] /
//! [`thevenin::fourier::fft_analysis`] to the resulting `SimPlot`.

mod common;

use thevenin::fourier::{FftOptions, fft_analysis, four_analysis};
use thevenin_types::Netlist;

fn run_tran(spice: &str) -> thevenin_types::SimResult {
    let netlist = Netlist::parse_single(spice).expect("parse");
    common::simulate_tran(&netlist)
}

/// Pure 1 kHz sine should give magnitude≈1 at the fundamental and very
/// small magnitudes at higher harmonics. THD should be near zero.
#[test]
fn pure_sine_input_has_clean_fundamental() {
    let spice = "Pure sine
V1 in 0 SIN(0 1 1k)
R1 in 0 1k
.tran 10u 10m
.end
";
    let result = run_tran(spice);
    let plot = result.plots.iter().find(|p| p.name == "tran1").unwrap();

    let r = four_analysis(plot, 1_000.0, &["v(in)"], 9)
        .unwrap()
        .remove(0);
    assert!(
        (r.harmonics[0].magnitude - 1.0).abs() < 0.05,
        "fundamental mag = {}",
        r.harmonics[0].magnitude
    );
    for h in &r.harmonics[1..] {
        assert!(
            h.magnitude < 0.05,
            "harmonic {} mag = {}",
            h.index,
            h.magnitude
        );
    }
    assert!(r.thd_percent < 5.0, "thd = {}", r.thd_percent);
}

/// A signal made from two summed sinusoids (1 kHz fundamental + 0.5*2 kHz)
/// should report the second harmonic at half the magnitude of the
/// fundamental. Two SIN sources are placed in series so the mid-node
/// voltage v(mid) sees the sum.
#[test]
fn distorted_input_reports_second_harmonic() {
    let spice = "Distorted
V1 a 0 SIN(0 1 1k)
V2 mid a SIN(0 0.5 2k)
R1 mid 0 1k
.tran 5u 10m
.end
";
    let result = run_tran(spice);
    let plot = result.plots.iter().find(|p| p.name == "tran1").unwrap();

    let r = four_analysis(plot, 1_000.0, &["v(mid)"], 5)
        .unwrap()
        .remove(0);
    let h1 = r.harmonics[0].magnitude;
    let h2 = r.harmonics[1].magnitude;
    assert!((h1 - 1.0).abs() < 0.1, "h1 = {h1}");
    assert!((h2 - 0.5).abs() < 0.1, "h2 = {h2}");
    let ratio = h2 / h1;
    assert!((ratio - 0.5).abs() < 0.1, "harmonic ratio h2/h1 = {ratio}");
}

/// `.fft` with `npoints=1000` should round up to 1024 internally and
/// return 1024/2 + 1 frequency bins.
#[test]
fn fft_rounds_npoints_to_power_of_two() {
    let spice = "FFT roundup
V1 in 0 SIN(0 1 1k)
R1 in 0 1k
.tran 10u 5m
.end
";
    let result = run_tran(spice);
    let plot = result.plots.iter().find(|p| p.name == "tran1").unwrap();
    let opts = FftOptions {
        vectors: vec!["v(in)".into()],
        npoints: 1000,
        ..FftOptions::default()
    };
    let r = fft_analysis(plot, &opts).unwrap().remove(0);
    assert_eq!(r.n, 1024);
    assert_eq!(r.frequencies.len(), 1024 / 2 + 1);
    assert_eq!(r.values.len(), 1024 / 2 + 1);
}

/// `.fft` peak should land at the bin closest to the input frequency.
/// 1 kHz sine sampled at 1 MHz over the last 1 ms (n=1024) → peak near
/// bin `1 ms × 1 kHz × 1024 / 1024 / 1` — i.e. close to k=1.
#[test]
fn fft_peak_at_input_frequency() {
    let spice = "FFT peak
V1 in 0 SIN(0 1 1k)
R1 in 0 1k
.tran 1u 10m
.end
";
    let result = run_tran(spice);
    let plot = result.plots.iter().find(|p| p.name == "tran1").unwrap();
    let opts = FftOptions {
        vectors: vec!["v(in)".into()],
        start: Some(5e-3),
        stop: Some(10e-3),
        npoints: 1024,
        window: cirq_ir::FftWindow::Hann,
        ..FftOptions::default()
    };
    let r = fft_analysis(plot, &opts).unwrap().remove(0);

    // The peak frequency should be near 1 kHz.
    let (peak_idx, _) = r
        .values
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.magnitude().partial_cmp(&b.1.magnitude()).unwrap())
        .unwrap();
    let peak_freq = r.frequencies[peak_idx];
    assert!(
        (peak_freq - 1_000.0).abs() < 100.0,
        "peak frequency {peak_freq} not near 1 kHz"
    );
}

/// Parser: `.four` directives should round-trip through the SPICE parser
/// and importer to produce an `Analysis::Four` IR variant.
#[test]
fn dot_four_parses_to_ir() {
    use cirq_spice_import::import_spice;
    let spice = "Fourier dispatch
V1 in 0 SIN(0 1 1k)
R1 in 0 1k
.tran 10u 10m
.four 1k V(in)
.end
";
    let circuits = import_spice(spice).unwrap();
    // The parser splits one Netlist per analysis directive, so .tran and
    // .four become two circuits. Find the .four circuit.
    let has_four = circuits.iter().any(|c| {
        c.analyses
            .iter()
            .any(|a| matches!(a, cirq_ir::Analysis::Four(_)))
    });
    assert!(has_four, "expected at least one Analysis::Four in circuits");
}

/// `.fft` directives should round-trip through the SPICE parser.
#[test]
fn dot_fft_parses_to_ir() {
    use cirq_spice_import::import_spice;
    let spice = "FFT dispatch
V1 in 0 SIN(0 1 1k)
R1 in 0 1k
.tran 10u 5m
.fft V(in) npoints=512 window=hann
.end
";
    let circuits = import_spice(spice).unwrap();
    let fft = circuits.iter().find_map(|c| {
        c.analyses.iter().find_map(|a| match a {
            cirq_ir::Analysis::Fft(f) => Some(f.clone()),
            _ => None,
        })
    });
    let fft = fft.expect("expected Analysis::Fft");
    assert_eq!(fft.npoints, 512);
    assert_eq!(fft.window, cirq_ir::FftWindow::Hann);
}
