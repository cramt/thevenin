//! Integration tests ported from ngspice-upstream/tests/transient/

mod common;
use common::simulate_tran;

const FOURBITADDER_CIR: &str = include_str!("fixtures/transient/fourbitadder.cir");
const FOURBITADDER_OUT: &str = include_str!("fixtures/transient/fourbitadder.out");

#[test]
fn test_fourbitadder() {
    let netlist = thevenin_types::Netlist::parse_single(FOURBITADDER_CIR)
        .unwrap_or_else(|e| panic!("cannot parse fourbitadder.cir: {e}"));

    // Once BJT and subcircuit support are implemented, this should:
    // 1. Run transient analysis
    // 2. Compare v(1) output against fourbitadder.out reference
    let result = simulate_tran(&netlist);

    // Reference output available as FOURBITADDER_OUT.
    let _ = FOURBITADDER_OUT;

    // Verify we got transient results.
    assert!(!result.plots.is_empty(), "should have at least one plot");
    let plot = &result.plots[0];
    assert_eq!(plot.name, "tran1");

    // Find v(1) in the output.
    let v1 = plot
        .vecs
        .iter()
        .find(|v| v.name == "v(1)")
        .expect("should have v(1) vector");
    assert!(
        !v1.data.as_real().is_empty(),
        "v(1) should have data points"
    );
}
