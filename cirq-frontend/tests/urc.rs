//! Native Cirq `urc` element lowering tests.
//!
//! URC is a macro that expands into an R/C(/D) ladder at compile time. These
//! tests pin the element/value shape, the model/inline/override equivalence,
//! and parity with the SPICE importer's expansion.

use cirq_ir::{Circuit, ElementKind, Value};

/// Sorted resistor values from the lowered circuit.
fn r_values(ir: &Circuit) -> Vec<f64> {
    let mut v: Vec<f64> = ir
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementKind::Resistor))
        .filter_map(|e| param_real(e, "value"))
        .collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v
}

/// Sorted capacitor values from the lowered circuit.
fn c_values(ir: &Circuit) -> Vec<f64> {
    let mut v: Vec<f64> = ir
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementKind::Capacitor))
        .filter_map(|e| param_real(e, "value"))
        .collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v
}

fn param_real(e: &cirq_ir::Element, key: &str) -> Option<f64> {
    e.params.iter().find(|(k, _)| k == key).and_then(|(_, v)| {
        if let Value::Real(r) = v {
            Some(*r)
        } else {
            None
        }
    })
}

fn approx_eq(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| (x - y).abs() <= 1e-9 * x.abs().max(1.0))
}

/// A model-based native `urc` lowers to an R/C ladder. With `lumps: 4` we get
/// 2 resistors per lump (8) and 7 caps (lo on all 4, hi on the 3 non-final).
// r[verify elem.urc]
// r[verify model.decl]
// r[verify model.device-kinds]
#[test]
fn native_urc_model_based_expands() {
    let src = r#"
        circuit urc_model {
            V1: vsource(in -> gnd, dc: 1)
            U1: urc(in -> out, model: rcline, len: 1, lumps: 4)
            model rcline: urc { rperl = 1k, cperl = 1n }
            analysis op {}
        }
    "#;
    let ir = cirq_frontend::compile(src).expect("compiles");
    assert_eq!(r_values(&ir).len(), 8, "2 resistors per lump");
    assert_eq!(c_values(&ir).len(), 7, "lo on every lump + hi on non-final");
    // No diode model when ISPERL == 0.
    assert!(
        !ir.elements
            .iter()
            .any(|e| matches!(e.kind, ElementKind::Diode))
    );
}

/// Inline params (no model) produce the identical ladder to the model-based
/// form with the same numbers.
// r[verify elem.urc]
// r[verify model.reference]
#[test]
fn native_urc_inline_matches_model() {
    let model_src = r#"
        circuit a {
            V1: vsource(in -> gnd, dc: 1)
            U1: urc(in -> out, model: rcline, len: 1, lumps: 4)
            Rload: resistor(out -> gnd, 1e6)
            model rcline: urc { rperl = 1k, cperl = 1n }
            analysis op {}
        }
    "#;
    let inline_src = r#"
        circuit b {
            V1: vsource(in -> gnd, dc: 1)
            U1: urc(in -> out, rperl: 1k, cperl: 1n, len: 1, lumps: 4)
            Rload: resistor(out -> gnd, 1e6)
            analysis op {}
        }
    "#;
    let m = cirq_frontend::compile(model_src).expect("model form compiles");
    let i = cirq_frontend::compile(inline_src).expect("inline form compiles");
    assert!(
        approx_eq(&r_values(&m), &r_values(&i)),
        "resistor ladders differ"
    );
    assert!(
        approx_eq(&c_values(&m), &c_values(&i)),
        "cap ladders differ"
    );
}

/// A per-instance inline override changes only the overridden param. Doubling
/// `cperl` scales every cap value by 2 relative to the un-overridden line.
// r[verify elem.urc]
// r[verify module.param-override]
#[test]
fn native_urc_inline_overrides_model() {
    let base = r#"
        circuit a {
            U1: urc(in -> out, model: rcline, len: 1, lumps: 4)
            model rcline: urc { rperl = 1k, cperl = 1n }
            analysis op {}
        }
    "#;
    let overridden = r#"
        circuit b {
            U1: urc(in -> out, model: rcline, len: 1, lumps: 4, cperl: 2n)
            model rcline: urc { rperl = 1k, cperl = 1n }
            analysis op {}
        }
    "#;
    let a = cirq_frontend::compile(base).expect("compiles");
    let b = cirq_frontend::compile(overridden).expect("compiles");
    // Resistors unchanged; caps doubled.
    assert!(approx_eq(&r_values(&a), &r_values(&b)));
    let ca = c_values(&a);
    let cb = c_values(&b);
    assert_eq!(ca.len(), cb.len());
    assert!(
        ca.iter()
            .zip(&cb)
            .all(|(x, y)| (2.0 * x - y).abs() <= 1e-9 * y.abs().max(1.0)),
        "expected cap values to double under cperl override"
    );
}

/// A native `urc` and the equivalent SPICE `U` + `.model URC` expand to the
/// same ladder (same resistor and capacitor value multisets).
// r[verify elem.urc]
// r[verify spice.element-map]
// r[verify model.decl]
#[test]
fn native_urc_matches_spice_import() {
    let native = r#"
        circuit n {
            V1: vsource(in -> gnd, dc: 1)
            U1: urc(in -> out, model: rcline, len: 2, lumps: 5)
            model rcline: urc { rperl = 500, cperl = 2n }
            analysis op {}
        }
    "#;
    let spice = r#"URC import parity
V1 in 0 DC 1
U1 in out 0 rcline L=2 N=5
.model rcline URC RPERL=500 CPERL=2n
.op
.end
"#;
    let n = cirq_frontend::compile(native).expect("native compiles");
    let imported = cirq_spice_import::import_spice(spice).expect("spice imports");
    let s = &imported[0];
    assert!(
        approx_eq(&r_values(&n), &r_values(s)),
        "resistor ladders differ:\nnative={:?}\nspice ={:?}",
        r_values(&n),
        r_values(s)
    );
    assert!(
        approx_eq(&c_values(&n), &c_values(s)),
        "cap ladders differ:\nnative={:?}\nspice ={:?}",
        c_values(&n),
        c_values(s)
    );
}

/// `isperl > 0` turns the shunts into diodes and synthesizes a diode model.
// r[verify elem.urc]
// r[verify elem.diode]
// r[verify model.decl]
// r[verify model.device-kinds]
#[test]
fn native_urc_isperl_uses_diodes() {
    let src = r#"
        circuit d {
            V1: vsource(in -> gnd, dc: 1)
            U1: urc(in -> out, model: rcd, len: 1, lumps: 3)
            Rload: resistor(out -> gnd, 1e6)
            model rcd: urc { rperl = 1k, cperl = 1n, isperl = 1e-9, rsperl = 10 }
            analysis op {}
        }
    "#;
    let ir = cirq_frontend::compile(src).expect("compiles");
    let n_diodes = ir
        .elements
        .iter()
        .filter(|e| matches!(e.kind, ElementKind::Diode))
        .count();
    assert!(n_diodes > 0, "expected diode shunts");
    assert_eq!(c_values(&ir).len(), 0, "no caps when ISPERL > 0");
    // A diode model was synthesized.
    assert!(
        ir.models
            .iter()
            .any(|m| matches!(m.device_type, cirq_ir::DeviceType::Diode)),
        "expected a synthesized diode model"
    );
}

/// `urc` without `len` is a hard error.
// r[verify elem.urc]
// r[verify param.required]
#[test]
fn native_urc_missing_len_errors() {
    let src = r#"
        circuit e {
            U1: urc(in -> out, model: rcline, lumps: 4)
            model rcline: urc { rperl = 1k, cperl = 1n }
            analysis op {}
        }
    "#;
    let diags = cirq_frontend::compile(src).expect_err("missing len must fail");
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("urc requires `len`")),
        "expected missing-len error, got: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

/// End-to-end: a URC-bearing circuit reaches a finite DC operating point (at
/// DC the ladder is a pure resistive divider into the load).
// r[verify elem.urc]
// r[verify analysis.op]
#[test]
fn native_urc_simulates_to_finite_op() {
    let src = r#"
        circuit s {
            V1: vsource(in -> gnd, dc: 1)
            U1: urc(in -> out, model: rcline, len: 1, lumps: 4)
            Rload: resistor(out -> gnd, 1e6)
            model rcline: urc { rperl = 1k, cperl = 1n }
            analysis op {}
        }
    "#;
    let ir = cirq_frontend::compile(src).expect("compiles");
    let result = thevenin::circuit::simulate(&ir).expect("op solves");
    let plot = &result.plots[0];
    let out = plot
        .vecs
        .iter()
        .find(|v| v.name.eq_ignore_ascii_case("v(out)"))
        .expect("v(out) present");
    let val = match &out.data {
        thevenin_types::VectorData::Real(r) => *r.last().unwrap(),
        _ => panic!("expected real"),
    };
    assert!(val.is_finite() && val > 0.0 && val < 1.0, "v(out)={val}");
}
