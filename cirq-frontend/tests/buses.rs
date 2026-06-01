//! Port-array / bus tests.
//!
//! A bus is sugar for N scalar nets named `base.0 … base.N-1`. Buses elaborate
//! at compile time: bus-line references `d[i]` become net `d.i`, and a
//! whole-bus port binding (`d -> caller_bus`) carries every line through.

use cirq_ir::Circuit;

fn net_names(ir: &Circuit) -> Vec<String> {
    ir.nets.iter().map(|n| n.name.clone()).collect()
}

fn has_net(ir: &Circuit, name: &str) -> bool {
    ir.nets.iter().any(|n| n.name == name)
}

/// A bus-line reference `d[i]` in a connection resolves to net `d.i`.
#[test]
fn bus_line_reference_creates_dotted_net() {
    let src = r#"
        circuit bus_lines {
            V0: vsource(d[0] -> gnd, dc: 1)
            V1: vsource(d[1] -> gnd, dc: 2)
            R0: resistor(d[0] -> q[0], 1k)
            R1: resistor(d[1] -> q[1], 1k)
            analysis op {}
        }
    "#;
    let ir = cirq_frontend::compile(src).expect("compiles");
    for n in ["d.0", "d.1", "q.0", "q.1"] {
        assert!(
            has_net(&ir, n),
            "expected net `{n}` in {:?}",
            net_names(&ir)
        );
    }
}

/// A fixed-width bus port carries each line through a whole-bus binding: the
/// module's internal `d[i]` references map onto the caller's `bus.i` nets.
#[test]
fn fixed_width_bus_port_passes_through() {
    let src = r#"
        module tap {
            port d[2]: in
            port q[2]: out
            R0: resistor(d[0] -> q[0], 1k)
            R1: resistor(d[1] -> q[1], 1k)
        }
        circuit top {
            V0: vsource(in[0] -> gnd, dc: 1)
            V1: vsource(in[1] -> gnd, dc: 2)
            T1: tap(d: in, q: out)
            Rl0: resistor(out[0] -> gnd, 1k)
            Rl1: resistor(out[1] -> gnd, 1k)
            analysis op {}
        }
    "#;
    let ir = cirq_frontend::compile(src).expect("compiles");
    // The module's d[i]/q[i] must have mapped onto the caller's in.i/out.i —
    // so no `T1.d.*` internal nets should survive, but `in.0/in.1/out.0/out.1`
    // must exist.
    for n in ["in.0", "in.1", "out.0", "out.1"] {
        assert!(
            has_net(&ir, n),
            "expected caller net `{n}` in {:?}",
            net_names(&ir)
        );
    }
    assert!(
        !ir.nets.iter().any(|n| n.name.starts_with("T1.d")),
        "bus port `d` should remap to the caller, not create T1.d.* nets: {:?}",
        net_names(&ir)
    );
}

/// A bus whose width is a module parameter (const-generics style): the same
/// module elaborates at the width passed at the call site. The body references
/// explicit lines, but the width override is accepted and the bus binding
/// carries the referenced lines through.
#[test]
fn param_width_bus_accepts_override() {
    let src = r#"
        module probe {
            port d[width]: in
            param width = 2
            R0: resistor(d[0] -> gnd, 1k)
            R1: resistor(d[1] -> gnd, 1k)
        }
        circuit top {
            V0: vsource(sig[0] -> gnd, dc: 1)
            V1: vsource(sig[1] -> gnd, dc: 1)
            P1: probe(d: sig, width: 4)
            analysis op {}
        }
    "#;
    let ir = cirq_frontend::compile(src).expect("compiles");
    assert!(has_net(&ir, "sig.0") && has_net(&ir, "sig.1"));
}

/// End-to-end: two independent RC channels expressed as buses simulate to the
/// expected per-channel DC operating point (a resistive divider per line).
#[test]
fn bus_channels_simulate_independently() {
    let src = r#"
        circuit channels {
            Va: vsource(in[0] -> gnd, dc: 3)
            Vb: vsource(in[1] -> gnd, dc: 9)
            Rt0: resistor(in[0] -> out[0], 1k)
            Rt1: resistor(in[1] -> out[1], 1k)
            Rb0: resistor(out[0] -> gnd, 1k)
            Rb1: resistor(out[1] -> gnd, 1k)
            analysis op {}
        }
    "#;
    let ir = cirq_frontend::compile(src).expect("compiles");
    let result = thevenin::circuit::simulate(&ir).expect("op solves");
    let plot = &result.plots[0];
    let read = |name: &str| -> f64 {
        let v = plot
            .vecs
            .iter()
            .find(|v| v.name.eq_ignore_ascii_case(name))
            .unwrap_or_else(|| panic!("missing {name}"));
        match &v.data {
            thevenin_types::VectorData::Real(r) => *r.last().unwrap(),
            _ => panic!("expected real"),
        }
    };
    // Each channel is a 1k:1k divider → out = in / 2.
    assert!((read("v(out.0)") - 1.5).abs() < 1e-6, "channel 0");
    assert!((read("v(out.1)") - 4.5).abs() < 1e-6, "channel 1");
}

/// A non-integer bus index is a hard error.
#[test]
fn fractional_bus_index_errors() {
    let src = r#"
        circuit bad {
            R0: resistor(d[1.5] -> gnd, 1k)
            analysis op {}
        }
    "#;
    let diags = cirq_frontend::compile(src).expect_err("fractional index must fail");
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("non-negative integer")),
        "expected bus-index error, got: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

/// Scalar (non-bus) ports and connections still behave exactly as before —
/// regression guard for the NetRef change.
#[test]
fn scalar_ports_unaffected() {
    let src = r#"
        module inv {
            port a: in
            port z: out
            R1: resistor(a -> z, 1k)
        }
        circuit top {
            V1: vsource(in -> gnd, dc: 1)
            X1: inv(a: in, z: out)
            Rl: resistor(out -> gnd, 1k)
            analysis op {}
        }
    "#;
    let ir = cirq_frontend::compile(src).expect("compiles");
    assert!(has_net(&ir, "in") && has_net(&ir, "out"));
    let result = thevenin::circuit::simulate(&ir).expect("op solves");
    assert!(!result.plots.is_empty());
}
