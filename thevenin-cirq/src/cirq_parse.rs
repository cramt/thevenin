//! Parse CirQ YAML/JSON documents into the circuit [`Circuit`] IR.
//!
//! Accepts both JSON and YAML input (auto-detected). The parsed document is
//! lowered into the same IR that the SPICE path produces, with domain inference
//! applied automatically.

use std::collections::BTreeMap;

use facet::Facet;

use crate::ir::*;

/// Dynamic value type for untyped YAML/JSON fields.
///
/// Replaces `DynVal` with an explicit enum of the variants CirQ
/// actually uses: booleans, numbers, strings, sequences, and mappings.
#[derive(Debug, Clone, Facet)]
#[facet(untagged)]
#[repr(u8)]
enum DynVal {
    Mapping(BTreeMap<String, DynVal>),
    Sequence(Vec<DynVal>),
    Number(f64),
    Bool(bool),
    String(String),
}

impl DynVal {
    fn as_str(&self) -> Option<&str> {
        match self {
            DynVal::String(s) => Some(s),
            _ => None,
        }
    }
}

/// Errors that can occur during CirQ parsing.
#[derive(Debug, thiserror::Error)]
pub enum CirqParseError {
    #[error("parse error: {0}")]
    Deserialize(#[from] facet_format::DeserializeError),
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("invalid component '{id}': {msg}")]
    InvalidComponent { id: String, msg: String },
    #[error("invalid value format: {0}")]
    InvalidValue(String),
    #[error("unknown direction '{0}' — expected input, output, inout, or passive")]
    UnknownDirection(String),
    #[error("unknown domain '{0}' — expected analog, digital, or mixed")]
    UnknownDomain(String),
    #[error("unknown waveform type '{0}'")]
    UnknownWaveformType(String),
    #[error("unsupported CirQ version '{0}'")]
    UnsupportedVersion(String),
}

// r[impl format.version]
// r[impl file.ext.yaml]
// r[impl file.ext.json]
/// Parse a CirQ document from a string (auto-detects JSON vs YAML).
pub fn parse_cirq(input: &str) -> Result<Circuit, CirqParseError> {
    let trimmed = input.trim_start();
    let doc: CirqDoc = if trimmed.starts_with('{') {
        facet_json::from_str(input)?
    } else {
        facet_yaml::from_str(input)?
    };
    lower_doc(doc)
}

// ---------------------------------------------------------------------------
// Facet document model (intermediate, not exposed)
// ---------------------------------------------------------------------------

#[derive(Facet)]
struct CirqDoc {
    cirq: String,
    name: String,
    #[facet(default)]
    description: Option<String>,
    #[facet(default)]
    components: Vec<CirqComponent>,
    #[facet(default)]
    subcircuits: Vec<CirqSubcircuit>,
    #[facet(default)]
    models: Vec<CirqModel>,
    #[facet(default)]
    params: BTreeMap<String, DynVal>,
    #[facet(default)]
    globals: Vec<String>,
    #[facet(default)]
    includes: Vec<CirqInclude>,
    #[facet(default)]
    functions: Vec<CirqFunction>,
    #[facet(default)]
    options: BTreeMap<String, DynVal>,
    #[facet(default)]
    temperature: Option<f64>,
}

#[derive(Facet)]
struct CirqComponent {
    id: String,
    #[facet(rename = "type")]
    comp_type: String,
    #[facet(default)]
    description: Option<String>,
    #[facet(default)]
    tags: Vec<String>,
    #[facet(default)]
    model: Option<String>,
    #[facet(default)]
    value: Option<DynVal>,
    #[facet(default)]
    pins: Option<DynVal>,
    #[facet(default)]
    params: BTreeMap<String, DynVal>,
    #[facet(default)]
    waveform: Option<CirqWaveform>,
    // Port-specific
    #[facet(default)]
    net: Option<String>,
    #[facet(default)]
    direction: Option<String>,
    #[facet(default)]
    order: Option<u32>,
    #[facet(default)]
    domain: Option<String>,
    // Coupling-specific
    #[facet(default)]
    inductors: Option<Vec<String>>,
    #[facet(default)]
    coefficient: Option<DynVal>,
    // Behavioral source
    #[facet(default)]
    off: Option<bool>,
}

#[derive(Facet)]
struct CirqWaveform {
    #[facet(rename = "type")]
    waveform_type: String,
    #[facet(flatten)]
    params: BTreeMap<String, DynVal>,
}

#[derive(Facet)]
struct CirqSubcircuit {
    name: String,
    #[facet(default)]
    description: Option<String>,
    #[facet(default)]
    params: BTreeMap<String, DynVal>,
    #[facet(default)]
    components: Vec<CirqComponent>,
    #[facet(default)]
    models: Vec<CirqModel>,
    #[facet(default)]
    subcircuits: Vec<CirqSubcircuit>,
}

#[derive(Facet)]
struct CirqModel {
    name: String,
    #[facet(rename = "type")]
    model_type: String,
    #[facet(default)]
    level: Option<u32>,
    #[facet(default)]
    params: BTreeMap<String, DynVal>,
}

#[derive(Facet)]
struct CirqInclude {
    file: String,
    #[facet(default)]
    section: Option<String>,
}

#[derive(Facet)]
struct CirqFunction {
    name: String,
    args: Vec<String>,
    body: String,
}

// ---------------------------------------------------------------------------
// Lowering from document model to IR
// ---------------------------------------------------------------------------

fn lower_doc(doc: CirqDoc) -> Result<Circuit, CirqParseError> {
    // r[format.version.semver]
    // Reject documents whose major version exceeds what we support (0.x).
    let major: u32 = doc
        .cirq
        .split('.')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if major > 0 {
        return Err(CirqParseError::UnsupportedVersion(doc.cirq));
    }

    let mut circuit = Circuit {
        name: doc.name,
        description: doc.description,
        components: Vec::new(),
        models: Vec::new(),
        subcircuits: Vec::new(),
        params: lower_value_map(&doc.params),
        globals: doc.globals,
        includes: doc
            .includes
            .into_iter()
            .map(|i| Include {
                file: i.file,
                section: i.section,
            })
            .collect(),
        functions: doc
            .functions
            .into_iter()
            .map(|f| Function {
                name: f.name,
                args: f.args,
                body: f.body,
            })
            .collect(),
        options: lower_value_map(&doc.options),
        temperature: doc.temperature,
        nets: BTreeMap::new(),
    };

    for m in &doc.models {
        circuit.models.push(lower_cirq_model(m));
    }

    for s in doc.subcircuits {
        circuit.subcircuits.push(lower_cirq_subcircuit(s)?);
    }

    // Track port order for auto-assignment
    let mut next_port_order: u32 = 0;
    for comp in doc.components {
        let c = lower_cirq_component(comp, &mut next_port_order, &circuit.models)?;
        circuit.components.push(c);
    }

    circuit.resolve_domains();
    Ok(circuit)
}

fn lower_cirq_component(
    comp: CirqComponent,
    next_port_order: &mut u32,
    _models: &[Model],
) -> Result<Component, CirqParseError> {
    let kind = match comp.comp_type.as_str() {
        "port" => {
            let net = comp.net.ok_or(CirqParseError::MissingField("net"))?;
            let dir_str = comp
                .direction
                .ok_or(CirqParseError::MissingField("direction"))?;
            let direction = parse_direction(&dir_str)?;
            let domain_override = comp.domain.as_deref().map(parse_domain).transpose()?;
            let order = match comp.order {
                Some(o) => {
                    *next_port_order = o + 1;
                    o
                }
                None => {
                    let o = *next_port_order;
                    *next_port_order += 1;
                    o
                }
            };
            ComponentKind::Port {
                net,
                direction,
                order,
                domain_override,
            }
        }

        "resistor" => {
            let (p, n) = extract_two_pin_map(&comp)?;
            ComponentKind::Resistor {
                p,
                n,
                value: extract_value(&comp)?,
                params: lower_value_map(&comp.params),
            }
        }

        "capacitor" => {
            let (p, n) = extract_two_pin_map(&comp)?;
            ComponentKind::Capacitor {
                p,
                n,
                value: extract_value(&comp)?,
                params: lower_value_map(&comp.params),
            }
        }

        "inductor" => {
            let (p, n) = extract_two_pin_map(&comp)?;
            ComponentKind::Inductor {
                p,
                n,
                value: extract_value(&comp)?,
                params: lower_value_map(&comp.params),
            }
        }

        "coupling" => {
            let inductors = comp
                .inductors
                .ok_or_else(|| CirqParseError::InvalidComponent {
                    id: comp.id.clone(),
                    msg: "coupling requires 'inductors' field".into(),
                })?;
            if inductors.len() != 2 {
                return Err(CirqParseError::InvalidComponent {
                    id: comp.id.clone(),
                    msg: "coupling requires exactly 2 inductors".into(),
                });
            }
            let coeff = comp
                .coefficient
                .as_ref()
                .map(dyn_to_value)
                .unwrap_or(Value::Num(1.0));
            ComponentKind::Coupling {
                l1: inductors[0].clone(),
                l2: inductors[1].clone(),
                coefficient: coeff,
            }
        }

        "diode" | "zener" | "led" | "schottky" => {
            let pins = extract_pin_map(&comp)?;
            ComponentKind::Diode {
                a: pin_or_err(&pins, "a", &comp.id)?,
                k: pin_or_err(&pins, "k", &comp.id)?,
                model: require_model(&comp)?,
                params: lower_value_map(&comp.params),
            }
        }

        "npn" => {
            let pins = extract_pin_map(&comp)?;
            ComponentKind::Bjt {
                polarity: BjtPolarity::Npn,
                c: pin_or_err(&pins, "c", &comp.id)?,
                b: pin_or_err(&pins, "b", &comp.id)?,
                e: pin_or_err(&pins, "e", &comp.id)?,
                s: pins.get("s").cloned(),
                model: require_model(&comp)?,
                params: lower_value_map(&comp.params),
                off: comp.off.unwrap_or(false),
            }
        }

        "pnp" => {
            let pins = extract_pin_map(&comp)?;
            ComponentKind::Bjt {
                polarity: BjtPolarity::Pnp,
                c: pin_or_err(&pins, "c", &comp.id)?,
                b: pin_or_err(&pins, "b", &comp.id)?,
                e: pin_or_err(&pins, "e", &comp.id)?,
                s: pins.get("s").cloned(),
                model: require_model(&comp)?,
                params: lower_value_map(&comp.params),
                off: comp.off.unwrap_or(false),
            }
        }

        "nmos" => {
            let pins = extract_pin_map(&comp)?;
            let s_pin = pin_or_err(&pins, "s", &comp.id)?;
            // r[prim.nmos]: bulk pin is optional; defaults to source when omitted.
            let b_pin = pins.get("b").cloned().unwrap_or_else(|| s_pin.clone());
            ComponentKind::Mosfet {
                polarity: MosfetPolarity::Nmos,
                d: pin_or_err(&pins, "d", &comp.id)?,
                g: pin_or_err(&pins, "g", &comp.id)?,
                s: s_pin,
                b: b_pin,
                body: pins.get("body").cloned(),
                model: require_model(&comp)?,
                params: lower_value_map(&comp.params),
            }
        }

        "pmos" => {
            let pins = extract_pin_map(&comp)?;
            let s_pin = pin_or_err(&pins, "s", &comp.id)?;
            // r[prim.pmos]: bulk pin is optional; defaults to source when omitted.
            let b_pin = pins.get("b").cloned().unwrap_or_else(|| s_pin.clone());
            ComponentKind::Mosfet {
                polarity: MosfetPolarity::Pmos,
                d: pin_or_err(&pins, "d", &comp.id)?,
                g: pin_or_err(&pins, "g", &comp.id)?,
                s: s_pin,
                b: b_pin,
                body: pins.get("body").cloned(),
                model: require_model(&comp)?,
                params: lower_value_map(&comp.params),
            }
        }

        "njfet" => {
            let pins = extract_pin_map(&comp)?;
            ComponentKind::Jfet {
                polarity: JfetPolarity::Njfet,
                d: pin_or_err(&pins, "d", &comp.id)?,
                g: pin_or_err(&pins, "g", &comp.id)?,
                s: pin_or_err(&pins, "s", &comp.id)?,
                model: require_model(&comp)?,
                params: lower_value_map(&comp.params),
            }
        }

        "pjfet" => {
            let pins = extract_pin_map(&comp)?;
            ComponentKind::Jfet {
                polarity: JfetPolarity::Pjfet,
                d: pin_or_err(&pins, "d", &comp.id)?,
                g: pin_or_err(&pins, "g", &comp.id)?,
                s: pin_or_err(&pins, "s", &comp.id)?,
                model: require_model(&comp)?,
                params: lower_value_map(&comp.params),
            }
        }

        "mesfet" => {
            let pins = extract_pin_map(&comp)?;
            ComponentKind::Mesfet {
                d: pin_or_err(&pins, "d", &comp.id)?,
                g: pin_or_err(&pins, "g", &comp.id)?,
                s: pin_or_err(&pins, "s", &comp.id)?,
                model: require_model(&comp)?,
                params: lower_value_map(&comp.params),
            }
        }

        "vsource" => {
            let (p, n) = extract_two_pin_map(&comp)?;
            let source = build_source_spec(&comp)?;
            ComponentKind::VSource { p, n, source }
        }

        "isource" => {
            let (p, n) = extract_two_pin_map(&comp)?;
            let source = build_source_spec(&comp)?;
            ComponentKind::ISource { p, n, source }
        }

        "vref" => {
            let (p, n) = extract_two_pin_map(&comp)?;
            let source = SourceSpec {
                dc: comp.value.as_ref().map(dyn_to_value),
                ..Default::default()
            };
            ComponentKind::VSource { p, n, source }
        }

        "vcvs" => {
            let pins = extract_pin_map(&comp)?;
            let gain = comp
                .params
                .get("gain")
                .map(dyn_to_value)
                .unwrap_or(Value::Num(1.0));
            ComponentKind::Vcvs {
                p: pin_or_err(&pins, "p", &comp.id)?,
                n: pin_or_err(&pins, "n", &comp.id)?,
                cp: pin_or_err(&pins, "cp", &comp.id)?,
                cn: pin_or_err(&pins, "cn", &comp.id)?,
                gain,
            }
        }

        "vccs" => {
            let pins = extract_pin_map(&comp)?;
            let gm = comp
                .params
                .get("gm")
                .map(dyn_to_value)
                .unwrap_or(Value::Num(0.001));
            ComponentKind::Vccs {
                p: pin_or_err(&pins, "p", &comp.id)?,
                n: pin_or_err(&pins, "n", &comp.id)?,
                cp: pin_or_err(&pins, "cp", &comp.id)?,
                cn: pin_or_err(&pins, "cn", &comp.id)?,
                gm,
            }
        }

        "cccs" => {
            let pins = extract_pin_map(&comp)?;
            let vsource = require_param_str(&comp, "vsource")?;
            let gain = comp
                .params
                .get("gain")
                .map(dyn_to_value)
                .unwrap_or(Value::Num(1.0));
            ComponentKind::Cccs {
                p: pin_or_err(&pins, "p", &comp.id)?,
                n: pin_or_err(&pins, "n", &comp.id)?,
                vsource,
                gain,
            }
        }

        "ccvs" => {
            let pins = extract_pin_map(&comp)?;
            let vsource = require_param_str(&comp, "vsource")?;
            let tr = comp
                .params
                .get("transresistance")
                .map(dyn_to_value)
                .unwrap_or(Value::Num(1.0));
            ComponentKind::Ccvs {
                p: pin_or_err(&pins, "p", &comp.id)?,
                n: pin_or_err(&pins, "n", &comp.id)?,
                vsource,
                transresistance: tr,
            }
        }

        "bsource" => {
            let pins = extract_pin_map(&comp)?;
            let expr = if let Some(v) = comp.params.get("v") {
                BehavioralExpr::Voltage(dyn_to_string(v))
            } else if let Some(i) = comp.params.get("i") {
                BehavioralExpr::Current(dyn_to_string(i))
            } else {
                return Err(CirqParseError::InvalidComponent {
                    id: comp.id.clone(),
                    msg: "bsource requires params.v or params.i".into(),
                });
            };
            ComponentKind::BehavioralSource {
                p: pin_or_err(&pins, "p", &comp.id)?,
                n: pin_or_err(&pins, "n", &comp.id)?,
                expr,
            }
        }

        "cell" => {
            let pins = extract_pin_map(&comp)?;
            ComponentKind::Cell {
                model: comp.model.unwrap_or_default(),
                pins,
            }
        }

        "xspice" => {
            // XSPICE pins are an ordered list of scalar/array connections
            let connections = extract_xspice_connections(&comp)?;
            ComponentKind::Xspice {
                connections,
                model: comp.model.unwrap_or_default(),
            }
        }

        // Digital primitives — not natively in SPICE, but valid CirQ.
        // r[domain.primitives.digital]
        "and" | "or" | "not" | "nand" | "nor" | "xor" | "xnor" | "buf" | "dff" | "dff_sr"
        | "mux2" | "latch" => {
            let pins = extract_pin_map(&comp)?;
            let gate_type = parse_digital_gate_type(comp.comp_type.as_str())?;
            ComponentKind::DigitalGate { gate_type, pins }
        }

        // r[prim.transformer]: primary (p1, n1) and secondary (p2, n2) with turns ratio
        "transformer" => {
            let pins = extract_pin_map(&comp)?;
            ComponentKind::Tline {
                p1: pin_or_err(&pins, "p1", &comp.id)?,
                n1: pin_or_err(&pins, "n1", &comp.id)?,
                p2: pin_or_err(&pins, "p2", &comp.id)?,
                n2: pin_or_err(&pins, "n2", &comp.id)?,
                params: lower_value_map(&comp.params),
            }
        }

        // r[prim.crystal]: two-pin passive, value = resonant frequency
        "crystal" => {
            let (p, n) = extract_two_pin_map(&comp)?;
            ComponentKind::Capacitor {
                p,
                n,
                value: extract_value(&comp)?,
                params: lower_value_map(&comp.params),
            }
        }

        // Transmission lines
        "tline" => {
            let pins = extract_pin_map(&comp)?;
            ComponentKind::Tline {
                p1: pin_or_err(&pins, "p1", &comp.id)?,
                n1: pin_or_err(&pins, "n1", &comp.id)?,
                p2: pin_or_err(&pins, "p2", &comp.id)?,
                n2: pin_or_err(&pins, "n2", &comp.id)?,
                params: lower_value_map(&comp.params),
            }
        }

        "ltra" => {
            let pins = extract_pin_map(&comp)?;
            ComponentKind::Ltra {
                p1: pin_or_err(&pins, "p1", &comp.id)?,
                n1: pin_or_err(&pins, "n1", &comp.id)?,
                p2: pin_or_err(&pins, "p2", &comp.id)?,
                n2: pin_or_err(&pins, "n2", &comp.id)?,
                model: require_model(&comp)?,
                params: lower_value_map(&comp.params),
            }
        }

        "txl" => {
            let pins = extract_pin_map(&comp)?;
            ComponentKind::Txl {
                p1: pin_or_err(&pins, "p1", &comp.id)?,
                n1: pin_or_err(&pins, "n1", &comp.id)?,
                p2: pin_or_err(&pins, "p2", &comp.id)?,
                n2: pin_or_err(&pins, "n2", &comp.id)?,
                model: require_model(&comp)?,
                params: lower_value_map(&comp.params),
            }
        }

        "vswitch" => {
            let pins = extract_pin_map(&comp)?;
            ComponentKind::VSwitch {
                p: pin_or_err(&pins, "p", &comp.id)?,
                n: pin_or_err(&pins, "n", &comp.id)?,
                cp: pin_or_err(&pins, "cp", &comp.id)?,
                cn: pin_or_err(&pins, "cn", &comp.id)?,
                model: require_model(&comp)?,
                params: lower_value_map(&comp.params),
            }
        }

        "iswitch" => {
            let pins = extract_pin_map(&comp)?;
            let vsource = require_param_str(&comp, "vsource")?;
            ComponentKind::ISwitch {
                p: pin_or_err(&pins, "p", &comp.id)?,
                n: pin_or_err(&pins, "n", &comp.id)?,
                vsource,
                model: require_model(&comp)?,
                params: lower_value_map(&comp.params),
            }
        }

        // Assume it's a subcircuit instance if we don't recognize the type
        other => {
            let pins = match &comp.pins {
                Some(DynVal::Sequence(seq)) => {
                    // Positional list → numeric keys
                    seq.iter()
                        .enumerate()
                        .map(|(i, v)| (i.to_string(), dyn_to_string(v)))
                        .collect()
                }
                Some(DynVal::Mapping(map)) => map
                    .iter()
                    .map(|(k, v)| (k.clone(), dyn_to_string(v)))
                    .collect(),
                _ => BTreeMap::new(),
            };
            ComponentKind::SubcktInstance {
                subckt: other.to_string(),
                pins,
                params: lower_value_map(&comp.params),
            }
        }
    };

    Ok(Component {
        id: comp.id,
        description: comp.description,
        tags: comp.tags,
        kind,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn dyn_to_value(v: &DynVal) -> Value {
    match v {
        DynVal::Number(n) => Value::Num(*n),
        DynVal::String(s) => parse_value_string(s),
        DynVal::Bool(b) => Value::Num(if *b { 1.0 } else { 0.0 }),
        other => Value::Expr(format!("{other:?}")),
    }
}

fn dyn_to_string(v: &DynVal) -> String {
    match v {
        DynVal::String(s) => s.clone(),
        DynVal::Number(n) => n.to_string(),
        DynVal::Bool(b) => b.to_string(),
        other => format!("{other:?}"),
    }
}

// r[impl component.value.format]
/// Parse a CirQ value string: either a bare number, a number+SI suffix, or
/// an expression in braces.
fn parse_value_string(s: &str) -> Value {
    let trimmed = s.trim();

    // Brace expression
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Value::Expr(trimmed[1..trimmed.len() - 1].to_string());
    }

    // Try as SPICE number (with SI suffix)
    if let Some(v) = try_parse_si_number(trimmed) {
        return Value::Num(v);
    }

    // Must be a parameter reference
    Value::Param(trimmed.to_string())
}

/// Try parsing a number with optional SI suffix.
fn try_parse_si_number(s: &str) -> Option<f64> {
    // Try plain float first
    if let Ok(v) = s.parse::<f64>() {
        return Some(v);
    }

    // Try with SI suffix
    let s_lower = s.to_lowercase();
    let suffixes: &[(&str, f64)] = &[
        ("meg", 1e6),
        ("mil", 25.4e-6),
        ("t", 1e12),
        ("g", 1e9),
        ("k", 1e3),
        ("m", 1e-3),
        ("u", 1e-6),
        ("n", 1e-9),
        ("p", 1e-12),
        ("f", 1e-15),
        ("a", 1e-18),
    ];

    for &(suffix, scale) in suffixes {
        if s_lower.ends_with(suffix) {
            let num_part = &s[..s.len() - suffix.len()];
            if let Ok(v) = num_part.parse::<f64>() {
                return Some(v * scale);
            }
        }
    }

    None
}

fn lower_value_map(map: &BTreeMap<String, DynVal>) -> BTreeMap<String, Value> {
    map.iter()
        .map(|(k, v)| (k.clone(), dyn_to_value(v)))
        .collect()
}

fn require_model(comp: &CirqComponent) -> Result<String, CirqParseError> {
    comp.model
        .clone()
        .ok_or_else(|| CirqParseError::InvalidComponent {
            id: comp.id.clone(),
            msg: "missing required 'model' field".into(),
        })
}

fn require_param_str(comp: &CirqComponent, key: &str) -> Result<String, CirqParseError> {
    comp.params
        .get(key)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .ok_or_else(|| CirqParseError::InvalidComponent {
            id: comp.id.clone(),
            msg: format!("missing required param '{key}'"),
        })
}

fn parse_digital_gate_type(s: &str) -> Result<DigitalGateType, CirqParseError> {
    match s {
        "and" => Ok(DigitalGateType::And),
        "or" => Ok(DigitalGateType::Or),
        "not" => Ok(DigitalGateType::Not),
        "nand" => Ok(DigitalGateType::Nand),
        "nor" => Ok(DigitalGateType::Nor),
        "xor" => Ok(DigitalGateType::Xor),
        "xnor" => Ok(DigitalGateType::Xnor),
        "buf" => Ok(DigitalGateType::Buf),
        "dff" => Ok(DigitalGateType::Dff),
        "dff_sr" => Ok(DigitalGateType::DffSr),
        "mux2" => Ok(DigitalGateType::Mux2),
        "latch" => Ok(DigitalGateType::Latch),
        other => Err(CirqParseError::InvalidComponent {
            id: String::new(),
            msg: format!("unknown digital gate type '{other}'"),
        }),
    }
}

fn extract_pin_map(comp: &CirqComponent) -> Result<BTreeMap<String, String>, CirqParseError> {
    match &comp.pins {
        Some(DynVal::Mapping(map)) => Ok(map
            .iter()
            .map(|(k, v)| (k.clone(), dyn_to_string(v)))
            .collect()),
        Some(DynVal::Sequence(seq)) => Ok(seq
            .iter()
            .enumerate()
            .map(|(i, v)| (i.to_string(), dyn_to_string(v)))
            .collect()),
        None => Ok(BTreeMap::new()),
        _ => Err(CirqParseError::InvalidComponent {
            id: comp.id.clone(),
            msg: "pins must be a mapping or sequence".into(),
        }),
    }
}

fn extract_two_pin_map(comp: &CirqComponent) -> Result<(String, String), CirqParseError> {
    let pins = extract_pin_map(comp)?;
    let p = pin_or_err(&pins, "p", &comp.id)?;
    let n = pin_or_err(&pins, "n", &comp.id)?;
    Ok((p, n))
}

fn pin_or_err(
    pins: &BTreeMap<String, String>,
    key: &str,
    comp_id: &str,
) -> Result<String, CirqParseError> {
    pins.get(key)
        .cloned()
        .ok_or_else(|| CirqParseError::InvalidComponent {
            id: comp_id.to_string(),
            msg: format!("missing required pin '{key}'"),
        })
}

fn extract_value(comp: &CirqComponent) -> Result<Value, CirqParseError> {
    comp.value
        .as_ref()
        .map(dyn_to_value)
        .ok_or_else(|| CirqParseError::InvalidComponent {
            id: comp.id.clone(),
            msg: "missing required 'value' field".into(),
        })
}

fn build_source_spec(comp: &CirqComponent) -> Result<SourceSpec, CirqParseError> {
    let dc = comp
        .value
        .as_ref()
        .map(dyn_to_value)
        .or_else(|| comp.params.get("dc").map(dyn_to_value));
    let ac_mag = comp.params.get("ac_mag").map(dyn_to_value);
    let ac_phase = comp.params.get("ac_phase").map(dyn_to_value);
    let waveform = comp.waveform.as_ref().map(lower_waveform).transpose()?;

    Ok(SourceSpec {
        dc,
        ac_mag,
        ac_phase,
        waveform,
    })
}

fn lower_waveform(w: &CirqWaveform) -> Result<Waveform, CirqParseError> {
    let p = &w.params;
    match w.waveform_type.as_str() {
        "pulse" => Ok(Waveform::Pulse {
            v1: req_param(p, "v1")?,
            v2: req_param(p, "v2")?,
            td: opt_param(p, "td"),
            tr: opt_param(p, "tr"),
            tf: opt_param(p, "tf"),
            pw: opt_param(p, "pw"),
            per: opt_param(p, "per"),
        }),
        "sin" => Ok(Waveform::Sin {
            v0: req_param(p, "v0")?,
            va: req_param(p, "va")?,
            freq: opt_param(p, "freq"),
            td: opt_param(p, "td"),
            theta: opt_param(p, "theta"),
            phi: opt_param(p, "phi"),
        }),
        "exp" => Ok(Waveform::Exp {
            v1: req_param(p, "v1")?,
            v2: req_param(p, "v2")?,
            td1: opt_param(p, "td1"),
            tau1: opt_param(p, "tau1"),
            td2: opt_param(p, "td2"),
            tau2: opt_param(p, "tau2"),
        }),
        "pwl" => {
            let points_val = p.get("points").ok_or_else(|| {
                CirqParseError::InvalidValue("pwl waveform requires 'points' parameter".into())
            })?;
            let points = match points_val {
                DynVal::Sequence(seq) => {
                    let mut pts = Vec::with_capacity(seq.len());
                    for (i, pair) in seq.iter().enumerate() {
                        match pair {
                            DynVal::Sequence(inner) if inner.len() == 2 => {
                                pts.push((dyn_to_value(&inner[0]), dyn_to_value(&inner[1])));
                            }
                            _ => {
                                return Err(CirqParseError::InvalidValue(format!(
                                    "pwl points[{i}]: expected [time, value] pair"
                                )));
                            }
                        }
                    }
                    pts
                }
                _ => {
                    return Err(CirqParseError::InvalidValue(
                        "pwl 'points' must be a sequence of [time, value] pairs".into(),
                    ));
                }
            };
            Ok(Waveform::Pwl { points })
        }
        "sffm" => Ok(Waveform::Sffm {
            v0: req_param(p, "v0")?,
            va: req_param(p, "va")?,
            fc: opt_param(p, "fc"),
            fs: opt_param(p, "fs"),
            md: opt_param(p, "md"),
        }),
        "am" => Ok(Waveform::Am {
            va: req_param(p, "va")?,
            vo: req_param(p, "vo")?,
            fc: req_param(p, "fc")?,
            fs: req_param(p, "fs")?,
            td: opt_param(p, "td"),
        }),
        other => Err(CirqParseError::UnknownWaveformType(other.to_string())),
    }
}

fn req_param(p: &BTreeMap<String, DynVal>, key: &str) -> Result<Value, CirqParseError> {
    p.get(key)
        .map(dyn_to_value)
        .ok_or_else(|| CirqParseError::InvalidValue(format!("missing waveform param '{key}'")))
}

fn opt_param(p: &BTreeMap<String, DynVal>, key: &str) -> Option<Value> {
    p.get(key).map(dyn_to_value)
}

fn extract_xspice_connections(comp: &CirqComponent) -> Result<Vec<XspicePort>, CirqParseError> {
    match &comp.pins {
        Some(DynVal::Sequence(seq)) => {
            let mut ports = Vec::new();
            for item in seq {
                match item {
                    DynVal::String(s) => {
                        ports.push(XspicePort::Scalar(s.clone()));
                    }
                    DynVal::Sequence(inner) => {
                        let arr: Vec<String> = inner.iter().map(dyn_to_string).collect();
                        ports.push(XspicePort::Array(arr));
                    }
                    _ => {
                        ports.push(XspicePort::Scalar(dyn_to_string(item)));
                    }
                }
            }
            Ok(ports)
        }
        _ => Ok(Vec::new()),
    }
}

// r[impl port.direction]
fn parse_direction(s: &str) -> Result<Direction, CirqParseError> {
    match s {
        "input" => Ok(Direction::Input),
        "output" => Ok(Direction::Output),
        "inout" => Ok(Direction::InOut),
        "passive" => Ok(Direction::Passive),
        other => Err(CirqParseError::UnknownDirection(other.to_string())),
    }
}

// r[impl port.domain.override]
fn parse_domain(s: &str) -> Result<Domain, CirqParseError> {
    match s {
        "analog" => Ok(Domain::Analog),
        "digital" => Ok(Domain::Digital),
        "mixed" => Ok(Domain::Mixed),
        other => Err(CirqParseError::UnknownDomain(other.to_string())),
    }
}

fn lower_cirq_model(m: &CirqModel) -> Model {
    let model_type = match m.model_type.as_str() {
        "diode" => ModelType::Diode,
        "npn" => ModelType::Npn,
        "pnp" => ModelType::Pnp,
        "nmos" => ModelType::Nmos,
        "pmos" => ModelType::Pmos,
        "njfet" => ModelType::Njfet,
        "pjfet" => ModelType::Pjfet,
        "mesfet" => ModelType::Mesfet,
        "ltra" => ModelType::Ltra,
        "txl" => ModelType::Txl,
        "cpl" => ModelType::Cpl,
        "vswitch" => ModelType::VSwitch,
        "iswitch" => ModelType::ISwitch,
        other => ModelType::Other(other.to_string()),
    };

    Model {
        name: m.name.clone(),
        model_type,
        level: m.level,
        params: lower_value_map(&m.params),
    }
}

fn lower_cirq_subcircuit(s: CirqSubcircuit) -> Result<Subcircuit, CirqParseError> {
    let mut subckt = Subcircuit {
        name: s.name,
        description: s.description,
        params: lower_value_map(&s.params),
        components: Vec::new(),
        models: s.models.iter().map(lower_cirq_model).collect(),
        subcircuits: Vec::new(),
    };

    let mut next_port_order: u32 = 0;
    for comp in s.components {
        let c = lower_cirq_component(comp, &mut next_port_order, &subckt.models)?;
        subckt.components.push(c);
    }

    for inner in s.subcircuits {
        subckt.subcircuits.push(lower_cirq_subcircuit(inner)?);
    }

    Ok(subckt)
}

#[cfg(test)]
mod tests {
    use super::*;

    // r[verify doc.name]
    // r[verify doc.components]
    // r[verify component.value.format]
    // r[verify prim.resistor]
    // r[verify prim.capacitor]
    // r[verify domain.inference.analog]
    #[test]
    fn parse_rc_filter_yaml() {
        let yaml = r#"
cirq: "0.3"
name: RC Low-Pass Filter

components:
  - id: IN
    type: port
    direction: input
    net: vin

  - id: OUT
    type: port
    direction: output
    net: vout

  - id: GND
    type: port
    direction: passive
    net: "0"

  - id: R1
    type: resistor
    value: "10k"
    pins: { p: vin, n: vout }

  - id: C1
    type: capacitor
    value: "10n"
    pins: { p: vout, n: "0" }
"#;

        let circuit = parse_cirq(yaml).unwrap();
        assert_eq!(circuit.name, "RC Low-Pass Filter");
        assert_eq!(circuit.components.len(), 5); // 3 ports + R1 + C1

        // Check R1 value
        match &circuit.components[3].kind {
            ComponentKind::Resistor { value, .. } => {
                assert!(matches!(value, Value::Num(v) if (*v - 10_000.0).abs() < 1e-6));
            }
            other => panic!("expected Resistor, got {other:?}"),
        }

        // Check domains
        assert_eq!(circuit.nets["vin"].domain, Domain::Analog);
        assert_eq!(circuit.nets["vout"].domain, Domain::Analog);
    }

    // r[verify file.ext.json]
    // r[verify component.value.format]
    #[test]
    fn parse_rc_filter_json() {
        let json = r#"{
  "cirq": "0.3",
  "name": "RC Filter",
  "components": [
    { "id": "R1", "type": "resistor", "value": 1000, "pins": { "p": "in", "n": "out" } },
    { "id": "C1", "type": "capacitor", "value": 1e-9, "pins": { "p": "out", "n": "0" } }
  ]
}"#;

        let circuit = parse_cirq(json).unwrap();
        assert_eq!(circuit.name, "RC Filter");
        assert_eq!(circuit.components.len(), 2);

        match &circuit.components[0].kind {
            ComponentKind::Resistor { value, .. } => {
                assert!(matches!(value, Value::Num(v) if (*v - 1000.0).abs() < 1e-9));
            }
            other => panic!("expected Resistor, got {other:?}"),
        }
    }

    // r[verify doc.models]
    // r[verify model.name]
    // r[verify model.type]
    // r[verify model.level]
    // r[verify prim.nmos]
    // r[verify prim.pmos]
    #[test]
    fn parse_cmos_inverter() {
        let yaml = r#"
cirq: "0.3"
name: CMOS Inverter

models:
  - name: NMOD
    type: nmos
    level: 1
    params: { vto: 0.7, kp: 110e-6 }
  - name: PMOD
    type: pmos
    level: 1
    params: { vto: -0.7, kp: 50e-6 }

components:
  - id: M1
    type: pmos
    model: PMOD
    pins: { d: out, g: in, s: vdd, b: vdd }
    params: { w: "10u", l: "0.5u" }

  - id: M2
    type: nmos
    model: NMOD
    pins: { d: out, g: in, s: "0", b: "0" }
    params: { w: "5u", l: "0.5u" }

  - id: VDD
    type: vsource
    value: 5
    pins: { p: vdd, n: "0" }
"#;

        let circuit = parse_cirq(yaml).unwrap();
        assert_eq!(circuit.models.len(), 2);
        assert_eq!(circuit.models[0].model_type, ModelType::Nmos);
        assert_eq!(circuit.models[0].level, Some(1));

        match &circuit.components[0].kind {
            ComponentKind::Mosfet {
                polarity, model, ..
            } => {
                assert_eq!(*polarity, MosfetPolarity::Pmos);
                assert_eq!(model, "PMOD");
            }
            other => panic!("expected Mosfet, got {other:?}"),
        }
    }

    // r[verify doc.subcircuits]
    // r[verify subckt.name]
    // r[verify subckt.instantiation]
    // r[verify port.order]
    #[test]
    fn parse_subcircuit_instance() {
        let yaml = r#"
cirq: "0.3"
name: Sub Test

subcircuits:
  - name: opamp
    components:
      - { id: inp, type: port, direction: input, net: inp, order: 0 }
      - { id: inn, type: port, direction: input, net: inn, order: 1 }
      - { id: out, type: port, direction: output, net: out, order: 2 }
      - { id: R1, type: resistor, value: "1meg", pins: { p: inp, n: inn } }

components:
  - id: X1
    type: opamp
    pins: [signal, ref, output]
"#;

        let circuit = parse_cirq(yaml).unwrap();
        assert_eq!(circuit.subcircuits.len(), 1);
        assert_eq!(circuit.subcircuits[0].components.len(), 4);

        match &circuit.components[0].kind {
            ComponentKind::SubcktInstance { subckt, pins, .. } => {
                assert_eq!(subckt, "opamp");
                assert_eq!(pins.len(), 3);
            }
            other => panic!("expected SubcktInstance, got {other:?}"),
        }
    }

    // r[verify source.waveform]
    // r[verify prim.vsource]
    #[test]
    fn parse_waveform() {
        let yaml = r#"
cirq: "0.3"
name: Pulse Test

components:
  - id: V1
    type: vsource
    value: 0
    pins: { p: in, n: "0" }
    waveform:
      type: pulse
      v1: 0
      v2: 5
      td: 1e-9
      tr: 1e-9
      tf: 1e-9
      pw: 50e-9
      per: 100e-9
"#;

        let circuit = parse_cirq(yaml).unwrap();
        match &circuit.components[0].kind {
            ComponentKind::VSource { source, .. } => {
                assert!(source.waveform.is_some());
                match &source.waveform {
                    Some(Waveform::Pulse { v2, .. }) => {
                        assert!(matches!(v2, Value::Num(v) if (*v - 5.0).abs() < 1e-9));
                    }
                    other => panic!("expected Pulse, got {other:?}"),
                }
            }
            other => panic!("expected VSource, got {other:?}"),
        }
    }

    // r[verify prim.coupling]
    #[test]
    fn parse_coupling() {
        let yaml = r#"
cirq: "0.3"
name: Coupling Test

components:
  - id: L1
    type: inductor
    value: "10m"
    pins: { p: in, n: "0" }
  - id: L2
    type: inductor
    value: "10m"
    pins: { p: out, n: "0" }
  - id: K1
    type: coupling
    inductors: [L1, L2]
    coefficient: 0.95
"#;

        let circuit = parse_cirq(yaml).unwrap();
        match &circuit.components[2].kind {
            ComponentKind::Coupling {
                l1,
                l2,
                coefficient,
            } => {
                assert_eq!(l1, "L1");
                assert_eq!(l2, "L2");
                assert!(matches!(coefficient, Value::Num(v) if (*v - 0.95).abs() < 1e-9));
            }
            other => panic!("expected Coupling, got {other:?}"),
        }
    }

    // r[verify doc.globals]
    // r[verify doc.params]
    // r[verify doc.temperature]
    // r[verify doc.options]
    #[test]
    fn parse_globals_and_params() {
        let yaml = r#"
cirq: "0.3"
name: Globals Test
globals: [vdd, vss]
params:
  Wn: "5u"
  Wp: "10u"
temperature: 27
options:
  reltol: 1e-4
components: []
"#;

        let circuit = parse_cirq(yaml).unwrap();
        assert_eq!(circuit.globals, vec!["vdd", "vss"]);
        assert_eq!(circuit.params.len(), 2);
        assert_eq!(circuit.temperature, Some(27.0));
        assert_eq!(circuit.options.len(), 1);
    }

    // r[verify component.value.format]
    #[test]
    fn numeric_value_accepted() {
        let json = r#"{
  "cirq": "0.3",
  "name": "Numeric",
  "components": [
    { "id": "R1", "type": "resistor", "value": 10000, "pins": { "p": "a", "n": "b" } }
  ]
}"#;
        let circuit = parse_cirq(json).unwrap();
        match &circuit.components[0].kind {
            ComponentKind::Resistor { value, .. } => {
                assert!(matches!(value, Value::Num(v) if (*v - 10_000.0).abs() < 1e-9));
            }
            other => panic!("expected Resistor, got {other:?}"),
        }
    }

    #[test]
    fn si_suffix_parsing() {
        assert!(matches!(
            try_parse_si_number("10k"),
            Some(v) if (v - 10_000.0).abs() < 1e-6
        ));
        assert!(matches!(
            try_parse_si_number("1meg"),
            Some(v) if (v - 1e6).abs() < 1e-3
        ));
        assert!(matches!(
            try_parse_si_number("100n"),
            Some(v) if (v - 100e-9).abs() < 1e-18
        ));
        assert!(matches!(
            try_parse_si_number("2.2u"),
            Some(v) if (v - 2.2e-6).abs() < 1e-15
        ));
        // m is milli, not mega
        assert!(matches!(
            try_parse_si_number("10m"),
            Some(v) if (v - 0.01).abs() < 1e-9
        ));
    }

    // r[verify net.implicit]
    // r[verify domain.inference.analog]
    #[test]
    fn both_paths_produce_same_ir() {
        // Same circuit described in SPICE and CirQ — the IR should be equivalent
        let spice = "\
RC Filter
R1 in out 1k
C1 out 0 100n
.end
";
        let yaml = r#"
cirq: "0.3"
name: RC Filter

components:
  - id: R1
    type: resistor
    value: "1k"
    pins: { p: in, n: out }

  - id: C1
    type: capacitor
    value: "100n"
    pins: { p: out, n: "0" }
"#;

        let from_sp = crate::from_spice::from_spice(spice).unwrap();
        let from_cq = parse_cirq(yaml).unwrap();

        // Both should have the same component count
        assert_eq!(from_sp.components.len(), from_cq.components.len());

        // Both should have the same nets
        assert_eq!(from_sp.nets.len(), from_cq.nets.len());
        for (name, net) in &from_sp.nets {
            assert!(from_cq.nets.contains_key(name), "CirQ missing net {name}");
            assert_eq!(
                net.domain, from_cq.nets[name].domain,
                "Domain mismatch for net {name}"
            );
        }

        // R1 values should match
        match (&from_sp.components[0].kind, &from_cq.components[0].kind) {
            (
                ComponentKind::Resistor {
                    value: Value::Num(a),
                    ..
                },
                ComponentKind::Resistor {
                    value: Value::Num(b),
                    ..
                },
            ) => {
                assert!((a - b).abs() < 1e-9, "R1 value mismatch: {a} vs {b}");
            }
            (a, b) => panic!("expected Resistor/Resistor, got {a:?} / {b:?}"),
        }
    }

    // -- Error path tests --

    #[test]
    fn missing_required_pin_errors() {
        let yaml = r#"
cirq: "0.3"
name: Bad
components:
  - id: R1
    type: resistor
    value: 100
    pins: { p: a }
"#;
        let err = parse_cirq(yaml).unwrap_err();
        assert!(err.to_string().contains("missing required pin 'n'"));
    }

    #[test]
    fn missing_model_errors() {
        let yaml = r#"
cirq: "0.3"
name: Bad
components:
  - id: D1
    type: diode
    pins: { a: anode, k: cathode }
"#;
        let err = parse_cirq(yaml).unwrap_err();
        assert!(err.to_string().contains("missing required 'model'"));
    }

    #[test]
    fn missing_vsource_param_errors() {
        let yaml = r#"
cirq: "0.3"
name: Bad
components:
  - id: F1
    type: cccs
    pins: { p: out, n: "0" }
    params: { gain: 5 }
"#;
        let err = parse_cirq(yaml).unwrap_err();
        assert!(err.to_string().contains("vsource"));
    }

    #[test]
    fn malformed_waveform_errors() {
        let yaml = r#"
cirq: "0.3"
name: Bad
components:
  - id: V1
    type: vsource
    value: 0
    pins: { p: in, n: "0" }
    waveform:
      type: pulse
      v1: 0
"#;
        // pulse requires v2 — should error, not silently drop
        let err = parse_cirq(yaml).unwrap_err();
        assert!(err.to_string().contains("v2"));
    }

    #[test]
    fn malformed_pwl_point_errors() {
        let yaml = r#"
cirq: "0.3"
name: Bad
components:
  - id: V1
    type: vsource
    value: 0
    pins: { p: in, n: "0" }
    waveform:
      type: pwl
      points: [[0, 0], "bad", [1, 5]]
"#;
        let err = parse_cirq(yaml).unwrap_err();
        assert!(err.to_string().contains("points[1]"));
    }

    #[test]
    fn unsupported_version_errors() {
        let yaml = r#"
cirq: "1.0"
name: Future
components: []
"#;
        let err = parse_cirq(yaml).unwrap_err();
        assert!(err.to_string().contains("unsupported"));
    }

    // -- MOSFET optional bulk pin --

    // r[verify prim.nmos]
    #[test]
    fn mosfet_bulk_defaults_to_source() {
        let yaml = r#"
cirq: "0.3"
name: Bulk Default
components:
  - id: M1
    type: nmos
    model: NMOD
    pins: { d: out, g: in, s: gnd }
"#;
        let circuit = parse_cirq(yaml).unwrap();
        match &circuit.components[0].kind {
            ComponentKind::Mosfet { s, b, .. } => {
                assert_eq!(b, s, "bulk should default to source pin");
                assert_eq!(s, "gnd");
            }
            other => panic!("expected Mosfet, got {other:?}"),
        }
    }

    // -- Digital domain inference --

    // r[verify domain.inference.digital]
    #[test]
    fn digital_gate_infers_digital_domain() {
        let yaml = r#"
cirq: "0.3"
name: Digital
components:
  - id: U1
    type: and
    pins: { a: net_a, b: net_b, y: net_y }
"#;
        let circuit = parse_cirq(yaml).unwrap();
        match &circuit.components[0].kind {
            ComponentKind::DigitalGate { gate_type, .. } => {
                assert_eq!(*gate_type, DigitalGateType::And);
            }
            other => panic!("expected DigitalGate, got {other:?}"),
        }
        // Nets connected only to digital gates should be digital
        assert_eq!(circuit.nets["net_a"].domain, Domain::Digital);
        assert_eq!(circuit.nets["net_y"].domain, Domain::Digital);
    }

    // r[verify domain.inference.mixed]
    #[test]
    fn mixed_domain_inference() {
        let yaml = r#"
cirq: "0.3"
name: Mixed
components:
  - id: R1
    type: resistor
    value: "1k"
    pins: { p: shared, n: gnd }
  - id: U1
    type: buf
    pins: { a: shared, y: out }
"#;
        let circuit = parse_cirq(yaml).unwrap();
        // 'shared' is touched by both analog (resistor) and digital (buf)
        assert_eq!(circuit.nets["shared"].domain, Domain::Mixed);
    }
}
