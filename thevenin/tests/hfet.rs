use approx::assert_abs_diff_eq;
use thevenin::{simulate_dc, simulate_op};
use thevenin_types::Netlist;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

/// Port of ngspice-upstream/tests/hfet/id_vgs.cir
///
/// HFET Id versus Vgs characteristic.
/// DC sweep VDS from 0 to 1V, NHFET level=5.
#[test]
fn test_hfet_id_vgs() {
    let cir = include_str!("fixtures/hfet/id_vgs.cir");
    let netlist = Netlist::parse_single(cir).unwrap();
    let result = simulate_dc(&netlist).unwrap();

    let plot = &result.plots[0];
    let ids = plot
        .vecs
        .iter()
        .find(|v| v.name == "vds#branch")
        .expect("no vds#branch");

    // 101 sweep points (0 to 1.0 in steps of 0.01)
    assert_eq!(ids.data.as_real().len(), 101, "expected 101 sweep points");

    // At vds=0 (index 0): current ≈ 0 (3.77e-11)
    assert!(
        ids.data.as_real()[0].abs() < 1e-6,
        "zero-bias current should be near zero"
    );

    // At vds=0.1 (index 10): ~-1.075e-4
    assert_abs_diff_eq!(ids.data.as_real()[10], -1.075027e-4, epsilon = 2e-5);

    // At vds=0.5 (index 50): ~-1.687e-4
    assert_abs_diff_eq!(ids.data.as_real()[50], -1.687463e-4, epsilon = 2e-5);

    // At vds=1.0 (index 100): ~-2.108e-4
    assert_abs_diff_eq!(ids.data.as_real()[100], -2.108001e-4, epsilon = 2e-5);
}

/// Port of ngspice-upstream/tests/hfet/inverter.cir
///
/// DCFL inverter with NHFET subcircuits and transient analysis.
/// This test is ignored until transient analysis for HFET is verified.
#[test]
fn test_hfet_inverter() {
    let cir = include_str!("fixtures/hfet/inverter.cir");
    let _netlist = Netlist::parse_single(cir).unwrap();
    // TODO: implement transient test once HFET transient support is verified
}

/// DCFL inverter chain DC operating point.
///
/// Two-stage DCFL inverter with depletion loads (VT0=-0.3) and enhancement
/// drivers (VT0=0.3).  With VIN=0V the first driver is OFF (VGS=0 < VT0=0.3)
/// and the depletion load pulls V(3) to ≈VDD.  The second inverter sees
/// VIN=V(3)≈2V, so its driver is ON hard and V(4) is pulled low.
///
/// NOTE: ngspice produces V(3)≈-0.275V for this circuit due to a bug in
/// hfetload.c where the `inverse` flag (line 83) is declared outside the
/// device iteration loop and never reset between instances.  When a driver
/// with vds<0 sets inverse=TRUE, subsequent load devices get their cdrain
/// wrongly negated.  Our code handles inverse correctly per-device and
/// produces the physically correct result.
#[test]
fn test_hfet_inverter_dc_op() {
    let cir = "\
DCFL inverter - OP only
.subckt inv 1 2 3
z1 1 3 3 aload l=1u w=10u
z2 3 2 0 adrv l=1u w=10u
.ends
vdd 1 0 dc 2
vin 2 0 dc 0
x1 1 2 3 inv
x2 1 3 4 inv
.model adrv nhfet level=5 rd=60 rs=60 m=2.57 lambda=0.17
+ vs=1.5e5 mu=0.385 vt0=0.3 eta=1.32 sigma0=0.04
+ vsigma=0.1 vsigmat=0.3 js1s=1e-12 js1d=1e-12
+ nmax=6e15
.model aload nhfet level=5 rd=60 rs=60 m=2.57 lambda=0.17
+ vs=1.5e5 mu=0.385 vt0=-0.3 eta=1.32 sigma0=0.04
+ vsigma=0.1 vsigmat=0.3 js1s=1e-12 js1d=1e-12
+ nmax=6e15
.end
";
    let netlist = Netlist::parse_single(cir).unwrap();
    let result = simulate_op(&netlist).unwrap();
    let plot = &result.plots[0];

    // V(3): first inverter output, driver OFF → load pulls to ≈VDD
    let v3 = plot
        .vecs
        .iter()
        .find(|v| v.name == "v(3)")
        .expect("no v(3)");
    assert_abs_diff_eq!(v3.data.as_real()[0], 1.9557, epsilon = 0.01);

    // V(4): second inverter output, driver ON hard → pulled low
    let v4 = plot
        .vecs
        .iter()
        .find(|v| v.name == "v(4)")
        .expect("no v(4)");
    assert_abs_diff_eq!(v4.data.as_real()[0], 0.206, epsilon = 0.01);
}
