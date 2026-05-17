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
fn bjt_common_emitter_npn() {
    // Single NPN in common-emitter config — exercises the level-1
    // Gummel-Poon path with no series resistances (no internal nodes).
    assert_paths_equal(
        "BJT NPN CE\n\
         .model qmod npn is=1e-15 bf=100\n\
         VCC vcc 0 5\n\
         VB  base 0 0.7\n\
         RC  vcc collector 1k\n\
         RB  base bint 10k\n\
         Q1  collector bint 0 qmod\n\
         .op\n\
         .end\n",
    );
}

#[test]
fn bjt_with_series_resistances() {
    // NPN with RB / RC / RE > 0 forces 3 internal-node allocations and
    // exercises push_bjt_caps for CJE / CJC > 0.
    assert_paths_equal(
        "BJT NPN RB RC RE\n\
         .model qmod npn is=1e-15 bf=100 rb=10 rc=5 re=2 cje=1p cjc=2p\n\
         VCC vcc 0 5\n\
         VB  base 0 0.7\n\
         RC  vcc collector 1k\n\
         RB  base bint 10k\n\
         Q1  collector bint 0 qmod\n\
         .op\n\
         .end\n",
    );
}

#[test]
fn bjt_pnp_high_side() {
    // PNP element kind — exercises the typed Pnp -> "PNP" kind through
    // convert_model, plus the default-NPN fallback when no model is
    // linked (it's not exercised here because we always supply a model,
    // but the path is reachable).
    assert_paths_equal(
        "BJT PNP\n\
         .model pmod pnp is=1e-15 bf=80\n\
         VEE 0 vee 5\n\
         VB  base 0 -0.7\n\
         RE  0 emitter 1k\n\
         RB  base bint 10k\n\
         Q1  vee bint emitter pmod\n\
         .op\n\
         .end\n",
    );
}

#[test]
fn bjt_vbic_level4() {
    // VBIC (level=4) with the full set of conditional series resistances
    // (RCX/RBX/RE/RS) and a thermal node (RTH > 0) — exercises every
    // conditional-internal-node branch in the direct IR path. Mirrors the
    // model parameters in ngspice-upstream/tests/vbic/CEamp.cir but trims
    // to a .op-only circuit so we can assert exact equivalence against the
    // lowered path.
    assert_paths_equal(
        "VBIC OP\n\
         .model n1 npn level=4\n\
         + is=1e-16 ibei=1e-18 ibci=2e-17 isp=1e-15\n\
         + rcx=10 rci=60 rbx=10 rbi=40 re=2 rs=20 rbp=40\n\
         + vef=10 ver=4 ikf=2e-3 ikr=2e-4 ikp=2e-4\n\
         + cje=1e-13 cjc=2e-14 cjep=1e-13 cjcp=4e-13\n\
         + rth=300\n\
         vcc vcc 0 5\n\
         vbb base 0 0.75\n\
         rc vcc collector 1k\n\
         q1 collector base 0 0 n1\n\
         .op\n\
         .end\n",
    );
}

#[test]
fn mosfet_level1_nmos() {
    // Default-level MOS1 with simple model — exercises the MosfetModel
    // branch + push_mosfet_caps (CGSO/CGDO/CGBO=0 here, so no caps push).
    assert_paths_equal(
        "MOS1 NMOS\n\
         .model nm nmos vto=0.7 kp=100u\n\
         VDD vdd 0 5\n\
         VGS gate 0 2.0\n\
         RD vdd drain 10k\n\
         M1 drain gate 0 0 nm w=10u l=1u\n\
         .op\n\
         .end\n",
    );
}

#[test]
fn mosfet_level1_pmos() {
    // PMOS in common-source — exercises the typed Pmos -> \"PMOS\" kind
    // through convert_model.
    assert_paths_equal(
        "MOS1 PMOS\n\
         .model pm pmos vto=-0.7 kp=50u\n\
         VDD vdd 0 5\n\
         VGS gate vdd -2.0\n\
         RD drain 0 10k\n\
         M1 drain gate vdd vdd pm w=10u l=1u\n\
         .op\n\
         .end\n",
    );
}

#[test]
fn mosfet_level2_with_series_resistances() {
    // MOS Level 2 with RD/RS > 0 — exercises the level=2 branch including
    // internal-node allocation and push_mosfet_caps with non-zero overlap.
    assert_paths_equal(
        "MOS2 NMOS\n\
         .model nm nmos level=2 vto=0.7 kp=100u rd=50 rs=30 cgso=1p cgdo=1p\n\
         VDD vdd 0 5\n\
         VGS gate 0 2.0\n\
         RD vdd drain 10k\n\
         M1 drain gate 0 0 nm w=10u l=1u\n\
         .op\n\
         .end\n",
    );
}

#[test]
fn mosfet_bsim3_level8() {
    // BSIM3 (level=8) — uses the full nmos parameter set from
    // ngspice-upstream/tests/bsim3/nmos/parameters/nmosParameters so the
    // NR solve actually converges. Exercises Bsim3Model + Bsim3Instance
    // with size_dep_param and the rsh*nrd internal-node gating.
    let cir = "\
BSIM3 OP
.model nmod nmos level=8 version=3.3
+ binunit=1 paramchk=1 mobmod=1 capmod=3 acnqsmod=0 noimod=1 tnom=27
+ nch=1.7e+17 tox=1.5e-08 toxm=1.5e-08
+ wint=0.0 lint=0.0 ll=0.0 wl=0.0 lln=1.0 wln=1.0
+ lw=0.0 ww=0.0 lwn=1.0 wwn=1.0 lwl=0.0 wwl=0.0
+ xpart=0.0 xl=-30e-09
+ vth0=0.7 k1=0.5 k2=0.0 k3=80 k3b=0.0 w0=2.5e-06
+ dvt0=2.2 dvt1=0.53 dvt2=-0.032 nlx=1.74e-07
+ dvt0w=0.0 dvt1w=5.3e6 dvt2w=-0.032 dsub=0.56
+ xj=1.5e-07 ngate=0.0
+ cdsc=2.4e-04 cdscb=0.0 cdscd=0.0 cit=0.0
+ voff=-0.08 nfactor=1.0 eta0=0.08 etab=-0.07
+ vfb=-0.55 u0=670 ua=2.25e-09 ub=5.87e-19 uc=-4.65e-11
+ vsat=8e+04 a0=1.0 ags=0.0 a1=0.0 a2=1.0
+ b0=0.0 b1=0.0 keta=-0.047 dwg=0.0 dwb=0.0
+ pclm=1.3 pdiblc1=0.39 pdiblc2=0.0086 pdiblcb=0.0
+ drout=0.56 pvag=0.0 delta=0.01
+ pscbe1=4.24e+8 pscbe2=1e-05
+ rsh=10.0 rdsw=100.0 prwg=0.0 prwb=0.0 wr=1.0
+ alpha0=0.0 alpha1=0.0 beta0=30
+ cgbo=0.0 cgdl=2e-10 cgsl=2e-10 ckappa=0.6
+ acde=1.0 moin=15 noff=0.9 voffcv=0.02
+ kt1=-0.11 kt1l=0.0 kt2=-0.022 ute=-1.48
+ ua1=4.31e-09 ub1=-7.61e-18 uc1=-5.6e-11 prt=0.0 at=3.3e+04
+ ijth=0.1 js=0.0001 jsw=0.0
+ pb=1.0 cj=0.0005 mj=0.5
+ pbsw=1.0 cjsw=5e-10 mjsw=0.33
+ pbswg=1.0 cjswg=5e-10 mjswg=0.33
+ tpb=0.005 tcj=0.001 tpbsw=0.005 tcjsw=0.001
+ tpbswg=0.005 tcjswg=0.001 xti=3
M1 d g 0 0 nmod W=10e-6 L=1e-6
Vgs g 0 1.8
Vds d 0 0.5
.op
.end
";
    assert_paths_equal(cir);
}

#[test]
fn jfet_njf_op() {
    // NJF in common-source — exercises JfetModel + JfetInstance with
    // RD > 0 internal-node allocation.
    assert_paths_equal(
        "JFET NJF\n\
         .model jmod njf vto=-2 beta=1m rd=10 rs=5\n\
         VDD vdd 0 5\n\
         VGS gate 0 -0.5\n\
         RD vdd drain 10k\n\
         J1 drain gate 0 jmod\n\
         .op\n\
         .end\n",
    );
}

#[test]
fn mesfet_nmf_op() {
    // NMF MESFET — model kind \"NMF\" with level=1 routes to MesfetModel
    // (not generic MESA).
    assert_paths_equal(
        "MESFET NMF\n\
         .model zmod nmf level=1 vto=-2 beta=1m rd=10 rs=5\n\
         VDD vdd 0 3\n\
         VGS gate 0 -0.5\n\
         RD vdd drain 10k\n\
         Z1 drain gate 0 zmod\n\
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
