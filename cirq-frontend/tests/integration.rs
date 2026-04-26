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
// Both the Cirq ir_lower and the SPICE importer now store passive element
// values under the canonical param name `"value"`, which matches what
// to_netlist expects. This means the full SPICE -> IR -> Netlist round-trip
// works for passives.

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

    // Verify the resistor values were imported correctly (as "value" param).
    let r1_val = r1
        .params
        .iter()
        .find(|p| p.0 == "value")
        .map(|p| match &p.1 {
            cirq_ir::Value::Real(v) => *v,
            _ => panic!("expected real"),
        })
        .expect("R1 should have value param");
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
// Test 3b: SPICE -> IR -> Netlist full round-trip (passives + voltage source)
// ---------------------------------------------------------------------------

#[test]
fn spice_ir_netlist_round_trip() {
    let spice_source = "\
Voltage Divider Round-Trip
V1 in 0 DC 10
R1 in mid 2k
R2 mid 0 2k
.op
.end
";

    // SPICE -> Netlist (direct parse).
    let original_netlists =
        thevenin_types::Netlist::parse(spice_source).expect("SPICE parse should succeed");
    let original = &original_netlists[0];

    // SPICE Netlist -> Cirq IR.
    let ir = cirq_spice_import::import_netlist(original).expect("import_netlist should succeed");

    // Cirq IR -> Netlist (via to_netlist adapter).
    let round_tripped = cirq_frontend::to_netlist::circuit_to_netlists(&ir)
        .expect("circuit_to_netlists should succeed");
    assert_eq!(round_tripped.len(), 1);
    let nl = &round_tripped[0];

    // Verify the round-tripped netlist has the same element count.
    let elem_count = nl
        .items
        .iter()
        .filter(|i| matches!(i, thevenin_types::Item::Element(_)))
        .count();
    assert_eq!(elem_count, 3, "should have V1, R1, R2");

    // Simulate the round-tripped netlist.
    let result =
        thevenin::simulate(nl).expect("simulation of round-tripped netlist should succeed");

    // Verify operating point: mid = 10V * (2k / (2k+2k)) = 5V.
    let v_mid = result.vector("v(mid)").expect("should have v(mid) vector");
    let data = v_mid.data.as_real();
    assert!(
        !data.is_empty(),
        "v(mid) should have at least one data point"
    );
    assert!(
        (data[0] - 5.0).abs() < 0.01,
        "v(mid) should be ~5V, got {}",
        data[0]
    );
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

    // Value params should be the same.
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
        .find(|p| p.0 == "value")
        .map(|p| match &p.1 {
            cirq_ir::Value::Real(v) => *v,
            _ => panic!("expected real"),
        })
        .expect("value param in A");
    let val_b = r1_b
        .params
        .iter()
        .find(|p| p.0 == "value")
        .map(|p| match &p.1 {
            cirq_ir::Value::Real(v) => *v,
            _ => panic!("expected real"),
        })
        .expect("value param in B");

    assert!(
        (val_a - val_b).abs() < 1e-6,
        "value params should match: {val_a} vs {val_b}"
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

// ---------------------------------------------------------------------------
// Test 7: Cirq transient with PULSE waveform -> simulate
// ---------------------------------------------------------------------------

#[test]
fn cirq_transient_pulse_waveform_simulate() {
    // RC circuit with a PULSE voltage source.
    // The pulse goes from 0V to 5V. With R=1k and C=1nF, the RC time constant
    // is 1us. We simulate for 100ns total with a pulse period of 50ns.
    let source = r#"
        circuit pulse_test {
            V1: vsource(in -> gnd, dc: 0, pulse: { v1: 0, v2: 5, td: 1e-9, tr: 0.5e-9, tf: 0.5e-9, pw: 20e-9, per: 50e-9 })
            R1: resistor(in -> out, 1000)
            C1: capacitor(out -> gnd, 1e-12)
            analysis tran {
                step: 1e-9
                stop: 100e-9
            }
        }
    "#;

    // Verify compile to IR succeeds and has the waveform.
    let ir = cirq_frontend::compile(source).expect("compile should succeed");
    assert_eq!(ir.elements.len(), 3); // V1, R1, C1
    let v1 = ir.elements.iter().find(|e| e.name == "V1").expect("V1");
    assert!(matches!(v1.kind, cirq_ir::ElementKind::VoltageSource));
    assert!(v1.source_spec.is_some());
    let spec = v1.source_spec.as_ref().unwrap();
    assert!(spec.waveform.is_some());
    assert!(matches!(
        spec.waveform.as_ref().unwrap(),
        cirq_ir::Waveform::Pulse { .. }
    ));

    // Verify transient analysis in IR.
    assert_eq!(ir.analyses.len(), 1);
    assert!(matches!(ir.analyses[0], cirq_ir::Analysis::Tran(_)));

    // Compile to netlist and simulate.
    let netlists =
        cirq_frontend::compile_to_netlist(source).expect("compile_to_netlist should succeed");
    assert_eq!(netlists.len(), 1);

    let nl = &netlists[0];
    assert!(matches!(nl.analysis, thevenin_types::Analysis::Tran { .. }));

    let result = thevenin::simulate(nl).expect("transient simulation should succeed");

    // Transient results should have multiple time points.
    let plot = result.plot().expect("should have at least one plot");
    let time_vec = plot
        .vecs
        .iter()
        .find(|v| v.name == "time")
        .expect("should have time vector");
    assert!(
        time_vec.len() > 10,
        "transient should produce many time points, got {}",
        time_vec.len()
    );

    // The output node should respond to the pulse.
    let vout = plot
        .vecs
        .iter()
        .find(|v| v.name == "v(out)")
        .expect("should have v(out)");
    assert!(
        vout.len() > 10,
        "v(out) should have many time points, got {}",
        vout.len()
    );
}

// ---------------------------------------------------------------------------
// Test 8: Cirq transient with SIN waveform -> compile
// ---------------------------------------------------------------------------

#[test]
fn cirq_sin_waveform_compile() {
    let source = r#"
        circuit sin_test {
            V1: vsource(in -> gnd, dc: 0, sin: { v0: 0, va: 1, freq: 1e6 })
            R1: resistor(in -> gnd, 1000)
            analysis tran {
                step: 1e-9
                stop: 10e-6
            }
        }
    "#;

    let ir = cirq_frontend::compile(source).expect("compile should succeed");

    // Verify the SIN waveform is in the IR.
    let v1 = ir.elements.iter().find(|e| e.name == "V1").expect("V1");
    assert!(matches!(v1.kind, cirq_ir::ElementKind::VoltageSource));
    let spec = v1.source_spec.as_ref().expect("V1 should have source_spec");
    match spec.waveform.as_ref().expect("should have waveform") {
        cirq_ir::Waveform::Sin { v0, va, freq, .. } => {
            assert!((*v0 - 0.0).abs() < 1e-12, "v0 should be 0, got {v0}");
            assert!((*va - 1.0).abs() < 1e-12, "va should be 1, got {va}");
            assert!(
                (freq.unwrap() - 1e6).abs() < 1.0,
                "freq should be 1MHz, got {:?}",
                freq
            );
        }
        other => panic!("expected Sin waveform, got {other:?}"),
    }

    // Verify the transient analysis.
    assert_eq!(ir.analyses.len(), 1);
    match &ir.analyses[0] {
        cirq_ir::Analysis::Tran(tran) => {
            assert!((tran.step - 1e-9).abs() < 1e-15);
            assert!((tran.stop - 10e-6).abs() < 1e-12);
        }
        other => panic!("expected Tran analysis, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Test 9: Cirq noise analysis -> compile to netlist
// ---------------------------------------------------------------------------

#[test]
fn cirq_noise_analysis_compile_to_netlist() {
    let source = r#"
        circuit noise_test {
            V1: vsource(in -> gnd, dc: 0, ac_mag: 1)
            R1: resistor(in -> out, 10000)
            R2: resistor(out -> gnd, 10000)
            analysis noise {
                output: out
                reference: gnd
                source: V1
                start: 1
                stop: 1e6
                points: 10
                scale: decade
            }
        }
    "#;

    // Verify IR has Noise analysis.
    let ir = cirq_frontend::compile(source).expect("compile should succeed");
    assert_eq!(ir.analyses.len(), 1);
    match &ir.analyses[0] {
        cirq_ir::Analysis::Noise(noise) => {
            assert!(noise.start == 1.0, "start should be 1 Hz");
            assert!((noise.stop - 1e6).abs() < 1.0, "stop should be 1 MHz");
            assert_eq!(noise.points, 10);
            assert_eq!(noise.scale, cirq_ir::FrequencyScale::Decade);
        }
        other => panic!("expected Noise analysis, got {other:?}"),
    }

    // Verify Netlist has Noise analysis.
    let netlists =
        cirq_frontend::compile_to_netlist(source).expect("compile_to_netlist should succeed");
    assert_eq!(netlists.len(), 1);
    let nl = &netlists[0];
    match &nl.analysis {
        thevenin_types::Analysis::Noise {
            output,
            src,
            variation,
            n,
            ..
        } => {
            assert_eq!(output, "out");
            assert_eq!(src, "V1");
            assert_eq!(*variation, thevenin_types::AcVariation::Dec);
            assert_eq!(*n, 10);
        }
        other => panic!("expected Noise analysis in netlist, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Test 10: Cirq PZ analysis -> compile to netlist
// ---------------------------------------------------------------------------

#[test]
fn cirq_pz_analysis_compile_to_netlist() {
    let source = r#"
        circuit pz_test {
            V1: vsource(in -> gnd, dc: 1)
            R1: resistor(in -> out, 1000)
            C1: capacitor(out -> gnd, 1e-9)
            analysis pz {
                input_pos: in
                input_neg: gnd
                output_pos: out
                output_neg: gnd
                transfer: voltage
                type: both
            }
        }
    "#;

    // Verify IR has PZ analysis.
    let ir = cirq_frontend::compile(source).expect("compile should succeed");
    assert_eq!(ir.analyses.len(), 1);
    match &ir.analyses[0] {
        cirq_ir::Analysis::Pz(pz) => {
            assert_eq!(pz.transfer, cirq_ir::TransferType::Voltage);
            assert_eq!(pz.analysis_type, cirq_ir::PzType::Both);
        }
        other => panic!("expected Pz analysis, got {other:?}"),
    }

    // Verify Netlist has PZ analysis with correct fields.
    let netlists =
        cirq_frontend::compile_to_netlist(source).expect("compile_to_netlist should succeed");
    assert_eq!(netlists.len(), 1);
    let nl = &netlists[0];
    match &nl.analysis {
        thevenin_types::Analysis::Pz {
            node_i,
            node_g,
            node_j,
            node_k,
            input_type,
            analysis_type,
        } => {
            assert_eq!(node_i, "in");
            assert_eq!(node_g, "0"); // gnd maps to "0"
            assert_eq!(node_j, "out");
            assert_eq!(node_k, "0"); // gnd maps to "0"
            assert_eq!(*input_type, thevenin_types::PzInputType::Vol);
            assert_eq!(*analysis_type, thevenin_types::PzAnalysisType::Pz);
        }
        other => panic!("expected Pz analysis in netlist, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Test 11: Cirq RLC circuit (capacitor + inductor) -> simulate OP
// ---------------------------------------------------------------------------

#[test]
fn cirq_rlc_circuit_simulate_op() {
    // RLC circuit: V=5V, R=100, L=1mH, C=1uF.
    // At DC (OP), the inductor is a short circuit and the capacitor is open.
    // So v(mid) = 5V (inductor shorts out, all current through R to ground via C which is open).
    // Actually: V -> R -> L -> C -> gnd. At DC: L is short, C is open.
    // So no current flows, v(mid) = v(out) = 5V.
    let source = r#"
        circuit rlc_test {
            V1: vsource(in -> gnd, dc: 5)
            R1: resistor(in -> mid, 100)
            L1: inductor(mid -> out, 1e-3)
            C1: capacitor(out -> gnd, 1e-6)
            analysis op {}
        }
    "#;

    // Verify IR has all element kinds.
    let ir = cirq_frontend::compile(source).expect("compile should succeed");
    assert_eq!(ir.elements.len(), 4);

    let r1 = ir.elements.iter().find(|e| e.name == "R1").expect("R1");
    assert!(matches!(r1.kind, cirq_ir::ElementKind::Resistor));

    let l1 = ir.elements.iter().find(|e| e.name == "L1").expect("L1");
    assert!(matches!(l1.kind, cirq_ir::ElementKind::Inductor));

    let c1 = ir.elements.iter().find(|e| e.name == "C1").expect("C1");
    assert!(matches!(c1.kind, cirq_ir::ElementKind::Capacitor));

    // Simulate OP.
    let netlists =
        cirq_frontend::compile_to_netlist(source).expect("compile_to_netlist should succeed");
    let result = thevenin::simulate(&netlists[0]).expect("simulation should succeed");

    // At DC steady state: inductor is short, capacitor is open.
    // No current flows through the series path, so v(mid) = v(out) = v(in) = 5V.
    let vin = result.vector("v(in)").expect("should have v(in)");
    assert!(
        (vin.data.as_real()[0] - 5.0).abs() < 0.01,
        "v(in) should be ~5V, got {}",
        vin.data.as_real()[0]
    );

    let vmid = result.vector("v(mid)").expect("should have v(mid)");
    assert!(
        (vmid.data.as_real()[0] - 5.0).abs() < 0.01,
        "v(mid) should be ~5V at DC, got {}",
        vmid.data.as_real()[0]
    );

    let vout = result.vector("v(out)").expect("should have v(out)");
    assert!(
        (vout.data.as_real()[0] - 5.0).abs() < 0.01,
        "v(out) should be ~5V at DC, got {}",
        vout.data.as_real()[0]
    );
}

// ---------------------------------------------------------------------------
// Test 12: Cirq coupling element -> compile
// ---------------------------------------------------------------------------

#[test]
fn cirq_coupling_element_compile() {
    let source = r#"
        circuit coupling_test {
            V1: vsource(in -> gnd, dc: 1)
            L1: inductor(in -> mid, 1e-3)
            L2: inductor(out -> gnd, 1e-3)
            R1: resistor(mid -> gnd, 100)
            R2: resistor(out -> gnd, 100)
            K1: coupling(l1: "L1", l2: "L2", coupling: 0.99)
            analysis op {}
        }
    "#;

    let ir = cirq_frontend::compile(source).expect("compile should succeed");

    // Should have 6 elements: V1, L1, L2, R1, R2, K1.
    assert_eq!(ir.elements.len(), 6);

    let k1 = ir.elements.iter().find(|e| e.name == "K1").expect("K1");
    assert!(matches!(k1.kind, cirq_ir::ElementKind::Coupling));

    // Verify coupling params: l1, l2, coupling.
    let l1_param = k1
        .params
        .iter()
        .find(|p| p.0 == "l1")
        .expect("should have l1 param");
    match &l1_param.1 {
        cirq_ir::Value::String(s) => assert_eq!(s, "L1"),
        other => panic!("expected String for l1, got {other:?}"),
    }

    let l2_param = k1
        .params
        .iter()
        .find(|p| p.0 == "l2")
        .expect("should have l2 param");
    match &l2_param.1 {
        cirq_ir::Value::String(s) => assert_eq!(s, "L2"),
        other => panic!("expected String for l2, got {other:?}"),
    }

    let coupling_param = k1
        .params
        .iter()
        .find(|p| p.0 == "coupling")
        .expect("should have coupling param");
    match &coupling_param.1 {
        cirq_ir::Value::Real(v) => {
            assert!((*v - 0.99).abs() < 1e-6, "coupling should be 0.99, got {v}")
        }
        other => panic!("expected Real for coupling, got {other:?}"),
    }

    // Verify the netlist produces a MutualCoupling element.
    let netlists =
        cirq_frontend::compile_to_netlist(source).expect("compile_to_netlist should succeed");
    let nl = &netlists[0];
    let elements: Vec<&thevenin_types::Element> = nl
        .items
        .iter()
        .filter_map(|i| {
            if let thevenin_types::Item::Element(e) = i {
                Some(e)
            } else {
                None
            }
        })
        .collect();

    let coupling_elem = elements
        .iter()
        .find(|e| e.name == "K1")
        .expect("K1 in netlist");
    assert!(
        matches!(
            &coupling_elem.kind,
            thevenin_types::ElementKind::MutualCoupling { .. }
        ),
        "expected MutualCoupling, got {:?}",
        coupling_elem.kind
    );
}

// ---------------------------------------------------------------------------
// Test 13: Cirq MOSFET with model -> compile
// ---------------------------------------------------------------------------

#[test]
fn cirq_mosfet_with_model_compile() {
    let source = r#"
        circuit mosfet_test {
            model nch: nmos { level = 1, vto = 0.7, kp = 110e-6 }
            V1: vsource(vdd -> gnd, dc: 3.3)
            V2: vsource(gate -> gnd, dc: 1.5)
            M1: nmos(drain: out, gate: gate, source: gnd, bulk: gnd, model: nch, w: 1e-6, l: 180e-9)
            R1: resistor(vdd -> out, 10000)
            analysis op {}
        }
    "#;

    let ir = cirq_frontend::compile(source).expect("compile should succeed");

    // Verify model.
    assert_eq!(ir.models.len(), 1);
    assert_eq!(ir.models[0].name, "nch");
    assert_eq!(ir.models[0].device_type, cirq_ir::DeviceType::Nmos);

    // Check model params.
    let vto_param = ir.models[0]
        .params
        .iter()
        .find(|p| p.0 == "vto")
        .expect("should have vto param");
    match &vto_param.1 {
        cirq_ir::Value::Real(v) => assert!((*v - 0.7).abs() < 1e-6),
        other => panic!("expected Real for vto, got {other:?}"),
    }

    // Verify MOSFET element.
    let m1 = ir.elements.iter().find(|e| e.name == "M1").expect("M1");
    assert!(matches!(m1.kind, cirq_ir::ElementKind::Nmos));
    assert!(m1.model.is_some(), "M1 should reference a model");

    // Verify MOSFET connections (all four terminals).
    let drain_conn = m1.connections.iter().find(|c| c.terminal == "drain");
    assert!(drain_conn.is_some(), "should have drain connection");

    let gate_conn = m1.connections.iter().find(|c| c.terminal == "gate");
    assert!(gate_conn.is_some(), "should have gate connection");

    let source_conn = m1.connections.iter().find(|c| c.terminal == "source");
    assert!(source_conn.is_some(), "should have source connection");

    let bulk_conn = m1.connections.iter().find(|c| c.terminal == "bulk");
    assert!(bulk_conn.is_some(), "should have bulk connection");

    // Verify W/L params are present.
    let w_param = m1.params.iter().find(|p| p.0 == "w");
    assert!(w_param.is_some(), "M1 should have w param");

    let l_param = m1.params.iter().find(|p| p.0 == "l");
    assert!(l_param.is_some(), "M1 should have l param");

    // Verify netlist produces a Mosfet element.
    let netlists =
        cirq_frontend::compile_to_netlist(source).expect("compile_to_netlist should succeed");
    let nl = &netlists[0];
    let elements: Vec<&thevenin_types::Element> = nl
        .items
        .iter()
        .filter_map(|i| {
            if let thevenin_types::Item::Element(e) = i {
                Some(e)
            } else {
                None
            }
        })
        .collect();

    let mosfet = elements
        .iter()
        .find(|e| e.name == "M1")
        .expect("M1 in netlist");
    match &mosfet.kind {
        thevenin_types::ElementKind::Mosfet {
            d,
            g,
            s,
            bulk,
            model,
            ..
        } => {
            assert_eq!(model, "nch");
            assert_eq!(g, "gate");
            // drain should map to "out", source to "0" (gnd), bulk to "0" (gnd)
            assert_eq!(d, "out");
            assert_eq!(s, "0");
            assert_eq!(bulk, "0");
        }
        other => panic!("expected Mosfet, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Test 14: SPICE waveform round-trip (PULSE source)
// ---------------------------------------------------------------------------

#[test]
fn spice_pulse_waveform_round_trip() {
    let spice_source = "\
Pulse Source Test
V1 in 0 PULSE(0 5 1e-9 0.5e-9 0.5e-9 20e-9 50e-9)
R1 in 0 1k
.tran 1n 100n
.end
";

    // Parse SPICE -> Netlist.
    let original_netlists =
        thevenin_types::Netlist::parse(spice_source).expect("SPICE parse should succeed");
    assert_eq!(original_netlists.len(), 1);

    // Import to Cirq IR.
    let ir = cirq_spice_import::import_netlist(&original_netlists[0])
        .expect("import_netlist should succeed");

    // Verify the waveform survived the import.
    let v1 = ir.elements.iter().find(|e| e.name == "V1").expect("V1");
    assert!(matches!(v1.kind, cirq_ir::ElementKind::VoltageSource));
    assert!(v1.source_spec.is_some(), "V1 should have source_spec");
    let spec = v1.source_spec.as_ref().unwrap();
    match spec.waveform.as_ref() {
        Some(cirq_ir::Waveform::Pulse { v1, v2, .. }) => {
            assert!((*v1 - 0.0).abs() < 1e-12, "v1 should be 0");
            assert!((*v2 - 5.0).abs() < 1e-12, "v2 should be 5");
        }
        other => panic!("expected Pulse waveform in IR, got {other:?}"),
    }

    // Convert back to Netlist.
    let round_tripped = cirq_frontend::to_netlist::circuit_to_netlists(&ir)
        .expect("circuit_to_netlists should succeed");
    assert_eq!(round_tripped.len(), 1);

    // Verify the waveform is preserved in the round-tripped netlist.
    let nl = &round_tripped[0];
    let v1_elem = nl
        .items
        .iter()
        .find_map(|i| {
            if let thevenin_types::Item::Element(e) = i {
                if e.name == "V1" { Some(e) } else { None }
            } else {
                None
            }
        })
        .expect("V1 in netlist");

    match &v1_elem.kind {
        thevenin_types::ElementKind::VoltageSource { source, .. } => {
            assert!(
                source.waveform.is_some(),
                "round-tripped V1 should have a waveform"
            );
            assert!(matches!(
                source.waveform.as_ref().unwrap(),
                thevenin_types::Waveform::Pulse { .. }
            ));
        }
        other => panic!("expected VoltageSource, got {other:?}"),
    }

    // Verify transient analysis survived.
    assert!(matches!(nl.analysis, thevenin_types::Analysis::Tran { .. }));
}

// ---------------------------------------------------------------------------
// Test 15: SPICE AC source round-trip
// ---------------------------------------------------------------------------

#[test]
fn spice_ac_source_round_trip() {
    let spice_source = "\
AC Source Test
V1 in 0 DC 0 AC 1
R1 in out 1k
C1 out 0 1u
.ac DEC 10 1 1MEG
.end
";

    // Parse SPICE -> Netlist -> IR.
    let original_netlists =
        thevenin_types::Netlist::parse(spice_source).expect("SPICE parse should succeed");
    let ir = cirq_spice_import::import_netlist(&original_netlists[0])
        .expect("import_netlist should succeed");

    // Verify the AC spec survived the import.
    let v1 = ir.elements.iter().find(|e| e.name == "V1").expect("V1");
    assert!(v1.source_spec.is_some(), "V1 should have source_spec");
    let spec = v1.source_spec.as_ref().unwrap();
    assert!(spec.ac.is_some(), "V1 should have AC spec");
    let ac = spec.ac.as_ref().unwrap();
    assert!(
        (ac.mag - 1.0).abs() < 1e-12,
        "AC magnitude should be 1, got {}",
        ac.mag
    );

    // Verify AC analysis.
    assert_eq!(ir.analyses.len(), 1);
    match &ir.analyses[0] {
        cirq_ir::Analysis::Ac(ac_analysis) => {
            assert!(
                (ac_analysis.start - 1.0).abs() < 1e-6,
                "AC start should be 1 Hz"
            );
            assert!(
                (ac_analysis.stop - 1e6).abs() < 1.0,
                "AC stop should be 1 MHz"
            );
            assert_eq!(ac_analysis.points, 10);
            assert_eq!(ac_analysis.scale, cirq_ir::FrequencyScale::Decade);
        }
        other => panic!("expected Ac analysis, got {other:?}"),
    }

    // Convert IR back to Netlist.
    let round_tripped = cirq_frontend::to_netlist::circuit_to_netlists(&ir)
        .expect("circuit_to_netlists should succeed");
    assert_eq!(round_tripped.len(), 1);
    let nl = &round_tripped[0];

    // Verify AC analysis in the netlist.
    match &nl.analysis {
        thevenin_types::Analysis::Ac {
            variation,
            n,
            fstart,
            fstop,
        } => {
            assert_eq!(*variation, thevenin_types::AcVariation::Dec);
            assert_eq!(*n, 10);
            assert!(matches!(fstart, thevenin_types::Expr::Num(v) if (*v - 1.0).abs() < 1e-6));
            assert!(matches!(fstop, thevenin_types::Expr::Num(v) if (*v - 1e6).abs() < 1.0));
        }
        other => panic!("expected Ac analysis in round-tripped netlist, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Test 16: SPICE options + temp round-trip
// ---------------------------------------------------------------------------

#[test]
fn spice_options_and_temp_round_trip() {
    let spice_source = "\
Options Test
R1 a 0 1k
V1 a 0 DC 5
.options RELTOL=1e-3 ABSTOL=1e-12
.temp 85
.op
.end
";

    // Parse SPICE -> Netlist -> IR.
    let original_netlists =
        thevenin_types::Netlist::parse(spice_source).expect("SPICE parse should succeed");
    let ir = cirq_spice_import::import_netlist(&original_netlists[0])
        .expect("import_netlist should succeed");

    // Verify options are in IR.
    assert!(
        !ir.options.is_empty(),
        "IR should have options, got {:?}",
        ir.options
    );
    let reltol = ir.options.iter().find(|o| o.0.to_lowercase() == "reltol");
    assert!(reltol.is_some(), "should have RELTOL option");

    // Verify temp is in IR.
    assert!(!ir.temps.is_empty(), "should have temperature");
    assert!(
        (ir.temps[0] - 85.0).abs() < 0.01,
        "temp should be 85C, got {:?}",
        ir.temps
    );

    // Round-trip IR -> Netlist.
    let round_tripped = cirq_frontend::to_netlist::circuit_to_netlists(&ir)
        .expect("circuit_to_netlists should succeed");
    let nl = &round_tripped[0];

    // Verify options survived in the netlist.
    let has_options = nl
        .items
        .iter()
        .any(|i| matches!(i, thevenin_types::Item::Options(_)));
    assert!(has_options, "netlist should have options");

    // Verify temp survived in the netlist.
    let has_temp = nl
        .items
        .iter()
        .any(|i| matches!(i, thevenin_types::Item::Temp(t) if (*t - 85.0).abs() < 0.01));
    assert!(has_temp, "netlist should have temperature 85C");
}

// ---------------------------------------------------------------------------
// Test 17: SPICE dependent sources round-trip (VCVS/VCCS)
// ---------------------------------------------------------------------------

#[test]
fn spice_dependent_sources_round_trip() {
    let spice_source = "\
Dependent Source Test
V1 in 0 DC 1
R1 in 0 1k
E1 out 0 in 0 2
R2 out 0 1k
.op
.end
";

    // Parse SPICE -> Netlist -> IR.
    let original_netlists =
        thevenin_types::Netlist::parse(spice_source).expect("SPICE parse should succeed");
    let ir = cirq_spice_import::import_netlist(&original_netlists[0])
        .expect("import_netlist should succeed");

    // Verify VCVS element.
    let e1 = ir.elements.iter().find(|e| e.name == "E1").expect("E1");
    assert!(
        matches!(e1.kind, cirq_ir::ElementKind::Vcvs),
        "E1 should be VCVS, got {:?}",
        e1.kind
    );

    // Verify connections.
    let out_pos = e1.connections.iter().find(|c| c.terminal == "out_pos");
    assert!(out_pos.is_some(), "E1 should have out_pos connection");
    let in_pos = e1.connections.iter().find(|c| c.terminal == "in_pos");
    assert!(in_pos.is_some(), "E1 should have in_pos connection");

    // Round-trip IR -> Netlist -> simulate.
    let round_tripped = cirq_frontend::to_netlist::circuit_to_netlists(&ir)
        .expect("circuit_to_netlists should succeed");
    let nl = &round_tripped[0];

    let result = thevenin::simulate(nl).expect("simulation should succeed");

    // VCVS with gain=2: if V(in)=1V, V(out)=2V.
    let vin = result.vector("v(in)").expect("should have v(in)");
    assert!(
        (vin.data.as_real()[0] - 1.0).abs() < 0.01,
        "v(in) should be ~1V, got {}",
        vin.data.as_real()[0]
    );

    let vout = result.vector("v(out)").expect("should have v(out)");
    assert!(
        (vout.data.as_real()[0] - 2.0).abs() < 0.01,
        "v(out) should be ~2V (gain=2), got {}",
        vout.data.as_real()[0]
    );
}

// ---------------------------------------------------------------------------
// Test 18: SPICE behavioral source -> IR
// ---------------------------------------------------------------------------

#[test]
fn spice_behavioral_source_to_ir() {
    let spice_source = "\
Behavioral Source Test
V1 in 0 DC 1
R1 in 0 1k
B1 out 0 V=v(in)*2
R2 out 0 1k
.op
.end
";

    // Parse SPICE -> Netlist -> IR.
    let original_netlists =
        thevenin_types::Netlist::parse(spice_source).expect("SPICE parse should succeed");
    let ir = cirq_spice_import::import_netlist(&original_netlists[0])
        .expect("import_netlist should succeed");

    // Verify BehavioralSource element.
    let b1 = ir.elements.iter().find(|e| e.name == "B1").expect("B1");
    match &b1.kind {
        cirq_ir::ElementKind::BehavioralSource { mode, spec } => {
            assert_eq!(
                *mode,
                cirq_ir::BehavioralMode::Voltage,
                "B1 should be voltage mode"
            );
            assert!(
                !spec.is_empty(),
                "B1 should have a non-empty expression spec"
            );
        }
        other => panic!("expected BehavioralSource, got {other:?}"),
    }

    // Verify connections.
    let pos = b1.connections.iter().find(|c| c.terminal == "pos");
    assert!(pos.is_some(), "B1 should have pos connection");
    let neg = b1.connections.iter().find(|c| c.terminal == "neg");
    assert!(neg.is_some(), "B1 should have neg connection");

    // Round-trip IR -> Netlist.
    let round_tripped = cirq_frontend::to_netlist::circuit_to_netlists(&ir)
        .expect("circuit_to_netlists should succeed");
    let nl = &round_tripped[0];

    let b1_elem = nl
        .items
        .iter()
        .find_map(|i| {
            if let thevenin_types::Item::Element(e) = i {
                if e.name == "B1" { Some(e) } else { None }
            } else {
                None
            }
        })
        .expect("B1 in netlist");

    assert!(
        matches!(&b1_elem.kind, thevenin_types::ElementKind::BehavioralSource { spec, .. } if spec.starts_with("V=")),
        "B1 should be a voltage behavioral source in netlist, got {:?}",
        b1_elem.kind
    );
}

// ---------------------------------------------------------------------------
// Test 19: Hierarchical modules — multi-instance flattening
// ---------------------------------------------------------------------------

#[test]
fn cirq_hierarchical_module_flattening() {
    // Module `inverter` instantiated twice inside `buffer`, then `buffer`
    // instantiated at circuit level. This exercises:
    //  - Module definition collection
    //  - Port-to-net remapping
    //  - Param scoping (params inside a module must not collide across instances)
    //  - Hierarchical element name prefixing (buf1.inv1.M1, buf1.inv2.M1)
    let source = r#"
        module inverter {
            port in: in
            port out: out
            port vdd: inout
            port vss: inout

            param wp = 2u
            param wn = 1u
            param l = 180n

            M1: pmos(drain: out, gate: in, source: vdd, bulk: vdd, model: pch, w: wp, l: l)
            M2: nmos(drain: out, gate: in, source: vss, bulk: vss, model: nch, w: wn, l: l)
        }

        module buffer {
            port a: in
            port z: out
            port vdd: inout
            port vss: inout

            inv1: inverter(in: a, out: mid, vdd: vdd, vss: vss)
            inv2: inverter(in: mid, out: z, vdd: vdd, vss: vss)
        }

        circuit hierarchical_test {
            model nch: nmos {
                level = 1
                vto = 0.7
                kp = 110u
            }

            model pch: pmos {
                level = 1
                vto = -0.7
                kp = 55u
            }

            Vdd: vsource(vdd -> gnd, dc: 1.8)
            Vin: vsource(in -> gnd,
                pulse: { v1: 0, v2: 1.8, td: 0, tr: 100p, tf: 100p, pw: 5n, per: 10n }
            )

            buf1: buffer(a: in, z: z, vdd: vdd, vss: gnd)

            analysis tran {
                step: 50p
                stop: 30n
            }
        }
    "#;

    // Should compile without duplicate-param errors.
    let ir = cirq_frontend::compile(source).expect("compile should succeed");
    assert_eq!(ir.name, "hierarchical_test");

    // After flattening, we should have:
    //   Vdd, Vin (top-level)
    //   buf1.inv1.M1, buf1.inv1.M2 (first inverter)
    //   buf1.inv2.M1, buf1.inv2.M2 (second inverter)
    // Total: 6 elements.
    assert_eq!(
        ir.elements.len(),
        6,
        "expected 6 elements after flattening, got {}: {:?}",
        ir.elements.len(),
        ir.elements.iter().map(|e| &e.name).collect::<Vec<_>>()
    );

    // Check hierarchical names are correct.
    assert!(
        ir.elements.iter().any(|e| e.name == "buf1.inv1.M1"),
        "should have buf1.inv1.M1"
    );
    assert!(
        ir.elements.iter().any(|e| e.name == "buf1.inv2.M1"),
        "should have buf1.inv2.M1"
    );
    assert!(
        ir.elements.iter().any(|e| e.name == "buf1.inv1.M2"),
        "should have buf1.inv1.M2"
    );
    assert!(
        ir.elements.iter().any(|e| e.name == "buf1.inv2.M2"),
        "should have buf1.inv2.M2"
    );

    // Params from both inverter instances should be prefixed and independent.
    assert!(
        ir.params.iter().any(|p| p.name == "buf1.inv1.wp"),
        "should have prefixed param buf1.inv1.wp"
    );
    assert!(
        ir.params.iter().any(|p| p.name == "buf1.inv2.wp"),
        "should have prefixed param buf1.inv2.wp"
    );

    // Should still produce a valid netlist (no crash during to_netlist).
    let netlists =
        cirq_frontend::compile_to_netlist(source).expect("compile_to_netlist should succeed");
    assert!(!netlists.is_empty());
}

// ---------------------------------------------------------------------------
// Test 20: Single module with multiple instantiations at circuit level
// ---------------------------------------------------------------------------

#[test]
fn cirq_module_multiple_instances_top_level() {
    // Simpler case: two instances of the same module at the circuit level.
    let source = r#"
        module voltage_divider {
            port vin: in
            port vout: out
            port gnd_ref: inout

            param r_top = 10k
            param r_bot = 10k
            R_top: resistor(vin -> vout, r_top)
            R_bot: resistor(vout -> gnd_ref, r_bot)
        }

        circuit multi_inst_test {
            V1: vsource(vin -> gnd, dc: 10)

            div1: voltage_divider(vin: vin, vout: mid1, gnd_ref: gnd)
            div2: voltage_divider(vin: vin, vout: mid2, gnd_ref: gnd)

            analysis op {}
        }
    "#;

    let ir = cirq_frontend::compile(source).expect("compile should succeed");

    // 5 elements: V1, div1.R_top, div1.R_bot, div2.R_top, div2.R_bot
    assert_eq!(
        ir.elements.len(),
        5,
        "expected 5 elements, got {}: {:?}",
        ir.elements.len(),
        ir.elements.iter().map(|e| &e.name).collect::<Vec<_>>()
    );

    // Verify both instances have correctly prefixed elements.
    assert!(ir.elements.iter().any(|e| e.name == "div1.R_top"));
    assert!(ir.elements.iter().any(|e| e.name == "div1.R_bot"));
    assert!(ir.elements.iter().any(|e| e.name == "div2.R_top"));
    assert!(ir.elements.iter().any(|e| e.name == "div2.R_bot"));

    // Params should be instance-scoped.
    assert!(ir.params.iter().any(|p| p.name == "div1.r_top"));
    assert!(ir.params.iter().any(|p| p.name == "div2.r_top"));

    // Simulate and check: each divider should produce v(mid) = 5V.
    let netlists =
        cirq_frontend::compile_to_netlist(source).expect("compile_to_netlist should succeed");
    let result = thevenin::simulate(&netlists[0]).expect("simulation should succeed");

    let vmid1 = result.vector("v(mid1)").expect("should have v(mid1)");
    assert!(
        (vmid1.data.as_real()[0] - 5.0).abs() < 0.01,
        "v(mid1) should be ~5V, got {}",
        vmid1.data.as_real()[0]
    );

    let vmid2 = result.vector("v(mid2)").expect("should have v(mid2)");
    assert!(
        (vmid2.data.as_real()[0] - 5.0).abs() < 0.01,
        "v(mid2) should be ~5V, got {}",
        vmid2.data.as_real()[0]
    );
}

// ---------------------------------------------------------------------------
// Test 21: Control block — verbatim SPICE control lines pass through
// ---------------------------------------------------------------------------

#[test]
fn cirq_control_block_round_trip() {
    let source = r#"
        circuit control_round_trip {
            V1: vsource(in -> gnd, dc: 10)
            R1: resistor(in -> out, 1000)
            R2: resistor(out -> gnd, 1000)

            analysis op {}

            code "control" {
                run
                let gain = v(out) / v(in)
                print gain
            }
        }
    "#;

    // Compile to IR — code block should be preserved.
    let ir = cirq_frontend::compile(source).expect("compile should succeed");
    assert_eq!(ir.code_blocks.len(), 1);
    assert_eq!(ir.code_blocks[0].language, "control");
    assert_eq!(ir.code_blocks[0].lines.len(), 3);
    assert_eq!(ir.code_blocks[0].lines[0], "run");
    assert_eq!(ir.code_blocks[0].lines[1], "let gain = v(out) / v(in)");
    assert_eq!(ir.code_blocks[0].lines[2], "print gain");

    // Compile to netlist — control block should appear as Item::Control.
    let netlists =
        cirq_frontend::compile_to_netlist(source).expect("compile_to_netlist should succeed");
    let nl = &netlists[0];

    let has_control = nl
        .items
        .iter()
        .any(|item| matches!(item, thevenin_types::Item::Control(_)));
    assert!(has_control, "netlist should contain Item::Control");

    // Execute via control-block interpreter.
    assert!(thevenin_control::has_control_block(nl));
    let ctrl_result = thevenin_control::execute_control_block(nl)
        .expect("control block execution should succeed");

    // The `run` command should have produced an OP plot with gain = 0.5.
    let gain_vec = ctrl_result
        .sim_result
        .plots
        .iter()
        .flat_map(|p| &p.vecs)
        .find(|v| v.name == "gain");
    assert!(
        gain_vec.is_some(),
        "should have gain vector from control let"
    );
    let gain_val = gain_vec.unwrap().data.as_real()[0];
    assert!(
        (gain_val - 0.5).abs() < 0.001,
        "gain should be ~0.5, got {gain_val}"
    );
}

// ===========================================================================
// Round-trip tests for newly wired simulator features
// ===========================================================================

// ---------------------------------------------------------------------------
// .ic node-level initial conditions in transient
// ---------------------------------------------------------------------------

/// Verify that `.ic` node voltages are applied in transient analysis.
///
/// A simple RC circuit with V1=5V, R1=1k, C1=1n. With `.ic v(cap)=5.0`,
/// the capacitor node starts at 5V (same as supply), so there should be
/// no initial transient — v(cap) should stay at ~5V throughout.
#[test]
fn cirq_ic_node_voltage_in_transient() {
    let source = r#"
        circuit ic_test {
            V1: vsource(in -> gnd, dc: 5)
            R1: resistor(in -> cap, 1000)
            C1: capacitor(cap -> gnd, 1e-9)
            ic { v(cap) = 5.0 }
            analysis tran { step: 1e-9, stop: 50e-9 }
        }
    "#;

    let netlists =
        cirq_frontend::compile_to_netlist(source).expect("compile_to_netlist should succeed");
    assert_eq!(netlists.len(), 1);

    // Verify the netlist contains .ic
    let has_ic = netlists[0]
        .items
        .iter()
        .any(|i| matches!(i, thevenin_types::Item::Ic(_)));
    assert!(has_ic, "netlist should contain .ic directive");

    let result = thevenin::simulate(&netlists[0]).expect("simulation should succeed");

    // v(cap) should start at 5V and stay near it (since supply is also 5V).
    let vcap = result.vector("v(cap)").expect("should have v(cap)");
    let data = vcap.data.as_real();
    assert!(!data.is_empty(), "transient should produce data points");
    assert!(
        (data[0] - 5.0).abs() < 0.1,
        "v(cap) should start at ~5V from .ic, got {}",
        data[0]
    );
}

// ---------------------------------------------------------------------------
// UIC flag — skip DC operating point
// ---------------------------------------------------------------------------

/// With UIC, the transient starts from zero + .ic values instead of DC OP.
///
/// R1=1k from in to out, V1=5V on in, C1 on out. Without UIC, DC OP gives
/// v(out)=5V. With UIC and ic{v(out)=0}, we start at 0V and charge up.
#[test]
fn cirq_uic_skips_dc_op() {
    let source = r#"
        circuit uic_test {
            V1: vsource(in -> gnd, dc: 5)
            R1: resistor(in -> out, 1000)
            C1: capacitor(out -> gnd, 1e-6)
            ic { v(out) = 0.0 }
            analysis tran { step: 1e-6, stop: 100e-6, uic: true }
        }
    "#;

    let netlists =
        cirq_frontend::compile_to_netlist(source).expect("compile_to_netlist should succeed");
    assert_eq!(netlists.len(), 1);

    // Verify UIC flag is set in the netlist analysis.
    match &netlists[0].analysis {
        thevenin_types::Analysis::Tran { uic, .. } => {
            assert!(uic, "UIC flag should be set");
        }
        _ => panic!("expected tran analysis"),
    }

    let result = thevenin::simulate(&netlists[0]).expect("simulation should succeed");

    let vout = result.vector("v(out)").expect("should have v(out)");
    let data = vout.data.as_real();
    assert!(!data.is_empty());

    // With UIC + ic{v(out)=0}, the first point should be near 0V, not 5V.
    assert!(
        data[0].abs() < 0.5,
        "with UIC + ic=0, v(out) should start near 0V, got {}",
        data[0]
    );

    // By the end of 100us with RC=1ms, we should be partially charged.
    let last = *data.last().unwrap();
    assert!(
        last > 0.1 && last < 5.0,
        "v(out) should be partially charged, got {last}"
    );
}

// ---------------------------------------------------------------------------
// .nodeset — convergence hint (structural test)
// ---------------------------------------------------------------------------

/// Verify that `.nodeset` passes through the pipeline and doesn't break
/// simulation. The circuit should converge to the same DC OP regardless.
#[test]
fn cirq_nodeset_passes_through_pipeline() {
    // Build a circuit with nodeset via the SPICE import path, since the Cirq
    // grammar doesn't have a `nodeset` keyword yet — but the IR does, and
    // to_netlist emits it.
    let spice = "\
Nodeset Test
V1 in 0 5
R1 in mid 1k
R2 mid 0 1k
.nodeset V(mid)=2.5
.op
.end
";

    let spice_netlists = thevenin_types::Netlist::parse(spice).expect("SPICE parse");
    let spice_nl = &spice_netlists[0];

    // Import to IR and back.
    let ir = cirq_spice_import::import_netlist(spice_nl).expect("import");
    assert!(!ir.nodeset.is_empty(), "IR should have nodeset entries");

    let round_trip_netlists =
        cirq_frontend::to_netlist::circuit_to_netlists(&ir).expect("to_netlist");
    let rt_nl = &round_trip_netlists[0];

    // Verify .nodeset survived the round-trip.
    let has_nodeset = rt_nl
        .items
        .iter()
        .any(|i| matches!(i, thevenin_types::Item::Nodeset(_)));
    assert!(has_nodeset, "round-tripped netlist should have .nodeset");

    // Simulate both: original SPICE and round-tripped.
    let r1 = thevenin::simulate(spice_nl).expect("SPICE sim");
    let r2 = thevenin::simulate(rt_nl).expect("round-trip sim");

    let v1 = r1.vector("v(mid)").unwrap().data.as_real()[0];
    let v2 = r2.vector("v(mid)").unwrap().data.as_real()[0];
    assert!(
        (v1 - v2).abs() < 1e-6,
        "round-trip should give same result: {v1} vs {v2}"
    );
    assert!((v1 - 2.5).abs() < 0.01, "v(mid) should be 2.5V, got {v1}");
}

// ---------------------------------------------------------------------------
// .meas — post-simulation measurement
// ---------------------------------------------------------------------------

/// Verify that `.meas` directives produce measurement results after simulation.
#[test]
fn cirq_meas_max_in_transient() {
    // Use SPICE path since Cirq grammar doesn't have .meas syntax yet,
    // but the IR→netlist→simulator pipeline supports it.
    let spice = "\
Meas Test
V1 in 0 DC 0 PULSE(0 5 0 1n 1n 25n 50n)
R1 in out 1k
C1 out 0 1n
.tran 1n 100n
.meas tran vout_max MAX v(out)
.meas tran vout_avg AVG v(out)
.end
";

    let netlists = thevenin_types::Netlist::parse(spice).expect("SPICE parse");
    let nl = &netlists[0];

    // Verify .meas items are in the netlist.
    let meas_count = nl
        .items
        .iter()
        .filter(|i| matches!(i, thevenin_types::Item::Meas(_)))
        .count();
    assert_eq!(meas_count, 2, "should have 2 .meas directives");

    let result = thevenin::simulate(nl).expect("simulation should succeed");

    // Check that a "measurements" plot was added.
    let meas_plot = result.plots.iter().find(|p| p.name == "measurements");
    assert!(
        meas_plot.is_some(),
        "result should contain a 'measurements' plot"
    );

    let meas = meas_plot.unwrap();

    // vout_max should be > 0 (the pulse drives the RC filter).
    let vout_max = meas.vector("vout_max");
    assert!(vout_max.is_some(), "should have vout_max measurement");
    let max_val = vout_max.unwrap().data.as_real()[0];
    assert!(
        max_val > 0.1,
        "vout_max should be positive (pulse-driven RC), got {max_val}"
    );

    // vout_avg should be between 0 and max.
    let vout_avg = meas.vector("vout_avg");
    assert!(vout_avg.is_some(), "should have vout_avg measurement");
    let avg_val = vout_avg.unwrap().data.as_real()[0];
    assert!(
        avg_val > 0.0 && avg_val <= max_val,
        "vout_avg ({avg_val}) should be between 0 and max ({max_val})"
    );
}

/// Verify .meas round-trips through SPICE → IR → netlist → simulate.
#[test]
fn spice_meas_round_trip_through_ir() {
    let spice = "\
Meas Round-Trip
V1 in 0 5
R1 in out 1k
R2 out 0 1k
.op
.meas dc vout_val FIND v(out) AT=5
.end
";

    let netlists = thevenin_types::Netlist::parse(spice).expect("SPICE parse");
    let nl = &netlists[0];

    // Import to IR.
    let ir = cirq_spice_import::import_netlist(nl).expect("import");
    assert!(!ir.measures.is_empty(), "IR should have measure specs");

    // Convert back to netlist.
    let rt_netlists = cirq_frontend::to_netlist::circuit_to_netlists(&ir).expect("to_netlist");
    let rt_nl = &rt_netlists[0];

    // Verify .meas survived.
    let meas_count = rt_nl
        .items
        .iter()
        .filter(|i| matches!(i, thevenin_types::Item::Meas(_)))
        .count();
    assert!(meas_count > 0, "round-tripped netlist should have .meas");
}

// ---------------------------------------------------------------------------
// Multi-temperature simulation
// ---------------------------------------------------------------------------

/// Multiple `.temp` values should produce multiple plots (one per temperature).
#[test]
fn cirq_multi_temp_produces_multiple_plots() {
    let source = r#"
        circuit multi_temp {
            V1: vsource(in -> gnd, dc: 5)
            R1: resistor(in -> mid, 1000)
            R2: resistor(mid -> gnd, 1000)
            temp 25
            temp 50
            analysis op {}
        }
    "#;

    let netlists =
        cirq_frontend::compile_to_netlist(source).expect("compile_to_netlist should succeed");
    assert_eq!(netlists.len(), 1);

    // Verify multiple .temp items are in the netlist.
    let temp_count = netlists[0]
        .items
        .iter()
        .filter(|i| matches!(i, thevenin_types::Item::Temp(_)))
        .count();
    assert_eq!(temp_count, 2, "should have 2 .temp directives");

    let result = thevenin::simulate(&netlists[0]).expect("simulation should succeed");

    // Multi-temp produces one plot per temperature.
    assert!(
        result.plots.len() >= 2,
        "multi-temp should produce >= 2 plots, got {}",
        result.plots.len()
    );

    // Each plot should have a temperature-annotated name.
    assert!(
        result.plots[0].name.contains("temp"),
        "first plot name should contain 'temp': {}",
        result.plots[0].name
    );
    assert!(
        result.plots[1].name.contains("temp"),
        "second plot name should contain 'temp': {}",
        result.plots[1].name
    );

    // Both should produce the same voltage divider result (resistors are
    // temperature-independent at this level).
    let v1 = result.plots[0]
        .vector("v(mid)")
        .expect("plot 0 should have v(mid)")
        .data
        .as_real()[0];
    let v2 = result.plots[1]
        .vector("v(mid)")
        .expect("plot 1 should have v(mid)")
        .data
        .as_real()[0];
    assert!(
        (v1 - 2.5).abs() < 0.01,
        "v(mid) at temp1 should be 2.5V, got {v1}"
    );
    assert!(
        (v2 - 2.5).abs() < 0.01,
        "v(mid) at temp2 should be 2.5V, got {v2}"
    );
}

/// SPICE multi-temp round-trip: `.temp 25 50` → IR → netlist → simulate.
#[test]
fn spice_multi_temp_round_trip() {
    let spice = "\
Multi-Temp RT
V1 in 0 5
R1 in mid 1k
R2 mid 0 1k
.temp 25
.temp 100
.op
.end
";

    let netlists = thevenin_types::Netlist::parse(spice).expect("SPICE parse");
    let nl = &netlists[0];

    // Import to IR.
    let ir = cirq_spice_import::import_netlist(nl).expect("import");
    assert!(
        ir.temps.len() >= 2,
        "IR should have multiple temps, got {}",
        ir.temps.len()
    );

    // Convert back to netlist.
    let rt_netlists = cirq_frontend::to_netlist::circuit_to_netlists(&ir).expect("to_netlist");
    let rt_nl = &rt_netlists[0];

    let rt_temp_count = rt_nl
        .items
        .iter()
        .filter(|i| matches!(i, thevenin_types::Item::Temp(_)))
        .count();
    assert!(
        rt_temp_count >= 2,
        "round-tripped netlist should have >= 2 .temp, got {rt_temp_count}"
    );

    // Simulate the round-tripped netlist.
    let result = thevenin::simulate(rt_nl).expect("simulation should succeed");
    assert!(
        result.plots.len() >= 2,
        "multi-temp simulation should produce >= 2 plots, got {}",
        result.plots.len()
    );
}

// ---------------------------------------------------------------------------
// SPICE → IR → Netlist → Simulate: .ic round-trip
// ---------------------------------------------------------------------------

/// Verify .ic passes through the full SPICE → IR → netlist pipeline and the
/// simulator consumes it.
#[test]
fn spice_ic_round_trip_simulate() {
    let spice = "\
IC Round-Trip
V1 in 0 5
R1 in cap 1k
C1 cap 0 1n
.ic V(cap)=5.0
.tran 1n 50n
.end
";

    let netlists = thevenin_types::Netlist::parse(spice).expect("SPICE parse");
    let nl = &netlists[0];

    // Import to IR and back.
    let ir = cirq_spice_import::import_netlist(nl).expect("import");
    assert!(
        !ir.initial_conditions.is_empty(),
        "IR should have initial conditions"
    );

    let rt_netlists = cirq_frontend::to_netlist::circuit_to_netlists(&ir).expect("to_netlist");
    let rt_nl = &rt_netlists[0];

    // Verify .ic survived.
    let has_ic = rt_nl
        .items
        .iter()
        .any(|i| matches!(i, thevenin_types::Item::Ic(_)));
    assert!(has_ic, "round-tripped netlist should have .ic");

    // Simulate and verify the initial voltage is applied.
    let result = thevenin::simulate(rt_nl).expect("simulation should succeed");
    let vcap = result.vector("v(cap)").expect("should have v(cap)");
    let data = vcap.data.as_real();
    assert!(!data.is_empty());
    assert!(
        (data[0] - 5.0).abs() < 0.2,
        "v(cap) should start near 5V from .ic, got {}",
        data[0]
    );
}
