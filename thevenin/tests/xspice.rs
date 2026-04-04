//! Integration tests for XSPICE code model framework.

use std::sync::Arc;

use thevenin_types::Netlist;
use thevenin_xspice::*;

/// Register a simple VCCS gain code model:
/// Reads V(in), outputs I(out) = gain * V(in)
fn gain_vccs_registry(gain: f64) -> Arc<CodeModelRegistry> {
    let mut registry = CodeModelRegistry::new();
    registry.register(
        CodeModelBuilder::new("d_gain")
            .port("in", PortDirection::In, PortType::Voltage)
            .port("out", PortDirection::Out, PortType::Current)
            .param_real("gain", 1.0)
            .build(move |inputs, _: &mut ()| {
                let v_in = inputs.port_values[0];
                let g = inputs.params[0].as_real().unwrap_or(gain);
                let mut out = CmOutputs::new();
                out.set_output(1, g * v_in);
                out.set_partial(1, 0, g);
                out
            }),
    );
    Arc::new(registry)
}

/// Test DC operating point with a VCCS gain code model.
///
/// Circuit:
///   V1 = 1V source
///   A1 = gain=0.01 VCCS (reads V(in), injects current at node out)
///   R1 = 100Ω load from out to ground
///
/// Expected: V(out) = gain * V(in) * R_load = 0.01 * 1.0 * 100 = 1.0V
#[test]
fn test_xspice_gain_vccs_dc_op() {
    let cir = "\
XSPICE Gain Test
V1 in 0 1.0
R1 out 0 100
A1 in out amp1
.model amp1 d_gain(gain=0.01)
.op
.end
";
    let netlist = Netlist::parse(cir).unwrap();
    let registry = gain_vccs_registry(0.01);
    let result = thevenin::simulate_op_with_xspice(&netlist, registry).unwrap();

    let plot = &result.plots[0];
    let v_out = plot
        .vecs
        .iter()
        .find(|v| v.name == "v(out)")
        .expect("v(out) not found");
    let v_in = plot
        .vecs
        .iter()
        .find(|v| v.name == "v(in)")
        .expect("v(in) not found");

    // V(in) should be 1.0V (set by voltage source)
    assert!(
        (v_in.data.as_real()[0] - 1.0).abs() < 1e-6,
        "V(in) = {} (expected 1.0)",
        v_in.data.as_real()[0]
    );

    // V(out) = gain * V(in) * R_load = 0.01 * 1.0 * 100 = 1.0V
    assert!(
        (v_out.data.as_real()[0] - 1.0).abs() < 1e-6,
        "V(out) = {} (expected 1.0)",
        v_out.data.as_real()[0]
    );
}

/// Register a conductance-port code model (nonlinear resistor):
/// G(v) = g0 + g1*v (voltage-dependent conductance)
fn nonlinear_conductance_registry() -> Arc<CodeModelRegistry> {
    let mut registry = CodeModelRegistry::new();
    registry.register(
        CodeModelBuilder::new("nl_cond")
            .port("port", PortDirection::InOut, PortType::Conductance)
            .param_real("g0", 0.01)
            .param_real("g1", 0.001)
            .build(|inputs, _: &mut ()| {
                let v = inputs.port_values[0];
                let g0 = inputs.params[0].as_real().unwrap_or(0.01);
                let g1 = inputs.params[1].as_real().unwrap_or(0.001);
                // I = (g0 + g1*v) * v = g0*v + g1*v^2
                // dI/dV = g0 + 2*g1*v
                let i_total = (g0 + g1 * v) * v;
                let g_eq = g0 + 2.0 * g1 * v;
                let mut out = CmOutputs::new();
                out.set_output(0, i_total);
                out.set_partial(0, 0, g_eq);
                out
            }),
    );
    Arc::new(registry)
}

/// Test nonlinear conductance code model in DC OP.
///
/// Circuit:
///   V1 = 5V, R_series = 100Ω, nonlinear conductance to ground
///   I = (g0 + g1*V) * V where g0=0.01, g1=0.001
///   KCL at node "mid": (5 - V)/100 = (0.01 + 0.001*V) * V
///   Solving: 0.05 - V/100 = 0.01*V + 0.001*V^2
///   0.001*V^2 + 0.02*V - 0.05 = 0
///   V = (-0.02 + sqrt(0.0004 + 0.0002)) / 0.002
///   V = (-0.02 + sqrt(0.0006)) / 0.002
///   V ≈ (-0.02 + 0.024495) / 0.002 ≈ 2.247
#[test]
fn test_xspice_nonlinear_conductance_dc_op() {
    let cir = "\
Nonlinear Conductance Test
V1 in 0 5.0
R1 in mid 100
A1 [mid 0] nlc1
.model nlc1 nl_cond(g0=0.01 g1=0.001)
.op
.end
";
    let netlist = Netlist::parse(cir).unwrap();
    let registry = nonlinear_conductance_registry();
    let result = thevenin::simulate_op_with_xspice(&netlist, registry).unwrap();

    let plot = &result.plots[0];
    let v_mid = plot
        .vecs
        .iter()
        .find(|v| v.name == "v(mid)")
        .expect("v(mid) not found");

    // Analytical: V = (-0.02 + sqrt(0.0006)) / 0.002
    let expected = (-0.02_f64 + (0.0006_f64).sqrt()) / 0.002;
    assert!(
        (v_mid.data.as_real()[0] - expected).abs() < 1e-4,
        "V(mid) = {} (expected {expected})",
        v_mid.data.as_real()[0]
    );
}

/// Test that existing circuits without XSPICE still work with the xspice API.
#[test]
fn test_xspice_backward_compat() {
    let cir = "\
Simple RC
V1 in 0 5.0
R1 in out 1k
R2 out 0 1k
.op
.end
";
    let netlist = Netlist::parse(cir).unwrap();
    let registry = Arc::new(CodeModelRegistry::new());
    let result = thevenin::simulate_op_with_xspice(&netlist, registry).unwrap();

    let plot = &result.plots[0];
    let v_out = plot
        .vecs
        .iter()
        .find(|v| v.name == "v(out)")
        .expect("v(out) not found");

    // Voltage divider: 5.0 * 1k / (1k + 1k) = 2.5V
    assert!(
        (v_out.data.as_real()[0] - 2.5).abs() < 1e-6,
        "V(out) = {} (expected 2.5)",
        v_out.data.as_real()[0]
    );
}
