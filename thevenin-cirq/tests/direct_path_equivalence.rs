//! Equivalence tests: `thevenin::circuit::simulate_op` (which auto-picks
//! the direct IR → MNA path for linear circuits via
//! [`thevenin::mna_ir::assemble_mna_from_circuit`]) must produce results
//! identical to going through `circuit_to_netlists` + the Netlist-shaped
//! simulator.
//!
//! Every test case here is a SPICE source whose imported `Circuit` contains
//! only elements supported by the direct path (R, V, I, C, L, E, G, H, F).
//! When the direct path doesn't apply, `simulate_op` falls back to the
//! lowered path — the test passes trivially but doesn't validate direct
//! stamping. Use this file to grow the direct-path coverage as more device
//! kinds gain direct stamping (per
//! `docs/migration/mna-ir-pivot-plan.md`).

use cirq_frontend::to_netlist::circuit_to_netlists;
use thevenin_types::VectorData;

/// Run a SPICE source through both paths and assert every output vector
/// matches bit-for-bit.
fn assert_paths_equal(spice: &str) {
    let circuits = cirq_spice_import::import_spice(spice)
        .unwrap_or_else(|e| panic!("import_spice failed: {e}\nsource:\n{spice}"));
    let circuit = &circuits[0];

    let via_direct = thevenin_cirq::simulate_op(circuit)
        .unwrap_or_else(|e| panic!("direct simulate_op failed: {e}\nsource:\n{spice}"));

    let nl = circuit_to_netlists(circuit)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let nl = thevenin::flatten_netlist(&nl).unwrap();
    let via_netlist = thevenin::simulate_op(&nl)
        .unwrap_or_else(|e| panic!("netlist simulate_op failed: {e}\nsource:\n{spice}"));

    assert_eq!(
        via_direct.plots.len(),
        via_netlist.plots.len(),
        "plot count mismatch for source:\n{spice}"
    );
    let direct_plot = &via_direct.plots[0];
    let netlist_plot = &via_netlist.plots[0];
    assert_eq!(
        direct_plot.vecs.len(),
        netlist_plot.vecs.len(),
        "vec count mismatch for source:\n{spice}\ndirect: {:?}\nlowered: {:?}",
        direct_plot.vecs.iter().map(|v| &v.name).collect::<Vec<_>>(),
        netlist_plot.vecs.iter().map(|v| &v.name).collect::<Vec<_>>(),
    );

    // Compare by name, not position — node ordering in the direct path
    // follows element-traversal order; the Netlist path may reorder via
    // node_map iteration. Both orderings are correct so long as every
    // expected vector exists and has the right value.
    for direct_vec in &direct_plot.vecs {
        let netlist_vec = netlist_plot
            .vecs
            .iter()
            .find(|v| v.name == direct_vec.name)
            .unwrap_or_else(|| {
                panic!(
                    "direct has vec '{}' but netlist path does not; source:\n{spice}",
                    direct_vec.name
                )
            });
        let dv = match &direct_vec.data {
            VectorData::Real(r) => r[0],
            _ => panic!("expected real data"),
        };
        let nv = match &netlist_vec.data {
            VectorData::Real(r) => r[0],
            _ => panic!("expected real data"),
        };
        assert_eq!(
            dv, nv,
            "drift in {}: direct={dv} netlist={nv}\nsource:\n{spice}",
            direct_vec.name,
        );
    }
}

#[test]
fn voltage_divider() {
    assert_paths_equal(
        "Voltage Divider\n\
         V1 in 0 1.0\n\
         R1 in mid 1k\n\
         R2 mid 0 2k\n\
         .op\n\
         .end\n",
    );
}

#[test]
fn current_source_into_resistor() {
    assert_paths_equal(
        "Current Source\n\
         I1 0 out 1m\n\
         R1 out 0 1k\n\
         .op\n\
         .end\n",
    );
}

#[test]
fn rc_op_capacitor_open() {
    // Cap should be DC-open: V(out) = 1V (since R doesn't carry current).
    assert_paths_equal(
        "RC OP\n\
         V1 in 0 1\n\
         R1 in out 1k\n\
         C1 out 0 1u\n\
         .op\n\
         .end\n",
    );
}

#[test]
fn rl_op_inductor_short() {
    // Inductor should be DC-short.
    assert_paths_equal(
        "RL OP\n\
         V1 in 0 1\n\
         L1 in mid 1m\n\
         R1 mid 0 1k\n\
         .op\n\
         .end\n",
    );
}

#[test]
fn vcvs_unity_buffer() {
    assert_paths_equal(
        "VCVS Buffer\n\
         V1 in 0 0.7\n\
         R1 in 0 1k\n\
         E1 out 0 in 0 1.0\n\
         R2 out 0 10k\n\
         .op\n\
         .end\n",
    );
}

#[test]
fn vccs_transconductor() {
    assert_paths_equal(
        "VCCS Transconductor\n\
         V1 in 0 0.5\n\
         R1 in 0 1k\n\
         G1 0 out in 0 1m\n\
         R2 out 0 1k\n\
         .op\n\
         .end\n",
    );
}

#[test]
fn ccvs_current_to_voltage() {
    assert_paths_equal(
        "CCVS\n\
         V1 in 0 1\n\
         R1 in mid 1k\n\
         Vsense mid 0 0\n\
         H1 out 0 Vsense 1k\n\
         R2 out 0 10k\n\
         .op\n\
         .end\n",
    );
}

#[test]
fn cccs_current_mirror() {
    assert_paths_equal(
        "CCCS\n\
         V1 in 0 1\n\
         R1 in mid 1k\n\
         Vsense mid 0 0\n\
         F1 0 out Vsense 1.0\n\
         R2 out 0 1k\n\
         .op\n\
         .end\n",
    );
}

#[test]
fn parallel_resistors() {
    // Two resistors in parallel: R_eff = (1k * 2k) / (1k + 2k) = 666.67 ohm
    // I_total = 1V / R_eff; V(mid) = 1V (= V1)
    assert_paths_equal(
        "Parallel R\n\
         V1 in 0 1\n\
         R1 in mid 1k\n\
         R2 in mid 2k\n\
         R3 mid 0 1k\n\
         .op\n\
         .end\n",
    );
}

#[test]
fn diode_voltage_drop() {
    // Single diode forward-biased through a 1k series resistor. The direct
    // IR path produces a MnaSystem with one DiodeInstance; downstream
    // NR converges via solve_op_raw_with_opts -> solve_nonlinear_op.
    assert_paths_equal(
        "Diode OP\n\
         .model dmod d is=1e-14\n\
         V1 in 0 1.0\n\
         R1 in mid 1k\n\
         D1 mid 0 dmod\n\
         .op\n\
         .end\n",
    );
}

#[test]
fn diode_with_series_resistance() {
    // Diode model with RS > 0 forces allocation of an internal node — the
    // direct path's internal-node counter and stamping must match the
    // Netlist path's bookkeeping.
    assert_paths_equal(
        "Diode RS OP\n\
         .model dmod d is=1e-14 rs=10\n\
         V1 in 0 0.7\n\
         R1 in mid 100\n\
         D1 mid 0 dmod\n\
         .op\n\
         .end\n",
    );
}

#[test]
fn diode_clamp_pair() {
    // Two diodes back-to-back clamp the centre node. Exercises multiple
    // DiodeInstance entries in mna.diodes.
    assert_paths_equal(
        "Diode Clamp\n\
         .model dmod d is=1e-14\n\
         V1 in 0 0.3\n\
         R1 in mid 1k\n\
         D1 mid 0 dmod\n\
         D2 0 mid dmod\n\
         .op\n\
         .end\n",
    );
}

#[test]
fn ladder_network() {
    // L-shaped R network with several internal nodes — stresses node
    // indexing.
    assert_paths_equal(
        "Ladder\n\
         V1 in 0 5\n\
         R1 in n1 100\n\
         R2 n1 n2 200\n\
         R3 n2 n3 300\n\
         R4 n3 0 400\n\
         R5 n2 0 1k\n\
         R6 n1 0 2k\n\
         .op\n\
         .end\n",
    );
}
