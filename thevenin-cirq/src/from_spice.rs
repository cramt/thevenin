//! Convert a `thevenin_types::Netlist` (SPICE) into the CirQ [`Circuit`] IR.
//!
//! Analysis commands, comments, `.save`, and other non-structural items are
//! silently ignored — they are simulation concerns, not circuit structure.

use std::collections::BTreeMap;

use thevenin_types::{
    ElementKind, Expr, Item, ModelDef, Netlist, Param, Source, SubcktDef,
    Waveform as SpiceWaveform, XspiceConnection,
};

use crate::ir::*;

/// Errors that can occur during SPICE → IR conversion.
#[derive(Debug, thiserror::Error)]
pub enum SpiceLowerError {
    #[error("parse error: {0}")]
    Parse(#[from] thevenin_types::ParseError),
}

/// Parse SPICE text and lower it to the CirQ IR.
///
/// Uses the first netlist fork (ignoring analysis type — CirQ is structural only).
pub fn from_spice(input: &str) -> Result<Circuit, SpiceLowerError> {
    let netlists = Netlist::parse(input)?;
    // CirQ only cares about circuit structure; pick the last fork which has
    // the most accumulated items.
    let netlist = netlists
        .last()
        .ok_or(SpiceLowerError::Parse(thevenin_types::ParseError::Empty))?;
    lower_netlist(netlist)
}

/// Lower an already-parsed SPICE netlist to the CirQ IR.
pub fn lower_netlist(netlist: &Netlist) -> Result<Circuit, SpiceLowerError> {
    // First pass: collect model definitions so we can resolve BJT/MOSFET/JFET polarity
    let model_types = collect_model_types(&netlist.items);

    let mut circuit = Circuit {
        name: netlist.title.clone(),
        description: None,
        components: Vec::new(),
        models: Vec::new(),
        subcircuits: Vec::new(),
        params: BTreeMap::new(),
        globals: Vec::new(),
        includes: Vec::new(),
        functions: Vec::new(),
        options: BTreeMap::new(),
        temperature: None,
        nets: BTreeMap::new(),
    };

    for item in &netlist.items {
        lower_item(item, &model_types, &mut circuit)?;
    }

    circuit.resolve_domains();
    Ok(circuit)
}

// ---------------------------------------------------------------------------
// Model type collection (for polarity resolution)
// ---------------------------------------------------------------------------

/// Map from model name (lowercased) → SPICE model kind string (uppercased).
fn collect_model_types(items: &[Item]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for item in items {
        match item {
            Item::Model(m) => {
                map.insert(m.name.to_lowercase(), m.kind.to_uppercase());
            }
            Item::Subckt(s) => {
                let inner = collect_model_types(&s.items);
                map.extend(inner);
            }
            _ => {}
        }
    }
    map
}

// ---------------------------------------------------------------------------
// Item lowering
// ---------------------------------------------------------------------------

fn lower_item(
    item: &Item,
    model_types: &BTreeMap<String, String>,
    circuit: &mut Circuit,
) -> Result<(), SpiceLowerError> {
    match item {
        Item::Element(el) => {
            let comp = lower_element(&el.name, &el.kind, model_types)?;
            circuit.components.push(comp);
        }
        Item::Model(m) => {
            circuit.models.push(lower_model(m));
        }
        Item::Subckt(s) => {
            circuit.subcircuits.push(lower_subckt(s, model_types)?);
        }
        Item::Param(ps) => {
            for p in ps {
                circuit.params.insert(p.name.clone(), lower_expr(&p.value));
            }
        }
        Item::Global(nodes) => {
            circuit.globals.extend(nodes.iter().cloned());
        }
        Item::Include(path) => {
            circuit.includes.push(Include {
                file: path.clone(),
                section: None,
            });
        }
        Item::Lib { file, entry } => {
            circuit.includes.push(Include {
                file: file.clone(),
                section: entry.clone(),
            });
        }
        Item::Func { name, args, body } => {
            circuit.functions.push(Function {
                name: name.clone(),
                args: args.clone(),
                body: body.clone(),
            });
        }
        Item::Options(ps) => {
            for p in ps {
                circuit.options.insert(p.name.clone(), lower_expr(&p.value));
            }
        }
        Item::Temp(t) => {
            circuit.temperature = Some(*t);
        }
        // Ignored: Comment, Save, Raw, Control
        Item::Comment(_) | Item::Save(_) | Item::Raw(_) | Item::Control(_) => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Element lowering
// ---------------------------------------------------------------------------

fn lower_element(
    name: &str,
    kind: &ElementKind,
    model_types: &BTreeMap<String, String>,
) -> Result<Component, SpiceLowerError> {
    let ckind = match kind {
        ElementKind::Resistor {
            pos,
            neg,
            value,
            params,
        } => ComponentKind::Resistor {
            p: pos.clone(),
            n: neg.clone(),
            value: lower_expr(value),
            params: lower_params(params),
        },

        ElementKind::Capacitor {
            pos,
            neg,
            value,
            params,
        } => ComponentKind::Capacitor {
            p: pos.clone(),
            n: neg.clone(),
            value: lower_expr(value),
            params: lower_params(params),
        },

        ElementKind::Inductor {
            pos,
            neg,
            value,
            params,
        } => ComponentKind::Inductor {
            p: pos.clone(),
            n: neg.clone(),
            value: lower_expr(value),
            params: lower_params(params),
        },

        ElementKind::VoltageSource { pos, neg, source } => ComponentKind::VSource {
            p: pos.clone(),
            n: neg.clone(),
            source: lower_source(source),
        },

        ElementKind::CurrentSource { pos, neg, source } => ComponentKind::ISource {
            p: pos.clone(),
            n: neg.clone(),
            source: lower_source(source),
        },

        ElementKind::Diode {
            anode,
            cathode,
            model,
            params,
        } => ComponentKind::Diode {
            a: anode.clone(),
            k: cathode.clone(),
            model: model.clone(),
            params: lower_params(params),
        },

        ElementKind::Bjt {
            c,
            b,
            e,
            substrate,
            model,
            params,
            off,
        } => {
            let polarity = resolve_bjt_polarity(name, model, model_types)?;
            ComponentKind::Bjt {
                polarity,
                c: c.clone(),
                b: b.clone(),
                e: e.clone(),
                s: substrate.clone(),
                model: model.clone(),
                params: lower_params(params),
                off: *off,
            }
        }

        ElementKind::Mosfet {
            d,
            g,
            s,
            bulk,
            body,
            model,
            params,
        } => {
            let polarity = resolve_mosfet_polarity(name, model, model_types)?;
            ComponentKind::Mosfet {
                polarity,
                d: d.clone(),
                g: g.clone(),
                s: s.clone(),
                b: bulk.clone(),
                body: body.clone(),
                model: model.clone(),
                params: lower_params(params),
            }
        }

        ElementKind::Jfet {
            d,
            g,
            s,
            model,
            params,
        } => {
            let polarity = resolve_jfet_polarity(name, model, model_types)?;
            ComponentKind::Jfet {
                polarity,
                d: d.clone(),
                g: g.clone(),
                s: s.clone(),
                model: model.clone(),
                params: lower_params(params),
            }
        }

        ElementKind::Mesa {
            d,
            g,
            s,
            model,
            params,
        } => ComponentKind::Mesfet {
            d: d.clone(),
            g: g.clone(),
            s: s.clone(),
            model: model.clone(),
            params: lower_params(params),
        },

        ElementKind::MutualCoupling { l1, l2, coupling } => ComponentKind::Coupling {
            l1: l1.clone(),
            l2: l2.clone(),
            coefficient: lower_expr(coupling),
        },

        ElementKind::Vcvs {
            out_pos,
            out_neg,
            in_pos,
            in_neg,
            gain,
        } => ComponentKind::Vcvs {
            p: out_pos.clone(),
            n: out_neg.clone(),
            cp: in_pos.clone(),
            cn: in_neg.clone(),
            gain: lower_expr(gain),
        },

        ElementKind::Cccs {
            out_pos,
            out_neg,
            vsrc,
            gain,
        } => ComponentKind::Cccs {
            p: out_pos.clone(),
            n: out_neg.clone(),
            vsource: vsrc.clone(),
            gain: lower_expr(gain),
        },

        ElementKind::Vccs {
            out_pos,
            out_neg,
            in_pos,
            in_neg,
            gm,
        } => ComponentKind::Vccs {
            p: out_pos.clone(),
            n: out_neg.clone(),
            cp: in_pos.clone(),
            cn: in_neg.clone(),
            gm: lower_expr(gm),
        },

        ElementKind::Ccvs {
            out_pos,
            out_neg,
            vsrc,
            rm,
        } => ComponentKind::Ccvs {
            p: out_pos.clone(),
            n: out_neg.clone(),
            vsource: vsrc.clone(),
            transresistance: lower_expr(rm),
        },

        ElementKind::BehavioralSource { pos, neg, spec } => {
            let expr = parse_behavioral_spec(spec);
            ComponentKind::BehavioralSource {
                p: pos.clone(),
                n: neg.clone(),
                expr,
            }
        }

        ElementKind::SubcktCall {
            ports,
            subckt,
            params,
        } => {
            // SPICE subckt calls are positional — we store them with numeric keys
            // until subcircuit resolution can map them to port names.
            let pins: BTreeMap<String, String> = ports
                .iter()
                .enumerate()
                .map(|(i, net)| (i.to_string(), net.clone()))
                .collect();
            ComponentKind::SubcktInstance {
                subckt: subckt.clone(),
                pins,
                params: lower_params(params),
            }
        }

        ElementKind::Ltra {
            pos1,
            neg1,
            pos2,
            neg2,
            model,
            params,
        } => ComponentKind::Ltra {
            p1: pos1.clone(),
            n1: neg1.clone(),
            p2: pos2.clone(),
            n2: neg2.clone(),
            model: model.clone(),
            params: lower_params(params),
        },

        ElementKind::Txl {
            pos1,
            neg1,
            pos2,
            neg2,
            model,
            params,
        } => ComponentKind::Txl {
            p1: pos1.clone(),
            n1: neg1.clone(),
            p2: pos2.clone(),
            n2: neg2.clone(),
            model: model.clone(),
            params: lower_params(params),
        },

        ElementKind::Cpl {
            in_nodes,
            out_nodes,
            gnd: _,
            model,
            params: _,
        } => {
            // CPL maps to a transmission line with all nodes as params
            // For now store as Xspice-like since CPL is exotic
            let mut all_ports: Vec<XspicePort> = Vec::new();
            for n in in_nodes {
                all_ports.push(XspicePort::Scalar(n.clone()));
            }
            for n in out_nodes {
                all_ports.push(XspicePort::Scalar(n.clone()));
            }
            ComponentKind::Xspice {
                connections: all_ports,
                model: model.clone(),
            }
        }

        ElementKind::Xspice { connections, model } => {
            let ports = connections
                .iter()
                .map(|c| match c {
                    XspiceConnection::Scalar(s) => XspicePort::Scalar(s.clone()),
                    XspiceConnection::Array(a) => XspicePort::Array(a.clone()),
                })
                .collect();
            ComponentKind::Xspice {
                connections: ports,
                model: model.clone(),
            }
        }

        ElementKind::Raw(rest) => {
            // Store as behavioral source with the raw text — best effort
            ComponentKind::BehavioralSource {
                p: "0".into(),
                n: "0".into(),
                expr: BehavioralExpr::Voltage(rest.clone()),
            }
        }
    };

    Ok(Component {
        id: name.to_string(),
        description: None,
        tags: Vec::new(),
        kind: ckind,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn lower_expr(expr: &Expr) -> Value {
    match expr {
        Expr::Num(v) => Value::Num(*v),
        Expr::Param(s) => Value::Param(s.clone()),
        Expr::Brace(s) => Value::Expr(s.clone()),
    }
}

fn lower_params(params: &[Param]) -> BTreeMap<String, Value> {
    params
        .iter()
        .map(|p| (p.name.clone(), lower_expr(&p.value)))
        .collect()
}

fn lower_source(source: &Source) -> SourceSpec {
    SourceSpec {
        dc: source.dc.as_ref().map(lower_expr),
        ac_mag: source.ac.as_ref().map(|a| lower_expr(&a.mag)),
        ac_phase: source
            .ac
            .as_ref()
            .and_then(|a| a.phase.as_ref().map(lower_expr)),
        waveform: source.waveform.as_ref().map(lower_waveform),
    }
}

fn lower_waveform(w: &SpiceWaveform) -> Waveform {
    match w {
        SpiceWaveform::Pulse {
            v1,
            v2,
            td,
            tr,
            tf,
            pw,
            per,
        } => Waveform::Pulse {
            v1: lower_expr(v1),
            v2: lower_expr(v2),
            td: td.as_ref().map(lower_expr),
            tr: tr.as_ref().map(lower_expr),
            tf: tf.as_ref().map(lower_expr),
            pw: pw.as_ref().map(lower_expr),
            per: per.as_ref().map(lower_expr),
        },
        SpiceWaveform::Sin {
            v0,
            va,
            freq,
            td,
            theta,
            phi,
        } => Waveform::Sin {
            v0: lower_expr(v0),
            va: lower_expr(va),
            freq: freq.as_ref().map(lower_expr),
            td: td.as_ref().map(lower_expr),
            theta: theta.as_ref().map(lower_expr),
            phi: phi.as_ref().map(lower_expr),
        },
        SpiceWaveform::Exp {
            v1,
            v2,
            td1,
            tau1,
            td2,
            tau2,
        } => Waveform::Exp {
            v1: lower_expr(v1),
            v2: lower_expr(v2),
            td1: td1.as_ref().map(lower_expr),
            tau1: tau1.as_ref().map(lower_expr),
            td2: td2.as_ref().map(lower_expr),
            tau2: tau2.as_ref().map(lower_expr),
        },
        SpiceWaveform::Pwl(points) => Waveform::Pwl {
            points: points
                .iter()
                .map(|pt| (lower_expr(&pt.time), lower_expr(&pt.value)))
                .collect(),
        },
        SpiceWaveform::Sffm { v0, va, fc, fs, md } => Waveform::Sffm {
            v0: lower_expr(v0),
            va: lower_expr(va),
            fc: fc.as_ref().map(lower_expr),
            fs: fs.as_ref().map(lower_expr),
            md: md.as_ref().map(lower_expr),
        },
        SpiceWaveform::Am { va, vo, fc, fs, td } => Waveform::Am {
            va: lower_expr(va),
            vo: lower_expr(vo),
            fc: lower_expr(fc),
            fs: lower_expr(fs),
            td: td.as_ref().map(lower_expr),
        },
    }
}

fn lower_model(m: &ModelDef) -> Model {
    let kind_upper = m.kind.to_uppercase();
    let (model_type, level) = parse_model_type_and_level(&kind_upper, &m.params);
    let mut params = lower_params(&m.params);
    // Remove LEVEL from params since it's a top-level field
    params.remove("LEVEL");
    params.remove("level");

    Model {
        name: m.name.clone(),
        model_type,
        level,
        params,
    }
}

fn parse_model_type_and_level(kind: &str, params: &[Param]) -> (ModelType, Option<u32>) {
    let mt = match kind {
        "D" | "DIODE" => ModelType::Diode,
        "NPN" => ModelType::Npn,
        "PNP" => ModelType::Pnp,
        "NMOS" => ModelType::Nmos,
        "PMOS" => ModelType::Pmos,
        "NJF" | "NJFET" => ModelType::Njfet,
        "PJF" | "PJFET" => ModelType::Pjfet,
        "MESA" => ModelType::Mesfet,
        "LTRA" => ModelType::Ltra,
        "TXL" => ModelType::Txl,
        "CPL" => ModelType::Cpl,
        "SW" | "VSWITCH" => ModelType::VSwitch,
        "CSW" | "ISWITCH" => ModelType::ISwitch,
        other => ModelType::Other(other.to_string()),
    };

    let level = params.iter().find_map(|p| {
        if p.name.eq_ignore_ascii_case("level") {
            if let Expr::Num(v) = &p.value {
                Some(*v as u32)
            } else {
                None
            }
        } else {
            None
        }
    });

    (mt, level)
}

fn lower_subckt(
    s: &SubcktDef,
    model_types: &BTreeMap<String, String>,
) -> Result<Subcircuit, SpiceLowerError> {
    // Merge parent model types with local ones
    let mut local_types = model_types.clone();
    let inner_types = collect_model_types(&s.items);
    local_types.extend(inner_types);

    let mut subckt = Subcircuit {
        name: s.name.clone(),
        description: None,
        params: lower_params(&s.params),
        components: Vec::new(),
        models: Vec::new(),
        subcircuits: Vec::new(),
    };

    // Create port components from the SPICE port list
    for (i, port_name) in s.ports.iter().enumerate() {
        subckt.components.push(Component {
            id: port_name.clone(),
            description: None,
            tags: Vec::new(),
            kind: ComponentKind::Port {
                net: port_name.clone(),
                direction: Direction::InOut, // SPICE doesn't specify direction
                order: i as u32,
                domain_override: None,
            },
        });
    }

    for item in &s.items {
        match item {
            Item::Element(el) => {
                let comp = lower_element(&el.name, &el.kind, &local_types)?;
                subckt.components.push(comp);
            }
            Item::Model(m) => {
                subckt.models.push(lower_model(m));
            }
            Item::Subckt(inner) => {
                subckt.subcircuits.push(lower_subckt(inner, &local_types)?);
            }
            Item::Param(ps) => {
                for p in ps {
                    subckt.params.insert(p.name.clone(), lower_expr(&p.value));
                }
            }
            // Ignore analysis, comments, etc. inside subcircuits too
            _ => {}
        }
    }

    Ok(subckt)
}

fn resolve_bjt_polarity(
    _name: &str,
    model_name: &str,
    model_types: &BTreeMap<String, String>,
) -> Result<BjtPolarity, SpiceLowerError> {
    match model_types
        .get(&model_name.to_lowercase())
        .map(|s| s.as_str())
    {
        Some("NPN") => Ok(BjtPolarity::Npn),
        Some("PNP") => Ok(BjtPolarity::Pnp),
        Some(_) | None => {
            // Heuristic: if model name contains npn/pnp (case-insensitive)
            let lower = model_name.to_lowercase();
            if lower.contains("pnp") || lower.starts_with('p') {
                Ok(BjtPolarity::Pnp)
            } else {
                // Default to NPN — most common, and model may be in an included file
                Ok(BjtPolarity::Npn)
            }
        }
    }
}

fn resolve_mosfet_polarity(
    _name: &str,
    model_name: &str,
    model_types: &BTreeMap<String, String>,
) -> Result<MosfetPolarity, SpiceLowerError> {
    match model_types
        .get(&model_name.to_lowercase())
        .map(|s| s.as_str())
    {
        Some("NMOS") => Ok(MosfetPolarity::Nmos),
        Some("PMOS") => Ok(MosfetPolarity::Pmos),
        Some(_) | None => {
            let lower = model_name.to_lowercase();
            if lower.contains("pmos") || lower.starts_with('p') {
                Ok(MosfetPolarity::Pmos)
            } else {
                // Default to NMOS — most common, and model may be in an included file
                Ok(MosfetPolarity::Nmos)
            }
        }
    }
}

fn resolve_jfet_polarity(
    _name: &str,
    model_name: &str,
    model_types: &BTreeMap<String, String>,
) -> Result<JfetPolarity, SpiceLowerError> {
    match model_types
        .get(&model_name.to_lowercase())
        .map(|s| s.as_str())
    {
        Some("NJF") | Some("NJFET") => Ok(JfetPolarity::Njfet),
        Some("PJF") | Some("PJFET") => Ok(JfetPolarity::Pjfet),
        Some(_) | None => {
            let lower = model_name.to_lowercase();
            if lower.contains("pjf") || lower.starts_with('p') {
                Ok(JfetPolarity::Pjfet)
            } else {
                // Default to NJFET — most common, and model may be in an included file
                Ok(JfetPolarity::Njfet)
            }
        }
    }
}

fn parse_behavioral_spec(spec: &str) -> BehavioralExpr {
    let trimmed = spec.trim();
    if let Some(rest) = trimmed.strip_prefix("V=") {
        BehavioralExpr::Voltage(rest.trim_matches(|c| c == '{' || c == '}').to_string())
    } else if let Some(rest) = trimmed.strip_prefix("I=") {
        BehavioralExpr::Current(rest.trim_matches(|c| c == '{' || c == '}').to_string())
    } else if let Some(rest) = trimmed.strip_prefix("v=") {
        BehavioralExpr::Voltage(rest.trim_matches(|c| c == '{' || c == '}').to_string())
    } else if let Some(rest) = trimmed.strip_prefix("i=") {
        BehavioralExpr::Current(rest.trim_matches(|c| c == '{' || c == '}').to_string())
    } else {
        // Default to voltage expression
        BehavioralExpr::Voltage(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // r[verify prim.resistor]
    // r[verify prim.capacitor]
    // r[verify net.implicit]
    // r[verify net.ground]
    // r[verify domain.inference.analog]
    #[test]
    fn basic_rc_circuit() {
        let spice = "\
RC Filter
R1 in out 1k
C1 out 0 100n
.end
";
        let circuit = from_spice(spice).unwrap();
        assert_eq!(circuit.name, "RC Filter");
        assert_eq!(circuit.components.len(), 2);

        // Check R1
        let r1 = &circuit.components[0];
        assert_eq!(r1.id, "R1");
        match &r1.kind {
            ComponentKind::Resistor { p, n, value, .. } => {
                assert_eq!(p, "in");
                assert_eq!(n, "out");
                assert!(matches!(value, Value::Num(v) if (*v - 1000.0).abs() < 1e-9));
            }
            other => panic!("expected Resistor, got {other:?}"),
        }

        // Check nets
        assert!(circuit.nets.contains_key("in"));
        assert!(circuit.nets.contains_key("out"));
        assert!(circuit.nets.contains_key("0"));
        // All nets should be analog (only passives connected)
        assert_eq!(circuit.nets["in"].domain, Domain::Analog);
        assert_eq!(circuit.nets["out"].domain, Domain::Analog);
    }

    // r[verify prim.nmos]
    // r[verify prim.pmos]
    // r[verify model.type]
    #[test]
    fn mosfet_polarity_from_model() {
        let spice = "\
CMOS
.MODEL NMOD NMOS LEVEL=1 VTO=0.7
.MODEL PMOD PMOS LEVEL=1 VTO=-0.7
M1 out in vdd vdd PMOD W=10u L=1u
M2 out in 0 0 NMOD W=5u L=1u
.end
";
        let circuit = from_spice(spice).unwrap();
        let m1 = &circuit.components[0];
        let m2 = &circuit.components[1];

        match &m1.kind {
            ComponentKind::Mosfet { polarity, .. } => {
                assert_eq!(*polarity, MosfetPolarity::Pmos);
            }
            other => panic!("expected Mosfet, got {other:?}"),
        }
        match &m2.kind {
            ComponentKind::Mosfet { polarity, .. } => {
                assert_eq!(*polarity, MosfetPolarity::Nmos);
            }
            other => panic!("expected Mosfet, got {other:?}"),
        }
    }

    #[test]
    fn analysis_commands_ignored() {
        let spice = "\
Test
V1 in 0 DC 5
R1 in out 1k
.op
.tran 1n 100n
.ac DEC 100 1 1G
.end
";
        let circuit = from_spice(spice).unwrap();
        // Only V1 and R1 should be in components — analysis is ignored
        assert_eq!(circuit.components.len(), 2);
    }

    // r[verify subckt.name]
    // r[verify subckt.components]
    // r[verify subckt.instantiation]
    #[test]
    fn subcircuit_lowering() {
        let spice = "\
Subckt Test
.SUBCKT buf in out
R1 in mid 1k
R2 mid out 1k
.ENDS buf
X1 a b buf
.end
";
        let circuit = from_spice(spice).unwrap();
        assert_eq!(circuit.subcircuits.len(), 1);
        assert_eq!(circuit.subcircuits[0].name, "buf");
        // 2 ports + 2 resistors
        assert_eq!(circuit.subcircuits[0].components.len(), 4);

        // X1 instance
        assert_eq!(circuit.components.len(), 1);
        match &circuit.components[0].kind {
            ComponentKind::SubcktInstance { subckt, pins, .. } => {
                assert_eq!(subckt, "buf");
                assert_eq!(pins.len(), 2);
            }
            other => panic!("expected SubcktInstance, got {other:?}"),
        }
    }

    // r[verify prim.vcvs]
    // r[verify prim.vccs]
    // r[verify prim.cccs]
    // r[verify prim.ccvs]
    #[test]
    fn controlled_sources() {
        let spice = "\
Controlled
E1 out 0 in 0 10
G1 out2 0 in 0 0.001
F1 out3 0 Vmeas 5
H1 out4 0 Vmeas 100
Vmeas sense 0 DC 0
.end
";
        let circuit = from_spice(spice).unwrap();
        assert_eq!(circuit.components.len(), 5);

        match &circuit.components[0].kind {
            ComponentKind::Vcvs { gain, .. } => {
                assert!(matches!(gain, Value::Num(v) if (*v - 10.0).abs() < 1e-9));
            }
            other => panic!("expected Vcvs, got {other:?}"),
        }
    }

    // r[verify source.waveform]
    // r[verify prim.vsource]
    #[test]
    fn source_with_waveform() {
        let spice = "\
Waveform
V1 in 0 DC 0 PULSE(0 5 1n 1n 1n 10n 20n)
.end
";
        let circuit = from_spice(spice).unwrap();
        match &circuit.components[0].kind {
            ComponentKind::VSource { source, .. } => {
                assert!(source.waveform.is_some());
                assert!(matches!(&source.waveform, Some(Waveform::Pulse { .. })));
            }
            other => panic!("expected VSource, got {other:?}"),
        }
    }

    // r[verify doc.params]
    // r[verify doc.globals]
    // r[verify doc.options]
    // r[verify doc.temperature]
    #[test]
    fn global_params_and_options() {
        let spice = "\
Params
.PARAM R1=1k R2=2k
.OPTIONS RELTOL=1e-4
.GLOBAL vdd vss
.TEMP 27
.end
";
        let circuit = from_spice(spice).unwrap();
        assert_eq!(circuit.params.len(), 2);
        assert_eq!(circuit.globals, vec!["vdd", "vss"]);
        assert_eq!(circuit.options.len(), 1);
        assert_eq!(circuit.temperature, Some(27.0));
    }

    // r[verify model.name]
    // r[verify model.type]
    // r[verify model.level]
    // r[verify model.params]
    #[test]
    fn model_lowering() {
        let spice = "\
Models
.MODEL D1N4148 D IS=2.52e-9 RS=0.568
.MODEL NMOD NMOS LEVEL=3 VTO=0.7 KP=110u
.end
";
        let circuit = from_spice(spice).unwrap();
        assert_eq!(circuit.models.len(), 2);

        assert_eq!(circuit.models[0].name, "D1N4148");
        assert_eq!(circuit.models[0].model_type, ModelType::Diode);
        assert!(circuit.models[0].level.is_none());

        assert_eq!(circuit.models[1].name, "NMOD");
        assert_eq!(circuit.models[1].model_type, ModelType::Nmos);
        assert_eq!(circuit.models[1].level, Some(3));
        // LEVEL should be extracted, not in params
        assert!(!circuit.models[1].params.contains_key("LEVEL"));
    }
}
