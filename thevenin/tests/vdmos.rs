//! Integration tests for the VDMOS (vertical-DMOS power MOSFET) model.
//!
//! VDMOS is a distinct device class from the lateral MOSFET hierarchy: it
//! is selected by `.model NAME VDMOS (…)` (no LEVEL) and uses different
//! topology assumptions (built-in body diode, Vgd-dependent Miller cap, no
//! W/L scaling). These tests cover the OP, switching transient, and
//! importer round-trip paths.

use cirq_ir::Analysis;
use cirq_spice_import::import_netlist;
use thevenin_types::Netlist;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

mod common;
use common::{simulate_op, simulate_tran};

const SIMPLE_OP: &str = include_str!("fixtures/vdmos/simple_op.cir");
const SWITCHING_TRAN: &str = include_str!("fixtures/vdmos/switching_tran.cir");

/// Smoke test: a `.model NAME VDMOS (...)` plus an M-element parses, lowers
/// to IR, and reaches a DC operating point.
#[test]
fn vdmos_simple_op_converges() {
    let netlist = Netlist::parse_single(SIMPLE_OP).expect("parse");
    let result = simulate_op(&netlist);
    assert!(!result.plots.is_empty(), "expected at least one plot");

    let plot = &result.plots[0];
    let v_d = plot
        .vecs
        .iter()
        .find(|v| v.name == "v(d)")
        .expect("v(d) should exist");
    // Drain pinned to 10V by Vds source.
    assert!(
        (v_d.data.as_real()[0] - 10.0).abs() < 1e-6,
        "v(d) should be 10V, got {}",
        v_d.data.as_real()[0]
    );
}

/// VDMOS drain current at the OP should be in the expected ballpark.
///
/// With Vto=3, Kp=5, Vgs=5 → Vgst=2, Vds=10 (saturation regime):
///   Id ≈ Kp/2 * Vgst² = 2.5 * 4 = 10 A.
/// The ksubthres-based smoothing skews this slightly upward; allow ±50%.
#[test]
fn vdmos_op_drain_current_matches_hand_calc() {
    let netlist = Netlist::parse_single(SIMPLE_OP).expect("parse");
    let result = simulate_op(&netlist);
    let plot = &result.plots[0];

    // The drain-source voltage source vds carries the drain current as
    // -i(vds) in SPICE convention.
    let i_vds = plot
        .vecs
        .iter()
        .find(|v| v.name == "vds#branch")
        .expect("drain branch current should be exposed");
    let id = -i_vds.data.as_real()[0];
    assert!(
        (5.0..30.0).contains(&id),
        "VDMOS Id at Vgs=5,Vds=10 should be ~10A (allow 5-30A), got {id}"
    );
}

/// Switching transient: Vgs ramps from 0 → 10V over 1us. Drain current
/// should be small at t=0 (Vgs below Vto) and rise monotonically once Vgs
/// crosses threshold.
#[test]
fn vdmos_switching_transient_rises_through_threshold() {
    let netlist = Netlist::parse_single(SWITCHING_TRAN).expect("parse");
    let result = simulate_tran(&netlist);
    assert!(!result.plots.is_empty());

    let plot = result
        .plots
        .iter()
        .find(|p| p.name.starts_with("tran"))
        .expect("expected a tran plot");

    // Use v(d) as a proxy for drain current: v(d) = Vdd - Id*Rload, so as
    // Id rises through the load, v(d) drops from ~5V toward 0V.
    let id_vec = plot
        .vecs
        .iter()
        .find(|v| v.name == "v(d)")
        .expect("v(d) should be exposed");
    let id = id_vec.data.as_real();

    // v(d) starts near 5V (FET off) and falls toward 0V as drain current
    // increases. Compare an early sample (Vgs well below Vto) against a
    // late sample (Vgs >> Vto). Use 30% / 90% of the sweep to skip
    // transient OP and any settling at the end.
    let n = id.len();
    assert!(n > 10, "need enough timesteps to compare ramp");
    let v_early = id[n * 3 / 10];
    let v_late = id[n * 9 / 10];
    assert!(
        v_early > 4.0,
        "v(d) before Vgs reaches Vto should still be near Vdd=5V: got {v_early}",
    );
    assert!(
        v_late < v_early - 0.5,
        "v(d) should drop measurably once channel turns on: early={v_early}, late={v_late}",
    );
    assert!(
        v_late.is_finite() && (0.0..=5.5).contains(&v_late),
        "v(d) should remain bounded between 0 and Vdd: got {v_late}"
    );
}

/// Round-trip: VDMOS model + M-element parses, lowers to IR, and lowers
/// back to a netlist via the to_netlist path. The IR's DeviceType should
/// be `Vdmos` (NMOS-channel by default) and the element kind should be
/// `Nmos` (since VDMOS is dispatched at MNA-stamping time, not at the IR
/// element-kind level).
#[test]
fn vdmos_round_trip_through_ir() {
    let netlist = Netlist::parse_single(SIMPLE_OP).expect("parse");
    let circuit = {
        let mut resolved = netlist.clone();
        thevenin::expr::resolve_netlist_exprs(&mut resolved).expect("resolve");
        import_netlist(&resolved).expect("import_netlist")
    };

    // Model kind: should map to DeviceType::Vdmos.
    let nch = circuit
        .models
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case("NCH"))
        .expect("NCH model");
    assert_eq!(nch.device_type, cirq_ir::DeviceType::Vdmos);

    // Element kind: dispatched via mosfet_kind → Nmos (the IR keeps the
    // existing four-terminal MOSFET shape; the simulator's mna_ir layer
    // notices the model's DeviceType and routes to the VDMOS stamper).
    let m1 = circuit
        .elements
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case("M1"))
        .expect("M1 element");
    assert!(matches!(m1.kind, cirq_ir::ElementKind::Nmos));

    // to_netlist round-trip preserves the VDMOS kind string.
    let netlists = cirq_frontend::to_netlist::circuit_to_netlists(&circuit).expect("to_netlist");
    assert!(!netlists.is_empty(), "should produce at least one netlist");
    let model_item = netlists
        .iter()
        .flat_map(|nl| nl.items.iter())
        .find_map(|i| {
            if let thevenin_types::Item::Model(m) = i {
                if m.name.eq_ignore_ascii_case("NCH") {
                    Some(m)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .expect("NCH model should round-trip");
    assert!(
        model_item.kind.eq_ignore_ascii_case("VDMOS"),
        "round-tripped kind should be VDMOS, got {}",
        model_item.kind
    );
}

/// PVDMOS (P-channel power MOSFET) round-trips and produces a Pmos
/// element-kind.
#[test]
fn pvdmos_round_trips_with_pmos_element_kind() {
    let spice = "Title\n\
        m1 d g s s PCH\n\
        vds d 0 -10\n\
        vgs g 0 -5\n\
        vs s 0 0\n\
        .model PCH VDMOSP Vto=-3 Kp=5 lambda=0 theta=0 ksubthres=0.1\n\
        + Cgdmin=1p Cgdmax=10p Cgs=10p Rg=1 rds=1e9\n\
        .op\n\
        .end\n";
    let netlist = Netlist::parse_single(spice).expect("parse");
    let mut resolved = netlist.clone();
    thevenin::expr::resolve_netlist_exprs(&mut resolved).expect("resolve");
    let circuit = import_netlist(&resolved).expect("import_netlist");

    let pch = circuit
        .models
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case("PCH"))
        .expect("PCH model");
    assert_eq!(pch.device_type, cirq_ir::DeviceType::Pvdmos);

    let m1 = circuit
        .elements
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case("M1"))
        .expect("M1 element");
    assert!(matches!(m1.kind, cirq_ir::ElementKind::Pmos));
}

/// Lift to IR explicitly with the OP analysis and confirm convergence.
/// Mirrors the structure used by other model-specific integration tests.
#[test]
fn vdmos_circuit_simulate_op_via_ir_path() {
    let netlist = Netlist::parse_single(SIMPLE_OP).expect("parse");
    let mut resolved = netlist.clone();
    thevenin::expr::resolve_netlist_exprs(&mut resolved).expect("resolve");
    let mut circuit = import_netlist(&resolved).expect("import_netlist");
    circuit.analyses = vec![Analysis::Op];
    let result = thevenin::circuit::simulate_op(&circuit).expect("simulate_op");
    assert!(!result.plots.is_empty());
}
