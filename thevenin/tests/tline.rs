//! Integration tests for the ideal lossless transmission line (T element).

use thevenin_types::{ElementKind, Netlist};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

mod common;
use common::{simulate_ac, simulate_op, simulate_tran};

/// Fetch a vector from a transient/AC result by name.
fn vector<'a>(result: &'a thevenin_types::SimResult, name: &str) -> &'a [f64] {
    result.plots[0]
        .vecs
        .iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("no vector '{name}'"))
        .data
        .as_real()
}

fn complex_vector<'a>(
    result: &'a thevenin_types::SimResult,
    name: &str,
) -> &'a [thevenin_types::Complex] {
    result.plots[0]
        .vecs
        .iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("no vector '{name}'"))
        .data
        .as_complex()
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

#[test]
fn parser_basic_t_element() {
    let netlist = Netlist::parse_single(
        "T element parse smoke test
T1 1 0 2 0 Z0=50 TD=1n
.op
.end
",
    )
    .expect("parse failed");

    let elems: Vec<&thevenin_types::Element> = netlist
        .items
        .iter()
        .filter_map(|i| match i {
            thevenin_types::Item::Element(e) => Some(e),
            _ => None,
        })
        .collect();

    assert_eq!(elems.len(), 1);
    match &elems[0].kind {
        ElementKind::Tline {
            pos1,
            neg1,
            pos2,
            neg2,
            td,
            ..
        } => {
            assert_eq!(pos1, "1");
            assert_eq!(neg1, "0");
            assert_eq!(pos2, "2");
            assert_eq!(neg2, "0");
            assert!(td.is_some());
        }
        other => panic!("expected Tline, got {other:?}"),
    }
}

#[test]
fn parser_requires_z0() {
    let err = Netlist::parse_single(
        "missing Z0
T1 1 0 2 0 TD=1n
.op
.end
",
    )
    .expect_err("expected parse error");
    let s = format!("{err:?}");
    assert!(s.contains("Z0"), "error did not mention Z0: {s}");
}

#[test]
fn parser_requires_td_or_f() {
    let err = Netlist::parse_single(
        "missing TD/F
T1 1 0 2 0 Z0=50
.op
.end
",
    )
    .expect_err("expected parse error");
    let s = format!("{err:?}");
    assert!(s.contains("TD") || s.contains("F"), "got {s}");
}

#[test]
fn parser_accepts_f_nl_form() {
    let netlist = Netlist::parse_single(
        "F+NL form
T1 1 0 2 0 Z0=50 F=1e9 NL=0.5
.op
.end
",
    )
    .expect("parse failed");
    let elems: Vec<&thevenin_types::Element> = netlist
        .items
        .iter()
        .filter_map(|i| match i {
            thevenin_types::Item::Element(e) => Some(e),
            _ => None,
        })
        .collect();
    match &elems[0].kind {
        ElementKind::Tline { td, f, nl, .. } => {
            assert!(td.is_none());
            assert!(f.is_some());
            assert!(nl.is_some());
        }
        other => panic!("expected Tline, got {other:?}"),
    }
}

#[test]
fn parser_ic_four_values() {
    let netlist = Netlist::parse_single(
        "T+IC
T1 1 0 2 0 Z0=50 TD=1n IC=0.5,0.01,-0.5,-0.01
.op
.end
",
    )
    .expect("parse failed");
    let elems: Vec<&thevenin_types::Element> = netlist
        .items
        .iter()
        .filter_map(|i| match i {
            thevenin_types::Item::Element(e) => Some(e),
            _ => None,
        })
        .collect();
    match &elems[0].kind {
        ElementKind::Tline { ic, .. } => {
            assert!(ic.is_some(), "ic should be Some");
        }
        other => panic!("expected Tline, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// DC / OP
// ---------------------------------------------------------------------------

/// Ideal lossless T-line at DC behaves as a wire: V1 = V2, I1 = -I2.
///
/// Configuration:
///   `V1 = 5V` at node 1, T-line `Z0=50, TD=1ns` between nodes 1 and 2,
///   `R = 50Ω` from node 2 to ground.
///
/// Expected: V(2) = 5V (line is a wire at DC, R drops the full source
/// voltage). The brief's "voltage divider giving V(2)=2.5V" description
/// is incorrect; the ngspice T element at DC matches a wire (see
/// `traload.c` MODEDC stamp: branch1 = V2 + Z0*I2 in series with branch2
/// = V1 + Z0*I1 collapses to V1 = V2 / I1 = -I2 — see math in
/// `thevenin/src/tline.rs` doc comment).
#[test]
fn op_behaves_as_wire_at_dc() {
    let netlist = Netlist::parse_single(
        "T-line DC wire test
V1 1 0 5
T1 1 0 2 0 Z0=50 TD=1n
R2 2 0 50
.op
.end
",
    )
    .expect("parse failed");

    let result = simulate_op(&netlist);
    let v2 = result.plots[0]
        .vecs
        .iter()
        .find(|v| v.name == "v(2)")
        .unwrap()
        .data
        .as_real();
    assert!(
        (v2[0] - 5.0).abs() < 1e-6,
        "expected V(2)=5.0 (wire), got {}",
        v2[0]
    );
}

// ---------------------------------------------------------------------------
// Transient
// ---------------------------------------------------------------------------

/// Matched-termination delay test: a step launched into a matched
/// 50Ω/50Ω/50Ω line should reach the far end (V(3)) after TD with
/// half-amplitude (because the source 50Ω forms a voltage divider with
/// the line's Z0=50).
#[test]
fn tran_matched_step_delay() {
    let netlist = Netlist::parse_single(
        "T-line matched step
V1 1 0 PULSE(0 2 0 0.05n 0.05n 50n 100n)
R1 1 2 50
T1 2 0 3 0 Z0=50 TD=2n
R2 3 0 50
.tran 0.05n 10n
.end
",
    )
    .expect("parse failed");

    let result = simulate_tran(&netlist);
    let time = vector(&result, "time");
    let v3 = vector(&result, "v(3)");

    // Before TD=2ns, v(3) should be near 0.
    let idx_before = time.iter().position(|&t| t >= 1.0e-9).unwrap();
    assert!(
        v3[idx_before].abs() < 0.05,
        "v(3) should be ~0 before TD, got {} at t={}",
        v3[idx_before],
        time[idx_before]
    );

    // After TD + transient settling, v(3) should hit ~1V (half of 2V).
    // Matched terminations → no reflection.
    let idx_after = time.iter().position(|&t| t >= 5.0e-9).unwrap();
    assert!(
        (v3[idx_after] - 1.0).abs() < 0.15,
        "v(3) should be ~1V at matched delay, got {} at t={}",
        v3[idx_after],
        time[idx_after]
    );
}

/// Open-end termination: an incident wave reflects with +1 reflection
/// coefficient, so the far-end voltage transient should reach ~2x the
/// incident wave amplitude before settling.
#[test]
fn tran_open_end_reflection_doubles() {
    let netlist = Netlist::parse_single(
        "T-line open-end reflection
V1 1 0 PULSE(0 1 0 0.05n 0.05n 50n 100n)
R1 1 2 50
T1 2 0 3 0 Z0=50 TD=2n
R2 3 0 1e9
.tran 0.05n 8n
.end
",
    )
    .expect("parse failed");

    let result = simulate_tran(&netlist);
    let time = vector(&result, "time");
    let v3 = vector(&result, "v(3)");

    // Source-end Thevenin (R1=50, V=1V) launches V_inc = 0.5V into the
    // matched line. Open-end reflection doubles it to ~1V at node 3.
    let idx_after = time.iter().position(|&t| t >= 4.5e-9).unwrap();
    assert!(
        v3[idx_after] > 0.7 && v3[idx_after] <= 1.05,
        "v(3) after open-end reflection should be ~1V, got {} at t={}",
        v3[idx_after],
        time[idx_after]
    );
}

// ---------------------------------------------------------------------------
// AC
// ---------------------------------------------------------------------------

/// At very low frequency (ω*TD ≪ 1) a T-line is a wire. With a matched
/// 50Ω termination, the transfer V(2)/V(1) ≈ 0.5 (the source divides 1:1
/// between R1 and the line's effective DC short to ground via R2).
#[test]
fn ac_low_freq_matched_divider() {
    // Sweep across several decades; check that the magnitude at the lowest
    // point is the matched-line divider value (Z0 = 50 in series with
    // R1 = 50, so V(3) = V1 / 2 = 0.5 V at frequencies low enough that the
    // line's phase delay is small but non-negligible — full DC pins the
    // sum-equation row to a structurally-singular sub-block).
    let netlist = Netlist::parse_single(
        "T-line low-frequency matched divider
V1 1 0 AC 1
R1 1 2 50
T1 2 0 3 0 Z0=50 TD=1n
R2 3 0 50
.ac dec 5 10Meg 1G
.end
",
    )
    .expect("parse failed");

    let result = simulate_ac(&netlist);
    let v3 = complex_vector(&result, "v(3)");
    let mag_low = v3[0].magnitude();
    assert!(
        (mag_low - 0.5).abs() < 0.01,
        "low-freq |V(3)| should be ~0.5 (matched divider), got {mag_low}"
    );
    // At higher frequency (ω*TD = π/2 when f = 250 MHz, beyond our sweep)
    // the impedance transformation kicks in — but the magnitude should
    // remain 0.5 across all frequencies for a matched line.
    let mag_high = v3.last().unwrap().magnitude();
    assert!(
        (mag_high - 0.5).abs() < 0.05,
        "high-freq |V(3)| should still be ~0.5 (matched), got {mag_high}"
    );
}

// ---------------------------------------------------------------------------
// SPICE → IR → SPICE round-trip
// ---------------------------------------------------------------------------

#[test]
fn round_trip_through_ir() {
    let src = "T-line round trip
V1 1 0 5
T1 1 0 2 0 Z0=50 TD=1n
R1 2 0 100
.op
.end
";
    let netlist = Netlist::parse_single(src).expect("parse failed");
    let mut resolved = netlist.clone();
    thevenin::expr::resolve_netlist_exprs(&mut resolved).expect("resolve");

    let mut circuit = cirq_spice_import::import_netlist(&resolved).expect("import");
    // Force OP so the round-trip doesn't depend on the test simulator
    // selecting a specific analysis.
    circuit.analyses = vec![cirq_ir::Analysis::Op];

    let back = cirq_frontend::to_netlist::circuit_to_netlists(&circuit)
        .expect("circuit_to_netlists")
        .into_iter()
        .next()
        .expect("at least one netlist");

    // The round-trip element list should contain a Tline with the same
    // topology + parameters we started with.
    let tline = back
        .items
        .iter()
        .find_map(|item| match item {
            thevenin_types::Item::Element(e) => match &e.kind {
                ElementKind::Tline { .. } => Some(e),
                _ => None,
            },
            _ => None,
        })
        .expect("Tline element missing after round-trip");
    match &tline.kind {
        ElementKind::Tline {
            pos1,
            neg1,
            pos2,
            neg2,
            z0,
            td,
            ..
        } => {
            assert_eq!(pos1, "1");
            assert_eq!(neg1, "0");
            assert_eq!(pos2, "2");
            assert_eq!(neg2, "0");
            // Z0 and TD should evaluate back to 50 and 1e-9.
            let z0_val = match z0 {
                thevenin_types::Expr::Num(v) => *v,
                _ => panic!("z0 not a number"),
            };
            assert!((z0_val - 50.0).abs() < 1e-9);
            let td_val = match td.as_ref().unwrap() {
                thevenin_types::Expr::Num(v) => *v,
                _ => panic!("td not a number"),
            };
            assert!((td_val - 1.0e-9).abs() < 1e-18);
        }
        other => panic!("expected Tline, got {other:?}"),
    }
}
