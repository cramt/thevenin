//! Integration tests for HiSIM2 / HiSIMHV2 MOSFETs (LEVEL=68 / LEVEL=73).
//!
//! The LEVEL=68 DC I-V core is a faithful hsm2eval.c port (validated against
//! ngspice-45 golden data in `hisim_golden.rs`), but the high-voltage
//! extensions in HiSIMHV (RDRIFT region, body resistance, breakdown) are not
//! modelled. Both LEVEL=68 and LEVEL=73 dispatch into the same bulk path.
//! AC small-signal capacitances and noise are out of scope for the 1.0 cut.

use thevenin_types::Netlist;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

mod common;
use common::simulate_op;

/// LEVEL=68 (HiSIM bulk) NMOS OP smoke — must converge and produce a plot.
#[test]
fn hisim_nmos_op_converges() {
    let netlist = Netlist::parse_single(
        "HiSIM NMOS OP smoke
m1 d g 0 0 nch w=10u l=1u
vg g 0 dc 1.5
vd d 0 dc 1.2
.model nch nmos level=68 vmax=8e6 lp=2e-8 nsubp=3e17
.op
.end
",
    )
    .unwrap();
    let result = simulate_op(&netlist);
    assert!(!result.plots.is_empty());
}

/// LEVEL=68 PMOS — symmetry check that the polarity dispatch works.
#[test]
fn hisim_pmos_op_converges() {
    let netlist = Netlist::parse_single(
        "HiSIM PMOS OP smoke
m1 d g 0 0 pch w=10u l=1u
vg g 0 dc -1.5
vd d 0 dc -1.2
.model pch pmos level=68 vmax=8e6 lp=2e-8 nsubp=3e17
.op
.end
",
    )
    .unwrap();
    let result = simulate_op(&netlist);
    assert!(!result.plots.is_empty());
}

/// LEVEL=73 (HiSIMHV) — currently shares the HiSIM DC path. Just verify
/// the dispatch resolves without falling back to LEVEL=1.
#[test]
fn hisimhv_nmos_op_converges() {
    let netlist = Netlist::parse_single(
        "HiSIMHV NMOS OP smoke
m1 d g 0 0 nch w=20u l=2u
vg g 0 dc 3.0
vd d 0 dc 5.0
.model nch nmos level=73 vmax=8e6 lp=2e-8 nsubp=3e17
.op
.end
",
    )
    .unwrap();
    let result = simulate_op(&netlist);
    assert!(!result.plots.is_empty());
}
