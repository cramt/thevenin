//! HiSIM2 (LEVEL=68) golden-reference tests against ngspice-45.
//!
//! The golden CSVs in `tests/fixtures/hisim2/` are produced by
//! `scripts/gen-hisim-golden.sh` (ngspice via nixpkgs, compiled with the real
//! HiSIM2 model). Each CSV is `wrdata`'s two-column format: the inner swept
//! variable in column 0, the drain current `-i(Vd)` in column 1. The sweep
//! structure (outer/inner axes) is known from the decks the script emits and
//! reconstructed here.
//!
//! These tests compare `HisimModel::companion(...).cdrain` directly against the
//! reference current — a device-level check that isolates the I-V physics from
//! the MNA/import path. They were the TDD anchor for the full hsm2eval.c port
//! (checklist A1); the faithful I-V core matches ngspice-45 to ~0.001%
//! relative on all three sweeps, so they now run unconditionally.

use std::path::PathBuf;

use thevenin::hisim::HisimModel;
use thevenin::model_params::ModelParams;

const W: f64 = 10e-6;
const L: f64 = 1e-6;

/// Build the HiSIM2 model matching `tests/fixtures/hisim2/nch.model`.
fn nch_model() -> HisimModel {
    let params = [
        ("LEVEL", 68.0),
        ("VERSION", 2.80),
        ("TOX", 2.0e-9),
        ("NSUBC", 5.0e17),
        ("NSUBP", 1.0e18),
        ("VFBC", -0.5),
        ("MUECB0", 130.0),
        ("MUECB1", 600.0),
        ("MUEPH1", 2.5e4),
        ("MUESR1", 2.0e15),
        ("VMAX", 7.0e6),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    HisimModel::from_params(&ModelParams {
        name: "nch".to_string(),
        kind: "NMOS".to_string(),
        params,
    })
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/hisim2")
        .join(name)
}

/// Parse a `wrdata` two-column CSV into `(swept, current)` rows.
fn load_golden(name: &str) -> Vec<(f64, f64)> {
    let text = std::fs::read_to_string(fixture(name)).expect("read golden CSV");
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut it = l.split_whitespace();
            let a: f64 = it.next().unwrap().parse().unwrap();
            let b: f64 = it.next().unwrap().parse().unwrap();
            (a, b)
        })
        .collect()
}

/// Relative error with an absolute floor so sub-nA subthreshold points don't
/// dominate (we are validating the strong-inversion I-V, not the leakage tail).
fn rel_err(got: f64, want: f64) -> f64 {
    let abs_floor = 1e-7; // 100 nA
    (got - want).abs() / want.abs().max(abs_floor)
}

/// Summarise the fit: max relative error over points above `floor` amps.
fn report(label: &str, pts: &[(f64, f64, f64)]) -> f64 {
    // pts: (got, want, vds-or-vgs for context)
    let mut max_re = 0.0_f64;
    let mut worst = (0.0, 0.0, 0.0);
    for &(got, want, ctx) in pts {
        if want.abs() < 1e-7 {
            continue;
        }
        let re = rel_err(got, want);
        if re > max_re {
            max_re = re;
            worst = (got, want, ctx);
        }
    }
    eprintln!(
        "{label}: max rel-err = {:.1}%  (got {:.4e} vs want {:.4e} @ ctx={:.3})",
        max_re * 100.0,
        worst.0,
        worst.1,
        worst.2
    );
    max_re
}

#[test]
fn idvds_family_matches_ngspice() {
    let m = nch_model();
    let golden = load_golden("idvds.csv");
    // Deck: outer Vg = 0.6..1.2 step 0.2 (4 values), inner Vd = 0..1.2 step
    // 0.05 (25 values). Row r -> vg index r/25, vd = column 0.
    let inner = 25;
    let mut pts = Vec::new();
    for (r, &(vd, iref)) in golden.iter().enumerate() {
        let vg = 0.6 + 0.2 * (r / inner) as f64;
        let comp = m.companion(vg, vd, 0.0, W, L);
        pts.push((comp.cdrain, iref, vd));
    }
    let max_re = report("idvds", &pts);
    assert!(max_re < 0.05, "Id-Vds family off by {:.1}%", max_re * 100.0);
}

#[test]
fn idvgs_transfer_matches_ngspice() {
    let m = nch_model();
    let golden = load_golden("idvgs.csv");
    // Deck: outer Vd = 0.05, 1.2 (2 values), inner Vg = 0..1.2 step 0.025
    // (49 values). Row r -> vd index r/49, vg = column 0.
    let inner = 49;
    let vds_axis = [0.05, 1.2];
    let mut pts = Vec::new();
    for (r, &(vg, iref)) in golden.iter().enumerate() {
        let vd = vds_axis[r / inner];
        let comp = m.companion(vg, vd, 0.0, W, L);
        pts.push((comp.cdrain, iref, vg));
    }
    let max_re = report("idvgs", &pts);
    assert!(
        max_re < 0.05,
        "Id-Vgs transfer off by {:.1}%",
        max_re * 100.0
    );
}

#[test]
fn body_effect_matches_ngspice() {
    let m = nch_model();
    let golden = load_golden("idvbs.csv");
    // Deck: outer Vb = 0, -0.5, -1.0 (3 values), inner Vg = 0..1.2 step 0.05
    // (25 values). Vds = 1.2.
    let inner = 25;
    let vbs_axis = [0.0, -0.5, -1.0];
    let mut pts = Vec::new();
    for (r, &(vg, iref)) in golden.iter().enumerate() {
        let vbs = vbs_axis[r / inner];
        let comp = m.companion(vg, 1.2, vbs, W, L);
        pts.push((comp.cdrain, iref, vg));
    }
    let max_re = report("idvbs", &pts);
    assert!(max_re < 0.08, "body effect off by {:.1}%", max_re * 100.0);
}
