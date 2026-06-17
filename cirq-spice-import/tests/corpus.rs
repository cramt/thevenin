//! C7 (1.0 checklist): a focused real-world netlist corpus that the SPICE
//! importer must accept. These decks are copied verbatim from the upstream
//! ngspice example set (`ngspice-upstream/examples/`, which is gitignored) into
//! `tests/fixtures/corpus/` so the suite runs on a clean clone.
//!
//! The bar here is **successful import** — each deck must lower to a non-empty
//! `cirq_ir::Circuit` without error. Full simulation of these (some are large
//! analog blocks) is out of scope for this test; correctness of the engine is
//! covered by the ngspice harness fixtures.
//!
//! The set deliberately spans device/circuit classes that exercise different
//! importer paths: a switching converter (VDMOS), a multi-stage op-amp /
//! transimpedance amplifier, a BSIM3 CMOS pair, a digital CMOS adder, coupled
//! transmission lines, and a VBIC bipolar ECL flip-flop.

use cirq_spice_import::import_spice;

/// Import a corpus deck and assert it produced at least one circuit with at
/// least one element. Returns the element count of the first circuit so the
/// individual tests can pin a coarse size, guarding against a deck silently
/// importing as empty.
fn import_corpus(name: &str, src: &str) -> usize {
    let circuits =
        import_spice(src).unwrap_or_else(|e| panic!("corpus deck '{name}' failed to import: {e}"));
    assert!(
        !circuits.is_empty(),
        "corpus deck '{name}' produced no circuits"
    );
    let n = circuits[0].elements.len();
    assert!(n > 0, "corpus deck '{name}' imported with zero elements");
    n
}

#[test]
fn vdmos_dcdc_converter_imports() {
    // Switching DC-DC converter built around VDMOS power MOSFETs written in
    // the 3-terminal `M d g s model` form.
    let n = import_corpus(
        "vdmos_dcdc_converter",
        include_str!("fixtures/corpus/vdmos_dcdc_converter.sp"),
    );
    assert!(n >= 5);
}

#[test]
fn transimpedance_amp_imports() {
    // A large multi-device analog amplifier — the heaviest deck in the set.
    let n = import_corpus(
        "transimpedance_amp",
        include_str!("fixtures/corpus/transimpedance_amp.net"),
    );
    assert!(n >= 100);
}

#[test]
fn nmos_pmos_bsim3_imports() {
    import_corpus(
        "nmos_pmos_bsim3",
        include_str!("fixtures/corpus/nmos_pmos_bsim3.sp"),
    );
}

#[test]
fn adder_mos_imports() {
    // Digital CMOS adder — many MOSFET instances + subcircuits.
    let n = import_corpus("adder_mos", include_str!("fixtures/corpus/adder_mos.cir"));
    assert!(n >= 50);
}

#[test]
fn coupled_lines_ibm_imports() {
    let n = import_corpus(
        "coupled_lines_ibm",
        include_str!("fixtures/corpus/coupled_lines_ibm.sp"),
    );
    assert!(n >= 50);
}

#[test]
fn vbic_ecl_dff_imports() {
    // VBIC bipolar emitter-coupled-logic D flip-flop.
    import_corpus(
        "vbic_ecl_dff",
        include_str!("fixtures/corpus/vbic_ecl_dff.sp"),
    );
}
