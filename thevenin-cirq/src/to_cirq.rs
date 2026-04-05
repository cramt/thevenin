//! Serialize a [`Circuit`] IR to CirQ YAML or JSON.
//!
//! This is the inverse of [`crate::cirq_parse`]: it takes a fully-resolved
//! circuit IR and produces a valid CirQ document string.

use std::collections::BTreeMap;

use facet::Facet;

use crate::ir::*;

/// Errors that can occur during CirQ serialization.
#[derive(Debug, thiserror::Error)]
#[error("serialization failed: {0}")]
pub struct SerializeError(String);

/// Serialize a circuit to CirQ YAML.
pub fn to_yaml(circuit: &Circuit) -> Result<String, SerializeError> {
    let doc = circuit_to_doc(circuit);
    facet_yaml::to_string(&doc).map_err(|e| SerializeError(e.to_string()))
}

/// Serialize a circuit to CirQ JSON (pretty-printed).
pub fn to_json(circuit: &Circuit) -> Result<String, SerializeError> {
    let doc = circuit_to_doc(circuit);
    facet_json::to_string_pretty(&doc).map_err(|e| SerializeError(e.to_string()))
}

// ---------------------------------------------------------------------------
// Serializable document model
// ---------------------------------------------------------------------------

#[derive(Facet)]
#[facet(skip_all_unless_truthy)]
struct CirqOutDoc {
    cirq: String,
    name: String,
    #[facet(default)]
    description: Option<String>,
    components: Vec<CirqOutComponent>,
    #[facet(default)]
    subcircuits: Vec<CirqOutSubcircuit>,
    #[facet(default)]
    models: Vec<CirqOutModel>,
    #[facet(default)]
    params: BTreeMap<String, CirqOutVal>,
    #[facet(default)]
    globals: Vec<String>,
    #[facet(default)]
    includes: Vec<CirqOutInclude>,
    #[facet(default)]
    functions: Vec<CirqOutFunction>,
    #[facet(default)]
    options: BTreeMap<String, CirqOutVal>,
    #[facet(default)]
    temperature: Option<f64>,
}

#[derive(Facet)]
#[facet(skip_all_unless_truthy)]
struct CirqOutComponent {
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
    value: Option<CirqOutVal>,
    #[facet(default)]
    pins: Option<BTreeMap<String, String>>,
    #[facet(default)]
    params: BTreeMap<String, CirqOutVal>,
    #[facet(default)]
    waveform: Option<CirqOutWaveform>,
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
    coefficient: Option<CirqOutVal>,
    // Behavioral source
    #[facet(default)]
    off: Option<bool>,
    // Raw text (unrecognized SPICE elements)
    #[facet(default)]
    text: Option<String>,
}

#[derive(Facet)]
struct CirqOutWaveform {
    #[facet(rename = "type")]
    waveform_type: String,
    #[facet(flatten)]
    params: BTreeMap<String, CirqOutVal>,
}

#[derive(Facet)]
#[facet(skip_all_unless_truthy)]
struct CirqOutSubcircuit {
    name: String,
    #[facet(default)]
    description: Option<String>,
    #[facet(default)]
    params: BTreeMap<String, CirqOutVal>,
    components: Vec<CirqOutComponent>,
    #[facet(default)]
    models: Vec<CirqOutModel>,
    #[facet(default)]
    subcircuits: Vec<CirqOutSubcircuit>,
}

#[derive(Facet)]
#[facet(skip_all_unless_truthy)]
struct CirqOutModel {
    name: String,
    #[facet(rename = "type")]
    model_type: String,
    #[facet(default)]
    level: Option<u32>,
    #[facet(default)]
    params: BTreeMap<String, CirqOutVal>,
}

#[derive(Facet)]
#[facet(skip_all_unless_truthy)]
struct CirqOutInclude {
    file: String,
    #[facet(default)]
    section: Option<String>,
}

#[derive(Facet)]
struct CirqOutFunction {
    name: String,
    args: Vec<String>,
    body: String,
}

/// A serializable value — either a number or a string.
#[derive(Debug, Clone, Facet)]
#[facet(untagged)]
#[repr(u8)]
#[allow(dead_code)]
enum CirqOutVal {
    Number(f64),
    Text(String),
}

// ---------------------------------------------------------------------------
// IR → document conversion
// ---------------------------------------------------------------------------

fn circuit_to_doc(circuit: &Circuit) -> CirqOutDoc {
    CirqOutDoc {
        cirq: "0.3".to_string(),
        name: circuit.name.clone(),
        description: circuit.description.clone(),
        components: circuit.components.iter().map(component_to_out).collect(),
        subcircuits: circuit.subcircuits.iter().map(subcircuit_to_out).collect(),
        models: circuit.models.iter().map(model_to_out).collect(),
        params: value_map_to_out(&circuit.params),
        globals: circuit.globals.clone(),
        includes: circuit
            .includes
            .iter()
            .map(|i| CirqOutInclude {
                file: i.file.clone(),
                section: i.section.clone(),
            })
            .collect(),
        functions: circuit
            .functions
            .iter()
            .map(|f| CirqOutFunction {
                name: f.name.clone(),
                args: f.args.clone(),
                body: f.body.clone(),
            })
            .collect(),
        options: value_map_to_out(&circuit.options),
        temperature: circuit.temperature,
    }
}

fn component_to_out(comp: &Component) -> CirqOutComponent {
    let mut out = CirqOutComponent {
        id: comp.id.clone(),
        comp_type: String::new(),
        description: comp.description.clone(),
        tags: comp.tags.clone(),
        model: None,
        value: None,
        pins: None,
        params: BTreeMap::new(),
        waveform: None,
        net: None,
        direction: None,
        order: None,
        domain: None,
        inductors: None,
        coefficient: None,
        off: None,
        text: None,
    };

    match &comp.kind {
        ComponentKind::Resistor {
            p,
            n,
            value,
            params,
        } => {
            out.comp_type = "resistor".into();
            out.pins = Some(two_pin_map(p, n));
            out.value = Some(value_to_out(value));
            out.params = value_map_to_out(params);
        }

        ComponentKind::Capacitor {
            p,
            n,
            value,
            params,
        } => {
            out.comp_type = "capacitor".into();
            out.pins = Some(two_pin_map(p, n));
            out.value = Some(value_to_out(value));
            out.params = value_map_to_out(params);
        }

        ComponentKind::Inductor {
            p,
            n,
            value,
            params,
        } => {
            out.comp_type = "inductor".into();
            out.pins = Some(two_pin_map(p, n));
            out.value = Some(value_to_out(value));
            out.params = value_map_to_out(params);
        }

        ComponentKind::Coupling {
            l1,
            l2,
            coefficient,
        } => {
            out.comp_type = "coupling".into();
            out.inductors = Some(vec![l1.clone(), l2.clone()]);
            out.coefficient = Some(value_to_out(coefficient));
        }

        ComponentKind::Diode {
            a,
            k,
            model,
            params,
        } => {
            out.comp_type = "diode".into();
            out.pins = Some(BTreeMap::from([
                ("a".into(), a.clone()),
                ("k".into(), k.clone()),
            ]));
            out.model = Some(model.clone());
            out.params = value_map_to_out(params);
        }

        ComponentKind::Bjt {
            polarity,
            c,
            b,
            e,
            s,
            model,
            params,
            off,
        } => {
            out.comp_type = match polarity {
                BjtPolarity::Npn => "npn",
                BjtPolarity::Pnp => "pnp",
            }
            .into();
            let mut pins = BTreeMap::from([
                ("c".into(), c.clone()),
                ("b".into(), b.clone()),
                ("e".into(), e.clone()),
            ]);
            if let Some(sub) = s {
                pins.insert("s".into(), sub.clone());
            }
            out.pins = Some(pins);
            out.model = Some(model.clone());
            out.params = value_map_to_out(params);
            if *off {
                out.off = Some(true);
            }
        }

        ComponentKind::Mosfet {
            polarity,
            d,
            g,
            s,
            b,
            body,
            model,
            params,
        } => {
            out.comp_type = match polarity {
                MosfetPolarity::Nmos => "nmos",
                MosfetPolarity::Pmos => "pmos",
            }
            .into();
            let mut pins = BTreeMap::from([
                ("d".into(), d.clone()),
                ("g".into(), g.clone()),
                ("s".into(), s.clone()),
                ("b".into(), b.clone()),
            ]);
            if let Some(bd) = body {
                pins.insert("body".into(), bd.clone());
            }
            out.pins = Some(pins);
            out.model = Some(model.clone());
            out.params = value_map_to_out(params);
        }

        ComponentKind::Jfet {
            polarity,
            d,
            g,
            s,
            model,
            params,
        } => {
            out.comp_type = match polarity {
                JfetPolarity::Njfet => "njfet",
                JfetPolarity::Pjfet => "pjfet",
            }
            .into();
            out.pins = Some(BTreeMap::from([
                ("d".into(), d.clone()),
                ("g".into(), g.clone()),
                ("s".into(), s.clone()),
            ]));
            out.model = Some(model.clone());
            out.params = value_map_to_out(params);
        }

        ComponentKind::Mesfet {
            d,
            g,
            s,
            model,
            params,
        } => {
            out.comp_type = "mesfet".into();
            out.pins = Some(BTreeMap::from([
                ("d".into(), d.clone()),
                ("g".into(), g.clone()),
                ("s".into(), s.clone()),
            ]));
            out.model = Some(model.clone());
            out.params = value_map_to_out(params);
        }

        ComponentKind::VSource { p, n, source } => {
            out.comp_type = "vsource".into();
            out.pins = Some(two_pin_map(p, n));
            apply_source_spec(&mut out, source);
        }

        ComponentKind::ISource { p, n, source } => {
            out.comp_type = "isource".into();
            out.pins = Some(two_pin_map(p, n));
            apply_source_spec(&mut out, source);
        }

        ComponentKind::Vcvs { p, n, cp, cn, gain } => {
            out.comp_type = "vcvs".into();
            out.pins = Some(four_pin_map(p, n, cp, cn));
            out.params.insert("gain".into(), value_to_out(gain));
        }

        ComponentKind::Vccs { p, n, cp, cn, gm } => {
            out.comp_type = "vccs".into();
            out.pins = Some(four_pin_map(p, n, cp, cn));
            out.params.insert("gm".into(), value_to_out(gm));
        }

        ComponentKind::Cccs {
            p,
            n,
            vsource,
            gain,
        } => {
            out.comp_type = "cccs".into();
            out.pins = Some(two_pin_map(p, n));
            out.params
                .insert("vsource".into(), CirqOutVal::Text(vsource.clone()));
            out.params.insert("gain".into(), value_to_out(gain));
        }

        ComponentKind::Ccvs {
            p,
            n,
            vsource,
            transresistance,
        } => {
            out.comp_type = "ccvs".into();
            out.pins = Some(two_pin_map(p, n));
            out.params
                .insert("vsource".into(), CirqOutVal::Text(vsource.clone()));
            out.params
                .insert("transresistance".into(), value_to_out(transresistance));
        }

        ComponentKind::BehavioralSource { p, n, expr } => {
            out.comp_type = "bsource".into();
            out.pins = Some(two_pin_map(p, n));
            match expr {
                BehavioralExpr::Voltage(e) => {
                    out.params.insert("v".into(), CirqOutVal::Text(e.clone()));
                }
                BehavioralExpr::Current(e) => {
                    out.params.insert("i".into(), CirqOutVal::Text(e.clone()));
                }
            }
        }

        ComponentKind::VSwitch {
            p,
            n,
            cp,
            cn,
            model,
            params,
        } => {
            out.comp_type = "vswitch".into();
            out.pins = Some(four_pin_map(p, n, cp, cn));
            out.model = Some(model.clone());
            out.params = value_map_to_out(params);
        }

        ComponentKind::ISwitch {
            p,
            n,
            vsource,
            model,
            params,
        } => {
            out.comp_type = "iswitch".into();
            out.pins = Some(two_pin_map(p, n));
            out.model = Some(model.clone());
            let mut p_out = value_map_to_out(params);
            p_out.insert("vsource".into(), CirqOutVal::Text(vsource.clone()));
            out.params = p_out;
        }

        ComponentKind::Tline {
            p1,
            n1,
            p2,
            n2,
            params,
        } => {
            out.comp_type = "tline".into();
            out.pins = Some(tline_pin_map(p1, n1, p2, n2));
            out.params = value_map_to_out(params);
        }

        ComponentKind::Ltra {
            p1,
            n1,
            p2,
            n2,
            model,
            params,
        } => {
            out.comp_type = "ltra".into();
            out.pins = Some(tline_pin_map(p1, n1, p2, n2));
            out.model = Some(model.clone());
            out.params = value_map_to_out(params);
        }

        ComponentKind::Txl {
            p1,
            n1,
            p2,
            n2,
            model,
            params,
        } => {
            out.comp_type = "txl".into();
            out.pins = Some(tline_pin_map(p1, n1, p2, n2));
            out.model = Some(model.clone());
            out.params = value_map_to_out(params);
        }

        ComponentKind::Xspice { connections, model } => {
            out.comp_type = "xspice".into();
            out.model = Some(model.clone());
            // Flatten XSPICE connections into a pin map with numeric keys
            let mut pins = BTreeMap::new();
            for (i, conn) in connections.iter().enumerate() {
                match conn {
                    XspicePort::Scalar(s) => {
                        pins.insert(i.to_string(), s.clone());
                    }
                    XspicePort::Array(arr) => {
                        for (j, s) in arr.iter().enumerate() {
                            pins.insert(format!("{i}.{j}"), s.clone());
                        }
                    }
                }
            }
            out.pins = Some(pins);
        }

        ComponentKind::SubcktInstance {
            subckt,
            pins,
            params,
        } => {
            out.comp_type = subckt.clone();
            out.pins = Some(pins.clone());
            out.params = value_map_to_out(params);
        }

        ComponentKind::Port {
            net,
            direction,
            order,
            domain_override,
        } => {
            out.comp_type = "port".into();
            out.net = Some(net.clone());
            out.direction = Some(direction.to_string());
            out.order = Some(*order);
            out.domain = domain_override.map(|d| d.to_string());
        }

        ComponentKind::Cell { model, pins } => {
            out.comp_type = "cell".into();
            out.model = Some(model.clone());
            out.pins = Some(pins.clone());
        }

        ComponentKind::DigitalGate { gate_type, pins } => {
            out.comp_type = gate_type.to_string();
            out.pins = Some(pins.clone());
        }

        ComponentKind::Raw { text } => {
            out.comp_type = "raw".into();
            out.text = Some(text.clone());
        }
    }

    out
}

fn subcircuit_to_out(sub: &Subcircuit) -> CirqOutSubcircuit {
    CirqOutSubcircuit {
        name: sub.name.clone(),
        description: sub.description.clone(),
        params: value_map_to_out(&sub.params),
        components: sub.components.iter().map(component_to_out).collect(),
        models: sub.models.iter().map(model_to_out).collect(),
        subcircuits: sub.subcircuits.iter().map(subcircuit_to_out).collect(),
    }
}

fn model_to_out(m: &Model) -> CirqOutModel {
    CirqOutModel {
        name: m.name.clone(),
        model_type: m.model_type.to_string(),
        level: m.level,
        params: value_map_to_out(&m.params),
    }
}

// ---------------------------------------------------------------------------
// Value helpers
// ---------------------------------------------------------------------------

fn value_to_out(v: &Value) -> CirqOutVal {
    match v {
        Value::Num(n) => CirqOutVal::Number(*n),
        Value::Param(s) => CirqOutVal::Text(s.clone()),
        Value::Expr(s) => CirqOutVal::Text(format!("{{{s}}}")),
    }
}

fn value_map_to_out(map: &BTreeMap<String, Value>) -> BTreeMap<String, CirqOutVal> {
    map.iter()
        .map(|(k, v)| (k.clone(), value_to_out(v)))
        .collect()
}

fn two_pin_map(p: &str, n: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("p".into(), p.into()), ("n".into(), n.into())])
}

fn four_pin_map(p: &str, n: &str, cp: &str, cn: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("p".into(), p.into()),
        ("n".into(), n.into()),
        ("cp".into(), cp.into()),
        ("cn".into(), cn.into()),
    ])
}

fn tline_pin_map(p1: &str, n1: &str, p2: &str, n2: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("p1".into(), p1.into()),
        ("n1".into(), n1.into()),
        ("p2".into(), p2.into()),
        ("n2".into(), n2.into()),
    ])
}

fn apply_source_spec(out: &mut CirqOutComponent, source: &SourceSpec) {
    if let Some(dc) = &source.dc {
        out.value = Some(value_to_out(dc));
    }
    if let Some(ac) = &source.ac_mag {
        out.params.insert("ac_mag".into(), value_to_out(ac));
    }
    if let Some(ph) = &source.ac_phase {
        out.params.insert("ac_phase".into(), value_to_out(ph));
    }
    if let Some(wf) = &source.waveform {
        out.waveform = Some(waveform_to_out(wf));
    }
}

fn waveform_to_out(wf: &Waveform) -> CirqOutWaveform {
    let mut params = BTreeMap::new();

    let waveform_type = match wf {
        Waveform::Pulse {
            v1,
            v2,
            td,
            tr,
            tf,
            pw,
            per,
        } => {
            params.insert("v1".into(), value_to_out(v1));
            params.insert("v2".into(), value_to_out(v2));
            insert_opt(&mut params, "td", td);
            insert_opt(&mut params, "tr", tr);
            insert_opt(&mut params, "tf", tf);
            insert_opt(&mut params, "pw", pw);
            insert_opt(&mut params, "per", per);
            "pulse"
        }
        Waveform::Sin {
            v0,
            va,
            freq,
            td,
            theta,
            phi,
        } => {
            params.insert("v0".into(), value_to_out(v0));
            params.insert("va".into(), value_to_out(va));
            insert_opt(&mut params, "freq", freq);
            insert_opt(&mut params, "td", td);
            insert_opt(&mut params, "theta", theta);
            insert_opt(&mut params, "phi", phi);
            "sin"
        }
        Waveform::Exp {
            v1,
            v2,
            td1,
            tau1,
            td2,
            tau2,
        } => {
            params.insert("v1".into(), value_to_out(v1));
            params.insert("v2".into(), value_to_out(v2));
            insert_opt(&mut params, "td1", td1);
            insert_opt(&mut params, "tau1", tau1);
            insert_opt(&mut params, "td2", td2);
            insert_opt(&mut params, "tau2", tau2);
            "exp"
        }
        Waveform::Pwl { points } => {
            // PWL points as a sequence of [time, value] pairs.
            // Stored as flattened key-value since CirqOutVal doesn't nest sequences.
            for (i, (t, v)) in points.iter().enumerate() {
                params.insert(format!("t{i}"), value_to_out(t));
                params.insert(format!("v{i}"), value_to_out(v));
            }
            "pwl"
        }
        Waveform::Sffm { v0, va, fc, fs, md } => {
            params.insert("v0".into(), value_to_out(v0));
            params.insert("va".into(), value_to_out(va));
            insert_opt(&mut params, "fc", fc);
            insert_opt(&mut params, "fs", fs);
            insert_opt(&mut params, "md", md);
            "sffm"
        }
        Waveform::Am { va, vo, fc, fs, td } => {
            params.insert("va".into(), value_to_out(va));
            params.insert("vo".into(), value_to_out(vo));
            params.insert("fc".into(), value_to_out(fc));
            params.insert("fs".into(), value_to_out(fs));
            insert_opt(&mut params, "td", td);
            "am"
        }
    };

    CirqOutWaveform {
        waveform_type: waveform_type.into(),
        params,
    }
}

fn insert_opt(map: &mut BTreeMap<String, CirqOutVal>, key: &str, val: &Option<Value>) {
    if let Some(v) = val {
        map.insert(key.into(), value_to_out(v));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cirq_parse::parse_cirq;
    use crate::from_spice::from_spice;

    #[test]
    fn round_trip_spice_to_yaml_to_ir() {
        let spice = "\
RC Filter
R1 in out 1k
C1 out 0 100n
.end
";
        let ir1 = from_spice(spice).unwrap();
        let yaml = to_yaml(&ir1).unwrap();
        let ir2 = parse_cirq(&yaml).unwrap();

        assert_eq!(ir1.name, ir2.name);
        assert_eq!(ir1.components.len(), ir2.components.len());

        // Check net domains match
        for (name, net) in &ir1.nets {
            assert!(ir2.nets.contains_key(name), "missing net {name}");
            assert_eq!(
                net.domain, ir2.nets[name].domain,
                "domain mismatch for {name}"
            );
        }
    }

    #[test]
    fn round_trip_spice_to_json_to_ir() {
        let spice = "\
CMOS
.MODEL NMOD NMOS LEVEL=1 VTO=0.7
.MODEL PMOD PMOS LEVEL=1 VTO=-0.7
M1 out in vdd vdd PMOD W=10u L=1u
M2 out in 0 0 NMOD W=5u L=1u
.end
";
        let ir1 = from_spice(spice).unwrap();
        let json = to_json(&ir1).unwrap();
        let ir2 = parse_cirq(&json).unwrap();

        assert_eq!(ir1.name, ir2.name);
        assert_eq!(ir1.components.len(), ir2.components.len());
        assert_eq!(ir1.models.len(), ir2.models.len());
    }

    #[test]
    fn yaml_output_contains_expected_fields() {
        let spice = "\
Test
V1 in 0 DC 5 PULSE(0 5 1n 1n 1n 10n 20n)
R1 in out 1k
.PARAM freq=1meg
.end
";
        let ir = from_spice(spice).unwrap();
        let yaml = to_yaml(&ir).unwrap();

        assert!(yaml.contains("cirq:"));
        assert!(yaml.contains("name:"));
        assert!(yaml.contains("resistor"));
        assert!(yaml.contains("vsource"));
        assert!(yaml.contains("pulse"));
    }
}
