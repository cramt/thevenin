//! AST-to-IR lowering -- semantic analysis pass that resolves names, evaluates
//! constant expressions, links models, and produces [`cirq_ir::Circuit`].

use std::collections::HashMap;

use cirq_ast::{
    AnalysisDecl, AnalysisItem, Argument, BinOp, CircuitItem, ElementInst, Expr, LetDecl, ModelDef,
    ParamDecl, SourceFile, TopLevel, UnaryOp,
};
use cirq_ir::{
    AcAnalysis, Analysis, Circuit, Connection, DcAnalysis, DcSweep, Element, ElementKind,
    FrequencyScale, Id, Model, Net, ResolvedParam, TranAnalysis, Value,
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
        "tline" | "transmission_line" => Some(ElementKind::TransmissionLine),
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

        let mut params = Vec::new();
        for mp in &m.params {
            match self.eval_expr(&mp.value) {
                Ok(val) => params.push((mp.name.name.clone(), val)),
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

        let id = self.alloc_element_id();
        self.element_by_name.insert(name.clone(), id);
        self.elements.push(Element {
            id,
            name: name.clone(),
            kind,
            connections,
            params: element_params,
            model: model_ref,
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
            "noise" => {
                // Noise requires detailed setup; emit a placeholder for now.
                self.diags.push(
                    Diagnostic::warning("noise analysis lowering is not fully implemented")
                        .with_span(a.span),
                );
                None
            }
            "pz" => {
                self.diags.push(
                    Diagnostic::warning("pz analysis lowering is not fully implemented")
                        .with_span(a.span),
                );
                None
            }
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
        })
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
}
