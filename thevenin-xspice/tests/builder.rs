//! Tests for the code model builder API and registry.

use thevenin_xspice::*;

#[test]
fn test_builder_simple_gain() {
    let model = CodeModelBuilder::new("my_gain")
        .port("in", PortDirection::In, PortType::Voltage)
        .port("out", PortDirection::Out, PortType::Current)
        .param_real("gain", 1.0)
        .build(|inputs, _state: &mut ()| {
            let v_in = inputs.port_values[0];
            let gain = inputs.params[0].as_real().unwrap_or(1.0);
            let mut out = CmOutputs::new();
            out.set_output(1, gain * v_in);
            out.set_partial(1, 0, gain);
            out
        });

    assert_eq!(model.name, "my_gain");
    assert_eq!(model.ports.len(), 2);
    assert_eq!(model.ports[0].name, "in");
    assert_eq!(model.ports[0].direction, PortDirection::In);
    assert_eq!(model.ports[0].port_type, PortType::Voltage);
    assert_eq!(model.ports[1].name, "out");
    assert_eq!(model.ports[1].direction, PortDirection::Out);
    assert_eq!(model.ports[1].port_type, PortType::Current);
    assert_eq!(model.params.len(), 1);
    assert_eq!(model.params[0].name, "gain");
    assert_eq!(model.params[0].param_type, ParamType::Real);
}

#[test]
fn test_builder_with_state() {
    let model = CodeModelBuilder::new("counter")
        .port("out", PortDirection::Out, PortType::Current)
        .state(|| 0u32)
        .build(|_inputs, count: &mut u32| {
            *count += 1;
            let mut out = CmOutputs::new();
            out.set_output(0, *count as f64);
            out
        });

    // Verify state factory works
    let state = model.create_state();
    assert_eq!(*state.downcast_ref::<u32>().unwrap(), 0);
}

#[test]
fn test_registry_register_and_lookup() {
    let mut registry = CodeModelRegistry::new();
    assert!(registry.is_empty());

    let model = CodeModelBuilder::new("d_gain")
        .port("in", PortDirection::In, PortType::Voltage)
        .port("out", PortDirection::Out, PortType::Current)
        .build(|_, _: &mut ()| CmOutputs::new());

    registry.register(model);
    assert_eq!(registry.len(), 1);
    assert!(!registry.is_empty());

    // Case-insensitive lookup
    assert!(registry.get("D_GAIN").is_some());
    assert!(registry.get("d_gain").is_some());
    assert!(registry.get("D_Gain").is_some());
    assert!(registry.get("nonexistent").is_none());
}

#[test]
fn test_registry_names() {
    let mut registry = CodeModelRegistry::new();

    registry.register(CodeModelBuilder::new("alpha").build(|_, _: &mut ()| CmOutputs::new()));
    registry.register(CodeModelBuilder::new("beta").build(|_, _: &mut ()| CmOutputs::new()));

    let names: Vec<&str> = registry.names().collect();
    assert_eq!(names, vec!["ALPHA", "BETA"]);
}

#[test]
fn test_param_value_accessors() {
    assert_eq!(ParamValue::Real(3.14).as_real(), Some(3.14));
    assert_eq!(ParamValue::Real(3.14).as_integer(), None);

    assert_eq!(ParamValue::Integer(42).as_integer(), Some(42));
    assert_eq!(ParamValue::Integer(42).as_real(), None);

    assert_eq!(ParamValue::Boolean(true).as_boolean(), Some(true));
    assert_eq!(ParamValue::Boolean(false).as_boolean(), Some(false));

    assert_eq!(ParamValue::String("hello".into()).as_str(), Some("hello"));
}

#[test]
fn test_builder_multiple_params() {
    let model = CodeModelBuilder::new("test")
        .param_real("r_val", 100.0)
        .param_integer("count", 5)
        .param_boolean("enable", true)
        .build(|_, _: &mut ()| CmOutputs::new());

    assert_eq!(model.params.len(), 3);
    assert_eq!(model.params[0].default.as_real(), Some(100.0));
    assert_eq!(model.params[1].default.as_integer(), Some(5));
    assert_eq!(model.params[2].default.as_boolean(), Some(true));
}

#[test]
fn test_cm_outputs_builder() {
    let mut out = CmOutputs::new();
    out.set_output(0, 1.5);
    out.set_output(1, -0.3);
    out.set_partial(1, 0, 2.0);

    assert_eq!(out.port_outputs.len(), 2);
    assert_eq!(out.port_outputs[0].port_index, 0);
    assert_eq!(out.port_outputs[0].value, 1.5);
    assert_eq!(out.partials.len(), 1);
    assert_eq!(out.partials[0].output_port, 1);
    assert_eq!(out.partials[0].input_port, 0);
    assert_eq!(out.partials[0].value, 2.0);
}

/// Test: registering two models with the same name overwrites the first.
#[test]
fn test_registry_overwrite() {
    let mut registry = CodeModelRegistry::new();

    let model1 = CodeModelBuilder::new("dup")
        .param_real("r", 1.0)
        .build(|_, _: &mut ()| CmOutputs::new());

    let model2 = CodeModelBuilder::new("dup")
        .param_real("r", 2.0)
        .param_real("extra", 3.0)
        .build(|_, _: &mut ()| CmOutputs::new());

    registry.register(model1);
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.get("dup").unwrap().params.len(), 1);

    registry.register(model2);
    assert_eq!(registry.len(), 1); // Still one entry, not two
    assert_eq!(registry.get("dup").unwrap().params.len(), 2); // Updated to second model
}

/// Test: registry lookup is case-insensitive across various cases.
#[test]
fn test_registry_case_insensitivity() {
    let mut registry = CodeModelRegistry::new();

    let model = CodeModelBuilder::new("myModel")
        .port("p", PortDirection::In, PortType::Voltage)
        .build(|_, _: &mut ()| CmOutputs::new());

    registry.register(model);

    // All case variations should resolve to the same model
    assert!(registry.get("MYMODEL").is_some());
    assert!(registry.get("mymodel").is_some());
    assert!(registry.get("MyModel").is_some());
    assert!(registry.get("myModel").is_some());
    assert!(registry.get("MYMODEL").unwrap().ports.len() == 1);
}

/// Test: builder with no ports (edge case).
#[test]
fn test_builder_no_ports() {
    let model = CodeModelBuilder::new("no_ports")
        .param_real("value", 42.0)
        .build(|_, _: &mut ()| {
            let mut out = CmOutputs::new();
            out.set_output(0, 42.0);
            out
        });

    assert_eq!(model.name, "no_ports");
    assert!(model.ports.is_empty());
    assert_eq!(model.params.len(), 1);
}

/// Test: empty CmOutputs is truly empty.
#[test]
fn test_cm_outputs_empty() {
    let out = CmOutputs::new();
    assert!(out.port_outputs.is_empty());
    assert!(out.partials.is_empty());
}

/// Test: CmOutputs default() also produces an empty result.
#[test]
fn test_cm_outputs_default() {
    let out = CmOutputs::default();
    assert!(out.port_outputs.is_empty());
    assert!(out.partials.is_empty());
}

/// Test: CodeModelDef evaluate — build a model, create state, call evaluate.
#[test]
fn test_code_model_def_evaluate() {
    let model = CodeModelBuilder::new("test_eval")
        .port("in", PortDirection::In, PortType::Voltage)
        .port("out", PortDirection::Out, PortType::Current)
        .param_real("gain", 2.0)
        .build(|inputs, _state: &mut ()| {
            let v_in = inputs.port_values[0];
            let gain = inputs.params[0].as_real().unwrap_or(1.0);
            let mut out = CmOutputs::new();
            out.set_output(1, gain * v_in);
            out.set_partial(1, 0, gain);
            out
        });

    let mut state = model.create_state();
    let inputs = CmInputs {
        port_values: &[3.0, 0.0],
        params: &[ParamValue::Real(2.0)],
        mode: AnalysisMode::DcOp,
    };

    let result = model.evaluate(&inputs, state.as_mut());

    assert_eq!(result.port_outputs.len(), 1);
    assert_eq!(result.port_outputs[0].port_index, 1);
    assert!((result.port_outputs[0].value - 6.0).abs() < 1e-12); // 2.0 * 3.0
    assert_eq!(result.partials.len(), 1);
    assert!((result.partials[0].value - 2.0).abs() < 1e-12);
}

/// Test: CodeModelDef with stateful evaluation — state persists across calls.
#[test]
fn test_code_model_def_stateful_evaluate() {
    let model = CodeModelBuilder::new("counter")
        .port("out", PortDirection::Out, PortType::Current)
        .state(|| 0u32)
        .build(|_inputs, count: &mut u32| {
            *count += 1;
            let mut out = CmOutputs::new();
            out.set_output(0, *count as f64);
            out
        });

    let mut state = model.create_state();
    let inputs = CmInputs {
        port_values: &[0.0],
        params: &[],
        mode: AnalysisMode::DcOp,
    };

    let result1 = model.evaluate(&inputs, state.as_mut());
    assert!((result1.port_outputs[0].value - 1.0).abs() < 1e-12);

    let result2 = model.evaluate(&inputs, state.as_mut());
    assert!((result2.port_outputs[0].value - 2.0).abs() < 1e-12);

    let result3 = model.evaluate(&inputs, state.as_mut());
    assert!((result3.port_outputs[0].value - 3.0).abs() < 1e-12);
}

/// Test: ParamValue cross-type access returns None.
#[test]
fn test_param_value_cross_type_returns_none() {
    let real = ParamValue::Real(1.0);
    assert!(real.as_integer().is_none());
    assert!(real.as_boolean().is_none());
    assert!(real.as_str().is_none());

    let integer = ParamValue::Integer(1);
    assert!(integer.as_real().is_none());
    assert!(integer.as_boolean().is_none());
    assert!(integer.as_str().is_none());

    let boolean = ParamValue::Boolean(true);
    assert!(boolean.as_real().is_none());
    assert!(boolean.as_integer().is_none());
    assert!(boolean.as_str().is_none());

    let string = ParamValue::String("test".into());
    assert!(string.as_real().is_none());
    assert!(string.as_integer().is_none());
    assert!(string.as_boolean().is_none());
}
