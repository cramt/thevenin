//! AST-to-IR lowering -- semantic analysis pass that resolves names, evaluates
//! constant expressions, links models, and produces [`cirq_ir::Circuit`].

use std::collections::HashMap;

use cirq_ast::{
    AnalysisDecl, AnalysisItem, Argument, BinOp, CircuitItem, ElementInst, Expr, LetDecl, ModelDef,
    ParamDecl, SourceFile, TopLevel, UnaryOp,
};
use cirq_ir::{
    AcAnalysis, AcSpec, Analysis, Circuit, Connection, DcAnalysis, DcSweep, Element, ElementKind,
    FrequencyScale, Id, Model, Net, ResolvedParam, SourceSpec, TranAnalysis, Value, Waveform,
};

use crate::diagnostics::{Diagnostic, Severity};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Lower a [`SourceFile`] AST into a [`Circuit`] IR.
///
/// Returns `Ok(circuit)` when lowering produces no errors. Returns
/// `Err(diagnostics)` when there are any error-level diagnostics, though the
/// lowering is best-effort (as much IR as possible is still produced
/// internally).
pub fn lower_to_ir(source_file: &SourceFile) -> Result<Circuit, Vec<Diagnostic>> {
    let mut ctx = IrCtx::new();

    // Find the first circuit declaration.
    let circuit_ast = source_file.items.iter().find_map(|item| {
        if let TopLevel::Circuit(c) = item {
            Some(c)
        } else {
            None
        }
    });

    // Also collect top-level models (outside circuits).
    for item in &source_file.items {
        if let TopLevel::Model(m) = item {
            ctx.lower_model_def(m);
        }
    }

    let circuit_name;

    if let Some(c) = circuit_ast {
        circuit_name = c.name.name.clone();
        ctx.lower_circuit_body(&c.body);
    } else {
        circuit_name = String::new();
        ctx.diags
            .push(Diagnostic::error("no circuit declaration found"));
    }

    let circuit = Circuit {
        name: circuit_name,
        nets: ctx.nets,
        elements: ctx.elements,
        models: ctx.models,
        analyses: ctx.analyses,
        params: ctx.resolved_params,
    };

    let has_errors = ctx.diags.iter().any(|d| d.severity == Severity::Error);
    if has_errors {
        Err(ctx.diags)
    } else {
        Ok(circuit)
    }
}

// ---------------------------------------------------------------------------
// Element type → pin order mapping
// ---------------------------------------------------------------------------

/// Standard pin names for each element kind, in positional order.
fn standard_pins(kind: &ElementKind) -> &'static [&'static str] {
    match kind {
        ElementKind::Resistor => &["pos", "neg"],
        ElementKind::Capacitor => &["pos", "neg"],
        ElementKind::Inductor => &["pos", "neg"],
        ElementKind::VoltageSource => &["pos", "neg"],
        ElementKind::CurrentSource => &["pos", "neg"],
        ElementKind::Diode => &["anode", "cathode"],
        ElementKind::Npn | ElementKind::Pnp => &["collector", "base", "emitter"],
        ElementKind::Nmos | ElementKind::Pmos => &["drain", "gate", "source", "bulk"],
        ElementKind::NJfet | ElementKind::PJfet => &["drain", "gate", "source"],
        ElementKind::NMesfet | ElementKind::PMesfet => &["drain", "gate", "source"],
        ElementKind::Vcvs | ElementKind::Vccs | ElementKind::Ccvs | ElementKind::Cccs => {
            &["out_pos", "out_neg", "in_pos", "in_neg"]
        }
        ElementKind::TransmissionLine => &["in_pos", "in_neg", "out_pos", "out_neg"],
        ElementKind::Coupling => &[],
    }
}

/// Map an element type name string to an [`ElementKind`].
fn element_kind_from_str(name: &str) -> Option<ElementKind> {
    match name {
        "resistor" => Some(ElementKind::Resistor),
        "capacitor" => Some(ElementKind::Capacitor),
        "inductor" => Some(ElementKind::Inductor),
        "vsource" | "voltage_source" => Some(ElementKind::VoltageSource),
        "isource" | "current_source" => Some(ElementKind::CurrentSource),
        "diode" => Some(ElementKind::Diode),
        "nmos" => Some(ElementKind::Nmos),
        "pmos" => Some(ElementKind::Pmos),
        "npn" => Some(ElementKind::Npn),
        "pnp" => Some(ElementKind::Pnp),
        "njfet" => Some(ElementKind::NJfet),
        "pjfet" => Some(ElementKind::PJfet),
        "vcvs" => Some(ElementKind::Vcvs),
        "vccs" => Some(ElementKind::Vccs),
        "ccvs" => Some(ElementKind::Ccvs),
        "cccs" => Some(ElementKind::Cccs),
        "coupling" => Some(ElementKind::Coupling),
        "tline" | "transmission_line" => Some(ElementKind::TransmissionLine),
        "nmesfet" => Some(ElementKind::NMesfet),
        "pmesfet" => Some(ElementKind::PMesfet),
        _ => None,
    }
}

/// Map a device type name string to a [`cirq_ir::DeviceType`].
fn device_type_from_str(name: &str) -> Option<cirq_ir::DeviceType> {
    match name {
        "diode" => Some(cirq_ir::DeviceType::Diode),
        "npn" => Some(cirq_ir::DeviceType::Npn),
        "pnp" => Some(cirq_ir::DeviceType::Pnp),
        "nmos" => Some(cirq_ir::DeviceType::Nmos),
        "pmos" => Some(cirq_ir::DeviceType::Pmos),
        "njfet" => Some(cirq_ir::DeviceType::NJfet),
        "pjfet" => Some(cirq_ir::DeviceType::PJfet),
        "nmesfet" => Some(cirq_ir::DeviceType::NMesfet),
        "pmesfet" => Some(cirq_ir::DeviceType::PMesfet),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Lowering context
// ---------------------------------------------------------------------------

struct IrCtx {
    diags: Vec<Diagnostic>,

    // Counters for Id allocation.
    next_net_id: u32,
    next_element_id: u32,
    next_model_id: u32,

    // Resolved IR collections.
    nets: Vec<Net>,
    elements: Vec<Element>,
    models: Vec<Model>,
    analyses: Vec<Analysis>,
    resolved_params: Vec<ResolvedParam>,

    // Name tables.
    net_by_name: HashMap<String, Id>,
    element_by_name: HashMap<String, Id>,
    model_by_name: HashMap<String, Id>,
    param_values: HashMap<String, Value>,

    // For cycle detection during param evaluation.
    param_eval_stack: Vec<String>,
}

impl IrCtx {
    fn new() -> Self {
        let mut ctx = Self {
            diags: Vec::new(),
            next_net_id: 0,
            next_element_id: 0,
            next_model_id: 0,
            nets: Vec::new(),
            elements: Vec::new(),
            models: Vec::new(),
            analyses: Vec::new(),
            resolved_params: Vec::new(),
            net_by_name: HashMap::new(),
            element_by_name: HashMap::new(),
            model_by_name: HashMap::new(),
            param_values: HashMap::new(),
            param_eval_stack: Vec::new(),
        };
        // `gnd` always gets Id(0).
        ctx.intern_net("gnd", true);
        ctx
    }

    // -------------------------------------------------------------------
    // Net management
    // -------------------------------------------------------------------

    /// Get or create a net with the given name. Returns the net's Id.
    fn intern_net(&mut self, name: &str, is_global: bool) -> Id {
        if let Some(&id) = self.net_by_name.get(name) {
            // If we're making it global now, update the existing net.
            if is_global && let Some(net) = self.nets.iter_mut().find(|n| n.id == id) {
                net.is_global = true;
            }
            return id;
        }
        let id = Id(self.next_net_id);
        self.next_net_id += 1;
        self.nets.push(Net {
            id,
            name: name.to_string(),
            is_global,
        });
        self.net_by_name.insert(name.to_string(), id);
        id
    }

    fn alloc_element_id(&mut self) -> Id {
        let id = Id(self.next_element_id);
        self.next_element_id += 1;
        id
    }

    fn alloc_model_id(&mut self) -> Id {
        let id = Id(self.next_model_id);
        self.next_model_id += 1;
        id
    }

    // -------------------------------------------------------------------
    // Circuit body lowering
    // -------------------------------------------------------------------

    fn lower_circuit_body(&mut self, items: &[CircuitItem]) {
        // Two-pass: first collect all declaration names to detect duplicates
        // and register models/params, then lower elements and analyses.

        // Pass 1: declarations (params, lets, models, globals).
        for item in items {
            match item {
                CircuitItem::Param(p) => self.lower_param(p),
                CircuitItem::Let(l) => self.lower_let(l),
                CircuitItem::ModelDef(m) => self.lower_model_def(m),
                CircuitItem::Global(g) => {
                    self.intern_net(&g.name.name, true);
                }
                _ => {}
            }
        }

        // Pass 2: elements, module instances, analyses.
        for item in items {
            match item {
                CircuitItem::Element(e) => self.lower_element(e),
                CircuitItem::Analysis(a) => self.lower_analysis(a),
                CircuitItem::ModuleInst(_) => {
                    // Module instantiation lowering is out of scope for this
                    // pass (would require subcircuit flattening).
                }
                CircuitItem::ModuleDef(_) => {
                    // Nested module definitions are collected but not inlined.
                }
                // Already handled in pass 1.
                CircuitItem::Param(_)
                | CircuitItem::Let(_)
                | CircuitItem::ModelDef(_)
                | CircuitItem::Global(_) => {}
            }
        }
    }

    // -------------------------------------------------------------------
    // Param / let lowering
    // -------------------------------------------------------------------

    fn lower_param(&mut self, p: &ParamDecl) {
        let name = &p.name.name;

        if self.param_values.contains_key(name) {
            self.diags.push(
                Diagnostic::error(format!("duplicate declaration: `{name}`"))
                    .with_span(p.name.span),
            );
            return;
        }

        if let Some(ref default) = p.default {
            match self.eval_expr(default) {
                Ok(val) => {
                    self.resolved_params.push(ResolvedParam {
                        name: name.clone(),
                        value: val.clone(),
                    });
                    self.param_values.insert(name.clone(), val);
                }
                Err(msg) => {
                    self.diags.push(
                        Diagnostic::error(format!("cannot evaluate param `{name}`: {msg}"))
                            .with_span(p.span),
                    );
                }
            }
        }
        // A param without a default is allowed (forward-declared for modules).
    }

    fn lower_let(&mut self, l: &LetDecl) {
        let name = &l.name.name;

        if self.param_values.contains_key(name) {
            self.diags.push(
                Diagnostic::error(format!("duplicate declaration: `{name}`"))
                    .with_span(l.name.span),
            );
            return;
        }

        match self.eval_expr(&l.value) {
            Ok(val) => {
                self.resolved_params.push(ResolvedParam {
                    name: name.clone(),
                    value: val.clone(),
                });
                self.param_values.insert(name.clone(), val);
            }
            Err(msg) => {
                self.diags.push(
                    Diagnostic::error(format!("cannot evaluate let `{name}`: {msg}"))
                        .with_span(l.span),
                );
            }
        }
    }

    // -------------------------------------------------------------------
    // Model lowering
    // -------------------------------------------------------------------

    fn lower_model_def(&mut self, m: &ModelDef) {
        let name = &m.name.name;

        if self.model_by_name.contains_key(name) {
            self.diags.push(
                Diagnostic::error(format!("duplicate model: `{name}`")).with_span(m.name.span),
            );
            return;
        }

        // Resolve device type. For inherited models (base != None), we look
        // up the base model's device_type string. But the device_type field in
        // the AST is always present (it's the token after the colon), so try
        // that first. If the device_type is actually a model name (inheritance),
        // fall back.
        let device_type = device_type_from_str(&m.device_type.name);

        let dev_type = match device_type {
            Some(dt) => dt,
            None => {
                // The device_type might be a base model name (inheritance).
                // If so, look up the base model's device type.
                if let Some(&base_id) = self.model_by_name.get(&m.device_type.name) {
                    if let Some(base) = self.models.iter().find(|model| model.id == base_id) {
                        base.device_type
                    } else {
                        self.diags.push(
                            Diagnostic::error(format!(
                                "unknown device type or base model: `{}`",
                                m.device_type.name
                            ))
                            .with_span(m.device_type.span),
                        );
                        return;
                    }
                } else {
                    self.diags.push(
                        Diagnostic::error(format!("unknown device type: `{}`", m.device_type.name))
                            .with_span(m.device_type.span),
                    );
                    return;
                }
            }
        };

        // Start with base model params if inheriting (Gap 2.6).
        let mut params: Vec<(String, Value)> = if device_type.is_none() {
            // This is an inherited model; copy base params first.
            if let Some(&base_id) = self.model_by_name.get(&m.device_type.name) {
                self.models
                    .iter()
                    .find(|model| model.id == base_id)
                    .map(|base| base.params.clone())
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        // Overlay child params (overwriting any base params with same name).
        for mp in &m.params {
            match self.eval_expr(&mp.value) {
                Ok(val) => {
                    // Replace existing param from base if present.
                    if let Some(existing) = params.iter_mut().find(|p| p.0 == mp.name.name) {
                        existing.1 = val;
                    } else {
                        params.push((mp.name.name.clone(), val));
                    }
                }
                Err(msg) => {
                    self.diags.push(
                        Diagnostic::error(format!(
                            "cannot evaluate model param `{}`: {msg}",
                            mp.name.name
                        ))
                        .with_span(mp.span),
                    );
                }
            }
        }

        let id = self.alloc_model_id();
        self.model_by_name.insert(name.clone(), id);
        self.models.push(Model {
            id,
            name: name.clone(),
            device_type: dev_type,
            params,
        });
    }

    // -------------------------------------------------------------------
    // Element lowering
    // -------------------------------------------------------------------

    fn lower_element(&mut self, e: &ElementInst) {
        let name = &e.name.name;

        if self.element_by_name.contains_key(name) {
            self.diags.push(
                Diagnostic::error(format!("duplicate element: `{name}`")).with_span(e.name.span),
            );
            return;
        }

        // Resolve element kind.
        let kind = match element_kind_from_str(&e.element_type.name) {
            Some(k) => k,
            None => {
                self.diags.push(
                    Diagnostic::error(format!("unknown element type: `{}`", e.element_type.name))
                        .with_span(e.element_type.span),
                );
                return;
            }
        };

        let pins = standard_pins(&kind);

        // Separate arguments into connections, named params, and positional params.
        let mut connections: Vec<Connection> = Vec::new();
        let mut element_params: Vec<(String, Value)> = Vec::new();
        let mut model_ref: Option<Id> = None;
        let mut positional_conn_idx: usize = 0;
        let mut source_spec = SourceSpec::default();
        let is_source = matches!(
            kind,
            ElementKind::VoltageSource | ElementKind::CurrentSource
        );

        for arg in &e.args {
            match arg {
                Argument::Connection { from, to } => {
                    // Unnamed connection: maps positional pins.
                    // A connection `a -> b` maps two pins.
                    if positional_conn_idx >= pins.len() {
                        self.diags.push(
                            Diagnostic::error(format!(
                                "too many connections for element type `{}`",
                                e.element_type.name
                            ))
                            .with_span(from.span),
                        );
                        continue;
                    }

                    let from_net = self.resolve_net_ident(from);
                    let pin_from = pins[positional_conn_idx].to_string();
                    connections.push(Connection {
                        terminal: pin_from,
                        net: from_net,
                    });
                    positional_conn_idx += 1;

                    if positional_conn_idx < pins.len() {
                        let to_net = self.resolve_net_ident(to);
                        let pin_to = pins[positional_conn_idx].to_string();
                        connections.push(Connection {
                            terminal: pin_to,
                            net: to_net,
                        });
                        positional_conn_idx += 1;
                    }
                }
                Argument::NamedConnection { name, from, to } => {
                    // Named connection pair, e.g. `control: a -> b`.
                    // We treat this as two connections with derived terminal names.
                    let from_net = self.resolve_net_ident(from);
                    let to_net = self.resolve_net_ident(to);
                    connections.push(Connection {
                        terminal: format!("{}_pos", name.name),
                        net: from_net,
                    });
                    connections.push(Connection {
                        terminal: format!("{}_neg", name.name),
                        net: to_net,
                    });
                }
                Argument::Named { name, value } => {
                    let param_name = &name.name;

                    // Special handling for known connection-like named args.
                    if is_connection_param(param_name) {
                        // The value should be an ident referencing a net.
                        if let Some(net_name) = expr_as_net_name(value) {
                            let net_id = self.intern_net(&net_name, net_name == "gnd");
                            connections.push(Connection {
                                terminal: param_name.clone(),
                                net: net_id,
                            });
                        } else {
                            self.diags.push(
                                Diagnostic::error(format!(
                                    "expected net name for connection `{param_name}`"
                                ))
                                .with_span(name.span),
                            );
                        }
                    } else if param_name == "model" {
                        // Model reference.
                        if let Some(model_name) = expr_as_ident(value) {
                            match self.model_by_name.get(model_name) {
                                Some(&mid) => model_ref = Some(mid),
                                None => {
                                    self.diags.push(
                                        Diagnostic::error(format!("unknown model: `{model_name}`"))
                                            .with_span(name.span),
                                    );
                                }
                            }
                        } else {
                            self.diags.push(
                                Diagnostic::error("model must be an identifier")
                                    .with_span(name.span),
                            );
                        }
                    } else if is_source && is_waveform_param(param_name) {
                        // Waveform block for a source element.
                        match self.lower_waveform(param_name, value) {
                            Ok(wf) => source_spec.waveform = Some(wf),
                            Err(msg) => {
                                self.diags.push(
                                    Diagnostic::error(format!(
                                        "invalid waveform `{param_name}`: {msg}"
                                    ))
                                    .with_span(name.span),
                                );
                            }
                        }
                    } else if is_source && param_name == "ac" {
                        // AC magnitude (scalar or block with mag/phase).
                        match self.lower_ac_spec(value) {
                            Ok(ac) => source_spec.ac = Some(ac),
                            Err(msg) => {
                                self.diags.push(
                                    Diagnostic::error(format!("invalid ac spec: {msg}"))
                                        .with_span(name.span),
                                );
                            }
                        }
                    } else if is_source && param_name == "dc" {
                        // DC value for a source.
                        match self.eval_to_f64(value) {
                            Some(v) => source_spec.dc = Some(v),
                            None => {
                                // Also store as a param for backward compat.
                                match self.eval_expr(value) {
                                    Ok(val) => element_params.push(("dc".to_string(), val)),
                                    Err(msg) => {
                                        self.diags.push(
                                            Diagnostic::error(format!(
                                                "cannot evaluate dc value: {msg}"
                                            ))
                                            .with_span(name.span),
                                        );
                                    }
                                }
                            }
                        }
                    } else {
                        // Regular parameter.
                        match self.eval_expr(value) {
                            Ok(val) => element_params.push((param_name.clone(), val)),
                            Err(msg) => {
                                self.diags.push(
                                    Diagnostic::error(format!(
                                        "cannot evaluate param `{param_name}`: {msg}"
                                    ))
                                    .with_span(name.span),
                                );
                            }
                        }
                    }
                }
                Argument::Positional(expr) => {
                    if is_source {
                        // For sources, positional value is DC.
                        match self.eval_to_f64(expr) {
                            Some(v) => source_spec.dc = Some(v),
                            None => match self.eval_expr(expr) {
                                Ok(val) => element_params.push(("value".to_string(), val)),
                                Err(msg) => {
                                    self.diags.push(
                                        Diagnostic::error(format!(
                                            "cannot evaluate positional param: {msg}"
                                        ))
                                        .with_span(e.span),
                                    );
                                }
                            },
                        }
                    } else {
                        // Positional value param (e.g. `10k` for a resistor value).
                        match self.eval_expr(expr) {
                            Ok(val) => element_params.push(("value".to_string(), val)),
                            Err(msg) => {
                                self.diags.push(
                                    Diagnostic::error(format!(
                                        "cannot evaluate positional param: {msg}"
                                    ))
                                    .with_span(e.span),
                                );
                            }
                        }
                    }
                }
            }
        }

        // Build source_spec: only store if this is a source element and has
        // at least some spec data.
        let final_source_spec = if is_source {
            let has_data = source_spec.dc.is_some()
                || source_spec.ac.is_some()
                || source_spec.waveform.is_some();
            if has_data {
                Some(source_spec)
            } else {
                // Check for legacy "dc" / "value" params (backward compat).
                let dc_from_params = element_params
                    .iter()
                    .find(|p| p.0 == "dc" || p.0 == "value");
                dc_from_params.map(|p| SourceSpec {
                    dc: Some(value_as_f64(&p.1).unwrap_or(0.0)),
                    ac: None,
                    waveform: None,
                })
            }
        } else {
            None
        };

        let id = self.alloc_element_id();
        self.element_by_name.insert(name.clone(), id);
        self.elements.push(Element {
            id,
            name: name.clone(),
            kind,
            connections,
            params: element_params,
            model: model_ref,
            source_spec: final_source_spec,
        });
    }

    /// Resolve an identifier to a net Id (creating the net if it doesn't exist).
    fn resolve_net_ident(&mut self, ident: &cirq_ast::Ident) -> Id {
        let name = &ident.name;
        self.intern_net(name, name == "gnd")
    }

    // -------------------------------------------------------------------
    // Analysis lowering
    // -------------------------------------------------------------------

    fn lower_analysis(&mut self, a: &AnalysisDecl) {
        let kind = a.kind.name.as_str();

        let analysis = match kind {
            "op" => Some(Analysis::Op),
            "dc" => Some(self.lower_dc_analysis(&a.body, a)),
            "ac" => Some(self.lower_ac_analysis(&a.body, a)),
            "tran" => Some(self.lower_tran_analysis(&a.body, a)),
            "noise" => Some(self.lower_noise_analysis(&a.body, a)),
            "pz" => Some(self.lower_pz_analysis(&a.body, a)),
            "sens" => {
                // Look for an "output" setting.
                let output = self
                    .get_setting_string(&a.body, "output")
                    .unwrap_or_default();
                Some(Analysis::Sens(cirq_ir::SensAnalysis { output }))
            }
            "tf" => {
                let output = self
                    .get_setting_string(&a.body, "output")
                    .unwrap_or_default();
                let source_name = self
                    .get_setting_string(&a.body, "source")
                    .unwrap_or_default();
                let source_id = if let Some(&eid) = self.element_by_name.get(&source_name) {
                    eid
                } else {
                    Id(0)
                };
                Some(Analysis::Tf(cirq_ir::TfAnalysis {
                    output,
                    source: source_id,
                }))
            }
            _ => {
                self.diags.push(
                    Diagnostic::error(format!("unknown analysis type: `{kind}`"))
                        .with_span(a.kind.span),
                );
                None
            }
        };

        if let Some(an) = analysis {
            self.analyses.push(an);
        }
    }

    fn lower_dc_analysis(&mut self, body: &[AnalysisItem], _decl: &AnalysisDecl) -> Analysis {
        let mut sweeps = Vec::new();

        for item in body {
            if let AnalysisItem::Sweep {
                source,
                start,
                stop,
                step,
            } = item
            {
                let source_id = if let Some(&eid) = self.element_by_name.get(&source.name) {
                    eid
                } else {
                    // The source might not be declared yet or may refer to a
                    // net name — record a diagnostic but still produce an entry.
                    self.diags.push(
                        Diagnostic::warning(format!(
                            "DC sweep source `{}` not found as an element",
                            source.name
                        ))
                        .with_span(source.span),
                    );
                    // Use the net as a fallback reference.
                    self.intern_net(&source.name, false)
                };

                let start_val = self.eval_to_f64(start).unwrap_or(0.0);
                let stop_val = self.eval_to_f64(stop).unwrap_or(0.0);
                let step_val = self.eval_to_f64(step).unwrap_or(0.0);

                sweeps.push(DcSweep {
                    source: source_id,
                    start: start_val,
                    stop: stop_val,
                    step: step_val,
                });
            }
        }

        Analysis::Dc(DcAnalysis { sweeps })
    }

    fn lower_ac_analysis(&mut self, body: &[AnalysisItem], decl: &AnalysisDecl) -> Analysis {
        let mut start = 0.0;
        let mut stop = 0.0;
        let mut points = 0u32;
        let mut scale = FrequencyScale::Decade;

        for item in body {
            if let AnalysisItem::Setting { name, value } = item {
                match name.name.as_str() {
                    "start" => {
                        start = self.eval_to_f64(value).unwrap_or_else(|| {
                            self.diags.push(
                                Diagnostic::error("cannot evaluate ac start").with_span(decl.span),
                            );
                            0.0
                        });
                    }
                    "stop" => {
                        stop = self.eval_to_f64(value).unwrap_or_else(|| {
                            self.diags.push(
                                Diagnostic::error("cannot evaluate ac stop").with_span(decl.span),
                            );
                            0.0
                        });
                    }
                    "points" => {
                        points = self.eval_to_f64(value).unwrap_or(0.0) as u32;
                    }
                    "scale" => {
                        if let Some(s) = expr_as_ident(value) {
                            scale = match s {
                                "decade" | "dec" => FrequencyScale::Decade,
                                "octave" | "oct" => FrequencyScale::Octave,
                                "linear" | "lin" => FrequencyScale::Linear,
                                _ => {
                                    self.diags.push(
                                        Diagnostic::error(format!(
                                            "unknown frequency scale: `{s}`"
                                        ))
                                        .with_span(name.span),
                                    );
                                    FrequencyScale::Decade
                                }
                            };
                        }
                    }
                    _ => {}
                }
            }
        }

        Analysis::Ac(AcAnalysis {
            start,
            stop,
            points,
            scale,
        })
    }

    fn lower_tran_analysis(&mut self, body: &[AnalysisItem], decl: &AnalysisDecl) -> Analysis {
        let mut step = 0.0;
        let mut stop = 0.0;
        let mut start = 0.0;
        let mut uic = false;
        let mut tmax = None;

        for item in body {
            if let AnalysisItem::Setting { name, value } = item {
                match name.name.as_str() {
                    "step" => {
                        step = self.eval_to_f64(value).unwrap_or_else(|| {
                            self.diags.push(
                                Diagnostic::error("cannot evaluate tran step").with_span(decl.span),
                            );
                            0.0
                        });
                    }
                    "stop" => {
                        stop = self.eval_to_f64(value).unwrap_or_else(|| {
                            self.diags.push(
                                Diagnostic::error("cannot evaluate tran stop").with_span(decl.span),
                            );
                            0.0
                        });
                    }
                    "start" => {
                        start = self.eval_to_f64(value).unwrap_or(0.0);
                    }
                    "tmax" => {
                        tmax = self.eval_to_f64(value);
                    }
                    "uic" => {
                        if let Expr::Bool { value: v, .. } = value {
                            uic = *v;
                        } else {
                            uic = true;
                        }
                    }
                    _ => {}
                }
            }
        }

        Analysis::Tran(TranAnalysis {
            step,
            stop,
            start,
            uic,
            tmax,
        })
    }

    fn lower_noise_analysis(&mut self, body: &[AnalysisItem], decl: &AnalysisDecl) -> Analysis {
        let mut output_name = String::new();
        let mut reference_name = String::new();
        let mut source_name = String::new();
        let mut start = 0.0;
        let mut stop = 0.0;
        let mut points = 0u32;
        let mut scale = FrequencyScale::Decade;

        for item in body {
            if let AnalysisItem::Setting { name, value } = item {
                match name.name.as_str() {
                    "output" => {
                        if let Some(s) = expr_as_ident(value) {
                            output_name = s.to_string();
                        }
                    }
                    "reference" | "ref" => {
                        if let Some(s) = expr_as_ident(value) {
                            reference_name = s.to_string();
                        }
                    }
                    "source" | "src" => {
                        if let Some(s) = expr_as_ident(value) {
                            source_name = s.to_string();
                        }
                    }
                    "start" => {
                        start = self.eval_to_f64(value).unwrap_or_else(|| {
                            self.diags.push(
                                Diagnostic::error("cannot evaluate noise start")
                                    .with_span(decl.span),
                            );
                            0.0
                        });
                    }
                    "stop" => {
                        stop = self.eval_to_f64(value).unwrap_or_else(|| {
                            self.diags.push(
                                Diagnostic::error("cannot evaluate noise stop")
                                    .with_span(decl.span),
                            );
                            0.0
                        });
                    }
                    "points" => {
                        points = self.eval_to_f64(value).unwrap_or(0.0) as u32;
                    }
                    "scale" => {
                        if let Some(s) = expr_as_ident(value) {
                            scale = match s {
                                "decade" | "dec" => FrequencyScale::Decade,
                                "octave" | "oct" => FrequencyScale::Octave,
                                "linear" | "lin" => FrequencyScale::Linear,
                                _ => {
                                    self.diags.push(
                                        Diagnostic::error(format!(
                                            "unknown frequency scale: `{s}`"
                                        ))
                                        .with_span(name.span),
                                    );
                                    FrequencyScale::Decade
                                }
                            };
                        }
                    }
                    _ => {}
                }
            }
        }

        let output_net = self.intern_net(&output_name, output_name == "gnd");
        let reference_net = if reference_name.is_empty() {
            Id(0) // default to gnd
        } else {
            self.intern_net(&reference_name, reference_name == "gnd")
        };
        let source_id = if let Some(&eid) = self.element_by_name.get(&source_name) {
            eid
        } else {
            if !source_name.is_empty() {
                self.diags.push(
                    Diagnostic::warning(format!(
                        "noise source `{source_name}` not found as an element"
                    ))
                    .with_span(decl.span),
                );
            }
            Id(0)
        };

        Analysis::Noise(cirq_ir::NoiseAnalysis {
            output_net,
            reference_net,
            source: source_id,
            start,
            stop,
            points,
            scale,
        })
    }

    fn lower_pz_analysis(&mut self, body: &[AnalysisItem], decl: &AnalysisDecl) -> Analysis {
        let mut input_pos_name = String::new();
        let mut input_neg_name = String::new();
        let mut output_pos_name = String::new();
        let mut output_neg_name = String::new();
        let mut transfer = cirq_ir::TransferType::Voltage;
        let mut analysis_type = cirq_ir::PzType::Both;

        for item in body {
            if let AnalysisItem::Setting { name, value } = item {
                match name.name.as_str() {
                    "input_pos" | "in_pos" => {
                        if let Some(s) = expr_as_ident(value) {
                            input_pos_name = s.to_string();
                        }
                    }
                    "input_neg" | "in_neg" => {
                        if let Some(s) = expr_as_ident(value) {
                            input_neg_name = s.to_string();
                        }
                    }
                    "output_pos" | "out_pos" => {
                        if let Some(s) = expr_as_ident(value) {
                            output_pos_name = s.to_string();
                        }
                    }
                    "output_neg" | "out_neg" => {
                        if let Some(s) = expr_as_ident(value) {
                            output_neg_name = s.to_string();
                        }
                    }
                    "transfer" => {
                        if let Some(s) = expr_as_ident(value) {
                            transfer = match s {
                                "voltage" | "vol" => cirq_ir::TransferType::Voltage,
                                "current" | "cur" => cirq_ir::TransferType::Current,
                                _ => {
                                    self.diags.push(
                                        Diagnostic::error(format!("unknown transfer type: `{s}`"))
                                            .with_span(name.span),
                                    );
                                    cirq_ir::TransferType::Voltage
                                }
                            };
                        }
                    }
                    "type" | "analysis_type" => {
                        if let Some(s) = expr_as_ident(value) {
                            analysis_type = match s {
                                "poles" | "pol" => cirq_ir::PzType::Poles,
                                "zeros" | "zer" => cirq_ir::PzType::Zeros,
                                "both" | "pz" => cirq_ir::PzType::Both,
                                _ => {
                                    self.diags.push(
                                        Diagnostic::error(format!(
                                            "unknown pz analysis type: `{s}`"
                                        ))
                                        .with_span(name.span),
                                    );
                                    cirq_ir::PzType::Both
                                }
                            };
                        }
                    }
                    _ => {}
                }
            }
        }

        let input_pos = if input_pos_name.is_empty() {
            self.diags
                .push(Diagnostic::error("pz analysis requires input_pos").with_span(decl.span));
            Id(0)
        } else {
            self.intern_net(&input_pos_name, input_pos_name == "gnd")
        };
        let input_neg = if input_neg_name.is_empty() {
            Id(0) // default to gnd
        } else {
            self.intern_net(&input_neg_name, input_neg_name == "gnd")
        };
        let output_pos = if output_pos_name.is_empty() {
            self.diags
                .push(Diagnostic::error("pz analysis requires output_pos").with_span(decl.span));
            Id(0)
        } else {
            self.intern_net(&output_pos_name, output_pos_name == "gnd")
        };
        let output_neg = if output_neg_name.is_empty() {
            Id(0) // default to gnd
        } else {
            self.intern_net(&output_neg_name, output_neg_name == "gnd")
        };

        Analysis::Pz(cirq_ir::PzAnalysis {
            input_pos,
            input_neg,
            output_pos,
            output_neg,
            transfer,
            analysis_type,
        })
    }

    // -------------------------------------------------------------------
    // Waveform and AC spec lowering
    // -------------------------------------------------------------------

    /// Lower a waveform block expression into a typed [`Waveform`].
    fn lower_waveform(&mut self, kind: &str, expr: &Expr) -> Result<Waveform, String> {
        let entries = match expr {
            Expr::Block { entries, .. } => entries,
            _ => return Err("expected a block `{ ... }` for waveform".to_string()),
        };

        let get_f64 = |name: &str, entries: &[(cirq_ast::Ident, Expr)]| -> Option<f64> {
            entries
                .iter()
                .find(|(k, _)| k.name == name)
                .and_then(|(_, v)| match v {
                    Expr::Number { value, .. } => Some(*value),
                    Expr::Integer { value, .. } => Some(*value as f64),
                    _ => None,
                })
        };

        let require_f64 =
            |name: &str, entries: &[(cirq_ast::Ident, Expr)]| -> Result<f64, String> {
                get_f64(name, entries).ok_or_else(|| format!("missing required field `{name}`"))
            };

        match kind {
            "pulse" => Ok(Waveform::Pulse {
                v1: require_f64("v1", entries)?,
                v2: require_f64("v2", entries)?,
                td: get_f64("td", entries),
                tr: get_f64("tr", entries),
                tf: get_f64("tf", entries),
                pw: get_f64("pw", entries),
                per: get_f64("per", entries),
            }),
            "sin" | "sine" => Ok(Waveform::Sin {
                v0: require_f64("v0", entries)?,
                va: require_f64("va", entries)?,
                freq: get_f64("freq", entries),
                td: get_f64("td", entries),
                theta: get_f64("theta", entries),
                phi: get_f64("phi", entries),
            }),
            "exp" => Ok(Waveform::Exp {
                v1: require_f64("v1", entries)?,
                v2: require_f64("v2", entries)?,
                td1: get_f64("td1", entries),
                tau1: get_f64("tau1", entries),
                td2: get_f64("td2", entries),
                tau2: get_f64("tau2", entries),
            }),
            "pwl" => {
                // PWL can be a list of (time, value) tuples inside the block,
                // or pairs of t/v entries.
                let mut points = Vec::new();
                // Try named pairs: t0/v0, t1/v1, ...
                let mut i = 0;
                loop {
                    let t_key = format!("t{i}");
                    let v_key = format!("v{i}");
                    match (get_f64(&t_key, entries), get_f64(&v_key, entries)) {
                        (Some(t), Some(v)) => points.push((t, v)),
                        _ => break,
                    }
                    i += 1;
                }
                // If no named pairs, try sequential time/value entries.
                if points.is_empty() {
                    let times: Vec<f64> = entries
                        .iter()
                        .filter(|(k, _)| k.name.starts_with('t'))
                        .filter_map(|(_, v)| match v {
                            Expr::Number { value, .. } => Some(*value),
                            _ => None,
                        })
                        .collect();
                    let values: Vec<f64> = entries
                        .iter()
                        .filter(|(k, _)| k.name.starts_with('v'))
                        .filter_map(|(_, v)| match v {
                            Expr::Number { value, .. } => Some(*value),
                            _ => None,
                        })
                        .collect();
                    for (t, v) in times.into_iter().zip(values) {
                        points.push((t, v));
                    }
                }
                if points.is_empty() {
                    return Err("PWL waveform requires at least one point".to_string());
                }
                Ok(Waveform::Pwl(points))
            }
            "sffm" => Ok(Waveform::Sffm {
                v0: require_f64("v0", entries)?,
                va: require_f64("va", entries)?,
                fc: get_f64("fc", entries),
                fs: get_f64("fs", entries),
                md: get_f64("md", entries),
            }),
            "am" => Ok(Waveform::Am {
                va: require_f64("va", entries)?,
                vo: require_f64("vo", entries)?,
                fc: require_f64("fc", entries)?,
                fs: require_f64("fs", entries)?,
                td: get_f64("td", entries),
            }),
            _ => Err(format!("unknown waveform type: `{kind}`")),
        }
    }

    /// Lower an AC specification from a value expression.
    fn lower_ac_spec(&mut self, expr: &Expr) -> Result<AcSpec, String> {
        match expr {
            // Scalar: `ac: 1.0` means mag=1.0, phase=0.0
            Expr::Number { value, .. } => Ok(AcSpec {
                mag: *value,
                phase: 0.0,
            }),
            Expr::Integer { value, .. } => Ok(AcSpec {
                mag: *value as f64,
                phase: 0.0,
            }),
            // Block: `ac: { mag: 1.0, phase: 90 }`
            Expr::Block { entries, .. } => {
                let mut mag = 1.0;
                let mut phase = 0.0;
                for (key, val) in entries {
                    match key.name.as_str() {
                        "mag" | "magnitude" => {
                            if let Expr::Number { value, .. } = val {
                                mag = *value;
                            } else if let Expr::Integer { value, .. } = val {
                                mag = *value as f64;
                            }
                        }
                        "phase" => {
                            if let Expr::Number { value, .. } = val {
                                phase = *value;
                            } else if let Expr::Integer { value, .. } = val {
                                phase = *value as f64;
                            }
                        }
                        _ => {}
                    }
                }
                Ok(AcSpec { mag, phase })
            }
            _ => Err("ac spec must be a number or block `{ mag: ..., phase: ... }`".to_string()),
        }
    }

    /// Extract a string from a setting with the given name.
    fn get_setting_string(&self, body: &[AnalysisItem], key: &str) -> Option<String> {
        for item in body {
            if let AnalysisItem::Setting { name, value } = item
                && name.name == key
            {
                if let Some(s) = expr_as_ident(value) {
                    return Some(s.to_string());
                }
                if let Expr::StringLit { value: s, .. } = value {
                    return Some(s.clone());
                }
            }
        }
        None
    }

    // -------------------------------------------------------------------
    // Expression evaluation (constant folding)
    // -------------------------------------------------------------------

    fn eval_expr(&mut self, expr: &Expr) -> Result<Value, String> {
        match expr {
            Expr::Number { value, .. } => Ok(Value::Real(*value)),
            Expr::Integer { value, .. } => Ok(Value::Integer(*value)),
            Expr::Bool { value, .. } => Ok(Value::Bool(*value)),
            Expr::StringLit { value, .. } => Ok(Value::String(value.clone())),
            Expr::Gnd { .. } => {
                // gnd as a value doesn't make sense for params, but we can
                // return 0.0 as the ground reference.
                Ok(Value::Real(0.0))
            }
            Expr::Ident(ident) => {
                let name = &ident.name;

                // Cycle detection.
                if self.param_eval_stack.contains(name) {
                    return Err(format!("cyclic dependency on `{name}`"));
                }

                if let Some(val) = self.param_values.get(name) {
                    return Ok(val.clone());
                }

                // Well-known constants.
                match name.as_str() {
                    "pi" => return Ok(Value::Real(std::f64::consts::PI)),
                    "e" => return Ok(Value::Real(std::f64::consts::E)),
                    _ => {}
                }

                Err(format!("undefined identifier: `{name}`"))
            }
            Expr::BinOp {
                op, lhs, rhs, span, ..
            } => {
                let l = self.eval_expr(lhs)?;
                let r = self.eval_expr(rhs)?;
                eval_binop(*op, &l, &r, *span)
            }
            Expr::UnaryOp {
                op, operand, span, ..
            } => {
                let v = self.eval_expr(operand)?;
                eval_unaryop(*op, &v, *span)
            }
            Expr::Call { func, args, .. } => {
                // Evaluate builtin math functions.
                let evaluated: Result<Vec<Value>, String> =
                    args.iter().map(|a| self.eval_expr(a)).collect();
                let vals = evaluated?;
                eval_builtin_call(&func.name, &vals)
            }
            Expr::Block { entries, .. } => {
                // A block like `pulse: { v1: 0, v2: 3.3, ... }` — we can't
                // reduce this to a single Value. Return a string repr as
                // placeholder.
                let _ = entries;
                Ok(Value::String("<block>".to_string()))
            }
            _ => Err("cannot evaluate expression at compile time".to_string()),
        }
    }

    /// Evaluate an expression and extract as f64.
    fn eval_to_f64(&mut self, expr: &Expr) -> Option<f64> {
        match self.eval_expr(expr) {
            Ok(Value::Real(v)) => Some(v),
            Ok(Value::Integer(v)) => Some(v as f64),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Extract an identifier name from an expression, if it's a bare Ident.
fn expr_as_ident(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(id) => Some(&id.name),
        _ => None,
    }
}

/// Extract a net name from an expression. Handles Ident and Gnd.
fn expr_as_net_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(id) => Some(id.name.clone()),
        Expr::Gnd { .. } => Some("gnd".to_string()),
        _ => None,
    }
}

/// Check if a param name is a waveform type (should be lowered as a waveform block).
fn is_waveform_param(name: &str) -> bool {
    matches!(
        name,
        "pulse" | "sin" | "sine" | "exp" | "pwl" | "sffm" | "am"
    )
}

/// Known terminal-name params that should be treated as net connections.
fn is_connection_param(name: &str) -> bool {
    matches!(
        name,
        "gate"
            | "source"
            | "drain"
            | "bulk"
            | "base"
            | "collector"
            | "emitter"
            | "anode"
            | "cathode"
            | "pos"
            | "neg"
            | "in_pos"
            | "in_neg"
            | "out_pos"
            | "out_neg"
    )
}

/// Evaluate a binary operation on two constant values.
fn eval_binop(
    op: BinOp,
    lhs: &Value,
    rhs: &Value,
    _span: cirq_ast::span::Span,
) -> Result<Value, String> {
    let l = value_as_f64(lhs)?;
    let r = value_as_f64(rhs)?;

    let result = match op {
        BinOp::Add => l + r,
        BinOp::Sub => l - r,
        BinOp::Mul => l * r,
        BinOp::Div => {
            if r == 0.0 {
                return Err("division by zero".to_string());
            }
            l / r
        }
        BinOp::Mod => {
            if r == 0.0 {
                return Err("modulo by zero".to_string());
            }
            l % r
        }
        BinOp::Pow => l.powf(r),
        BinOp::Eq => return Ok(Value::Bool(l == r)),
        BinOp::Ne => return Ok(Value::Bool(l != r)),
        BinOp::Lt => return Ok(Value::Bool(l < r)),
        BinOp::Gt => return Ok(Value::Bool(l > r)),
        BinOp::Le => return Ok(Value::Bool(l <= r)),
        BinOp::Ge => return Ok(Value::Bool(l >= r)),
        BinOp::And => return Ok(Value::Bool(l != 0.0 && r != 0.0)),
        BinOp::Or => return Ok(Value::Bool(l != 0.0 || r != 0.0)),
    };
    Ok(Value::Real(result))
}

/// Evaluate a unary operation on a constant value.
fn eval_unaryop(op: UnaryOp, val: &Value, _span: cirq_ast::span::Span) -> Result<Value, String> {
    match op {
        UnaryOp::Neg => {
            let v = value_as_f64(val)?;
            Ok(Value::Real(-v))
        }
        UnaryOp::Not => match val {
            Value::Bool(b) => Ok(Value::Bool(!b)),
            _ => {
                let v = value_as_f64(val)?;
                Ok(Value::Bool(v == 0.0))
            }
        },
    }
}

/// Coerce a Value to f64.
fn value_as_f64(val: &Value) -> Result<f64, String> {
    match val {
        Value::Real(v) => Ok(*v),
        Value::Integer(v) => Ok(*v as f64),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::String(_) => Err("cannot use string in arithmetic".to_string()),
    }
}

/// Evaluate a built-in function call.
fn eval_builtin_call(name: &str, args: &[Value]) -> Result<Value, String> {
    match name {
        "sqrt" => {
            let v = one_arg_f64(args, "sqrt")?;
            Ok(Value::Real(v.sqrt()))
        }
        "abs" => {
            let v = one_arg_f64(args, "abs")?;
            Ok(Value::Real(v.abs()))
        }
        "log" | "ln" => {
            let v = one_arg_f64(args, name)?;
            Ok(Value::Real(v.ln()))
        }
        "log10" => {
            let v = one_arg_f64(args, "log10")?;
            Ok(Value::Real(v.log10()))
        }
        "exp" => {
            let v = one_arg_f64(args, "exp")?;
            Ok(Value::Real(v.exp()))
        }
        "sin" => {
            let v = one_arg_f64(args, "sin")?;
            Ok(Value::Real(v.sin()))
        }
        "cos" => {
            let v = one_arg_f64(args, "cos")?;
            Ok(Value::Real(v.cos()))
        }
        "tan" => {
            let v = one_arg_f64(args, "tan")?;
            Ok(Value::Real(v.tan()))
        }
        "pow" => {
            if args.len() != 2 {
                return Err(format!("pow expects 2 arguments, got {}", args.len()));
            }
            let base = value_as_f64(&args[0])?;
            let exp = value_as_f64(&args[1])?;
            Ok(Value::Real(base.powf(exp)))
        }
        "min" => {
            if args.len() != 2 {
                return Err(format!("min expects 2 arguments, got {}", args.len()));
            }
            let a = value_as_f64(&args[0])?;
            let b = value_as_f64(&args[1])?;
            Ok(Value::Real(a.min(b)))
        }
        "max" => {
            if args.len() != 2 {
                return Err(format!("max expects 2 arguments, got {}", args.len()));
            }
            let a = value_as_f64(&args[0])?;
            let b = value_as_f64(&args[1])?;
            Ok(Value::Real(a.max(b)))
        }
        _ => Err(format!("unknown function: `{name}`")),
    }
}

fn one_arg_f64(args: &[Value], name: &str) -> Result<f64, String> {
    if args.len() != 1 {
        return Err(format!("{name} expects 1 argument, got {}", args.len()));
    }
    value_as_f64(&args[0])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    fn compile(source: &str) -> Result<Circuit, Vec<Diagnostic>> {
        let ast = parse(source).map_err(|d| d)?;
        lower_to_ir(&ast)
    }

    fn compile_unwrap(source: &str) -> Circuit {
        compile(source).unwrap_or_else(|diags| {
            for d in &diags {
                eprintln!("{:?}: {}", d.severity, d.message);
            }
            panic!("lowering failed with {} diagnostics", diags.len());
        })
    }

    #[test]
    fn simple_resistor_circuit() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                R1: resistor(a -> b, 10000)
            }
            "#,
        );

        assert_eq!(circuit.name, "test");
        assert_eq!(circuit.elements.len(), 1);

        let r1 = &circuit.elements[0];
        assert_eq!(r1.name, "R1");
        assert!(matches!(r1.kind, ElementKind::Resistor));
        // Should have 2 connections: pos->a, neg->b.
        assert_eq!(r1.connections.len(), 2);
        assert_eq!(r1.connections[0].terminal, "pos");
        assert_eq!(r1.connections[1].terminal, "neg");

        // The value param should be 10000.
        assert_eq!(r1.params.len(), 1);
        assert_eq!(r1.params[0].0, "value");
        match &r1.params[0].1 {
            Value::Real(v) => assert!((v - 10000.0).abs() < 1e-6),
            Value::Integer(v) => assert_eq!(*v, 10000),
            _ => panic!("expected numeric value"),
        }

        // Should have nets: gnd (always), a, b.
        assert!(circuit.nets.len() >= 3);
        assert_eq!(circuit.nets[0].name, "gnd");
        assert!(circuit.nets[0].is_global);
    }

    #[test]
    fn gnd_net_handling() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                R1: resistor(a -> gnd, 1000)
            }
            "#,
        );

        // gnd should be Id(0).
        assert_eq!(circuit.nets[0].id, Id(0));
        assert_eq!(circuit.nets[0].name, "gnd");
        assert!(circuit.nets[0].is_global);

        // R1's neg connection should point to gnd (Id(0)).
        let r1 = &circuit.elements[0];
        let neg_conn = r1.connections.iter().find(|c| c.terminal == "neg").unwrap();
        assert_eq!(neg_conn.net, Id(0));
    }

    #[test]
    fn model_reference() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                model d1: diode {
                    is = 2.52e-9
                    rs = 0.568
                }

                D1: diode(a -> gnd, model: d1)
            }
            "#,
        );

        assert_eq!(circuit.models.len(), 1);
        assert_eq!(circuit.models[0].name, "d1");
        assert_eq!(circuit.models[0].device_type, cirq_ir::DeviceType::Diode);

        let d1 = &circuit.elements[0];
        assert!(d1.model.is_some());
        assert_eq!(d1.model.unwrap(), circuit.models[0].id);
    }

    #[test]
    fn duplicate_name_detection() {
        let result = compile(
            r#"
            circuit test {
                param x = 1
                param x = 2
            }
            "#,
        );

        assert!(result.is_err());
        let diags = result.unwrap_err();
        let dup = diags.iter().find(|d| d.message.contains("duplicate"));
        assert!(dup.is_some(), "expected duplicate declaration diagnostic");
    }

    #[test]
    fn expression_evaluation_arithmetic() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                param a = 10
                param b = 20
                let c = a + b
            }
            "#,
        );

        // c should be 30.
        let c = circuit.params.iter().find(|p| p.name == "c").unwrap();
        match &c.value {
            Value::Real(v) => assert!((v - 30.0).abs() < 1e-6),
            _ => panic!("expected Real value for c"),
        }
    }

    #[test]
    fn dc_sweep_analysis() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                V1: vsource(vdd -> gnd, dc: 5)
                analysis dc {
                    sweep V1: 0..5 step 0.1
                }
            }
            "#,
        );

        assert_eq!(circuit.analyses.len(), 1);
        match &circuit.analyses[0] {
            Analysis::Dc(dc) => {
                assert_eq!(dc.sweeps.len(), 1);
                let s = &dc.sweeps[0];
                assert!((s.start - 0.0).abs() < 1e-6);
                assert!((s.stop - 5.0).abs() < 1e-6);
                assert!((s.step - 0.1).abs() < 1e-6);
            }
            _ => panic!("expected DC analysis"),
        }
    }

    #[test]
    fn ac_analysis_lowering() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                analysis ac {
                    start: 1
                    stop: 1000000000
                    points: 100
                    scale: decade
                }
            }
            "#,
        );

        assert_eq!(circuit.analyses.len(), 1);
        match &circuit.analyses[0] {
            Analysis::Ac(ac) => {
                assert!((ac.start - 1.0).abs() < 1e-6);
                assert!((ac.stop - 1e9).abs() < 1.0);
                assert_eq!(ac.points, 100);
                assert_eq!(ac.scale, FrequencyScale::Decade);
            }
            _ => panic!("expected AC analysis"),
        }
    }

    #[test]
    fn tran_analysis_lowering() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                analysis tran {
                    step: 1e-9
                    stop: 100e-9
                }
            }
            "#,
        );

        assert_eq!(circuit.analyses.len(), 1);
        match &circuit.analyses[0] {
            Analysis::Tran(tran) => {
                assert!((tran.step - 1e-9).abs() < 1e-15);
                assert!((tran.stop - 100e-9).abs() < 1e-15);
                assert!((tran.start - 0.0).abs() < 1e-15);
                assert!(!tran.uic);
            }
            _ => panic!("expected Tran analysis"),
        }
    }

    #[test]
    fn module_with_parameters() {
        // Modules are collected but not inlined; we just verify params.
        let circuit = compile_unwrap(
            r#"
            circuit test {
                param vdd_voltage = 1.8
                param wp = 2e-6
                param wn = 1e-6
                let ratio = wp / wn
            }
            "#,
        );

        assert_eq!(circuit.params.len(), 4);
        let ratio = circuit.params.iter().find(|p| p.name == "ratio").unwrap();
        match &ratio.value {
            Value::Real(v) => assert!((v - 2.0).abs() < 1e-6),
            _ => panic!("expected Real value for ratio"),
        }
    }

    #[test]
    fn unknown_element_type_diagnostic() {
        let result = compile(
            r#"
            circuit test {
                X1: foobar(a -> b)
            }
            "#,
        );

        assert!(result.is_err());
        let diags = result.unwrap_err();
        let unknown = diags
            .iter()
            .find(|d| d.message.contains("unknown element type"));
        assert!(
            unknown.is_some(),
            "expected unknown element type diagnostic, got: {diags:?}"
        );
    }

    #[test]
    fn op_analysis() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                analysis op {}
            }
            "#,
        );

        assert_eq!(circuit.analyses.len(), 1);
        assert!(matches!(circuit.analyses[0], Analysis::Op));
    }

    // -------------------------------------------------------------------
    // Gap 1.5: Coupling element
    // -------------------------------------------------------------------

    #[test]
    fn coupling_element() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                L1: inductor(a -> gnd, 10e-6)
                L2: inductor(b -> gnd, 10e-6)
                K1: coupling(l1: "L1", l2: "L2", coupling: 0.99)
            }
            "#,
        );

        let k1 = circuit.elements.iter().find(|e| e.name == "K1").unwrap();
        assert!(matches!(k1.kind, ElementKind::Coupling));
    }

    // -------------------------------------------------------------------
    // Gap 1.1/1.2: Waveforms and AC spec
    // -------------------------------------------------------------------

    #[test]
    fn voltage_source_with_pulse_waveform() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                V1: vsource(a -> gnd, dc: 0, pulse: { v1: 0, v2: 3.3, td: 1e-9, tr: 0.5e-9, tf: 0.5e-9, pw: 5e-9, per: 10e-9 })
            }
            "#,
        );

        let v1 = circuit.elements.iter().find(|e| e.name == "V1").unwrap();
        assert!(matches!(v1.kind, ElementKind::VoltageSource));

        let spec = v1.source_spec.as_ref().expect("should have source_spec");
        assert_eq!(spec.dc, Some(0.0));

        match &spec.waveform {
            Some(Waveform::Pulse {
                v1,
                v2,
                td,
                tr,
                tf,
                pw,
                per,
            }) => {
                assert!((*v1 - 0.0).abs() < 1e-12);
                assert!((*v2 - 3.3).abs() < 1e-12);
                assert!((td.unwrap() - 1e-9).abs() < 1e-18);
                assert!((tr.unwrap() - 0.5e-9).abs() < 1e-18);
                assert!((tf.unwrap() - 0.5e-9).abs() < 1e-18);
                assert!((pw.unwrap() - 5e-9).abs() < 1e-18);
                assert!((per.unwrap() - 10e-9).abs() < 1e-18);
            }
            other => panic!("expected Pulse waveform, got {other:?}"),
        }
    }

    #[test]
    fn voltage_source_with_sin_waveform() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                V1: vsource(a -> gnd, sin: { v0: 0, va: 1, freq: 1000 })
            }
            "#,
        );

        let v1 = circuit.elements.iter().find(|e| e.name == "V1").unwrap();
        let spec = v1.source_spec.as_ref().expect("should have source_spec");

        match &spec.waveform {
            Some(Waveform::Sin { v0, va, freq, .. }) => {
                assert!((*v0 - 0.0).abs() < 1e-12);
                assert!((*va - 1.0).abs() < 1e-12);
                assert!((freq.unwrap() - 1000.0).abs() < 1e-6);
            }
            other => panic!("expected Sin waveform, got {other:?}"),
        }
    }

    #[test]
    fn voltage_source_with_ac_spec_scalar() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                V1: vsource(a -> gnd, dc: 0, ac: 1)
            }
            "#,
        );

        let v1 = circuit.elements.iter().find(|e| e.name == "V1").unwrap();
        let spec = v1.source_spec.as_ref().expect("should have source_spec");

        let ac = spec.ac.as_ref().expect("should have ac spec");
        assert!((ac.mag - 1.0).abs() < 1e-12);
        assert!((ac.phase - 0.0).abs() < 1e-12);
    }

    #[test]
    fn voltage_source_with_ac_spec_block() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                V1: vsource(a -> gnd, ac: { mag: 1.5, phase: 90 })
            }
            "#,
        );

        let v1 = circuit.elements.iter().find(|e| e.name == "V1").unwrap();
        let spec = v1.source_spec.as_ref().expect("should have source_spec");

        let ac = spec.ac.as_ref().expect("should have ac spec");
        assert!((ac.mag - 1.5).abs() < 1e-12);
        assert!((ac.phase - 90.0).abs() < 1e-12);
    }

    // -------------------------------------------------------------------
    // Gap 1.3: Noise analysis
    // -------------------------------------------------------------------

    #[test]
    fn noise_analysis_lowering() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                V1: vsource(in -> gnd, dc: 0, ac: 1)
                R1: resistor(in -> out, 1000)
                analysis noise {
                    output: out
                    ref: gnd
                    source: V1
                    start: 1
                    stop: 1000000000
                    points: 100
                    scale: decade
                }
            }
            "#,
        );

        assert_eq!(circuit.analyses.len(), 1);
        match &circuit.analyses[0] {
            Analysis::Noise(n) => {
                assert!((n.start - 1.0).abs() < 1e-6);
                assert!((n.stop - 1e9).abs() < 1.0);
                assert_eq!(n.points, 100);
                assert_eq!(n.scale, FrequencyScale::Decade);
            }
            other => panic!("expected Noise analysis, got {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // Gap 1.4: PZ analysis
    // -------------------------------------------------------------------

    #[test]
    fn pz_analysis_lowering() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                R1: resistor(in -> out, 1000)
                C1: capacitor(out -> gnd, 1e-12)
                analysis pz {
                    input_pos: in
                    input_neg: gnd
                    output_pos: out
                    output_neg: gnd
                    transfer: voltage
                    type: both
                }
            }
            "#,
        );

        assert_eq!(circuit.analyses.len(), 1);
        match &circuit.analyses[0] {
            Analysis::Pz(pz) => {
                assert_eq!(pz.transfer, cirq_ir::TransferType::Voltage);
                assert_eq!(pz.analysis_type, cirq_ir::PzType::Both);
            }
            other => panic!("expected Pz analysis, got {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // Gap 2.6: Model inheritance param merging
    // -------------------------------------------------------------------

    #[test]
    fn model_inheritance_merges_params() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                model nch_base: nmos {
                    vto = 0.7
                    kp = 110e-6
                    lambda = 0.04
                }
                model nch_fast: nch_base {
                    vto = 0.5
                }
            }
            "#,
        );

        assert_eq!(circuit.models.len(), 2);

        let derived = circuit
            .models
            .iter()
            .find(|m| m.name == "nch_fast")
            .unwrap();
        assert_eq!(derived.device_type, cirq_ir::DeviceType::Nmos);

        // Should have all 3 params from base, with vto overridden.
        assert_eq!(derived.params.len(), 3);

        let vto = derived.params.iter().find(|p| p.0 == "vto").unwrap();
        match &vto.1 {
            Value::Real(v) => assert!((*v - 0.5).abs() < 1e-12, "vto should be 0.5, got {v}"),
            _ => panic!("expected Real for vto"),
        }

        let kp = derived.params.iter().find(|p| p.0 == "kp").unwrap();
        match &kp.1 {
            Value::Real(v) => assert!((*v - 110e-6).abs() < 1e-12, "kp should be 110e-6, got {v}"),
            _ => panic!("expected Real for kp"),
        }

        let lambda = derived.params.iter().find(|p| p.0 == "lambda").unwrap();
        match &lambda.1 {
            Value::Real(v) => assert!((*v - 0.04).abs() < 1e-12, "lambda should be 0.04, got {v}"),
            _ => panic!("expected Real for lambda"),
        }
    }

    // -------------------------------------------------------------------
    // Gap 2.3: MESFET element kind
    // -------------------------------------------------------------------

    #[test]
    fn mesfet_element_kind() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                model nm: nmesfet {
                    vto = -1.3
                }
                Z1: nmesfet(drain: d, gate: g, source: s, model: nm)
            }
            "#,
        );

        let z1 = circuit.elements.iter().find(|e| e.name == "Z1").unwrap();
        assert!(matches!(z1.kind, ElementKind::NMesfet));
        assert!(z1.model.is_some());
    }

    // -------------------------------------------------------------------
    // Gap 3.7: Tran tmax
    // -------------------------------------------------------------------

    #[test]
    fn tran_analysis_with_tmax() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                analysis tran {
                    step: 1e-9
                    stop: 100e-9
                    tmax: 0.5e-9
                }
            }
            "#,
        );

        assert_eq!(circuit.analyses.len(), 1);
        match &circuit.analyses[0] {
            Analysis::Tran(tran) => {
                assert!((tran.step - 1e-9).abs() < 1e-15);
                assert!((tran.stop - 100e-9).abs() < 1e-15);
                assert!(tran.tmax.is_some());
                assert!((tran.tmax.unwrap() - 0.5e-9).abs() < 1e-18);
            }
            other => panic!("expected Tran analysis, got {other:?}"),
        }
    }
}
