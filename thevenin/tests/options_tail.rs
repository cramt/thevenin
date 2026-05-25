//! Integration tests for the `.options` long-tail surfaced in r6:
//! DEFAD / DEFAS / DEFL / DEFW (MOSFET geometry defaults),
//! NOOPALTER (parse-only flag for the `.alter` re-solve pathway), and
//! GMINPRIORITY (try Gmin stepping before direct NR).
//!
//! The pattern mirrors `options_convergence.rs`: parser plumbing goes
//! through the IR-side `nr_options_from_circuit`, and behavioural
//! assertions drive the full Circuit-input simulator surface.

use cirq_spice_import::import_netlist;
use thevenin::mna_ir::nr_options_from_circuit;
use thevenin_types::Netlist;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

mod common;
use common::simulate_op;

/// Snapshot of the six new option fields, captured through the IR-side
/// resolver.
struct Opts {
    defad: f64,
    defas: f64,
    defl: f64,
    defw: f64,
    noopalter: bool,
    gminpriority: bool,
}

fn opts_from_spice(spice: &str) -> Opts {
    let mut netlist = Netlist::parse_single(spice).expect("parse");
    thevenin::expr::resolve_netlist_exprs(&mut netlist).expect("resolve");
    let circuit = import_netlist(&netlist).expect("import");
    let o = nr_options_from_circuit(&circuit);
    Opts {
        defad: o.defad,
        defas: o.defas,
        defl: o.defl,
        defw: o.defw,
        noopalter: o.noopalter,
        gminpriority: o.gminpriority,
    }
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

/// Defaults match ngspice `cktinit.c`: DEFL = DEFW = 1e-4,
/// DEFAD = DEFAS = 0, NOOPALTER = GMINPRIORITY = false.
#[test]
fn options_tail_defaults_match_ngspice_cktinit() {
    let opts = opts_from_spice(
        "defaults
V1 1 0 1
R1 1 0 1k
.op
.end
",
    );
    assert_eq!(opts.defad, 0.0);
    assert_eq!(opts.defas, 0.0);
    assert_eq!(opts.defl, 1e-4);
    assert_eq!(opts.defw, 1e-4);
    assert!(!opts.noopalter);
    assert!(!opts.gminpriority);
}

// ---------------------------------------------------------------------------
// Parser plumbing
// ---------------------------------------------------------------------------

/// `.options DEFL=1u DEFW=2u DEFAD=3p DEFAS=4p` round-trips into the
/// resolved options struct.
#[test]
fn options_tail_mosfet_geometry_defaults_parse() {
    let opts = opts_from_spice(
        "mos geometry defaults
V1 1 0 1
R1 1 0 1k
.options DEFL=1u DEFW=2u DEFAD=3p DEFAS=4p
.op
.end
",
    );
    approx::assert_abs_diff_eq!(opts.defl, 1e-6, epsilon = 1e-18);
    approx::assert_abs_diff_eq!(opts.defw, 2e-6, epsilon = 1e-18);
    approx::assert_abs_diff_eq!(opts.defad, 3e-12, epsilon = 1e-24);
    approx::assert_abs_diff_eq!(opts.defas, 4e-12, epsilon = 1e-24);
}

/// `.options NOOPALTER=1 GMINPRIORITY=1` parses to true.
#[test]
fn options_tail_noopalter_and_gminpriority_parse() {
    let opts = opts_from_spice(
        "noopalter+gminpriority
V1 1 0 1
R1 1 0 1k
.options NOOPALTER=1 GMINPRIORITY=1
.op
.end
",
    );
    assert!(opts.noopalter);
    assert!(opts.gminpriority);
}

/// `.options NOOPALTER=0 GMINPRIORITY=0` parses to false (the historical
/// default behaviour).
#[test]
fn options_tail_noopalter_and_gminpriority_zero_is_false() {
    let opts = opts_from_spice(
        "noopalter+gminpriority zeros
V1 1 0 1
R1 1 0 1k
.options NOOPALTER=0 GMINPRIORITY=0
.op
.end
",
    );
    assert!(!opts.noopalter);
    assert!(!opts.gminpriority);
}

// ---------------------------------------------------------------------------
// Behaviour: DEFL / DEFW supply MOSFET geometry when the instance omits L/W
// ---------------------------------------------------------------------------

/// **DEFL / DEFW are applied at MOSFET stamp time when the instance
/// omits L/W**.
///
/// Two simple NMOS amplifiers, identical except for whether L/W is set
/// per-instance vs. via `.options DEFL/DEFW`. The drain currents must
/// agree to numerical precision — the DEFL/DEFW values must reach the
/// device-stamp arithmetic.
#[test]
fn defl_defw_are_applied_when_instance_omits_l_w() {
    // Per-instance L/W on M1; nothing on the device.
    let explicit = "explicit LW
M1 D G 0 0 nmos1 L=2u W=20u
VDD D 0 5
VGS G 0 2
.model nmos1 NMOS LEVEL=1 VTO=1.0 KP=50u
.op
.end
";
    // Same circuit but L/W come from `.options DEFL=2u DEFW=20u`.
    let from_options = "via DEFL/DEFW
M1 D G 0 0 nmos1
VDD D 0 5
VGS G 0 2
.model nmos1 NMOS LEVEL=1 VTO=1.0 KP=50u
.options DEFL=2u DEFW=20u
.op
.end
";

    let r1 = simulate_op(&Netlist::parse_single(explicit).expect("parse explicit"));
    let r2 = simulate_op(&Netlist::parse_single(from_options).expect("parse from_options"));

    // Both runs should produce identical node voltages — DEFL/DEFW is the
    // only difference, and it must land on the same per-instance numbers.
    let p1 = &r1.plots[0];
    let p2 = &r2.plots[0];
    for vec1 in &p1.vecs {
        let vec2 = p2
            .vecs
            .iter()
            .find(|v| v.name == vec1.name)
            .unwrap_or_else(|| panic!("missing vector {} in DEFL/DEFW run", vec1.name));
        let a = vec1.data.as_real();
        let b = vec2.data.as_real();
        assert_eq!(a.len(), b.len(), "{}: length mismatch", vec1.name);
        for (x, y) in a.iter().zip(b.iter()) {
            approx::assert_abs_diff_eq!(*x, *y, epsilon = 1e-9);
        }
    }
}

/// **DEFAD / DEFAS round-trip without breaking a MOSFET OP**.
///
/// MOSFET junction-area parameters affect bulk-drain / bulk-source
/// junction capacitances at DC only through reverse leakage (typically
/// negligible). The assertion here is that the OP still converges with
/// a non-zero DEFAD/DEFAS — the option is wired through without
/// destabilising the stamp.
#[test]
fn defad_defas_do_not_break_mos_op() {
    let spice = "defad/defas
M1 D G 0 0 nmos1 L=2u W=20u
VDD D 0 5
VGS G 0 2
.model nmos1 NMOS LEVEL=1 VTO=1.0 KP=50u CJ=1e-4
.options DEFAD=1p DEFAS=1p
.op
.end
";
    let netlist = Netlist::parse_single(spice).unwrap();
    let result = simulate_op(&netlist);
    assert!(!result.plots.is_empty());
}

// ---------------------------------------------------------------------------
// Behaviour: GMINPRIORITY still converges an easy circuit
// ---------------------------------------------------------------------------

/// **GMINPRIORITY runs Gmin first but still converges a trivial circuit**.
///
/// A linear resistor divider doesn't exercise the NR path at all, so the
/// option is purely a plumbing assertion at this level: setting it must
/// not break easy circuits. The full convergence-order swap is exercised
/// implicitly by the unit tests in `newton.rs`.
#[test]
fn gminpriority_does_not_break_easy_circuit() {
    let netlist = Netlist::parse_single(
        "gminpriority easy
V1 1 0 1
R1 1 2 1k
R2 2 0 1k
.options GMINPRIORITY=1
.op
.end
",
    )
    .unwrap();
    let result = simulate_op(&netlist);
    let plot = &result.plots[0];
    let v2 = plot
        .vecs
        .iter()
        .find(|v| v.name == "v(2)")
        .expect("v(2) missing")
        .data
        .as_real()[0];
    approx::assert_abs_diff_eq!(v2, 0.5, epsilon = 1e-9);
}

/// **GMINPRIORITY converges a nonlinear (diode) circuit**.
///
/// A diode bridges the direct-NR success path on most fixtures. With
/// GMINPRIORITY set, the solver tries Gmin stepping first and the
/// direct attempt becomes the fallback. The result must still be the
/// correct OP — proving the reorder didn't break the NR loop.
#[test]
fn gminpriority_converges_nonlinear_circuit() {
    let with_priority = Netlist::parse_single(
        "diode with gminpriority
V1 1 0 1
R1 1 2 1k
D1 2 0 dmod
.model dmod D IS=1e-14
.options GMINPRIORITY=1
.op
.end
",
    )
    .unwrap();
    let r1 = simulate_op(&with_priority);
    let v2_priority = r1.plots[0]
        .vecs
        .iter()
        .find(|v| v.name == "v(2)")
        .expect("v(2) missing")
        .data
        .as_real()[0];

    // Same circuit without the option — must produce the same OP.
    let without_priority = Netlist::parse_single(
        "diode without gminpriority
V1 1 0 1
R1 1 2 1k
D1 2 0 dmod
.model dmod D IS=1e-14
.op
.end
",
    )
    .unwrap();
    let r2 = simulate_op(&without_priority);
    let v2_baseline = r2.plots[0]
        .vecs
        .iter()
        .find(|v| v.name == "v(2)")
        .expect("v(2) missing")
        .data
        .as_real()[0];

    approx::assert_abs_diff_eq!(v2_priority, v2_baseline, epsilon = 1e-6);
}

// ---------------------------------------------------------------------------
// Behaviour: NOOPALTER is parse-only for now
// ---------------------------------------------------------------------------

/// **NOOPALTER parses without affecting OP**.
///
/// `.alter` mutates the IR but does not itself trigger a solve — the
/// next analysis picks up the mutation. NOOPALTER therefore has no
/// live re-solve to short-circuit yet; the option is stored on
/// `NrOptions::noopalter` and waits for the alter-and-re-solve pathway.
#[test]
fn noopalter_is_parse_only_today() {
    let opts = opts_from_spice(
        "noopalter only
V1 1 0 1
R1 1 0 1k
.options NOOPALTER=1
.op
.end
",
    );
    assert!(opts.noopalter);

    // And the OP still converges.
    let netlist = Netlist::parse_single(
        "noopalter+op
V1 1 0 1
R1 1 0 1k
.options NOOPALTER=1
.op
.end
",
    )
    .unwrap();
    let result = simulate_op(&netlist);
    assert!(!result.plots.is_empty());
}
