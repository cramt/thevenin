//! C2 (1.0 checklist): `.model` cards may carry brace expressions that
//! reference top-level `.param` values, e.g. `.model nch nmos (vto={vt0+dvt})`.
//! These must resolve end-to-end so the device sees the computed value.

use cirq_spice_import::import_spice;

/// Operating-point drain-node voltage of a level-1 NMOS gain stage. The drain
/// current — and therefore `v(d)` through the 10k load — depends strongly on
/// the threshold voltage `VTO`, which makes it a sensitive probe of whether a
/// model parameter actually took the intended value.
fn vd_op(deck: &str) -> f64 {
    let circuits = import_spice(deck).expect("import");
    let result = thevenin::circuit::simulate(&circuits[0]).expect("simulate");
    let plot = &result.plots[0];
    plot.vecs
        .iter()
        .find(|v| v.name.eq_ignore_ascii_case("v(d)"))
        .expect("v(d) present")
        .data
        .as_real()[0]
}

fn deck_with_vto(param_line: &str, vto_field: &str) -> String {
    format!(
        "mos vto probe\n\
         {param_line}\
         Vdd dd 0 5\n\
         Rd dd d 10k\n\
         Vg g 0 3\n\
         M1 d g 0 0 nch L=1u W=10u\n\
         .model nch nmos (level=1 kp=200u {vto_field})\n\
         .op\n\
         .end\n"
    )
}

#[test]
fn model_brace_param_resolves_against_top_level_param() {
    // Brace expression referencing two top-level params: vt0 + dvt = 1.0.
    let brace = deck_with_vto(".param vt0=0.7 dvt=0.3\n", "vto={vt0+dvt}");
    let literal = deck_with_vto("", "vto=1.0");
    let wrong = deck_with_vto("", "vto=0.0");

    let v_brace = vd_op(&brace);
    let v_literal = vd_op(&literal);
    let v_wrong = vd_op(&wrong);

    // The brace deck must match the literal vto=1.0 deck...
    assert!(
        (v_brace - v_literal).abs() < 1e-6,
        "brace vto must resolve to 1.0: v(d) brace={v_brace}, literal={v_literal}"
    );
    // ...and must NOT match the vto=0 deck, proving the brace value actually
    // took effect rather than silently defaulting.
    assert!(
        (v_brace - v_wrong).abs() > 1e-3,
        "brace vto must differ from the vto=0 default: brace={v_brace}, wrong={v_wrong}"
    );
}
