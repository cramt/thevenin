//! Integration tests for the SPICE `S` (voltage-controlled) and `W`
//! (current-controlled) switch elements.

use approx::assert_abs_diff_eq;
use thevenin_types::Netlist;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

mod common;
use common::{simulate_op, simulate_tran};

fn vec_value(plot: &thevenin_types::SimPlot, name: &str) -> f64 {
    plot.vecs
        .iter()
        .find(|v| v.name.eq_ignore_ascii_case(name))
        .unwrap_or_else(|| panic!("missing vector `{name}` in plot {}", plot.name))
        .data
        .as_real()[0]
}

#[test]
fn s_element_parses_into_ir() {
    // The parser shape coverage: ensure an S element doesn't fail
    // parsing or import.
    let netlist = Netlist::parse_single(
        "S element parse smoke test
V1 1 0 5
V2 3 0 2
S1 1 2 3 0 SWMOD ON
RL 2 0 1k
.model SWMOD SW (Vt=1 Vh=0.2 Ron=0.5 Roff=1Meg)
.op
.end
",
    )
    .unwrap();

    let result = simulate_op(&netlist);
    let plot = &result.plots[0];
    let v1 = vec_value(plot, "v(1)");
    assert_abs_diff_eq!(v1, 5.0, epsilon = 1e-9);
}

#[test]
fn s_element_on_state_passes_voltage_with_ron_drop() {
    // Control voltage well above the threshold → switch ON.
    // The switch sits between V1 and a 1k load to ground; in the ON state
    // its conductance is 1/Ron = 1/0.5 = 2 S. The load 1k draws roughly
    // 5V/1000.5 ≈ 4.9975 mA, so the load voltage v(out) ≈ 4.9975 V.
    let netlist = Netlist::parse_single(
        "S element ON behaviour
V1 1 0 5
Vctl ctl 0 3
S1 1 out ctl 0 SWMOD
RL out 0 1k
.model SWMOD SW (Vt=1 Vh=0.2 Ron=0.5 Roff=1Meg)
.op
.end
",
    )
    .unwrap();

    let result = simulate_op(&netlist);
    let plot = &result.plots[0];
    let v_out = vec_value(plot, "v(out)");
    // Expected: 5 * 1000/(1000+0.5) ≈ 4.99750
    assert_abs_diff_eq!(v_out, 5.0 * 1000.0 / 1000.5, epsilon = 1e-6);
}

#[test]
fn s_element_off_state_blocks_with_roff() {
    // Control voltage well below the threshold → switch OFF.
    // The switch's conductance is 1/Roff = 1 µS. With a 1k load:
    //   v(out) = 5 * 1k / (1k + 1MEG) ≈ 4.995 mV.
    let netlist = Netlist::parse_single(
        "S element OFF behaviour
V1 1 0 5
Vctl ctl 0 -1
S1 1 out ctl 0 SWMOD
RL out 0 1k
.model SWMOD SW (Vt=1 Vh=0.2 Ron=0.5 Roff=1Meg)
.op
.end
",
    )
    .unwrap();

    let result = simulate_op(&netlist);
    let plot = &result.plots[0];
    let v_out = vec_value(plot, "v(out)");
    let expected = 5.0 * 1000.0 / (1000.0 + 1.0e6);
    assert_abs_diff_eq!(v_out, expected, epsilon = 1e-6);
}

#[test]
fn s_element_latches_off_inside_hysteresis_window_when_initial_off() {
    // Control sits inside the hysteresis window from the start. With no
    // ON/OFF flag the first iteration resolves below mid-window → OFF →
    // g = 1/Roff. v(out) divides 5V across 1MEG + 1k.
    let netlist = Netlist::parse_single(
        "S element latched OFF inside window
V1 1 0 5
Vctl ctl 0 0.25
S1 1 out ctl 0 SWMOD
RL out 0 1k
.model SWMOD SW (Vt=0.5 Vh=0.5 Ron=1 Roff=1Meg)
.op
.end
",
    )
    .unwrap();

    let result = simulate_op(&netlist);
    let plot = &result.plots[0];
    let v_out = vec_value(plot, "v(out)");
    let expected = 5.0 * 1000.0 / (1000.0 + 1.0e6);
    assert_abs_diff_eq!(v_out, expected, epsilon = 1e-5);
}

#[test]
fn w_element_current_controlled_on() {
    // Current-controlled switch. The W element senses current through Vmon
    // (the sense voltage source has 0 V, just measures current).
    //   I(Vmon) = 5 V / 1 kΩ = 5 mA, well above It + Ih.
    // Switch is ON, so V(out) ≈ 5 V * 1000 / 1000.5 ≈ 4.9975 V.
    let netlist = Netlist::parse_single(
        "W element ON behaviour
Vsig sig 0 5
Rsig sig mon 1k
Vmon mon 0 0
V1 1 0 5
W1 1 out Vmon CSWMOD
RL out 0 1k
.model CSWMOD CSW It=1m Ih=0.1m Ron=0.5 Roff=1Meg
.op
.end
",
    )
    .unwrap();

    let result = simulate_op(&netlist);
    let plot = &result.plots[0];
    let v_out = vec_value(plot, "v(out)");
    assert_abs_diff_eq!(v_out, 5.0 * 1000.0 / 1000.5, epsilon = 1e-5);
}

#[test]
fn w_element_current_controlled_off() {
    // Sense current is 0 mA (sense resistor connects to ground via Vmon
    // with no source on the sense side), so we're well below It. Switch
    // is OFF.
    let netlist = Netlist::parse_single(
        "W element OFF behaviour
Vmon mon 0 0
Rsig mon 0 1k
V1 1 0 5
W1 1 out Vmon CSWMOD
RL out 0 1k
.model CSWMOD CSW It=1m Ih=0.1m Ron=0.5 Roff=1Meg
.op
.end
",
    )
    .unwrap();

    let result = simulate_op(&netlist);
    let plot = &result.plots[0];
    let v_out = vec_value(plot, "v(out)");
    let expected = 5.0 * 1000.0 / (1000.0 + 1.0e6);
    assert_abs_diff_eq!(v_out, expected, epsilon = 1e-6);
}

#[test]
fn s_element_hysteresis_latches_during_transient_sweep() {
    // A slow control ramp drives the switch through the hysteresis window
    // and back. With Vt=2, Vh=1 the switch trips ON at V(ctl)=3 and OFF
    // again only when V(ctl)<1. Sample at V(ctl) ≈ 1.5 on the upward leg
    // (still OFF) and on the downward leg (latched ON).
    let netlist = Netlist::parse_single(
        "S element hysteresis sweep
V1 1 0 5
Vctl ctl 0 PWL(0 0 8u 5 16u 0)
S1 1 out ctl 0 SWMOD
RL out 0 1k
.model SWMOD SW (Vt=2 Vh=1 Ron=0.5 Roff=1Meg)
.tran 0.2u 16u
.end
",
    )
    .unwrap();

    let result = simulate_tran(&netlist);
    let plot = &result.plots[0];
    let times = plot
        .vecs
        .iter()
        .find(|v| v.name == "time")
        .unwrap()
        .data
        .as_real();
    let v_out = plot
        .vecs
        .iter()
        .find(|v| v.name.eq_ignore_ascii_case("v(out)"))
        .unwrap()
        .data
        .as_real();
    let v_ctl = plot
        .vecs
        .iter()
        .find(|v| v.name.eq_ignore_ascii_case("v(ctl)"))
        .unwrap()
        .data
        .as_real();

    // Pick samples on each leg where ctl is around 1.5 V (inside the
    // window, between Vt-Vh=1 and Vt+Vh=3).
    let mut upward_idx = None;
    let mut downward_idx = None;
    for (i, t) in times.iter().enumerate() {
        if (v_ctl[i] - 1.5).abs() < 0.2 {
            if *t < 8e-6 && upward_idx.is_none() {
                upward_idx = Some(i);
            } else if *t > 8e-6 && downward_idx.is_none() {
                downward_idx = Some(i);
            }
        }
    }
    let upward = upward_idx.expect("upward leg sample");
    let downward = downward_idx.expect("downward leg sample");

    // On the upward leg the switch hasn't crossed Vt+Vh=3 yet → OFF →
    // v(out) ≈ 0 V.  On the downward leg it has latched ON → v(out) ≈ 5 V.
    let v_up = v_out[upward];
    let v_dn = v_out[downward];
    assert!(
        v_up < 1.0,
        "expected OFF (v(out) ≈ 0) on upward leg at v_ctl={}, got {v_up}",
        v_ctl[upward]
    );
    assert!(
        v_dn > 4.0,
        "expected latched ON (v(out) ≈ 5) on downward leg at v_ctl={}, got {v_dn}",
        v_ctl[downward]
    );
}

#[test]
fn s_element_transient_pulse_toggles_state() {
    // A control pulse that crosses Vt drives the switch ON, and then OFF
    // again. Check the output voltage tracks the state transitions.
    let netlist = Netlist::parse_single(
        "S element transient pulse
V1 1 0 5
Vctl ctl 0 PULSE(0 3 0 1n 1n 5u 20u)
S1 1 out ctl 0 SWMOD
RL out 0 1k
.model SWMOD SW (Vt=1 Vh=0.2 Ron=0.5 Roff=1Meg)
.tran 0.5u 10u
.end
",
    )
    .unwrap();

    let result = simulate_tran(&netlist);
    let plot = &result.plots[0];
    let times = &plot
        .vecs
        .iter()
        .find(|v| v.name == "time")
        .unwrap()
        .data
        .as_real();
    let v_out = plot
        .vecs
        .iter()
        .find(|v| v.name.eq_ignore_ascii_case("v(out)"))
        .unwrap()
        .data
        .as_real();

    // Sample at t = 3 µs (pulse high, switch ON) and at the very first
    // step (t ≈ 0, pulse still 0, switch OFF). The output must differ by
    // many orders of magnitude.
    let i_on = times
        .iter()
        .position(|&t| t >= 3.0e-6)
        .expect("transient should reach t=3us");
    let i_off = 0usize;
    let v_on = v_out[i_on];
    let v_off = v_out[i_off];
    assert!(
        v_on > 4.5,
        "expected v(out) > 4.5V when switch is ON, got {v_on}"
    );
    assert!(
        v_off.abs() < 0.1,
        "expected v(out) ≈ 0V when switch is OFF, got {v_off}"
    );
}
