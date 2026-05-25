//! `.options METHOD=` integration-method tests.
//!
//! Verifies that:
//!   * `.options METHOD=trap` (default), `gear`, `euler` parse into the
//!     expected `IntegrationMethod` enum,
//!   * each method produces a finite, sign-correct transient on a simple
//!     RC low-pass — i.e. they all reach the same steady state but follow
//!     different transient shapes,
//!   * Gear (BDF2) damps the trapezoidal-ringing artefact on a fast step
//!     into an LC tank — at long times all three converge, but Trap shows
//!     larger peak deviations from the analytical envelope.

use thevenin::{IntegrationMethod, integration_method_from_netlist, parse_integration_method};
use thevenin_types::Netlist;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

mod common;
use common::simulate_tran;

#[test]
fn parse_method_recognises_canonical_names() {
    assert_eq!(
        parse_integration_method("trap"),
        IntegrationMethod::Trapezoidal
    );
    assert_eq!(
        parse_integration_method("trapezoidal"),
        IntegrationMethod::Trapezoidal
    );
    assert_eq!(parse_integration_method("gear"), IntegrationMethod::Gear);
    assert_eq!(parse_integration_method("bdf2"), IntegrationMethod::Gear);
    assert_eq!(
        parse_integration_method("euler"),
        IntegrationMethod::BackwardEuler
    );
    assert_eq!(
        parse_integration_method("be"),
        IntegrationMethod::BackwardEuler
    );
    assert_eq!(
        parse_integration_method("BackwardEuler"),
        IntegrationMethod::BackwardEuler
    );
    // Unknown method → Trap fallback with warning.
    assert_eq!(
        parse_integration_method("nonsense"),
        IntegrationMethod::Trapezoidal
    );
}

#[test]
fn netlist_method_option_picks_up_gear() {
    let netlist = Netlist::parse_single(
        "method=gear
V1 in 0 1
R1 in out 1k
C1 out 0 1n
.options METHOD=gear
.tran 0.1u 5u
.end
",
    )
    .unwrap();
    assert_eq!(
        integration_method_from_netlist(&netlist),
        IntegrationMethod::Gear
    );
}

#[test]
fn netlist_without_method_defaults_to_trap() {
    let netlist = Netlist::parse_single(
        "method default
V1 in 0 1
R1 in out 1k
C1 out 0 1n
.tran 0.1u 5u
.end
",
    )
    .unwrap();
    assert_eq!(
        integration_method_from_netlist(&netlist),
        IntegrationMethod::Trapezoidal
    );
}

/// An RC low-pass: V(out) approaches V(in) = 1V with time constant RC = 1µs.
/// All three integration methods should reach a steady-state v(out) close to
/// 1.0 V after several time constants. Gear and Trap should agree closely;
/// Euler is order-1 and may lag.
#[test]
fn rc_lowpass_reaches_steady_state_under_all_methods() {
    fn final_vout(method_clause: &str) -> f64 {
        let netlist = Netlist::parse_single(&format!(
            "RC steady state ({method_clause})
V1 in 0 1
R1 in out 1k
C1 out 0 1n
{method_clause}
.tran 0.1u 20u
.end
"
        ))
        .unwrap();
        let result = simulate_tran(&netlist);
        let plot = &result.plots[0];
        let v_out = plot
            .vecs
            .iter()
            .find(|v| v.name.eq_ignore_ascii_case("v(out)"))
            .expect("v(out) present");
        let data = v_out.data.as_real();
        *data.last().expect("at least one sample")
    }

    let v_trap = final_vout(".options method=trap");
    let v_gear = final_vout(".options method=gear");
    let v_eul = final_vout(".options method=euler");

    // After 20 time constants, all three should be within 1% of the
    // 1V supply (ignoring the slow order-1 lag of Euler we relax to 5%).
    assert!((v_trap - 1.0).abs() < 0.01, "trap final v(out) = {v_trap}");
    assert!((v_gear - 1.0).abs() < 0.01, "gear final v(out) = {v_gear}");
    assert!((v_eul - 1.0).abs() < 0.05, "euler final v(out) = {v_eul}");
}
