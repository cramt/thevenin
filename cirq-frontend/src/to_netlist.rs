//! Cirq IR to Thevenin Netlist adapter.
//!
//! Converts a [`cirq_ir::Circuit`] into one or more [`thevenin_types::Netlist`]
//! values ready for the simulator. Each analysis in the circuit produces a
//! separate netlist (same items, different analysis command).

use std::collections::HashMap;

use cirq_ir::{
    AcSpec as IrAcSpec, Circuit, DeviceType, Element as IrElement, ElementKind as IrElementKind,
    FrequencyScale, Id, Value, Waveform as IrWaveform,
};
use thevenin_types::{
    AcSpec, AcVariation, Analysis, DcSweep, Element, ElementKind, Expr, Item, ModelDef, Netlist,
    Param, PwlPoint, PzAnalysisType, PzInputType, Source, Waveform,
};

/// Errors that can occur during IR-to-Netlist conversion.
#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("missing connection terminal `{terminal}` on element `{element}`")]
    MissingTerminal { element: String, terminal: String },

    #[error("element `{0}` has no value parameter")]
    MissingValue(String),

    #[error("element `{0}` references unknown model")]
    UnknownModel(String),

    #[error("element `{0}` references unknown element for controlled source")]
    UnknownElement(String),
}

/// Convert a [`cirq_ir::Circuit`] into a vector of [`Netlist`] values.
///
/// Produces one netlist per analysis. If the circuit has no analyses, a single
/// netlist with `.op` is returned.
pub fn circuit_to_netlists(circuit: &Circuit) -> Result<Vec<Netlist>, ConvertError> {
    let net_names = build_net_map(circuit);
    let element_names = build_element_name_map(circuit);
    let model_names = build_model_name_map(circuit);

    // Build circuit items (shared across all analyses).
    let mut items = Vec::new();

    // Global nets.
    let globals: Vec<String> = circuit
        .nets
        .iter()
        .filter(|n| n.is_global && n.name != "gnd")
        .map(|n| n.name.clone())
        .collect();
    if !globals.is_empty() {
        items.push(Item::Global(globals));
    }

    // Parameters.
    if !circuit.params.is_empty() {
        let params: Vec<Param> = circuit
            .params
            .iter()
            .map(|p| Param {
                name: p.name.clone(),
                value: value_to_expr(&p.value),
            })
            .collect();
        items.push(Item::Param(params));
    }

    // Models.
    for model in &circuit.models {
        items.push(Item::Model(convert_model(model)));
    }

    // Elements.
    for elem in &circuit.elements {
        let converted = convert_element(elem, &net_names, &element_names, &model_names, circuit)?;
        items.push(Item::Element(converted));
    }

    // Options.
    if !circuit.options.is_empty() {
        let params: Vec<Param> = circuit
            .options
            .iter()
            .map(|o| Param {
                name: o.0.clone(),
                value: value_to_expr(&o.1),
            })
            .collect();
        items.push(Item::Options(params));
    }

    // Temperature.
    if let Some(temp) = circuit.temp {
        items.push(Item::Temp(temp));
    }

    // Save targets.
    if !circuit.save.is_empty() {
        items.push(Item::Save(circuit.save.clone()));
    }

    // Build analyses.
    let analyses: Vec<Analysis> = if circuit.analyses.is_empty() {
        vec![Analysis::Op]
    } else {
        circuit
            .analyses
            .iter()
            .map(|a| convert_analysis(a, &net_names, &element_names))
            .collect()
    };

    let netlists = analyses
        .into_iter()
        .map(|analysis| Netlist {
            title: circuit.name.clone(),
            items: items.clone(),
            analysis,
            source: String::new(),
        })
        .collect();

    Ok(netlists)
}

// ---------------------------------------------------------------------------
// Lookup maps
// ---------------------------------------------------------------------------

fn build_net_map(circuit: &Circuit) -> HashMap<Id, String> {
    let mut map = HashMap::new();
    for net in &circuit.nets {
        let name = if net.name == "gnd" {
            "0".to_string()
        } else {
            net.name.clone()
        };
        map.insert(net.id, name);
    }
    map
}

fn build_element_name_map(circuit: &Circuit) -> HashMap<Id, String> {
    let mut map = HashMap::new();
    for elem in &circuit.elements {
        map.insert(elem.id, elem.name.clone());
    }
    map
}

fn build_model_name_map(circuit: &Circuit) -> HashMap<Id, String> {
    let mut map = HashMap::new();
    for model in &circuit.models {
        map.insert(model.id, model.name.clone());
    }
    map
}

// ---------------------------------------------------------------------------
// Value / Expr conversion
// ---------------------------------------------------------------------------

fn value_to_expr(val: &Value) -> Expr {
    match val {
        Value::Real(f) => Expr::Num(*f),
        Value::Integer(i) => Expr::Num(*i as f64),
        Value::Bool(b) => Expr::Num(if *b { 1.0 } else { 0.0 }),
        Value::String(s) => Expr::Param(s.clone()),
    }
}

fn value_to_f64(val: &Value) -> f64 {
    match val {
        Value::Real(f) => *f,
        Value::Integer(i) => *i as f64,
        Value::Bool(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        Value::String(_) => 0.0,
    }
}

// ---------------------------------------------------------------------------
// SPICE element naming
// ---------------------------------------------------------------------------

/// Build the SPICE element name with the traditional prefix letter.
fn spice_name(cirq_name: &str, prefix: char) -> String {
    let first = cirq_name
        .chars()
        .next()
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or(prefix.to_ascii_uppercase());
    if first == prefix.to_ascii_uppercase() {
        cirq_name.to_string()
    } else {
        format!("{}{}", prefix.to_ascii_uppercase(), cirq_name)
    }
}

// ---------------------------------------------------------------------------
// Connection lookup helper
// ---------------------------------------------------------------------------

fn get_conn(
    elem: &IrElement,
    terminal: &str,
    net_names: &HashMap<Id, String>,
) -> Result<String, ConvertError> {
    elem.connections
        .iter()
        .find(|c| c.terminal == terminal)
        .and_then(|c| net_names.get(&c.net))
        .cloned()
        .ok_or_else(|| ConvertError::MissingTerminal {
            element: elem.name.clone(),
            terminal: terminal.to_string(),
        })
}

fn get_param_value(elem: &IrElement) -> Option<Expr> {
    elem.params
        .iter()
        .find(|p| p.0 == "value")
        .map(|p| value_to_expr(&p.1))
}

fn get_param_f64(elem: &IrElement, name: &str) -> Option<f64> {
    elem.params
        .iter()
        .find(|p| p.0 == name)
        .map(|p| value_to_f64(&p.1))
}

fn extra_params(elem: &IrElement, exclude: &[&str]) -> Vec<Param> {
    elem.params
        .iter()
        .filter(|p| !exclude.contains(&p.0.as_str()))
        .map(|p| Param {
            name: p.0.clone(),
            value: value_to_expr(&p.1),
        })
        .collect()
}

fn convert_waveform(w: &IrWaveform) -> Waveform {
    match w {
        IrWaveform::Pulse {
            v1,
            v2,
            td,
            tr,
            tf,
            pw,
            per,
        } => Waveform::Pulse {
            v1: Expr::Num(*v1),
            v2: Expr::Num(*v2),
            td: td.map(Expr::Num),
            tr: tr.map(Expr::Num),
            tf: tf.map(Expr::Num),
            pw: pw.map(Expr::Num),
            per: per.map(Expr::Num),
        },
        IrWaveform::Sin {
            v0,
            va,
            freq,
            td,
            theta,
            phi,
        } => Waveform::Sin {
            v0: Expr::Num(*v0),
            va: Expr::Num(*va),
            freq: freq.map(Expr::Num),
            td: td.map(Expr::Num),
            theta: theta.map(Expr::Num),
            phi: phi.map(Expr::Num),
        },
        IrWaveform::Exp {
            v1,
            v2,
            td1,
            tau1,
            td2,
            tau2,
        } => Waveform::Exp {
            v1: Expr::Num(*v1),
            v2: Expr::Num(*v2),
            td1: td1.map(Expr::Num),
            tau1: tau1.map(Expr::Num),
            td2: td2.map(Expr::Num),
            tau2: tau2.map(Expr::Num),
        },
        IrWaveform::Pwl(points) => Waveform::Pwl(
            points
                .iter()
                .map(|(t, v)| PwlPoint {
                    time: Expr::Num(*t),
                    value: Expr::Num(*v),
                })
                .collect(),
        ),
        IrWaveform::Sffm { v0, va, fc, fs, md } => Waveform::Sffm {
            v0: Expr::Num(*v0),
            va: Expr::Num(*va),
            fc: fc.map(Expr::Num),
            fs: fs.map(Expr::Num),
            md: md.map(Expr::Num),
        },
        IrWaveform::Am { va, vo, fc, fs, td } => Waveform::Am {
            va: Expr::Num(*va),
            vo: Expr::Num(*vo),
            fc: Expr::Num(*fc),
            fs: Expr::Num(*fs),
            td: td.map(Expr::Num),
        },
    }
}

fn convert_ac_spec(ac: &IrAcSpec) -> AcSpec {
    AcSpec {
        mag: Expr::Num(ac.mag),
        phase: if ac.phase != 0.0 {
            Some(Expr::Num(ac.phase))
        } else {
            None
        },
    }
}

fn convert_source_spec(elem: &IrElement) -> Source {
    if let Some(spec) = &elem.source_spec {
        Source {
            dc: spec.dc.map(Expr::Num),
            ac: spec.ac.as_ref().map(convert_ac_spec),
            waveform: spec.waveform.as_ref().map(convert_waveform),
        }
    } else {
        let dc = elem
            .params
            .iter()
            .find(|p| p.0 == "value" || p.0 == "dc")
            .map(|p| value_to_expr(&p.1));
        Source {
            dc,
            ac: None,
            waveform: None,
        }
    }
}

fn resolve_model_name(
    elem: &IrElement,
    model_names: &HashMap<Id, String>,
) -> Result<String, ConvertError> {
    match elem.model {
        Some(mid) => model_names
            .get(&mid)
            .cloned()
            .ok_or_else(|| ConvertError::UnknownModel(elem.name.clone())),
        None => Err(ConvertError::UnknownModel(elem.name.clone())),
    }
}

// ---------------------------------------------------------------------------
// Element conversion
// ---------------------------------------------------------------------------

fn convert_element(
    elem: &IrElement,
    net_names: &HashMap<Id, String>,
    element_names: &HashMap<Id, String>,
    model_names: &HashMap<Id, String>,
    circuit: &Circuit,
) -> Result<Element, ConvertError> {
    match &elem.kind {
        IrElementKind::Resistor => {
            let pos = get_conn(elem, "pos", net_names)?;
            let neg = get_conn(elem, "neg", net_names)?;
            let value = get_param_value(elem)
                .ok_or_else(|| ConvertError::MissingValue(elem.name.clone()))?;
            let params = extra_params(elem, &["value"]);
            Ok(Element {
                name: spice_name(&elem.name, 'R'),
                kind: ElementKind::Resistor {
                    pos,
                    neg,
                    value,
                    params,
                },
            })
        }

        IrElementKind::Capacitor => {
            let pos = get_conn(elem, "pos", net_names)?;
            let neg = get_conn(elem, "neg", net_names)?;
            let value = get_param_value(elem)
                .ok_or_else(|| ConvertError::MissingValue(elem.name.clone()))?;
            let params = extra_params(elem, &["value"]);
            Ok(Element {
                name: spice_name(&elem.name, 'C'),
                kind: ElementKind::Capacitor {
                    pos,
                    neg,
                    value,
                    params,
                },
            })
        }

        IrElementKind::Inductor => {
            let pos = get_conn(elem, "pos", net_names)?;
            let neg = get_conn(elem, "neg", net_names)?;
            let value = get_param_value(elem)
                .ok_or_else(|| ConvertError::MissingValue(elem.name.clone()))?;
            let params = extra_params(elem, &["value"]);
            Ok(Element {
                name: spice_name(&elem.name, 'L'),
                kind: ElementKind::Inductor {
                    pos,
                    neg,
                    value,
                    params,
                },
            })
        }

        IrElementKind::VoltageSource => {
            let pos = get_conn(elem, "pos", net_names)?;
            let neg = get_conn(elem, "neg", net_names)?;
            let source = convert_source_spec(elem);
            Ok(Element {
                name: spice_name(&elem.name, 'V'),
                kind: ElementKind::VoltageSource { pos, neg, source },
            })
        }

        IrElementKind::CurrentSource => {
            let pos = get_conn(elem, "pos", net_names)?;
            let neg = get_conn(elem, "neg", net_names)?;
            let source = convert_source_spec(elem);
            Ok(Element {
                name: spice_name(&elem.name, 'I'),
                kind: ElementKind::CurrentSource { pos, neg, source },
            })
        }

        IrElementKind::Diode => {
            let anode = get_conn(elem, "anode", net_names)?;
            let cathode = get_conn(elem, "cathode", net_names)?;
            let model = resolve_model_name(elem, model_names)?;
            let params = extra_params(elem, &["value"]);
            Ok(Element {
                name: spice_name(&elem.name, 'D'),
                kind: ElementKind::Diode {
                    anode,
                    cathode,
                    model,
                    params,
                },
            })
        }

        IrElementKind::Npn | IrElementKind::Pnp => {
            let c = get_conn(elem, "collector", net_names)?;
            let b = get_conn(elem, "base", net_names)?;
            let e = get_conn(elem, "emitter", net_names)?;
            let model = resolve_model_name(elem, model_names)?;
            let params = extra_params(elem, &["value"]);
            Ok(Element {
                name: spice_name(&elem.name, 'Q'),
                kind: ElementKind::Bjt {
                    c,
                    b,
                    e,
                    substrate: None,
                    model,
                    params,
                    off: false,
                },
            })
        }

        IrElementKind::Nmos | IrElementKind::Pmos => {
            let d = get_conn(elem, "drain", net_names)?;
            let g = get_conn(elem, "gate", net_names)?;
            let s = get_conn(elem, "source", net_names)?;
            let bulk = get_conn(elem, "bulk", net_names)?;
            let model = resolve_model_name(elem, model_names)?;
            let params = extra_params(elem, &["value"]);
            Ok(Element {
                name: spice_name(&elem.name, 'M'),
                kind: ElementKind::Mosfet {
                    d,
                    g,
                    s,
                    bulk,
                    body: None,
                    model,
                    params,
                },
            })
        }

        IrElementKind::NJfet | IrElementKind::PJfet => {
            let d = get_conn(elem, "drain", net_names)?;
            let g = get_conn(elem, "gate", net_names)?;
            let s = get_conn(elem, "source", net_names)?;
            let model = resolve_model_name(elem, model_names)?;
            let params = extra_params(elem, &["value"]);
            Ok(Element {
                name: spice_name(&elem.name, 'J'),
                kind: ElementKind::Jfet {
                    d,
                    g,
                    s,
                    model,
                    params,
                },
            })
        }

        IrElementKind::NMesfet | IrElementKind::PMesfet => {
            let d = get_conn(elem, "drain", net_names)?;
            let g = get_conn(elem, "gate", net_names)?;
            let s = get_conn(elem, "source", net_names)?;
            let model = resolve_model_name(elem, model_names)?;
            let params = extra_params(elem, &["value"]);
            Ok(Element {
                name: spice_name(&elem.name, 'Z'),
                kind: ElementKind::Mesa {
                    d,
                    g,
                    s,
                    model,
                    params,
                },
            })
        }

        IrElementKind::Vcvs => {
            let out_pos = get_conn(elem, "out_pos", net_names)?;
            let out_neg = get_conn(elem, "out_neg", net_names)?;
            let in_pos = get_conn(elem, "in_pos", net_names)?;
            let in_neg = get_conn(elem, "in_neg", net_names)?;
            let gain = get_param_value(elem)
                .or_else(|| {
                    elem.params
                        .iter()
                        .find(|p| p.0 == "gain")
                        .map(|p| value_to_expr(&p.1))
                })
                .ok_or_else(|| ConvertError::MissingValue(elem.name.clone()))?;
            Ok(Element {
                name: spice_name(&elem.name, 'E'),
                kind: ElementKind::Vcvs {
                    out_pos,
                    out_neg,
                    in_pos,
                    in_neg,
                    gain,
                },
            })
        }

        IrElementKind::Vccs => {
            let out_pos = get_conn(elem, "out_pos", net_names)?;
            let out_neg = get_conn(elem, "out_neg", net_names)?;
            let in_pos = get_conn(elem, "in_pos", net_names)?;
            let in_neg = get_conn(elem, "in_neg", net_names)?;
            let gm = get_param_value(elem)
                .or_else(|| {
                    elem.params
                        .iter()
                        .find(|p| p.0 == "gm")
                        .map(|p| value_to_expr(&p.1))
                })
                .ok_or_else(|| ConvertError::MissingValue(elem.name.clone()))?;
            Ok(Element {
                name: spice_name(&elem.name, 'G'),
                kind: ElementKind::Vccs {
                    out_pos,
                    out_neg,
                    in_pos,
                    in_neg,
                    gm,
                },
            })
        }

        IrElementKind::Ccvs => {
            let out_pos = get_conn(elem, "out_pos", net_names)?;
            let out_neg = get_conn(elem, "out_neg", net_names)?;
            let vsrc = elem
                .params
                .iter()
                .find(|p| p.0 == "vsrc")
                .map(|p| match &p.1 {
                    Value::String(s) => s.clone(),
                    _ => format!("{}", value_to_f64(&p.1)),
                })
                .ok_or_else(|| ConvertError::MissingValue(elem.name.clone()))?;
            let rm = elem
                .params
                .iter()
                .find(|p| p.0 == "rm" || p.0 == "value")
                .map(|p| value_to_expr(&p.1))
                .ok_or_else(|| ConvertError::MissingValue(elem.name.clone()))?;
            Ok(Element {
                name: spice_name(&elem.name, 'H'),
                kind: ElementKind::Ccvs {
                    out_pos,
                    out_neg,
                    vsrc,
                    rm,
                },
            })
        }

        IrElementKind::Cccs => {
            let out_pos = get_conn(elem, "out_pos", net_names)?;
            let out_neg = get_conn(elem, "out_neg", net_names)?;
            let vsrc = elem
                .params
                .iter()
                .find(|p| p.0 == "vsrc")
                .map(|p| match &p.1 {
                    Value::String(s) => s.clone(),
                    _ => format!("{}", value_to_f64(&p.1)),
                })
                .ok_or_else(|| ConvertError::MissingValue(elem.name.clone()))?;
            let gain = elem
                .params
                .iter()
                .find(|p| p.0 == "gain" || p.0 == "value")
                .map(|p| value_to_expr(&p.1))
                .ok_or_else(|| ConvertError::MissingValue(elem.name.clone()))?;
            Ok(Element {
                name: spice_name(&elem.name, 'F'),
                kind: ElementKind::Cccs {
                    out_pos,
                    out_neg,
                    vsrc,
                    gain,
                },
            })
        }

        IrElementKind::TransmissionLine => {
            let in_pos = get_conn(elem, "in_pos", net_names)?;
            let in_neg = get_conn(elem, "in_neg", net_names)?;
            let out_pos = get_conn(elem, "out_pos", net_names)?;
            let out_neg = get_conn(elem, "out_neg", net_names)?;
            let model = resolve_model_name(elem, model_names).unwrap_or_default();
            let params = extra_params(elem, &["value"]);
            Ok(Element {
                name: spice_name(&elem.name, 'O'),
                kind: ElementKind::Ltra {
                    pos1: in_pos,
                    neg1: in_neg,
                    pos2: out_pos,
                    neg2: out_neg,
                    model,
                    params,
                },
            })
        }

        IrElementKind::Coupling => {
            // Coupling references two inductor names and a coupling coefficient.
            // The Cirq IR stores these as params: l1, l2, coupling.
            let l1 = elem
                .params
                .iter()
                .find(|p| p.0 == "l1")
                .map(|p| match &p.1 {
                    Value::String(s) => s.clone(),
                    _ => format!("{}", value_to_f64(&p.1)),
                })
                .ok_or_else(|| ConvertError::MissingValue(elem.name.clone()))?;
            let l2 = elem
                .params
                .iter()
                .find(|p| p.0 == "l2")
                .map(|p| match &p.1 {
                    Value::String(s) => s.clone(),
                    _ => format!("{}", value_to_f64(&p.1)),
                })
                .ok_or_else(|| ConvertError::MissingValue(elem.name.clone()))?;
            let coupling = get_param_f64(elem, "coupling")
                .or_else(|| get_param_f64(elem, "value"))
                .map(Expr::Num)
                .ok_or_else(|| ConvertError::MissingValue(elem.name.clone()))?;

            // Resolve inductor SPICE names: find the elements by name and apply
            // the 'L' prefix rule.
            let l1_spice = resolve_inductor_spice_name(&l1, element_names, circuit);
            let l2_spice = resolve_inductor_spice_name(&l2, element_names, circuit);

            Ok(Element {
                name: spice_name(&elem.name, 'K'),
                kind: ElementKind::MutualCoupling {
                    l1: l1_spice,
                    l2: l2_spice,
                    coupling,
                },
            })
        }
    }
}

/// Resolve an inductor name from Cirq to its SPICE name.
fn resolve_inductor_spice_name(
    name: &str,
    _element_names: &HashMap<Id, String>,
    _circuit: &Circuit,
) -> String {
    spice_name(name, 'L')
}

// ---------------------------------------------------------------------------
// Model conversion
// ---------------------------------------------------------------------------

fn convert_model(model: &cirq_ir::Model) -> ModelDef {
    let kind = match model.device_type {
        DeviceType::Diode => "D",
        DeviceType::Npn => "NPN",
        DeviceType::Pnp => "PNP",
        DeviceType::Nmos => "NMOS",
        DeviceType::Pmos => "PMOS",
        DeviceType::NJfet => "NJF",
        DeviceType::PJfet => "PJF",
        DeviceType::NMesfet => "NMF",
        DeviceType::PMesfet => "PMF",
    };

    let params = model
        .params
        .iter()
        .map(|p| Param {
            name: p.0.clone(),
            value: value_to_expr(&p.1),
        })
        .collect();

    ModelDef {
        name: model.name.clone(),
        kind: kind.to_string(),
        params,
    }
}

// ---------------------------------------------------------------------------
// Analysis conversion
// ---------------------------------------------------------------------------

fn convert_analysis(
    analysis: &cirq_ir::Analysis,
    net_names: &HashMap<Id, String>,
    element_names: &HashMap<Id, String>,
) -> Analysis {
    match analysis {
        cirq_ir::Analysis::Op => Analysis::Op,

        cirq_ir::Analysis::Dc(dc) => {
            if dc.sweeps.is_empty() {
                return Analysis::Op;
            }
            let first = &dc.sweeps[0];
            let src = element_names
                .get(&first.source)
                .cloned()
                .unwrap_or_else(|| format!("V{}", first.source.0));
            let src2 = if dc.sweeps.len() > 1 {
                let s2 = &dc.sweeps[1];
                let src2_name = element_names
                    .get(&s2.source)
                    .cloned()
                    .unwrap_or_else(|| format!("V{}", s2.source.0));
                Some(DcSweep {
                    src: src2_name,
                    start: Expr::Num(s2.start),
                    stop: Expr::Num(s2.stop),
                    step: Expr::Num(s2.step),
                })
            } else {
                None
            };
            Analysis::Dc {
                src,
                start: Expr::Num(first.start),
                stop: Expr::Num(first.stop),
                step: Expr::Num(first.step),
                src2,
            }
        }

        cirq_ir::Analysis::Ac(ac) => {
            let variation = match ac.scale {
                FrequencyScale::Decade => AcVariation::Dec,
                FrequencyScale::Octave => AcVariation::Oct,
                FrequencyScale::Linear => AcVariation::Lin,
            };
            Analysis::Ac {
                variation,
                n: ac.points,
                fstart: Expr::Num(ac.start),
                fstop: Expr::Num(ac.stop),
            }
        }

        cirq_ir::Analysis::Tran(tran) => {
            let tstart = if tran.start != 0.0 {
                Some(Expr::Num(tran.start))
            } else {
                None
            };
            Analysis::Tran {
                tstep: Expr::Num(tran.step),
                tstop: Expr::Num(tran.stop),
                tstart,
                tmax: tran.tmax.map(Expr::Num),
            }
        }

        cirq_ir::Analysis::Noise(noise) => {
            let output = net_names
                .get(&noise.output_net)
                .cloned()
                .unwrap_or_else(|| "0".to_string());
            let ref_node = net_names.get(&noise.reference_net).cloned();
            let src = element_names
                .get(&noise.source)
                .cloned()
                .unwrap_or_else(|| format!("V{}", noise.source.0));
            let variation = match noise.scale {
                FrequencyScale::Decade => AcVariation::Dec,
                FrequencyScale::Octave => AcVariation::Oct,
                FrequencyScale::Linear => AcVariation::Lin,
            };
            Analysis::Noise {
                output,
                ref_node,
                src,
                variation,
                n: noise.points,
                fstart: Expr::Num(noise.start),
                fstop: Expr::Num(noise.stop),
            }
        }

        cirq_ir::Analysis::Pz(pz) => {
            let node_i = net_names
                .get(&pz.input_pos)
                .cloned()
                .unwrap_or_else(|| "0".to_string());
            let node_g = net_names
                .get(&pz.input_neg)
                .cloned()
                .unwrap_or_else(|| "0".to_string());
            let node_j = net_names
                .get(&pz.output_pos)
                .cloned()
                .unwrap_or_else(|| "0".to_string());
            let node_k = net_names
                .get(&pz.output_neg)
                .cloned()
                .unwrap_or_else(|| "0".to_string());
            let input_type = match pz.transfer {
                cirq_ir::TransferType::Voltage => PzInputType::Vol,
                cirq_ir::TransferType::Current => PzInputType::Cur,
            };
            let analysis_type = match pz.analysis_type {
                cirq_ir::PzType::Poles => PzAnalysisType::Pol,
                cirq_ir::PzType::Zeros => PzAnalysisType::Zer,
                cirq_ir::PzType::Both => PzAnalysisType::Pz,
            };
            Analysis::Pz {
                node_i,
                node_g,
                node_j,
                node_k,
                input_type,
                analysis_type,
            }
        }

        cirq_ir::Analysis::Sens(sens) => Analysis::Sens {
            output: vec![sens.output.clone()],
        },

        cirq_ir::Analysis::Tf(tf) => {
            let input = element_names
                .get(&tf.source)
                .cloned()
                .unwrap_or_else(|| format!("V{}", tf.source.0));
            Analysis::Tf {
                output: tf.output.clone(),
                input,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cirq_ir::{
        AcAnalysis, Connection, DcAnalysis, DcSweep as IrDcSweep, Element as IrElement,
        FrequencyScale, Id, Model, Net, ResolvedParam, TranAnalysis,
    };

    /// Helper: build a minimal circuit with the given elements, models,
    /// analyses, and nets.
    fn make_circuit(
        name: &str,
        nets: Vec<Net>,
        elements: Vec<IrElement>,
        models: Vec<Model>,
        analyses: Vec<cirq_ir::Analysis>,
        params: Vec<ResolvedParam>,
    ) -> Circuit {
        Circuit {
            name: name.to_string(),
            nets,
            elements,
            models,
            analyses,
            params,
            options: Vec::new(),
            temp: None,
            save: Vec::new(),
        }
    }

    fn net(id: u32, name: &str, is_global: bool) -> Net {
        Net {
            id: Id(id),
            name: name.to_string(),
            is_global,
        }
    }

    fn conn(terminal: &str, net_id: u32) -> Connection {
        Connection {
            terminal: terminal.to_string(),
            net: Id(net_id),
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: Simple resistor circuit
    // -----------------------------------------------------------------------
    #[test]
    fn simple_resistor_netlist() {
        let circuit = make_circuit(
            "resistor_test",
            vec![net(0, "gnd", true), net(1, "a", false), net(2, "b", false)],
            vec![IrElement {
                id: Id(0),
                name: "r1".to_string(),
                kind: IrElementKind::Resistor,
                connections: vec![conn("pos", 1), conn("neg", 2)],
                params: vec![("value".to_string(), Value::Real(1000.0))],
                model: None,
                source_spec: None,
            }],
            vec![],
            vec![cirq_ir::Analysis::Op],
            vec![],
        );

        let netlists = circuit_to_netlists(&circuit).unwrap();
        assert_eq!(netlists.len(), 1);

        let nl = &netlists[0];
        assert_eq!(nl.title, "resistor_test");
        assert!(matches!(nl.analysis, Analysis::Op));

        // Find the element.
        let elem = nl.items.iter().find_map(|i| {
            if let Item::Element(e) = i {
                Some(e)
            } else {
                None
            }
        });
        assert!(elem.is_some());
        let elem = elem.unwrap();
        // "r1" already starts with 'r' (case-insensitive match to 'R'), so
        // the name is kept as-is.
        assert_eq!(elem.name, "r1");
        match &elem.kind {
            ElementKind::Resistor {
                pos, neg, value, ..
            } => {
                assert_eq!(pos, "a");
                assert_eq!(neg, "b");
                assert!(matches!(value, Expr::Num(v) if (*v - 1000.0).abs() < 1e-6));
            }
            _ => panic!("expected Resistor element kind"),
        }
    }

    // -----------------------------------------------------------------------
    // Test 2: CMOS inverter with models
    // -----------------------------------------------------------------------
    #[test]
    fn cmos_inverter_with_models() {
        let circuit = make_circuit(
            "cmos_inv",
            vec![
                net(0, "gnd", true),
                net(1, "vdd", true),
                net(2, "in", false),
                net(3, "out", false),
            ],
            vec![
                IrElement {
                    id: Id(0),
                    name: "Mp".to_string(),
                    kind: IrElementKind::Pmos,
                    connections: vec![
                        conn("drain", 3),
                        conn("gate", 2),
                        conn("source", 1),
                        conn("bulk", 1),
                    ],
                    params: vec![],
                    model: Some(Id(0)),
                    source_spec: None,
                },
                IrElement {
                    id: Id(1),
                    name: "Mn".to_string(),
                    kind: IrElementKind::Nmos,
                    connections: vec![
                        conn("drain", 3),
                        conn("gate", 2),
                        conn("source", 0),
                        conn("bulk", 0),
                    ],
                    params: vec![],
                    model: Some(Id(1)),
                    source_spec: None,
                },
            ],
            vec![
                Model {
                    id: Id(0),
                    name: "pmod".to_string(),
                    device_type: DeviceType::Pmos,
                    params: vec![("vto".to_string(), Value::Real(-0.7))],
                },
                Model {
                    id: Id(1),
                    name: "nmod".to_string(),
                    device_type: DeviceType::Nmos,
                    params: vec![("vto".to_string(), Value::Real(0.7))],
                },
            ],
            vec![cirq_ir::Analysis::Op],
            vec![],
        );

        let netlists = circuit_to_netlists(&circuit).unwrap();
        assert_eq!(netlists.len(), 1);

        let nl = &netlists[0];

        // Check globals: vdd should be emitted (gnd is excluded since SPICE
        // ground "0" is implicit).
        let globals = nl.items.iter().find_map(|i| {
            if let Item::Global(g) = i {
                Some(g)
            } else {
                None
            }
        });
        assert!(globals.is_some());
        assert!(globals.unwrap().contains(&"vdd".to_string()));

        // Check models.
        let model_items: Vec<&ModelDef> = nl
            .items
            .iter()
            .filter_map(|i| {
                if let Item::Model(m) = i {
                    Some(m)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(model_items.len(), 2);
        assert_eq!(model_items[0].name, "pmod");
        assert_eq!(model_items[0].kind, "PMOS");
        assert_eq!(model_items[1].name, "nmod");
        assert_eq!(model_items[1].kind, "NMOS");

        // Check MOSFET elements.
        let elems: Vec<&Element> = nl
            .items
            .iter()
            .filter_map(|i| {
                if let Item::Element(e) = i {
                    Some(e)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(elems.len(), 2);
        assert_eq!(elems[0].name, "Mp");
        match &elems[0].kind {
            ElementKind::Mosfet {
                d,
                g,
                s,
                bulk,
                model,
                ..
            } => {
                assert_eq!(d, "out");
                assert_eq!(g, "in");
                assert_eq!(s, "vdd");
                assert_eq!(bulk, "vdd");
                assert_eq!(model, "pmod");
            }
            _ => panic!("expected Mosfet"),
        }
        assert_eq!(elems[1].name, "Mn");
        match &elems[1].kind {
            ElementKind::Mosfet {
                d,
                g,
                s,
                bulk,
                model,
                ..
            } => {
                assert_eq!(d, "out");
                assert_eq!(g, "in");
                assert_eq!(s, "0");
                assert_eq!(bulk, "0");
                assert_eq!(model, "nmod");
            }
            _ => panic!("expected Mosfet"),
        }
    }

    // -----------------------------------------------------------------------
    // Test 3: DC sweep analysis
    // -----------------------------------------------------------------------
    #[test]
    fn dc_sweep_analysis() {
        let circuit = make_circuit(
            "dc_test",
            vec![net(0, "gnd", true), net(1, "out", false)],
            vec![IrElement {
                id: Id(0),
                name: "V1".to_string(),
                kind: IrElementKind::VoltageSource,
                connections: vec![conn("pos", 1), conn("neg", 0)],
                params: vec![("dc".to_string(), Value::Real(5.0))],
                model: None,
                source_spec: None,
            }],
            vec![],
            vec![cirq_ir::Analysis::Dc(DcAnalysis {
                sweeps: vec![IrDcSweep {
                    source: Id(0),
                    start: 0.0,
                    stop: 5.0,
                    step: 0.1,
                }],
            })],
            vec![],
        );

        let netlists = circuit_to_netlists(&circuit).unwrap();
        assert_eq!(netlists.len(), 1);

        match &netlists[0].analysis {
            Analysis::Dc {
                src,
                start,
                stop,
                step,
                src2,
            } => {
                assert_eq!(src, "V1");
                assert!(matches!(start, Expr::Num(v) if *v == 0.0));
                assert!(matches!(stop, Expr::Num(v) if (*v - 5.0).abs() < 1e-6));
                assert!(matches!(step, Expr::Num(v) if (*v - 0.1).abs() < 1e-6));
                assert!(src2.is_none());
            }
            _ => panic!("expected DC analysis"),
        }
    }

    // -----------------------------------------------------------------------
    // Test 4: AC analysis
    // -----------------------------------------------------------------------
    #[test]
    fn ac_analysis_conversion() {
        let circuit = make_circuit(
            "ac_test",
            vec![net(0, "gnd", true)],
            vec![],
            vec![],
            vec![cirq_ir::Analysis::Ac(AcAnalysis {
                start: 1.0,
                stop: 1e9,
                points: 100,
                scale: FrequencyScale::Decade,
            })],
            vec![],
        );

        let netlists = circuit_to_netlists(&circuit).unwrap();
        assert_eq!(netlists.len(), 1);

        match &netlists[0].analysis {
            Analysis::Ac {
                variation,
                n,
                fstart,
                fstop,
            } => {
                assert_eq!(*variation, AcVariation::Dec);
                assert_eq!(*n, 100);
                assert!(matches!(fstart, Expr::Num(v) if (*v - 1.0).abs() < 1e-6));
                assert!(matches!(fstop, Expr::Num(v) if (*v - 1e9).abs() < 1.0));
            }
            _ => panic!("expected AC analysis"),
        }
    }

    // -----------------------------------------------------------------------
    // Test 5: Transient analysis
    // -----------------------------------------------------------------------
    #[test]
    fn tran_analysis_conversion() {
        let circuit = make_circuit(
            "tran_test",
            vec![net(0, "gnd", true)],
            vec![],
            vec![],
            vec![cirq_ir::Analysis::Tran(TranAnalysis {
                step: 1e-9,
                stop: 100e-9,
                start: 0.0,
                uic: false,
                tmax: None,
            })],
            vec![],
        );

        let netlists = circuit_to_netlists(&circuit).unwrap();
        assert_eq!(netlists.len(), 1);

        match &netlists[0].analysis {
            Analysis::Tran {
                tstep,
                tstop,
                tstart,
                tmax,
            } => {
                assert!(matches!(tstep, Expr::Num(v) if (*v - 1e-9).abs() < 1e-15));
                assert!(matches!(tstop, Expr::Num(v) if (*v - 100e-9).abs() < 1e-15));
                assert!(tstart.is_none());
                assert!(tmax.is_none());
            }
            _ => panic!("expected Tran analysis"),
        }
    }

    // -----------------------------------------------------------------------
    // Test 6: Global net handling
    // -----------------------------------------------------------------------
    #[test]
    fn global_net_handling() {
        let circuit = make_circuit(
            "global_test",
            vec![
                net(0, "gnd", true),
                net(1, "vdd", true),
                net(2, "vss", true),
                net(3, "sig", false),
            ],
            vec![],
            vec![],
            vec![],
            vec![],
        );

        let netlists = circuit_to_netlists(&circuit).unwrap();
        assert_eq!(netlists.len(), 1);

        let globals = netlists[0].items.iter().find_map(|i| {
            if let Item::Global(g) = i {
                Some(g)
            } else {
                None
            }
        });
        assert!(globals.is_some());
        let g = globals.unwrap();
        assert!(g.contains(&"vdd".to_string()));
        assert!(g.contains(&"vss".to_string()));
        // gnd should NOT be in globals (it maps to "0" implicitly).
        assert!(!g.contains(&"gnd".to_string()));
        // sig is not global.
        assert!(!g.contains(&"sig".to_string()));
    }

    // -----------------------------------------------------------------------
    // Test 7: Multiple analyses produce multiple netlists
    // -----------------------------------------------------------------------
    #[test]
    fn multiple_analyses() {
        let circuit = make_circuit(
            "multi",
            vec![net(0, "gnd", true), net(1, "out", false)],
            vec![IrElement {
                id: Id(0),
                name: "R1".to_string(),
                kind: IrElementKind::Resistor,
                connections: vec![conn("pos", 1), conn("neg", 0)],
                params: vec![("value".to_string(), Value::Real(100.0))],
                model: None,
                source_spec: None,
            }],
            vec![],
            vec![
                cirq_ir::Analysis::Op,
                cirq_ir::Analysis::Ac(AcAnalysis {
                    start: 1.0,
                    stop: 1e6,
                    points: 10,
                    scale: FrequencyScale::Linear,
                }),
            ],
            vec![],
        );

        let netlists = circuit_to_netlists(&circuit).unwrap();
        assert_eq!(netlists.len(), 2);
        assert!(matches!(netlists[0].analysis, Analysis::Op));
        assert!(matches!(netlists[1].analysis, Analysis::Ac { .. }));
        // Both share the same items.
        assert_eq!(netlists[0].items.len(), netlists[1].items.len());
    }

    // -----------------------------------------------------------------------
    // Test 8: No analyses defaults to Op
    // -----------------------------------------------------------------------
    #[test]
    fn no_analysis_defaults_to_op() {
        let circuit = make_circuit(
            "no_analysis",
            vec![net(0, "gnd", true)],
            vec![],
            vec![],
            vec![],
            vec![],
        );

        let netlists = circuit_to_netlists(&circuit).unwrap();
        assert_eq!(netlists.len(), 1);
        assert!(matches!(netlists[0].analysis, Analysis::Op));
    }

    // -----------------------------------------------------------------------
    // Test 9: SPICE naming prefix
    // -----------------------------------------------------------------------
    #[test]
    fn spice_naming_conventions() {
        // Already starts with correct letter.
        assert_eq!(spice_name("R1", 'R'), "R1");
        assert_eq!(spice_name("r1", 'R'), "r1");
        // Does not start with correct letter.
        assert_eq!(spice_name("myres", 'R'), "Rmyres");
        assert_eq!(spice_name("load", 'C'), "Cload");
        // MOSFET prefix.
        assert_eq!(spice_name("Mp", 'M'), "Mp");
        assert_eq!(spice_name("pfet", 'M'), "Mpfet");
    }

    // -----------------------------------------------------------------------
    // Test 10: Diode with model
    // -----------------------------------------------------------------------
    #[test]
    fn diode_with_model() {
        let circuit = make_circuit(
            "diode_test",
            vec![net(0, "gnd", true), net(1, "a", false)],
            vec![IrElement {
                id: Id(0),
                name: "D1".to_string(),
                kind: IrElementKind::Diode,
                connections: vec![conn("anode", 1), conn("cathode", 0)],
                params: vec![],
                model: Some(Id(0)),
                source_spec: None,
            }],
            vec![Model {
                id: Id(0),
                name: "d_model".to_string(),
                device_type: DeviceType::Diode,
                params: vec![("is".to_string(), Value::Real(1e-14))],
            }],
            vec![cirq_ir::Analysis::Op],
            vec![],
        );

        let netlists = circuit_to_netlists(&circuit).unwrap();
        let nl = &netlists[0];

        // Check model.
        let model_item = nl.items.iter().find_map(|i| {
            if let Item::Model(m) = i {
                Some(m)
            } else {
                None
            }
        });
        assert!(model_item.is_some());
        let m = model_item.unwrap();
        assert_eq!(m.name, "d_model");
        assert_eq!(m.kind, "D");

        // Check element.
        let elem = nl.items.iter().find_map(|i| {
            if let Item::Element(e) = i {
                Some(e)
            } else {
                None
            }
        });
        assert!(elem.is_some());
        let e = elem.unwrap();
        assert_eq!(e.name, "D1");
        match &e.kind {
            ElementKind::Diode {
                anode,
                cathode,
                model,
                ..
            } => {
                assert_eq!(anode, "a");
                assert_eq!(cathode, "0");
                assert_eq!(model, "d_model");
            }
            _ => panic!("expected Diode"),
        }
    }
}
