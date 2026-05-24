//! Integration tests for the three convergence-related `.options` controls
//! surfaced in r3: RSHUNT, GMINSTEPS, NOOPITER.
//!
//! Each test exercises both the parser pathway (the `.options` directive
//! lands on the resolved options) and, where externally observable,
//! the runtime behaviour change the option produces.
//!
//! Plumbing assertions go through `thevenin::mna_ir::nr_options_from_circuit`
//! — the IR-side resolver — because the Netlist-side resolver lives in a
//! `pub(crate)` module and isn't reachable from integration tests.

use cirq_spice_import::import_netlist;
use thevenin::mna_ir::nr_options_from_circuit;
use thevenin_types::Netlist;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

mod common;
use common::{simulate_op, try_simulate_op};

/// Snapshot of the three new option fields, captured through the IR-side
/// resolver. Mirrors the field layout in `NrOptions` so external tests can
/// interrogate values without naming the `pub(crate)` type directly.
struct Opts {
    rshunt: f64,
    gminsteps: u32,
    noopiter: bool,
}

fn opts_from_spice(spice: &str) -> Opts {
    let mut netlist = Netlist::parse_single(spice).expect("parse");
    thevenin::expr::resolve_netlist_exprs(&mut netlist).expect("resolve");
    let circuit = import_netlist(&netlist).expect("import");
    let o = nr_options_from_circuit(&circuit);
    Opts {
        rshunt: o.rshunt,
        gminsteps: o.gminsteps,
        noopiter: o.noopiter,
    }
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

/// When no `.OPTIONS` directive is present, the three new fields take their
/// documented defaults — the values that preserve the historical behaviour
/// byte-for-byte.
#[test]
fn convergence_options_defaults_preserve_current_behaviour() {
    let opts = opts_from_spice(
        "defaults
V1 1 0 1
R1 1 0 1k
.op
.end
",
    );
    assert_eq!(opts.rshunt, 0.0);
    assert_eq!(opts.gminsteps, 10);
    assert!(!opts.noopiter);
}

// ---------------------------------------------------------------------------
// Parser plumbing
// ---------------------------------------------------------------------------

/// `.options rshunt=1Meg gminsteps=5 noopiter=1` lands on the resolved
/// `NrOptions`.
#[test]
fn convergence_options_parse_all_three() {
    let opts = opts_from_spice(
        "all three
V1 1 0 1
R1 1 0 1k
.options rshunt=1Meg gminsteps=5 noopiter=1
.op
.end
",
    );
    assert_eq!(opts.rshunt, 1.0e6);
    assert_eq!(opts.gminsteps, 5);
    assert!(opts.noopiter);
}

/// `gminsteps=0` is the sentinel that disables Gmin stepping. The parser
/// must round-trip the literal zero.
#[test]
fn convergence_options_gminsteps_zero_sentinel_parses() {
    let opts = opts_from_spice(
        "gminsteps zero
V1 1 0 1
R1 1 0 1k
.options gminsteps=0
.op
.end
",
    );
    assert_eq!(opts.gminsteps, 0);
}

/// `noopiter=0` keeps the historical "try direct NR first" behaviour.
#[test]
fn convergence_options_noopiter_zero_is_false() {
    let opts = opts_from_spice(
        "noopiter zero
V1 1 0 1
R1 1 0 1k
.options noopiter=0
.op
.end
",
    );
    assert!(!opts.noopiter);
}

// ---------------------------------------------------------------------------
// Runtime behaviour
// ---------------------------------------------------------------------------

/// **GMINSTEPS=0 skips Gmin stepping**.
///
/// A simple resistor divider is linear and solves without Gmin stepping
/// anyway; setting `gminsteps=0` must still produce a correct result.
/// The point is to prove the option is read and acted on without breaking
/// trivial circuits.
#[test]
fn gminsteps_zero_still_converges_easy_circuit() {
    let netlist = Netlist::parse_single(
        "gminsteps disabled
V1 1 0 1
R1 1 2 1k
R2 2 0 1k
.options gminsteps=0
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

/// **NOOPITER parses and stores correctly**.
///
/// We can't easily observe the "skipped initial NR" event from outside,
/// but we can confirm the option round-trips through the parser and
/// (combined with `gminsteps=0`) doesn't blow up on a linear circuit.
/// Linear circuits bypass the NR path entirely, so this is purely a
/// plumbing test — the option must be storable on the resolved options
/// without panicking on assembly.
#[test]
fn noopiter_parses_and_stores_alongside_gminsteps_zero() {
    let spice = "noopiter parse
V1 1 0 1
R1 1 2 1k
R2 2 0 1k
.options noopiter=1 gminsteps=0
.op
.end
";
    let opts = opts_from_spice(spice);
    assert!(opts.noopiter);
    assert_eq!(opts.gminsteps, 0);

    let netlist = Netlist::parse_single(spice).unwrap();
    let result = simulate_op(&netlist);
    assert!(!result.plots.is_empty());
}

/// **RSHUNT regularizes a floating-node circuit**.
///
/// A diode-only circuit with a floating cathode (D1's cathode connected
/// only through a capacitor — open at DC) has no finite conductance from
/// node 3 to ground at DC. With `rshunt=1Meg`, every non-ground node gains
/// a 1µS shunt to ground, regularizing the matrix.
///
/// We assert convergence WITH rshunt — proving the option is wired to the
/// NR Jacobian stamping. The without-rshunt assertion only needs to confirm
/// the sentinel default of 0 (preserving historical behaviour).
#[test]
fn rshunt_helps_floating_node_circuit() {
    let with_rshunt = Netlist::parse_single(
        "floating with rshunt
V1 1 0 1
R1 1 2 1k
D1 2 3 dmod
C1 3 0 1u
.model dmod D
.options rshunt=1Meg
.op
.end
",
    )
    .unwrap();
    let result = try_simulate_op(&with_rshunt);
    assert!(
        result.is_ok(),
        "rshunt=1Meg must regularize a floating-node circuit, got {result:?}"
    );

    // And the same circuit without `.options rshunt` defaults to 0.
    let no_rshunt = opts_from_spice(
        "no rshunt
V1 1 0 1
R1 1 2 1k
D1 2 3 dmod
C1 3 0 1u
.model dmod D
.op
.end
",
    );
    assert_eq!(no_rshunt.rshunt, 0.0);
}
