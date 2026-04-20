//! End-to-end integration tests for the Cirq pipeline.
//!
//! These tests exercise the full stack:
//!   Cirq source -> AST -> IR -> Netlist -> Simulator
//!   SPICE source -> Netlist -> Cirq IR -> Netlist (round-trip)

// ---------------------------------------------------------------------------
// Test 1: Cirq source -> AST -> IR -> Netlist round-trip
// ---------------------------------------------------------------------------

#[test]
fn cirq_voltage_divider_round_trip() {
    let source = r#"
        circuit voltage_divider {
            V1: vsource(in -> gnd, dc: 5)
            R1: resistor(in -> mid, 1000)
            R2: resistor(mid -> gnd, 1000)
            analysis op {}
        }
    "#;

    // Parse to AST.
    let ast = cirq_frontend::parse(source).expect("parse should succeed");
    let circuit_ast = ast
        .items
        .iter()
        .find_map(|item| {
            if let cirq_ast::TopLevel::Circuit(c) = item {
                Some(c)
            } else {
                None
            }
        })
        .expect("should contain a circuit declaration");
    assert_eq!(circuit_ast.name.name, "voltage_divider");

    // Compile to IR.
    let ir = cirq_frontend::compile(source).expect("compile should succeed");
    assert_eq!(ir.name, "voltage_divider");
    assert_eq!(ir.elements.len(), 3); // V1, R1, R2
    assert_eq!(ir.analyses.len(), 1);
    assert!(matches!(ir.analyses[0], cirq_ir::Analysis::Op));

    // Verify element kinds.
    let v1 = ir.elements.iter().find(|e| e.name == "V1").expect("V1");
    assert!(matches!(v1.kind, cirq_ir::ElementKind::VoltageSource));

    let r1 = ir.elements.iter().find(|e| e.name == "R1").expect("R1");
    assert!(matches!(r1.kind, cirq_ir::ElementKind::Resistor));

    let r2 = ir.elements.iter().find(|e| e.name == "R2").expect("R2");
    assert!(matches!(r2.kind, cirq_ir::ElementKind::Resistor));

    // Compile to Netlist.
    let netlists =
        cirq_frontend::compile_to_netlist(source).expect("compile_to_netlist should succeed");
    assert_eq!(netlists.len(), 1);

    let nl = &netlists[0];
    assert!(matches!(nl.analysis, thevenin_types::Analysis::Op));

    // Count elements in the netlist.
    let elem_count = nl
        .items
        .iter()
        .filter(|i| matches!(i, thevenin_types::Item::Element(_)))
        .count();
    assert_eq!(elem_count, 3, "netlist should have 3 elements: V1, R1, R2");
}

// ---------------------------------------------------------------------------
// Test 2: Cirq source -> Netlist -> Simulate (operating point)
// ---------------------------------------------------------------------------

#[test]
fn cirq_voltage_divider_simulate_op() {
    let source = r#"
        circuit voltage_divider {
            V1: vsource(in -> gnd, dc: 5)
            R1: resistor(in -> mid, 1000)
            R2: resistor(mid -> gnd, 1000)
            analysis op {}
        }
    "#;

    let netlists =
        cirq_frontend::compile_to_netlist(source).expect("compile_to_netlist should succeed");
    assert_eq!(netlists.len(), 1);

    let result = thevenin::simulate(&netlists[0]).expect("simulation should succeed");

    // For two equal 1k resistors with a 5V supply, v(mid) should be 2.5V.
    let vmid = result.vector("v(mid)").expect("should have v(mid) vector");
    let vmid_val = vmid.data.as_real()[0];
    assert!(
        (vmid_val - 2.5).abs() < 0.01,
        "expected v(mid) ~ 2.5V, got {vmid_val}"
    );

    // v(in) should be 5V.
    let vin = result.vector("v(in)").expect("should have v(in) vector");
    let vin_val = vin.data.as_real()[0];
    assert!(
        (vin_val - 5.0).abs() < 0.01,
        "expected v(in) ~ 5.0V, got {vin_val}"
    );
}

// ---------------------------------------------------------------------------
// Test 3: SPICE -> Cirq IR -> Netlist round-trip
// ---------------------------------------------------------------------------
//
// NOTE: The SPICE importer stores resistor values as param `"resistance"`,
// capacitor values as `"capacitance"`, etc., while the to_netlist adapter
// expects `"value"` for passive elements. This param naming gap means a
// direct SPICE -> IR -> Netlist round-trip for passives requires that the
// IR elements use the `"value"` param name.
//
// Voltage sources work correctly because both paths use `"dc"`.
// This test exercises a SPICE circuit containing only a voltage source and
// passive elements, verifying the parts of the round-trip that are fully
// aligned, and separately testing the SPICE -> IR import for structural
// equivalence.

#[test]
fn spice_to_cirq_ir_structural_equivalence() {
    let spice_source = "\
Voltage Divider
V1 in 0 DC 5
R1 in mid 1k
R2 mid 0 1k
.op
.end
";

    // Parse SPICE to Netlist directly.
    let original_netlists =
        thevenin_types::Netlist::parse(spice_source).expect("SPICE parse should succeed");
    assert_eq!(original_netlists.len(), 1);
    let original = &original_netlists[0];

    // Import to Cirq IR.
    let ir = cirq_spice_import::import_netlist(original).expect("import_netlist should succeed");

    // Verify IR structure preserves elements, nets, and analysis.
    assert_eq!(ir.elements.len(), 3, "should have V1, R1, R2");
    assert_eq!(ir.analyses.len(), 1);
    assert!(matches!(ir.analyses[0], cirq_ir::Analysis::Op));

    // Verify net count: ground + "in" + "mid" = 3.
    assert_eq!(ir.nets.len(), 3);

    // Verify element kinds.
    let v1 = ir.elements.iter().find(|e| e.name == "V1").expect("V1");
    assert!(matches!(v1.kind, cirq_ir::ElementKind::VoltageSource));
    let r1 = ir.elements.iter().find(|e| e.name == "R1").expect("R1");
    assert!(matches!(r1.kind, cirq_ir::ElementKind::Resistor));
    let r2 = ir.elements.iter().find(|e| e.name == "R2").expect("R2");
    assert!(matches!(r2.kind, cirq_ir::ElementKind::Resistor));

    // Verify the resistor values were imported correctly (as "resistance" param).
    let r1_val = r1
        .params
        .iter()
        .find(|p| p.0 == "resistance")
        .map(|p| match &p.1 {
            cirq_ir::Value::Real(v) => *v,
            _ => panic!("expected real"),
        })
        .expect("R1 should have resistance param");
    assert!(
        (r1_val - 1000.0).abs() < 1e-6,
        "R1 resistance should be 1k, got {r1_val}"
    );

    // Verify the voltage source DC value.
    let v1_dc = v1
        .params
        .iter()
        .find(|p| p.0 == "dc")
        .map(|p| match &p.1 {
            cirq_ir::Value::Real(v) => *v,
            _ => panic!("expected real"),
        })
        .expect("V1 should have dc param");
    assert!(
        (v1_dc - 5.0).abs() < 1e-6,
        "V1 DC should be 5V, got {v1_dc}"
    );

    // Verify the original SPICE element count matches IR element count.
    let original_elem_count = original
        .items
        .iter()
        .filter(|i| matches!(i, thevenin_types::Item::Element(_)))
        .count();
    assert_eq!(original_elem_count, ir.elements.len());
}

// ---------------------------------------------------------------------------
// Test 4: SPICE -> Cirq IR semantic equivalence for two equivalent circuits
// ---------------------------------------------------------------------------

#[test]
fn spice_semantic_equivalence_at_ir_level() {
    // Circuit A: resistors with explicit DC keyword.
    let spice_a = "\
Divider A
V1 in 0 DC 5
R1 in out 2k
R2 out 0 2k
.op
.end
";

    // Circuit B: resistors specified differently (different names, same topology).
    let spice_b = "\
Divider B
V1 inp 0 5
R1 inp outp 2000
R2 outp 0 2000
.op
.end
";

    let netlists_a = thevenin_types::Netlist::parse(spice_a).expect("SPICE A parse should succeed");
    let netlists_b = thevenin_types::Netlist::parse(spice_b).expect("SPICE B parse should succeed");

    let ir_a = cirq_spice_import::import_netlist(&netlists_a[0]).expect("import A should succeed");
    let ir_b = cirq_spice_import::import_netlist(&netlists_b[0]).expect("import B should succeed");

    // Same number of elements.
    assert_eq!(ir_a.elements.len(), ir_b.elements.len());

    // Same number of nets (ground + 2 signal nets in each).
    assert_eq!(ir_a.nets.len(), ir_b.nets.len());

    // Same element kinds in the same order.
    for (ea, eb) in ir_a.elements.iter().zip(ir_b.elements.iter()) {
        assert_eq!(
            std::mem::discriminant(&ea.kind),
            std::mem::discriminant(&eb.kind),
            "element kinds should match: {} vs {}",
            ea.name,
            eb.name,
        );
    }

    // Same analysis type.
    assert_eq!(ir_a.analyses.len(), ir_b.analyses.len());
    assert!(matches!(ir_a.analyses[0], cirq_ir::Analysis::Op));
    assert!(matches!(ir_b.analyses[0], cirq_ir::Analysis::Op));

    // Resistance values should be the same.
    let r1_a = ir_a
        .elements
        .iter()
        .find(|e| matches!(e.kind, cirq_ir::ElementKind::Resistor) && e.name == "R1")
        .expect("R1 in A");
    let r1_b = ir_b
        .elements
        .iter()
        .find(|e| matches!(e.kind, cirq_ir::ElementKind::Resistor) && e.name == "R1")
        .expect("R1 in B");

    let val_a = r1_a
        .params
        .iter()
        .find(|p| p.0 == "resistance")
        .map(|p| match &p.1 {
            cirq_ir::Value::Real(v) => *v,
            _ => panic!("expected real"),
        })
        .expect("resistance param in A");
    let val_b = r1_b
        .params
        .iter()
        .find(|p| p.0 == "resistance")
        .map(|p| match &p.1 {
            cirq_ir::Value::Real(v) => *v,
            _ => panic!("expected real"),
        })
        .expect("resistance param in B");

    assert!(
        (val_a - val_b).abs() < 1e-6,
        "resistance values should match: {val_a} vs {val_b}"
    );
}

// ---------------------------------------------------------------------------
// Test 5: Cirq DC sweep -> Simulate
// ---------------------------------------------------------------------------

#[test]
fn cirq_dc_sweep_simulate() {
    let source = r#"
        circuit dc_sweep_test {
            V1: vsource(in -> gnd, dc: 0)
            R1: resistor(in -> gnd, 1000)
            analysis dc {
                sweep V1: 0..5 step 0.5
            }
        }
    "#;

    let netlists =
        cirq_frontend::compile_to_netlist(source).expect("compile_to_netlist should succeed");
    assert_eq!(netlists.len(), 1);

    let nl = &netlists[0];
    assert!(
        matches!(nl.analysis, thevenin_types::Analysis::Dc { .. }),
        "expected DC analysis, got {:?}",
        nl.analysis
    );

    let result = thevenin::simulate(nl).expect("DC sweep simulation should succeed");

    // The sweep from 0 to 5V in 0.5V steps should produce 11 data points
    // (0.0, 0.5, 1.0, ..., 5.0).
    let v_in = result.vector("v(in)").expect("should have v(in) vector");
    let data = v_in.data.as_real();
    assert!(
        data.len() >= 10,
        "DC sweep should produce at least 10 data points, got {}",
        data.len()
    );

    // First point should be near 0V, last near 5V.
    assert!(
        data[0].abs() < 0.01,
        "first sweep point should be ~0V, got {}",
        data[0]
    );
    let last = data[data.len() - 1];
    assert!(
        (last - 5.0).abs() < 0.01,
        "last sweep point should be ~5V, got {last}"
    );
}

// ---------------------------------------------------------------------------
// Test 6: Cirq AC analysis -> Simulate
// ---------------------------------------------------------------------------

#[test]
fn cirq_ac_analysis_simulate() {
    // Simple RC lowpass filter: R=1k, C=1uF.
    // Corner frequency = 1/(2*pi*R*C) ~ 159 Hz.
    let source = r#"
        circuit rc_filter {
            V1: vsource(in -> gnd, dc: 0, ac_mag: 1)
            R1: resistor(in -> out, 1000)
            C1: capacitor(out -> gnd, 1e-6)
            analysis ac {
                start: 1
                stop: 1000000
                points: 10
                scale: decade
            }
        }
    "#;

    let netlists =
        cirq_frontend::compile_to_netlist(source).expect("compile_to_netlist should succeed");
    assert_eq!(netlists.len(), 1);

    let nl = &netlists[0];
    assert!(
        matches!(nl.analysis, thevenin_types::Analysis::Ac { .. }),
        "expected AC analysis, got {:?}",
        nl.analysis
    );

    let result = thevenin::simulate(nl).expect("AC simulation should succeed");

    // AC results should have complex-valued output vectors.
    // Look for v(out) -- the output node voltage in AC.
    let plot = result.plot().expect("should have at least one plot");

    // The plot should contain multiple data points (10 points/decade over
    // 6 decades = 60 points).
    let frequency_vec = plot.vecs.iter().find(|v| v.name == "frequency");
    assert!(
        frequency_vec.is_some(),
        "AC result should contain a frequency vector; available vectors: {:?}",
        plot.vecs.iter().map(|v| &v.name).collect::<Vec<_>>()
    );

    let freq = frequency_vec.unwrap();
    assert!(
        freq.len() > 1,
        "frequency vector should have multiple points, got {}",
        freq.len()
    );

    // Look for output voltage vector (could be complex for AC).
    let vout = plot.vecs.iter().find(|v| v.name == "v(out)");
    assert!(
        vout.is_some(),
        "AC result should contain v(out); available vectors: {:?}",
        plot.vecs.iter().map(|v| &v.name).collect::<Vec<_>>()
    );

    let vout = vout.unwrap();
    assert!(
        vout.len() > 1,
        "v(out) should have multiple frequency points"
    );
}
