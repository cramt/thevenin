//! URC (uniform RC transmission line) expansion tests.
//!
//! The U element is a macro that expands at SPICE import time into N stages
//! of R / C (or R / C / D when the model gives `ISPERL > 0`). Verify the
//! expansion topology and parameter math against ngspice's `urcsetup.c`.

use cirq_spice_import::import_spice;

/// A URC with `L=1 N=3` and the default RPERL/CPERL expands into 6 resistors
/// (2 per lump × 3 lumps) and 5 capacitors (1 per lump from the lo midnode
/// to ground + 1 per non-terminal lump from the hi midnode to ground, with
/// the terminal lump's hi/lo collapsed to a single shared midnode → so the
/// terminal lump contributes 1 cap and each earlier lump contributes 2).
#[test]
fn urc_default_expands_to_r_c_stages() {
    let source = r#"URC default 3-lump expansion
V1 in 0 1.0
U1 in out 0 urcmod L=1.0 N=3
Rload out 0 1k
.model urcmod URC RPERL=1000 CPERL=1n
.op
.end
"#;
    let circuits = import_spice(source).unwrap();
    let circuit = &circuits[0];

    // Count resistors and capacitors that came from the URC expansion
    // (names start with `__urc__U1__`).
    let mut urc_resistors = 0usize;
    let mut urc_capacitors = 0usize;
    for elem in &circuit.elements {
        if elem.name.starts_with("__urc__U1__") {
            match elem.kind {
                cirq_ir::ElementKind::Resistor => urc_resistors += 1,
                cirq_ir::ElementKind::Capacitor => urc_capacitors += 1,
                _ => {}
            }
        }
    }
    assert_eq!(urc_resistors, 6, "expected 6 R from 3-lump expansion");
    assert_eq!(urc_capacitors, 5, "expected 5 C from 3-lump expansion");
}

/// A URC with `ISPERL > 0` expands its shunt-to-ground elements as diodes
/// rather than capacitors, and synthesises a `D`-kind model for them.
#[test]
fn urc_with_isperl_uses_diodes() {
    let source = r#"URC with diode-shunt
V1 in 0 1.0
U1 in out 0 urcd L=1.0 N=3
Rload out 0 1k
.model urcd URC RPERL=1000 CPERL=1n ISPERL=1e-12 RSPERL=10
.op
.end
"#;
    let circuits = import_spice(source).unwrap();
    let circuit = &circuits[0];

    // Resistors remain at 6 from the two ladders.
    let urc_resistors = circuit
        .elements
        .iter()
        .filter(|e| {
            e.name.starts_with("__urc__U1__") && matches!(e.kind, cirq_ir::ElementKind::Resistor)
        })
        .count();
    assert_eq!(urc_resistors, 6);

    // Shunts are diodes — no capacitors. (The synthesised diode model is
    // visible in the model table.)
    let urc_caps = circuit
        .elements
        .iter()
        .filter(|e| {
            e.name.starts_with("__urc__U1__") && matches!(e.kind, cirq_ir::ElementKind::Capacitor)
        })
        .count();
    assert_eq!(urc_caps, 0);

    let synthesised_model = circuit
        .models
        .iter()
        .find(|m| m.name == "__urc__U1__dio")
        .expect("URC should synthesise a diode model when ISPERL > 0");
    assert!(matches!(
        synthesised_model.device_type,
        cirq_ir::DeviceType::Diode
    ));
}

/// The URC default-lumps path picks at least 3 lumps when no N= is given.
#[test]
fn urc_default_lumps_is_at_least_three() {
    let source = r#"URC implicit-N lumps
V1 in 0 1.0
U1 in out 0 urc2 L=1.0
Rload out 0 1k
.model urc2 URC RPERL=1000 CPERL=1n
.op
.end
"#;
    let circuits = import_spice(source).unwrap();
    let circuit = &circuits[0];
    let urc_resistors = circuit
        .elements
        .iter()
        .filter(|e| {
            e.name.starts_with("__urc__U1__") && matches!(e.kind, cirq_ir::ElementKind::Resistor)
        })
        .count();
    // 2 R per lump, minimum 3 lumps → ≥ 6 R.
    assert!(
        urc_resistors >= 6,
        "expected ≥ 6 resistors from ≥ 3 lumps, got {urc_resistors}",
    );
    // And resistor count must be even (2 per lump).
    assert_eq!(urc_resistors % 2, 0);
}

/// End-to-end: a URC-bearing circuit simulates to a finite DC operating point.
/// The URC acts as a low-pass network — at DC it's just a resistance ladder
/// from `in` to `out` with shunts to ground.
#[test]
fn urc_op_point_is_finite() {
    let source = r#"URC OP smoke
V1 in 0 5.0
U1 in out 0 urcmod L=1.0 N=4
Rload out 0 1k
.model urcmod URC RPERL=100 CPERL=1n
.op
.end
"#;
    let circuits = import_spice(source).unwrap();
    let circuit = &circuits[0];
    let result = thevenin::circuit::simulate(circuit).unwrap();
    // OP plot should have at least the input + output node voltages.
    let plot = &result.plots[0];
    let v_out = plot
        .vecs
        .iter()
        .find(|v| v.name.eq_ignore_ascii_case("v(out)"))
        .expect("v(out) should be present");
    let v = v_out.data.as_real()[0];
    assert!(v.is_finite());
    // At DC the URC reduces to a series-resistance path; v(out) must lie
    // strictly between 0 and the 5V source.
    assert!(
        (0.0..5.0).contains(&v),
        "v(out) should be in (0, 5): got {v}",
    );
}
