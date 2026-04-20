//! SPICE import — converts `thevenin_types::Netlist` into `cirq_ir::Circuit`.
//!
//! This crate provides the bridge from legacy SPICE netlists into the canonical
//! Cirq IR. It enables gradual migration: existing SPICE files can be imported
//! into the Cirq toolchain without manual rewriting.

use std::collections::HashMap;

use cirq_ir::{
    AcAnalysis, Analysis as IrAnalysis, Circuit, Connection, DcAnalysis, DcSweep as IrDcSweep,
    Element as IrElement, ElementKind as IrElementKind, FrequencyScale, Id, Model as IrModel, Net,
    NoiseAnalysis, PzAnalysis, PzType, ResolvedParam, SensAnalysis, TfAnalysis, TranAnalysis,
    TransferType, Value,
};
use thevenin_types::{
    AcVariation, Analysis as SpiceAnalysis, ElementKind as SpiceElementKind, Expr, Item, Netlist,
    Param, PzAnalysisType, PzInputType,
};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during SPICE-to-Cirq import.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// The underlying SPICE parser failed.
    #[error("SPICE parse error: {0}")]
    Parse(#[from] thevenin_types::ParseError),

    /// An element type that has no Cirq IR equivalent was encountered.
    #[error("unsupported element: {0}")]
    UnsupportedElement(String),

    /// A model kind string could not be mapped to a `DeviceType`.
    #[error("unknown model kind: {0}")]
    UnknownModelKind(String),

    /// A model referenced by an element was not found in the netlist.
    #[error("model not found: {0}")]
    ModelNotFound(String),

    /// A source referenced in an analysis command was not found.
    #[error("source not found: {0}")]
    SourceNotFound(String),

    /// An expression could not be evaluated to a numeric value.
    #[error("unevaluable expression: {0}")]
    UnevaluableExpr(String),
}

// ---------------------------------------------------------------------------
// Net interning table
// ---------------------------------------------------------------------------

/// Assigns unique `Id`s to node name strings. Ground ("0") always gets `Id(0)`.
struct NetTable {
    map: HashMap<String, Id>,
    next_id: u32,
    globals: Vec<String>,
}

impl NetTable {
    fn new() -> Self {
        let mut map = HashMap::new();
        map.insert("0".to_owned(), Id(0));
        Self {
            map,
            next_id: 1,
            globals: Vec::new(),
        }
    }

    /// Intern a node name, returning its `Id`. Creates a new entry if unseen.
    fn intern(&mut self, name: &str) -> Id {
        if let Some(&id) = self.map.get(name) {
            return id;
        }
        let id = Id(self.next_id);
        self.next_id += 1;
        self.map.insert(name.to_owned(), id);
        id
    }

    /// Mark a set of node names as global.
    fn mark_global(&mut self, names: &[String]) {
        for name in names {
            self.globals.push(name.clone());
            // Ensure the node is interned.
            self.intern(name);
        }
    }

    /// Produce the final `Vec<Net>`, sorted by id.
    fn into_nets(self) -> Vec<Net> {
        let mut nets: Vec<Net> = self
            .map
            .iter()
            .map(|(name, &id)| {
                let is_global = name == "0" || self.globals.iter().any(|g| g == name);
                Net {
                    id,
                    name: name.clone(),
                    is_global,
                }
            })
            .collect();
        nets.sort_by_key(|n| n.id.0);
        nets
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Try to extract a numeric f64 from an `Expr`. Returns `Err` for parameter
/// references and brace expressions that require evaluation.
fn expr_to_f64(expr: &Expr) -> Result<f64, ImportError> {
    match expr {
        Expr::Num(v) => Ok(*v),
        Expr::Param(name) => Err(ImportError::UnevaluableExpr(format!(
            "parameter reference: {name}"
        ))),
        Expr::Brace(s) => Err(ImportError::UnevaluableExpr(format!(
            "brace expression: {{{s}}}"
        ))),
    }
}

/// Convert an `Expr` to a `Value`.
fn expr_to_value(expr: &Expr) -> Value {
    match expr {
        Expr::Num(v) => Value::Real(*v),
        Expr::Param(s) => Value::String(s.clone()),
        Expr::Brace(s) => Value::String(format!("{{{s}}}")),
    }
}

/// Convert a slice of `thevenin_types::Param` to Cirq IR param pairs.
fn convert_params(params: &[Param]) -> Vec<(String, Value)> {
    params
        .iter()
        .map(|p| (p.name.clone(), expr_to_value(&p.value)))
        .collect()
}

fn connection(terminal: &str, net: Id) -> Connection {
    Connection {
        terminal: terminal.to_owned(),
        net,
    }
}

/// Map a SPICE model-kind string (e.g. "NPN", "D", "NMOS") to a `DeviceType`.
fn map_device_type(kind: &str) -> Result<cirq_ir::DeviceType, ImportError> {
    match kind.to_ascii_uppercase().as_str() {
        "D" => Ok(cirq_ir::DeviceType::Diode),
        "NPN" => Ok(cirq_ir::DeviceType::Npn),
        "PNP" => Ok(cirq_ir::DeviceType::Pnp),
        "NMOS" => Ok(cirq_ir::DeviceType::Nmos),
        "PMOS" => Ok(cirq_ir::DeviceType::Pmos),
        "NJF" => Ok(cirq_ir::DeviceType::NJfet),
        "PJF" => Ok(cirq_ir::DeviceType::PJfet),
        "NMF" | "GASFET" | "MESA" => Ok(cirq_ir::DeviceType::NMesfet),
        "PMF" => Ok(cirq_ir::DeviceType::PMesfet),
        other => Err(ImportError::UnknownModelKind(other.to_owned())),
    }
}

/// Determine `ElementKind` for a BJT based on its model type.
fn bjt_kind(
    model_name: &str,
    model_table: &HashMap<String, cirq_ir::DeviceType>,
) -> Result<IrElementKind, ImportError> {
    match model_table.get(&model_name.to_ascii_uppercase()) {
        Some(cirq_ir::DeviceType::Pnp) => Ok(IrElementKind::Pnp),
        Some(cirq_ir::DeviceType::Npn) | Some(_) => Ok(IrElementKind::Npn),
        None => Err(ImportError::ModelNotFound(model_name.to_owned())),
    }
}

/// Determine `ElementKind` for a MOSFET based on its model type.
fn mosfet_kind(
    model_name: &str,
    model_table: &HashMap<String, cirq_ir::DeviceType>,
) -> Result<IrElementKind, ImportError> {
    match model_table.get(&model_name.to_ascii_uppercase()) {
        Some(cirq_ir::DeviceType::Pmos) => Ok(IrElementKind::Pmos),
        Some(cirq_ir::DeviceType::Nmos) | Some(_) => Ok(IrElementKind::Nmos),
        None => Err(ImportError::ModelNotFound(model_name.to_owned())),
    }
}

/// Determine `ElementKind` for a JFET based on its model type.
fn jfet_kind(
    model_name: &str,
    model_table: &HashMap<String, cirq_ir::DeviceType>,
) -> Result<IrElementKind, ImportError> {
    match model_table.get(&model_name.to_ascii_uppercase()) {
        Some(cirq_ir::DeviceType::PJfet) => Ok(IrElementKind::PJfet),
        Some(cirq_ir::DeviceType::NJfet) | Some(_) => Ok(IrElementKind::NJfet),
        None => Err(ImportError::ModelNotFound(model_name.to_owned())),
    }
}

// ---------------------------------------------------------------------------
// Main import function
// ---------------------------------------------------------------------------

/// Convert a parsed `thevenin_types::Netlist` into a `cirq_ir::Circuit`.
pub fn import_netlist(netlist: &Netlist) -> Result<Circuit, ImportError> {
    // 1. Build model table: model name (uppercased) → DeviceType.
    //    Also collect model IR objects.
    let mut model_type_table: HashMap<String, cirq_ir::DeviceType> = HashMap::new();
    let mut ir_models: Vec<IrModel> = Vec::new();
    let mut model_id_counter: u32 = 0;
    let mut model_id_table: HashMap<String, Id> = HashMap::new();

    for item in &netlist.items {
        if let Item::Model(mdef) = item {
            let device_type = match map_device_type(&mdef.kind) {
                Ok(dt) => dt,
                Err(_) => continue, // skip unknown model kinds
            };
            let id = Id(model_id_counter);
            model_id_counter += 1;
            model_type_table.insert(mdef.name.to_ascii_uppercase(), device_type);
            model_id_table.insert(mdef.name.to_ascii_uppercase(), id);
            ir_models.push(IrModel {
                id,
                name: mdef.name.clone(),
                device_type,
                params: convert_params(&mdef.params),
            });
        }
    }

    // 2. Discover nets: scan all elements for node names.
    let mut net_table = NetTable::new();

    // Also handle .global directives.
    for item in &netlist.items {
        if let Item::Global(nodes) = item {
            net_table.mark_global(nodes);
        }
    }

    // Pre-scan elements for node names.
    for item in &netlist.items {
        if let Item::Element(elem) = item {
            intern_element_nodes(&elem.kind, &mut net_table);
        }
    }

    // Also intern nodes referenced in analyses (e.g. PZ node names).
    intern_analysis_nodes(&netlist.analysis, &mut net_table);

    // 3. Build element name → Id table for source lookups in analyses.
    let mut element_name_to_id: HashMap<String, Id> = HashMap::new();

    // 4. Convert elements.
    let mut ir_elements: Vec<IrElement> = Vec::new();
    let mut elem_id_counter: u32 = 0;

    for item in &netlist.items {
        let elem = match item {
            Item::Element(e) => e,
            _ => continue,
        };

        let id = Id(elem_id_counter);
        elem_id_counter += 1;

        element_name_to_id.insert(elem.name.to_ascii_uppercase(), id);

        let ir_elem =
            convert_element(id, elem, &mut net_table, &model_type_table, &model_id_table)?;

        if let Some(e) = ir_elem {
            ir_elements.push(e);
        }
    }

    // 5. Convert analysis.
    let ir_analyses = convert_analysis(&netlist.analysis, &element_name_to_id, &mut net_table)?;

    // 6. Collect .param items.
    let mut ir_params: Vec<ResolvedParam> = Vec::new();
    for item in &netlist.items {
        if let Item::Param(params) = item {
            for p in params {
                ir_params.push(ResolvedParam {
                    name: p.name.clone(),
                    value: expr_to_value(&p.value),
                });
            }
        }
    }

    // 7. Build circuit.
    let nets = net_table.into_nets();

    Ok(Circuit {
        name: netlist.title.clone(),
        nets,
        elements: ir_elements,
        models: ir_models,
        analyses: ir_analyses,
        params: ir_params,
    })
}

/// Parse SPICE source text and convert each resulting netlist into a `Circuit`.
pub fn import_spice(source: &str) -> Result<Vec<Circuit>, ImportError> {
    let netlists = Netlist::parse(source)?;
    netlists.iter().map(import_netlist).collect()
}

// ---------------------------------------------------------------------------
// Node interning for elements
// ---------------------------------------------------------------------------

fn intern_element_nodes(kind: &SpiceElementKind, nets: &mut NetTable) {
    match kind {
        SpiceElementKind::Resistor { pos, neg, .. }
        | SpiceElementKind::Capacitor { pos, neg, .. }
        | SpiceElementKind::Inductor { pos, neg, .. }
        | SpiceElementKind::VoltageSource { pos, neg, .. }
        | SpiceElementKind::CurrentSource { pos, neg, .. }
        | SpiceElementKind::BehavioralSource { pos, neg, .. } => {
            nets.intern(pos);
            nets.intern(neg);
        }
        SpiceElementKind::Diode { anode, cathode, .. } => {
            nets.intern(anode);
            nets.intern(cathode);
        }
        SpiceElementKind::Bjt {
            c, b, e, substrate, ..
        } => {
            nets.intern(c);
            nets.intern(b);
            nets.intern(e);
            if let Some(sub) = substrate {
                nets.intern(sub);
            }
        }
        SpiceElementKind::Mosfet {
            d,
            g,
            s,
            bulk,
            body,
            ..
        } => {
            nets.intern(d);
            nets.intern(g);
            nets.intern(s);
            nets.intern(bulk);
            if let Some(b) = body {
                nets.intern(b);
            }
        }
        SpiceElementKind::Jfet { d, g, s, .. } | SpiceElementKind::Mesa { d, g, s, .. } => {
            nets.intern(d);
            nets.intern(g);
            nets.intern(s);
        }
        SpiceElementKind::MutualCoupling { .. } => {
            // Coupling references inductor names, not nodes directly.
        }
        SpiceElementKind::Vcvs {
            out_pos,
            out_neg,
            in_pos,
            in_neg,
            ..
        }
        | SpiceElementKind::Vccs {
            out_pos,
            out_neg,
            in_pos,
            in_neg,
            ..
        } => {
            nets.intern(out_pos);
            nets.intern(out_neg);
            nets.intern(in_pos);
            nets.intern(in_neg);
        }
        SpiceElementKind::Ccvs {
            out_pos, out_neg, ..
        }
        | SpiceElementKind::Cccs {
            out_pos, out_neg, ..
        } => {
            nets.intern(out_pos);
            nets.intern(out_neg);
        }
        SpiceElementKind::SubcktCall { ports, .. } => {
            for p in ports {
                nets.intern(p);
            }
        }
        SpiceElementKind::Ltra {
            pos1,
            neg1,
            pos2,
            neg2,
            ..
        }
        | SpiceElementKind::Txl {
            pos1,
            neg1,
            pos2,
            neg2,
            ..
        } => {
            nets.intern(pos1);
            nets.intern(neg1);
            nets.intern(pos2);
            nets.intern(neg2);
        }
        SpiceElementKind::Cpl {
            in_nodes,
            out_nodes,
            gnd,
            ..
        } => {
            for n in in_nodes {
                nets.intern(n);
            }
            for n in out_nodes {
                nets.intern(n);
            }
            nets.intern(gnd);
        }
        SpiceElementKind::Xspice { connections, .. } => {
            for conn in connections {
                match conn {
                    thevenin_types::XspiceConnection::Scalar(s) => {
                        nets.intern(s);
                    }
                    thevenin_types::XspiceConnection::Array(arr) => {
                        for s in arr {
                            nets.intern(s);
                        }
                    }
                }
            }
        }
        SpiceElementKind::Raw(_) => {}
    }
}

fn intern_analysis_nodes(analysis: &SpiceAnalysis, nets: &mut NetTable) {
    match analysis {
        SpiceAnalysis::Noise {
            output, ref_node, ..
        } => {
            nets.intern(output);
            if let Some(r) = ref_node {
                nets.intern(r);
            }
        }
        SpiceAnalysis::Pz {
            node_i,
            node_g,
            node_j,
            node_k,
            ..
        } => {
            nets.intern(node_i);
            nets.intern(node_g);
            nets.intern(node_j);
            nets.intern(node_k);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Element conversion
// ---------------------------------------------------------------------------

/// Convert a single SPICE element to an IR element. Returns `None` for elements
/// that are intentionally skipped (e.g., subcircuit calls).
fn convert_element(
    id: Id,
    elem: &thevenin_types::Element,
    nets: &mut NetTable,
    model_types: &HashMap<String, cirq_ir::DeviceType>,
    model_ids: &HashMap<String, Id>,
) -> Result<Option<IrElement>, ImportError> {
    let name = &elem.name;

    match &elem.kind {
        SpiceElementKind::Resistor {
            pos,
            neg,
            value,
            params,
        } => {
            let mut ir_params = vec![("resistance".to_owned(), expr_to_value(value))];
            ir_params.extend(convert_params(params));
            Ok(Some(IrElement {
                id,
                name: name.clone(),
                kind: IrElementKind::Resistor,
                connections: vec![
                    connection("pos", nets.intern(pos)),
                    connection("neg", nets.intern(neg)),
                ],
                params: ir_params,
                model: None,
            }))
        }

        SpiceElementKind::Capacitor {
            pos,
            neg,
            value,
            params,
        } => {
            let mut ir_params = vec![("capacitance".to_owned(), expr_to_value(value))];
            ir_params.extend(convert_params(params));
            Ok(Some(IrElement {
                id,
                name: name.clone(),
                kind: IrElementKind::Capacitor,
                connections: vec![
                    connection("pos", nets.intern(pos)),
                    connection("neg", nets.intern(neg)),
                ],
                params: ir_params,
                model: None,
            }))
        }

        SpiceElementKind::Inductor {
            pos,
            neg,
            value,
            params,
        } => {
            let mut ir_params = vec![("inductance".to_owned(), expr_to_value(value))];
            ir_params.extend(convert_params(params));
            Ok(Some(IrElement {
                id,
                name: name.clone(),
                kind: IrElementKind::Inductor,
                connections: vec![
                    connection("pos", nets.intern(pos)),
                    connection("neg", nets.intern(neg)),
                ],
                params: ir_params,
                model: None,
            }))
        }

        SpiceElementKind::VoltageSource { pos, neg, source } => {
            let mut ir_params = Vec::new();
            if let Some(dc) = &source.dc {
                ir_params.push(("dc".to_owned(), expr_to_value(dc)));
            }
            if let Some(ac) = &source.ac {
                ir_params.push(("ac_mag".to_owned(), expr_to_value(&ac.mag)));
                if let Some(phase) = &ac.phase {
                    ir_params.push(("ac_phase".to_owned(), expr_to_value(phase)));
                }
            }
            Ok(Some(IrElement {
                id,
                name: name.clone(),
                kind: IrElementKind::VoltageSource,
                connections: vec![
                    connection("pos", nets.intern(pos)),
                    connection("neg", nets.intern(neg)),
                ],
                params: ir_params,
                model: None,
            }))
        }

        SpiceElementKind::CurrentSource { pos, neg, source } => {
            let mut ir_params = Vec::new();
            if let Some(dc) = &source.dc {
                ir_params.push(("dc".to_owned(), expr_to_value(dc)));
            }
            if let Some(ac) = &source.ac {
                ir_params.push(("ac_mag".to_owned(), expr_to_value(&ac.mag)));
                if let Some(phase) = &ac.phase {
                    ir_params.push(("ac_phase".to_owned(), expr_to_value(phase)));
                }
            }
            Ok(Some(IrElement {
                id,
                name: name.clone(),
                kind: IrElementKind::CurrentSource,
                connections: vec![
                    connection("pos", nets.intern(pos)),
                    connection("neg", nets.intern(neg)),
                ],
                params: ir_params,
                model: None,
            }))
        }

        SpiceElementKind::Diode {
            anode,
            cathode,
            model,
            params,
        } => {
            let model_id = model_ids.get(&model.to_ascii_uppercase()).copied();
            Ok(Some(IrElement {
                id,
                name: name.clone(),
                kind: IrElementKind::Diode,
                connections: vec![
                    connection("anode", nets.intern(anode)),
                    connection("cathode", nets.intern(cathode)),
                ],
                params: convert_params(params),
                model: model_id,
            }))
        }

        SpiceElementKind::Bjt {
            c,
            b,
            e,
            substrate,
            model,
            params,
            off,
        } => {
            let kind = bjt_kind(model, model_types)?;
            let model_id = model_ids.get(&model.to_ascii_uppercase()).copied();
            let mut conns = vec![
                connection("collector", nets.intern(c)),
                connection("base", nets.intern(b)),
                connection("emitter", nets.intern(e)),
            ];
            if let Some(sub) = substrate {
                conns.push(connection("substrate", nets.intern(sub)));
            }
            let mut ir_params = convert_params(params);
            if *off {
                ir_params.push(("off".to_owned(), Value::Bool(true)));
            }
            Ok(Some(IrElement {
                id,
                name: name.clone(),
                kind,
                connections: conns,
                params: ir_params,
                model: model_id,
            }))
        }

        SpiceElementKind::Mosfet {
            d,
            g,
            s,
            bulk,
            body,
            model,
            params,
        } => {
            let kind = mosfet_kind(model, model_types)?;
            let model_id = model_ids.get(&model.to_ascii_uppercase()).copied();
            let mut conns = vec![
                connection("drain", nets.intern(d)),
                connection("gate", nets.intern(g)),
                connection("source", nets.intern(s)),
                connection("bulk", nets.intern(bulk)),
            ];
            if let Some(b) = body {
                conns.push(connection("body", nets.intern(b)));
            }
            Ok(Some(IrElement {
                id,
                name: name.clone(),
                kind,
                connections: conns,
                params: convert_params(params),
                model: model_id,
            }))
        }

        SpiceElementKind::Jfet {
            d,
            g,
            s,
            model,
            params,
        } => {
            let kind = jfet_kind(model, model_types)?;
            let model_id = model_ids.get(&model.to_ascii_uppercase()).copied();
            Ok(Some(IrElement {
                id,
                name: name.clone(),
                kind,
                connections: vec![
                    connection("drain", nets.intern(d)),
                    connection("gate", nets.intern(g)),
                    connection("source", nets.intern(s)),
                ],
                params: convert_params(params),
                model: model_id,
            }))
        }

        SpiceElementKind::Mesa {
            d,
            g,
            s,
            model,
            params,
        } => {
            // MESA devices map to JFET kind based on model, defaulting to NJfet.
            let kind = jfet_kind(model, model_types).unwrap_or(IrElementKind::NJfet);
            let model_id = model_ids.get(&model.to_ascii_uppercase()).copied();
            Ok(Some(IrElement {
                id,
                name: name.clone(),
                kind,
                connections: vec![
                    connection("drain", nets.intern(d)),
                    connection("gate", nets.intern(g)),
                    connection("source", nets.intern(s)),
                ],
                params: convert_params(params),
                model: model_id,
            }))
        }

        SpiceElementKind::MutualCoupling { l1, l2, coupling } => {
            // Coupling references inductor element names, not net nodes.
            // We store the inductor names as string params.
            Ok(Some(IrElement {
                id,
                name: name.clone(),
                kind: IrElementKind::Coupling,
                connections: Vec::new(),
                params: vec![
                    ("l1".to_owned(), Value::String(l1.clone())),
                    ("l2".to_owned(), Value::String(l2.clone())),
                    ("coupling".to_owned(), expr_to_value(coupling)),
                ],
                model: None,
            }))
        }

        SpiceElementKind::Vcvs {
            out_pos,
            out_neg,
            in_pos,
            in_neg,
            gain,
        } => Ok(Some(IrElement {
            id,
            name: name.clone(),
            kind: IrElementKind::Vcvs,
            connections: vec![
                connection("out_pos", nets.intern(out_pos)),
                connection("out_neg", nets.intern(out_neg)),
                connection("in_pos", nets.intern(in_pos)),
                connection("in_neg", nets.intern(in_neg)),
            ],
            params: vec![("gain".to_owned(), expr_to_value(gain))],
            model: None,
        })),

        SpiceElementKind::Vccs {
            out_pos,
            out_neg,
            in_pos,
            in_neg,
            gm,
        } => Ok(Some(IrElement {
            id,
            name: name.clone(),
            kind: IrElementKind::Vccs,
            connections: vec![
                connection("out_pos", nets.intern(out_pos)),
                connection("out_neg", nets.intern(out_neg)),
                connection("in_pos", nets.intern(in_pos)),
                connection("in_neg", nets.intern(in_neg)),
            ],
            params: vec![("gm".to_owned(), expr_to_value(gm))],
            model: None,
        })),

        SpiceElementKind::Ccvs {
            out_pos,
            out_neg,
            vsrc,
            rm,
        } => Ok(Some(IrElement {
            id,
            name: name.clone(),
            kind: IrElementKind::Ccvs,
            connections: vec![
                connection("out_pos", nets.intern(out_pos)),
                connection("out_neg", nets.intern(out_neg)),
            ],
            params: vec![
                ("vsrc".to_owned(), Value::String(vsrc.clone())),
                ("rm".to_owned(), expr_to_value(rm)),
            ],
            model: None,
        })),

        SpiceElementKind::Cccs {
            out_pos,
            out_neg,
            vsrc,
            gain,
        } => Ok(Some(IrElement {
            id,
            name: name.clone(),
            kind: IrElementKind::Cccs,
            connections: vec![
                connection("out_pos", nets.intern(out_pos)),
                connection("out_neg", nets.intern(out_neg)),
            ],
            params: vec![
                ("vsrc".to_owned(), Value::String(vsrc.clone())),
                ("gain".to_owned(), expr_to_value(gain)),
            ],
            model: None,
        })),

        SpiceElementKind::Ltra {
            pos1,
            neg1,
            pos2,
            neg2,
            model,
            params,
        }
        | SpiceElementKind::Txl {
            pos1,
            neg1,
            pos2,
            neg2,
            model,
            params,
        } => {
            let model_id = model_ids.get(&model.to_ascii_uppercase()).copied();
            Ok(Some(IrElement {
                id,
                name: name.clone(),
                kind: IrElementKind::TransmissionLine,
                connections: vec![
                    connection("pos1", nets.intern(pos1)),
                    connection("neg1", nets.intern(neg1)),
                    connection("pos2", nets.intern(pos2)),
                    connection("neg2", nets.intern(neg2)),
                ],
                params: convert_params(params),
                model: model_id,
            }))
        }

        SpiceElementKind::SubcktCall { .. } => {
            // Subcircuit flattening is a separate concern; skip for now.
            Ok(None)
        }

        SpiceElementKind::BehavioralSource { .. }
        | SpiceElementKind::Cpl { .. }
        | SpiceElementKind::Xspice { .. }
        | SpiceElementKind::Raw(_) => Err(ImportError::UnsupportedElement(name.clone())),
    }
}

// ---------------------------------------------------------------------------
// Analysis conversion
// ---------------------------------------------------------------------------

fn convert_analysis(
    analysis: &SpiceAnalysis,
    element_names: &HashMap<String, Id>,
    nets: &mut NetTable,
) -> Result<Vec<IrAnalysis>, ImportError> {
    let ir = match analysis {
        SpiceAnalysis::Op => IrAnalysis::Op,

        SpiceAnalysis::Dc {
            src,
            start,
            stop,
            step,
            src2,
        } => {
            let src_id = element_names
                .get(&src.to_ascii_uppercase())
                .copied()
                .ok_or_else(|| ImportError::SourceNotFound(src.clone()))?;
            let mut sweeps = vec![IrDcSweep {
                source: src_id,
                start: expr_to_f64(start)?,
                stop: expr_to_f64(stop)?,
                step: expr_to_f64(step)?,
            }];
            if let Some(s2) = src2 {
                let s2_id = element_names
                    .get(&s2.src.to_ascii_uppercase())
                    .copied()
                    .ok_or_else(|| ImportError::SourceNotFound(s2.src.clone()))?;
                sweeps.push(IrDcSweep {
                    source: s2_id,
                    start: expr_to_f64(&s2.start)?,
                    stop: expr_to_f64(&s2.stop)?,
                    step: expr_to_f64(&s2.step)?,
                });
            }
            IrAnalysis::Dc(DcAnalysis { sweeps })
        }

        SpiceAnalysis::Ac {
            variation,
            n,
            fstart,
            fstop,
        } => {
            let scale = match variation {
                AcVariation::Dec => FrequencyScale::Decade,
                AcVariation::Oct => FrequencyScale::Octave,
                AcVariation::Lin => FrequencyScale::Linear,
            };
            IrAnalysis::Ac(AcAnalysis {
                start: expr_to_f64(fstart)?,
                stop: expr_to_f64(fstop)?,
                points: *n,
                scale,
            })
        }

        SpiceAnalysis::Tran {
            tstep,
            tstop,
            tstart,
            tmax: _,
        } => IrAnalysis::Tran(TranAnalysis {
            step: expr_to_f64(tstep)?,
            stop: expr_to_f64(tstop)?,
            start: tstart.as_ref().map(expr_to_f64).transpose()?.unwrap_or(0.0),
            uic: false,
        }),

        SpiceAnalysis::Noise {
            output,
            ref_node,
            src,
            variation,
            n,
            fstart,
            fstop,
        } => {
            let output_id = nets.intern(output);
            let ref_id = ref_node.as_ref().map(|r| nets.intern(r)).unwrap_or(Id(0));
            let src_id = element_names
                .get(&src.to_ascii_uppercase())
                .copied()
                .ok_or_else(|| ImportError::SourceNotFound(src.clone()))?;
            let scale = match variation {
                AcVariation::Dec => FrequencyScale::Decade,
                AcVariation::Oct => FrequencyScale::Octave,
                AcVariation::Lin => FrequencyScale::Linear,
            };
            IrAnalysis::Noise(NoiseAnalysis {
                output_net: output_id,
                reference_net: ref_id,
                source: src_id,
                start: expr_to_f64(fstart)?,
                stop: expr_to_f64(fstop)?,
                points: *n,
                scale,
            })
        }

        SpiceAnalysis::Tf { output, input } => {
            let src_id = element_names
                .get(&input.to_ascii_uppercase())
                .copied()
                .ok_or_else(|| ImportError::SourceNotFound(input.clone()))?;
            IrAnalysis::Tf(TfAnalysis {
                output: output.clone(),
                source: src_id,
            })
        }

        SpiceAnalysis::Sens { output } => IrAnalysis::Sens(SensAnalysis {
            output: output.join(", "),
        }),

        SpiceAnalysis::Pz {
            node_i,
            node_g,
            node_j,
            node_k,
            input_type,
            analysis_type,
        } => {
            let transfer = match input_type {
                PzInputType::Vol => TransferType::Voltage,
                PzInputType::Cur => TransferType::Current,
            };
            let pz_type = match analysis_type {
                PzAnalysisType::Pol => PzType::Poles,
                PzAnalysisType::Zer => PzType::Zeros,
                PzAnalysisType::Pz => PzType::Both,
            };
            IrAnalysis::Pz(PzAnalysis {
                input_pos: nets.intern(node_i),
                input_neg: nets.intern(node_g),
                output_pos: nets.intern(node_j),
                output_neg: nets.intern(node_k),
                transfer,
                analysis_type: pz_type,
            })
        }
    };

    Ok(vec![ir])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passive_network_two_resistors() {
        let spice = "\
Passive network
R1 a 0 1k
R2 a b 2k
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        assert_eq!(circuits.len(), 1);
        let c = &circuits[0];

        // 3 nets: 0, a, b
        assert_eq!(c.nets.len(), 3);
        let ground = c.nets.iter().find(|n| n.name == "0").unwrap();
        assert_eq!(ground.id, Id(0));
        assert!(ground.is_global);

        // 2 elements
        assert_eq!(c.elements.len(), 2);

        let r1 = c.elements.iter().find(|e| e.name == "R1").unwrap();
        assert!(matches!(r1.kind, IrElementKind::Resistor));
        assert_eq!(r1.connections.len(), 2);
        // Resistance param
        let resistance = r1.params.iter().find(|p| p.0 == "resistance").unwrap();
        match &resistance.1 {
            Value::Real(v) => assert!((v - 1000.0).abs() < 1e-6),
            other => panic!("expected Real, got {other:?}"),
        }

        let r2 = c.elements.iter().find(|e| e.name == "R2").unwrap();
        assert!(matches!(r2.kind, IrElementKind::Resistor));
        let resistance2 = r2.params.iter().find(|p| p.0 == "resistance").unwrap();
        match &resistance2.1 {
            Value::Real(v) => assert!((v - 2000.0).abs() < 1e-6),
            other => panic!("expected Real, got {other:?}"),
        }

        // Analysis is Op
        assert_eq!(c.analyses.len(), 1);
        assert!(matches!(c.analyses[0], IrAnalysis::Op));
    }

    #[test]
    fn mos_inverter() {
        let spice = "\
MOS inverter
.model NMOD NMOS
.model PMOD PMOS
M1 out in vdd vdd PMOD W=10u L=1u
M2 out in 0 0 NMOD W=5u L=1u
V1 vdd 0 DC 3.3
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        assert_eq!(circuits.len(), 1);
        let c = &circuits[0];

        assert_eq!(c.models.len(), 2);

        let m1 = c.elements.iter().find(|e| e.name == "M1").unwrap();
        assert!(matches!(m1.kind, IrElementKind::Pmos));
        assert_eq!(m1.connections.len(), 4);
        assert!(m1.model.is_some());

        let m2 = c.elements.iter().find(|e| e.name == "M2").unwrap();
        assert!(matches!(m2.kind, IrElementKind::Nmos));
        assert!(m2.model.is_some());
    }

    #[test]
    fn dc_sweep_analysis() {
        let spice = "\
DC sweep test
V1 in 0 DC 0
R1 in 0 1k
.dc V1 0 5 0.1
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];

        assert_eq!(c.analyses.len(), 1);
        match &c.analyses[0] {
            IrAnalysis::Dc(dc) => {
                assert_eq!(dc.sweeps.len(), 1);
                let sw = &dc.sweeps[0];
                assert!((sw.start - 0.0).abs() < 1e-12);
                assert!((sw.stop - 5.0).abs() < 1e-12);
                assert!((sw.step - 0.1).abs() < 1e-12);
            }
            other => panic!("expected Dc, got {other:?}"),
        }
    }

    #[test]
    fn ac_analysis() {
        let spice = "\
AC test
V1 in 0 DC 0 AC 1
R1 in 0 1k
.ac DEC 10 1 1Meg
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];

        assert_eq!(c.analyses.len(), 1);
        match &c.analyses[0] {
            IrAnalysis::Ac(ac) => {
                assert_eq!(ac.scale, FrequencyScale::Decade);
                assert_eq!(ac.points, 10);
                assert!((ac.start - 1.0).abs() < 1e-12);
                assert!((ac.stop - 1e6).abs() < 1e-6);
            }
            other => panic!("expected Ac, got {other:?}"),
        }
    }

    #[test]
    fn tran_analysis() {
        let spice = "\
Tran test
V1 in 0 DC 1
R1 in 0 1k
.tran 1n 100n
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];

        match &c.analyses[0] {
            IrAnalysis::Tran(tran) => {
                assert!((tran.step - 1e-9).abs() < 1e-18);
                assert!((tran.stop - 100e-9).abs() < 1e-18);
                assert!((tran.start - 0.0).abs() < 1e-18);
                assert!(!tran.uic);
            }
            other => panic!("expected Tran, got {other:?}"),
        }
    }

    #[test]
    fn model_mapping_all_types() {
        // Verify map_device_type for all known kinds.
        assert!(matches!(
            map_device_type("D"),
            Ok(cirq_ir::DeviceType::Diode)
        ));
        assert!(matches!(
            map_device_type("NPN"),
            Ok(cirq_ir::DeviceType::Npn)
        ));
        assert!(matches!(
            map_device_type("PNP"),
            Ok(cirq_ir::DeviceType::Pnp)
        ));
        assert!(matches!(
            map_device_type("NMOS"),
            Ok(cirq_ir::DeviceType::Nmos)
        ));
        assert!(matches!(
            map_device_type("PMOS"),
            Ok(cirq_ir::DeviceType::Pmos)
        ));
        assert!(matches!(
            map_device_type("NJF"),
            Ok(cirq_ir::DeviceType::NJfet)
        ));
        assert!(matches!(
            map_device_type("PJF"),
            Ok(cirq_ir::DeviceType::PJfet)
        ));
        assert!(matches!(
            map_device_type("NMF"),
            Ok(cirq_ir::DeviceType::NMesfet)
        ));
        assert!(matches!(
            map_device_type("PMF"),
            Ok(cirq_ir::DeviceType::PMesfet)
        ));
        assert!(matches!(
            map_device_type("GASFET"),
            Ok(cirq_ir::DeviceType::NMesfet)
        ));
        // Case insensitive
        assert!(matches!(
            map_device_type("nmos"),
            Ok(cirq_ir::DeviceType::Nmos)
        ));
        // Unknown
        assert!(map_device_type("BOGUS").is_err());
    }

    #[test]
    fn global_nets_marked() {
        let spice = "\
Global test
.global vdd vss
R1 vdd vss 1k
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];

        let vdd = c.nets.iter().find(|n| n.name == "vdd").unwrap();
        assert!(vdd.is_global);

        let vss = c.nets.iter().find(|n| n.name == "vss").unwrap();
        assert!(vss.is_global);
    }

    #[test]
    fn subckt_call_skipped() {
        let spice = "\
Subckt test
.subckt INV in out vdd vss
M1 out in vdd vdd PMOD
M2 out in vss vss NMOD
.ends INV
.model PMOD PMOS
.model NMOD NMOS
X1 a b vcc gnd INV
R1 a 0 1k
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];

        // X1 subckt call is skipped; only R1 should appear as an element.
        assert_eq!(c.elements.len(), 1);
        assert_eq!(c.elements[0].name, "R1");
    }

    #[test]
    fn voltage_source_with_dc_and_ac() {
        let spice = "\
Source test
V1 in 0 DC 1.5 AC 1 90
R1 in 0 1k
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];

        let v1 = c.elements.iter().find(|e| e.name == "V1").unwrap();
        assert!(matches!(v1.kind, IrElementKind::VoltageSource));
        let dc = v1.params.iter().find(|p| p.0 == "dc").unwrap();
        match &dc.1 {
            Value::Real(v) => assert!((v - 1.5).abs() < 1e-12),
            other => panic!("expected Real, got {other:?}"),
        }
        let ac_mag = v1.params.iter().find(|p| p.0 == "ac_mag").unwrap();
        match &ac_mag.1 {
            Value::Real(v) => assert!((v - 1.0).abs() < 1e-12),
            other => panic!("expected Real, got {other:?}"),
        }
        let ac_phase = v1.params.iter().find(|p| p.0 == "ac_phase").unwrap();
        match &ac_phase.1 {
            Value::Real(v) => assert!((v - 90.0).abs() < 1e-12),
            other => panic!("expected Real, got {other:?}"),
        }
    }

    #[test]
    fn params_collected() {
        let spice = "\
Param test
.param Rval=1k Cval=10p
R1 a 0 1k
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];

        assert_eq!(c.params.len(), 2);
        assert_eq!(c.params[0].name, "Rval");
        assert_eq!(c.params[1].name, "Cval");
    }

    #[test]
    fn diode_with_model() {
        let spice = "\
Diode test
.model D1N4148 D
D1 anode cathode D1N4148
R1 anode 0 1k
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];

        let d1 = c.elements.iter().find(|e| e.name == "D1").unwrap();
        assert!(matches!(d1.kind, IrElementKind::Diode));
        assert!(d1.model.is_some());
        assert_eq!(d1.connections[0].terminal, "anode");
        assert_eq!(d1.connections[1].terminal, "cathode");
    }

    #[test]
    fn bjt_npn_pnp() {
        let spice = "\
BJT test
.model QN NPN
.model QP PNP
Q1 c1 b1 e1 QN
Q2 c2 b2 e2 QP
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];

        let q1 = c.elements.iter().find(|e| e.name == "Q1").unwrap();
        assert!(matches!(q1.kind, IrElementKind::Npn));
        assert_eq!(q1.connections[0].terminal, "collector");
        assert_eq!(q1.connections[1].terminal, "base");
        assert_eq!(q1.connections[2].terminal, "emitter");

        let q2 = c.elements.iter().find(|e| e.name == "Q2").unwrap();
        assert!(matches!(q2.kind, IrElementKind::Pnp));
    }

    #[test]
    fn controlled_sources() {
        let spice = "\
Controlled sources
E1 out1 0 in1 0 10
G1 out2 0 in2 0 0.5
R1 in1 0 1k
R2 in2 0 1k
R3 out1 0 1k
R4 out2 0 1k
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];

        let e1 = c.elements.iter().find(|e| e.name == "E1").unwrap();
        assert!(matches!(e1.kind, IrElementKind::Vcvs));
        assert_eq!(e1.connections.len(), 4);
        let gain = e1.params.iter().find(|p| p.0 == "gain").unwrap();
        match &gain.1 {
            Value::Real(v) => assert!((v - 10.0).abs() < 1e-12),
            other => panic!("expected Real, got {other:?}"),
        }

        let g1 = c.elements.iter().find(|e| e.name == "G1").unwrap();
        assert!(matches!(g1.kind, IrElementKind::Vccs));
    }

    #[test]
    fn circuit_name_is_title() {
        let spice = "\
My Great Circuit
R1 a 0 1k
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        assert_eq!(circuits[0].name, "My Great Circuit");
    }

    #[test]
    fn ground_always_id_zero() {
        let spice = "\
Ground test
R1 a 0 1k
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];

        let ground = c.nets.iter().find(|n| n.id == Id(0)).unwrap();
        assert_eq!(ground.name, "0");
        assert!(ground.is_global);
    }
}
