//! AC sensitivity (`.sens v(out) ac ...`) integration tests.
//!
//! Mirrors the DC sensitivity test pattern in `sensitivity.rs`, but drives
//! the AC sens path in `thevenin/src/sens.rs::run_ac_sens`. Verification
//! strategy:
//!
//! * **Analytical RC filter** — a one-pole RC has a closed-form transfer
//!   function H(jω) = 1/(1+jωRC). The sensitivities of V(out) to R, C and
//!   the source AC magnitude all have clean closed-form expressions and
//!   are cheap to recompute per test point. We use that as ground truth.
//! * **Smoke tests** for DEC/OCT/LIN sweep variants — frequency-count and
//!   well-known parameter names show up, and no scrambling between
//!   frequency points.
//! * **Multi-frequency consistency** — the parameter index for a given
//!   name is identical at every frequency point.
//!
//! ngspice does not ship a `.sens v(...) ac ...` regression fixture, so we
//! anchor on the analytical formulas rather than a binary-identical port.

use std::f64::consts::PI;

use approx::assert_abs_diff_eq;
use thevenin_types::{Complex, Netlist, SimResult};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

mod common;
use common::simulate_sens;

// ── helpers ─────────────────────────────────────────────────────────────────

fn sens_vec<'a>(result: &'a SimResult, name: &str) -> &'a [Complex] {
    result.plots[0]
        .vecs
        .iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| {
            let names: Vec<_> = result.plots[0].vecs.iter().map(|v| &v.name).collect();
            panic!("no vector '{name}', available: {names:?}")
        })
        .data
        .as_complex()
}

fn sens_at(result: &SimResult, name: &str, fi: usize) -> Complex {
    sens_vec(result, name)[fi]
}

/// RC low-pass transfer function H(jω) = 1 / (1 + jωRC).
fn rc_transfer(omega: f64, r: f64, c: f64) -> (f64, f64) {
    let x = omega * r * c;
    let denom = 1.0 + x * x;
    (1.0 / denom, -x / denom)
}

fn complex_div(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let denom = b.0 * b.0 + b.1 * b.1;
    (
        (a.0 * b.0 + a.1 * b.1) / denom,
        (a.1 * b.0 - a.0 * b.1) / denom,
    )
}

fn complex_mul(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}

/// Sensitivity dV/dR for RC low-pass: V(out) = 1/(1+jωRC), input AC mag = 1.
/// dV/dR = -jωC / (1 + jωRC)²
fn rc_dv_dr(omega: f64, r: f64, c: f64) -> (f64, f64) {
    let one_plus = (1.0, omega * r * c);
    let denom_sq = complex_mul(one_plus, one_plus);
    let num = (0.0, -omega * c);
    complex_div(num, denom_sq)
}

/// Sensitivity dV/dC for RC low-pass.
/// dV/dC = -jωR / (1 + jωRC)²
fn rc_dv_dc(omega: f64, r: f64, c: f64) -> (f64, f64) {
    let one_plus = (1.0, omega * r * c);
    let denom_sq = complex_mul(one_plus, one_plus);
    let num = (0.0, -omega * r);
    complex_div(num, denom_sq)
}

const RC_R: f64 = 1.0e3;
const RC_C: f64 = 1.0e-9;

// ── tests ───────────────────────────────────────────────────────────────────

/// Smoke test: simple RC low-pass with `.sens v(out) ac dec 10 1k 1Meg`
/// runs, produces the expected number of frequency points, and exposes
/// the named parameters r1 / c1 / v1_acmag.
#[test]
fn ac_sens_rc_smoke() {
    let netlist = Netlist::parse_single(
        "ac sens rc smoke
v1 in 0 ac 1
r1 in out 1k
c1 out 0 1n
.sens v(out) ac dec 10 1k 1Meg
.end
",
    )
    .unwrap();

    let result = simulate_sens(&netlist);

    // Top-level shape: one sens plot.
    assert_eq!(result.plots.len(), 1);
    assert_eq!(result.plots[0].name, "sens1");

    // 3 decades × 10 pts/decade + 1 = 31 points (matches generate_ac_sweep).
    let r1 = sens_vec(&result, "r1");
    assert_eq!(r1.len(), 31);

    // Expected named parameters are present.
    let names: Vec<&str> = result.plots[0]
        .vecs
        .iter()
        .map(|v| v.name.as_str())
        .collect();
    assert!(names.contains(&"r1"), "missing r1, got {names:?}");
    assert!(names.contains(&"c1"), "missing c1, got {names:?}");
    assert!(
        names.contains(&"v1_acmag"),
        "missing v1_acmag, got {names:?}"
    );

    // Every sensitivity is a finite complex number at every frequency.
    for vec in &result.plots[0].vecs {
        let data = vec.data.as_complex();
        for (i, c) in data.iter().enumerate() {
            assert!(
                c.re.is_finite() && c.im.is_finite(),
                "non-finite sens value for {} at fi={i}: ({}, {})",
                vec.name,
                c.re,
                c.im,
            );
        }
    }
}

/// Numerical check vs. analytical RC formula.
///
/// Picks two frequencies (well below and around the corner) and asserts
/// dV(out)/dR and dV(out)/dC match the closed-form expressions.
#[test]
fn ac_sens_rc_matches_analytical() {
    let netlist = Netlist::parse_single(
        "ac sens rc analytical
v1 in 0 ac 1
r1 in out 1k
c1 out 0 1n
.sens v(out) ac dec 10 1k 1Meg
.end
",
    )
    .unwrap();

    let result = simulate_sens(&netlist);

    // generate_ac_sweep with dec=10, fstart=1k, fstop=1Meg produces points
    // at fi=0 → 1kHz, fi=10 → 10kHz, fi=20 → 100kHz, fi=30 → 1MHz.
    // The corner frequency is 1/(2πRC) ≈ 159.155 kHz which is between
    // fi=21 and fi=22. We pick fi=0 (low) and fi=20 (decade below corner)
    // so both magnitudes are well-conditioned.
    let cases = [(0usize, 1.0e3), (20usize, 1.0e5)];

    for (fi, freq) in cases {
        let omega = 2.0 * PI * freq;

        // dV/dR
        let actual_r = sens_at(&result, "r1", fi);
        let expected_r = rc_dv_dr(omega, RC_R, RC_C);
        assert_abs_diff_eq!(
            actual_r.re,
            expected_r.0,
            epsilon = 1e-6 * expected_r.0.abs().max(1e-12)
        );
        assert_abs_diff_eq!(
            actual_r.im,
            expected_r.1,
            epsilon = 1e-6 * expected_r.1.abs().max(1e-12)
        );

        // dV/dC
        let actual_c = sens_at(&result, "c1", fi);
        let expected_c = rc_dv_dc(omega, RC_R, RC_C);
        assert_abs_diff_eq!(
            actual_c.re,
            expected_c.0,
            epsilon = 1e-6 * expected_c.0.abs().max(1e-12)
        );
        assert_abs_diff_eq!(
            actual_c.im,
            expected_c.1,
            epsilon = 1e-6 * expected_c.1.abs().max(1e-12)
        );

        // dV/dACmag = H(jω)
        let actual_v = sens_at(&result, "v1_acmag", fi);
        let expected_v = rc_transfer(omega, RC_R, RC_C);
        assert_abs_diff_eq!(
            actual_v.re,
            expected_v.0,
            epsilon = 1e-9 * expected_v.0.abs().max(1.0)
        );
        assert_abs_diff_eq!(
            actual_v.im,
            expected_v.1,
            epsilon = 1e-9 * expected_v.1.abs().max(1.0)
        );
    }
}

/// Sensitivities to a current-source AC magnitude.
///
/// For `i1 0 out ac 1; rl out 0 R`: V(out) = R, dV/dAC_mag = R.
/// (Pure resistive node — H(s) = R, no frequency dependence.)
#[test]
fn ac_sens_current_source_acmag() {
    let netlist = Netlist::parse_single(
        "ac sens isource
i1 0 out ac 1
rl out 0 2k
.sens v(out) ac lin 3 1k 10k
.end
",
    )
    .unwrap();

    let result = simulate_sens(&netlist);

    // 3 linearly-spaced points: 1k, 5.5k, 10k.
    let i1 = sens_vec(&result, "i1_acmag");
    assert_eq!(i1.len(), 3);
    for c in i1.iter() {
        assert_abs_diff_eq!(c.re, 2.0e3, epsilon = 1e-6);
        assert_abs_diff_eq!(c.im, 0.0, epsilon = 1e-9);
    }

    // dV/dR = I_ac = 1 (for AC mag 1).  (Real, no frequency dependence.)
    let rl = sens_vec(&result, "rl");
    for c in rl.iter() {
        assert_abs_diff_eq!(c.re, 1.0, epsilon = 1e-9);
        assert_abs_diff_eq!(c.im, 0.0, epsilon = 1e-9);
    }
}

/// Inductor sensitivity numerical check.
///
/// `v1 in 0 ac 1; r1 in out 1k; l1 out 0 1m` is a high-pass formed by
/// the inductor's |jωL| rising against a series R.  H(jω) = jωL / (R + jωL).
/// dV/dL = jω·R / (R + jωL)²
#[test]
fn ac_sens_inductor_matches_analytical() {
    let l_val = 1.0e-3;
    let r_val = 1.0e3;
    let netlist = Netlist::parse_single(
        "ac sens rl
v1 in 0 ac 1
r1 in out 1k
l1 out 0 1m
.sens v(out) ac dec 4 1Meg 100Meg
.end
",
    )
    .unwrap();

    let result = simulate_sens(&netlist);

    // Pick a mid-band point (fi=4 corresponds to fstart * 10^(4/4) = 10MHz).
    let freq = 1.0e7;
    let omega = 2.0 * PI * freq;

    // H(jω) = jωL / (R + jωL)
    let num_h = (0.0, omega * l_val);
    let den_h = (r_val, omega * l_val);
    let _h = complex_div(num_h, den_h);

    // dV/dL = jω·R / (R + jωL)²
    let den_sq = complex_mul(den_h, den_h);
    let num_dl = (0.0, omega * r_val);
    let expected_l = complex_div(num_dl, den_sq);

    let actual_l = sens_at(&result, "l1", 4);
    let scale = expected_l.0.abs().hypot(expected_l.1.abs()).max(1e-12);
    assert_abs_diff_eq!(actual_l.re, expected_l.0, epsilon = 1e-6 * scale);
    assert_abs_diff_eq!(actual_l.im, expected_l.1, epsilon = 1e-6 * scale);
}

/// Multi-frequency consistency: the parameter index for each name is the
/// same at every frequency, and the underlying vectors have identical
/// length.  Catches subtle ordering bugs where sens_idx walks out of sync.
#[test]
fn ac_sens_multi_frequency_consistency() {
    // Use a heterogenous circuit so we exercise R + C + L + V + I.
    let netlist = Netlist::parse_single(
        "ac sens consistency
v1 in 0 ac 1
i1 0 mid ac 0.5
r1 in mid 1k
c1 mid out 1n
l1 out 0 1m
rb out 0 10k
.sens v(out) ac dec 5 1k 1Meg
.end
",
    )
    .unwrap();

    let result = simulate_sens(&netlist);

    // Expected param-name set (no requirement on order — just that each is
    // present and has identical length).
    let expected = ["r1", "rb", "c1", "l1", "v1_acmag", "i1_acmag"];

    let n_points = sens_vec(&result, expected[0]).len();
    // 3 decades × 5 pts/decade + 1 = 16 points.
    assert_eq!(n_points, 16);

    for name in expected {
        let v = sens_vec(&result, name);
        assert_eq!(v.len(), n_points, "vector '{name}' has wrong length");
        for (fi, c) in v.iter().enumerate() {
            assert!(
                c.re.is_finite() && c.im.is_finite(),
                "non-finite at fi={fi} for {name}"
            );
        }
    }
}

/// OCT sweep variant smoke test.
#[test]
fn ac_sens_oct_sweep() {
    let netlist = Netlist::parse_single(
        "ac sens oct
v1 in 0 ac 1
r1 in out 1k
c1 out 0 1n
.sens v(out) ac oct 2 1k 8k
.end
",
    )
    .unwrap();

    let result = simulate_sens(&netlist);

    let v = sens_vec(&result, "r1");
    // 3 octaves × 2 pts/oct + 1 = 7 points.
    assert_eq!(v.len(), 7);
    // Sanity: first point matches the analytical formula at fstart.
    let omega0 = 2.0 * PI * 1.0e3;
    let exp = rc_dv_dr(omega0, RC_R, RC_C);
    let act = v[0];
    let scale = exp.0.abs().max(exp.1.abs()).max(1e-12);
    assert_abs_diff_eq!(act.re, exp.0, epsilon = 1e-5 * scale);
    assert_abs_diff_eq!(act.im, exp.1, epsilon = 1e-5 * scale);
}

/// LIN sweep variant smoke test.
#[test]
fn ac_sens_lin_sweep() {
    let netlist = Netlist::parse_single(
        "ac sens lin
v1 in 0 ac 1
r1 in out 1k
c1 out 0 1n
.sens v(out) ac lin 5 1k 5k
.end
",
    )
    .unwrap();

    let result = simulate_sens(&netlist);

    let v = sens_vec(&result, "c1");
    assert_eq!(v.len(), 5);
    // Sanity: last point matches analytical dV/dC at fstop = 5kHz.
    let omega = 2.0 * PI * 5.0e3;
    let exp = rc_dv_dc(omega, RC_R, RC_C);
    let act = v[4];
    let scale = exp.0.abs().max(exp.1.abs()).max(1e-12);
    assert_abs_diff_eq!(act.re, exp.0, epsilon = 1e-5 * scale);
    assert_abs_diff_eq!(act.im, exp.1, epsilon = 1e-5 * scale);
}

/// V(out) differential form: `.sens v(a, b) ac ...` should record the
/// sensitivity of (V(a) − V(b)), not just V(a).
#[test]
fn ac_sens_differential_output() {
    let netlist = Netlist::parse_single(
        "ac sens differential
v1 in 0 ac 1
r1 in a 1k
c1 a 0 1n
r2 in b 1k
c2 b 0 1n
.sens v(a, b) ac lin 1 1k 1k
.end
",
    )
    .unwrap();

    let result = simulate_sens(&netlist);

    // Symmetric branches: dV(a,b)/dR1 = dV(a)/dR1, dV(a,b)/dR2 = -dV(b)/dR2.
    // By symmetry, dV(a,b)/dR1 should be the analytical dV/dR of one
    // branch; dV(a,b)/dR2 should be its negation.
    let omega = 2.0 * PI * 1.0e3;
    let exp_r = rc_dv_dr(omega, RC_R, RC_C);

    let s_r1 = sens_at(&result, "r1", 0);
    let s_r2 = sens_at(&result, "r2", 0);

    let scale = exp_r.0.abs().max(exp_r.1.abs()).max(1e-12);
    assert_abs_diff_eq!(s_r1.re, exp_r.0, epsilon = 1e-5 * scale);
    assert_abs_diff_eq!(s_r1.im, exp_r.1, epsilon = 1e-5 * scale);
    assert_abs_diff_eq!(s_r2.re, -exp_r.0, epsilon = 1e-5 * scale);
    assert_abs_diff_eq!(s_r2.im, -exp_r.1, epsilon = 1e-5 * scale);
}
