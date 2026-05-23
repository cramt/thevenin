//! AST-to-IR lowering -- semantic analysis pass that resolves names, evaluates
//! constant expressions, links models, and produces [`cirq_ir::Circuit`].

use std::collections::HashMap;

use cirq_ast::{
    AnalysisDecl, AnalysisItem, Argument, BinOp, CircuitItem, CoupledLineDecl, ElementInst, Expr,
    FuncDecl, IcDecl, LetDecl, ModelDef, ModuleDef, ModuleInst, OptionsDecl, ParamDecl, SaveDecl,
    SaveTarget, SourceFile, TempDecl, TopLevel, UnaryOp,
};
use cirq_ir::{
    AcAnalysis, AcSpec, Analysis, BehavioralMode, Circuit, Connection, DcAnalysis, DcSweep,
    Element, ElementKind, FrequencyScale, FuncDef, Id, Model, Net, ResolvedParam, SourceSpec,
    TranAnalysis, Value, Waveform,
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

    // Also collect top-level models, functions, and modules (outside circuits).
    for item in &source_file.items {
        match item {
            TopLevel::Model(m) => ctx.lower_model_def(m),
            TopLevel::Func(f) => ctx.lower_func_decl(f),
            TopLevel::Module(m) => {
                ctx.module_defs.insert(m.name.name.clone(), m.clone());
            }
            _ => {}
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
        csparams: Vec::new(),
        options: ctx.options,
        temps: ctx.temps,
        nodeset: Vec::new(),
        measures: Vec::new(),
        save: ctx.save,
        funcs: ctx.funcs,
        initial_conditions: ctx.initial_conditions,
        code_blocks: ctx.code_blocks,
        // Cirq source has no Item::Raw analogue: every directive is typed in
        // the AST. Round-tripping a SPICE Item::Raw through Cirq source is not
        // supported (would require a syntax for it); the lossless preservation
        // path is the SPICE importer ↔ to_netlist adapter.
        raw_directives: Vec::new(),
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
        ElementKind::BehavioralSource { .. } => &["pos", "neg"],
        ElementKind::Diode => &["anode", "cathode"],
        ElementKind::Npn | ElementKind::Pnp => &["collector", "base", "emitter"],
        ElementKind::Nmos | ElementKind::Pmos => &["drain", "gate", "source", "bulk"],
        ElementKind::NJfet | ElementKind::PJfet => &["drain", "gate", "source"],
        ElementKind::NMesfet | ElementKind::PMesfet => &["drain", "gate", "source"],
        ElementKind::Vcvs | ElementKind::Vccs | ElementKind::Ccvs | ElementKind::Cccs => {
            &["out_pos", "out_neg", "in_pos", "in_neg"]
        }
        ElementKind::TransmissionLine | ElementKind::Txl => {
            &["in_pos", "in_neg", "out_pos", "out_neg"]
        }
        ElementKind::Coupling => &[],
        // CoupledLine and Xspice have variable-width connections; no static pin list.
        ElementKind::CoupledLine { .. } | ElementKind::Xspice { .. } => &[],
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
        "tline" | "transmission_line" | "ltra" => Some(ElementKind::TransmissionLine),
        "txl" => Some(ElementKind::Txl),
        "nmesfet" => Some(ElementKind::NMesfet),
        "pmesfet" => Some(ElementKind::PMesfet),
        "behavioral" => Some(ElementKind::BehavioralSource {
            mode: BehavioralMode::Voltage,
            spec: String::new(),
        }),
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

    // Module definitions for subcircuit flattening.
    module_defs: HashMap<String, ModuleDef>,

    // For cycle detection during param evaluation.
    param_eval_stack: Vec<String>,

    // For cycle detection during module instantiation.
    module_inst_stack: Vec<String>,

    // Net name remapping during module inlining: port name → actual net name.
    // Stacked per module instantiation level.
    net_remap: HashMap<String, String>,

    // Param overrides supplied at the current module instantiation site,
    // evaluated in the caller's scope. Consumed by `lower_param` to shadow
    // the param's default. Cleared/saved per instantiation level.
    param_overrides: HashMap<String, Value>,

    // Verbatim embedded code blocks (language + lines).
    code_blocks: Vec<cirq_ir::CodeBlock>,

    // Simulation options, temperature, and save targets.
    options: Vec<(String, Value)>,
    temps: Vec<f64>,
    save: Vec<String>,
    funcs: Vec<FuncDef>,
    initial_conditions: Vec<(Id, f64)>,
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
            module_defs: HashMap::new(),
            param_eval_stack: Vec::new(),
            module_inst_stack: Vec::new(),
            net_remap: HashMap::new(),
            code_blocks: Vec::new(),
            options: Vec::new(),
            temps: Vec::new(),
            save: Vec::new(),
            funcs: Vec::new(),
            initial_conditions: Vec::new(),
            param_overrides: HashMap::new(),
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
        self.lower_circuit_body_prefixed(items, "");
    }

    fn lower_circuit_body_prefixed(&mut self, items: &[CircuitItem], prefix: &str) {
        // Two-pass: first collect all declaration names to detect duplicates
        // and register models/params, then lower elements and analyses.

        // Pass 1: declarations (params, lets, models, globals, module defs).
        for item in items {
            match item {
                CircuitItem::Param(p) => self.lower_param(p, prefix),
                CircuitItem::Let(l) => self.lower_let(l, prefix),
                CircuitItem::ModelDef(m) => self.lower_model_def(m),
                CircuitItem::Global(g) => {
                    self.intern_net(&g.name.name, true);
                }
                CircuitItem::ModuleDef(m) => {
                    self.module_defs.insert(m.name.name.clone(), m.clone());
                }
                _ => {}
            }
        }

        // Pass 2: elements, module instances, analyses, options, temp.
        for item in items {
            match item {
                CircuitItem::Element(e) => self.lower_element_prefixed(e, prefix),
                CircuitItem::Analysis(a) => self.lower_analysis(a),
                CircuitItem::ModuleInst(mi) => self.lower_module_inst(mi, prefix),
                CircuitItem::Options(o) => self.lower_options_decl(o),
                CircuitItem::Temp(t) => self.lower_temp_decl(t),
                CircuitItem::Save(s) => self.lower_save_decl(s),
                CircuitItem::Func(f) => self.lower_func_decl(f),
                CircuitItem::Ic(ic) => self.lower_ic_decl(ic),
                CircuitItem::CoupledLine(cl) => self.lower_coupled_line_decl(cl, prefix),
                CircuitItem::Code(c) => {
                    self.code_blocks.push(cirq_ir::CodeBlock::from_lines(
                        c.language.clone(),
                        c.lines.clone(),
                    ));
                }
                // Already handled in pass 1, or collected above.
                CircuitItem::Param(_)
                | CircuitItem::Let(_)
                | CircuitItem::ModelDef(_)
                | CircuitItem::ModuleDef(_)
                | CircuitItem::Global(_) => {}
            }
        }
    }

    // -------------------------------------------------------------------
    // Param / let lowering
    // -------------------------------------------------------------------

    fn lower_param(&mut self, p: &ParamDecl, prefix: &str) {
        let name = &p.name.name;

        if self.param_values.contains_key(name) {
            self.diags.push(
                Diagnostic::error(format!("duplicate declaration: `{name}`"))
                    .with_span(p.name.span),
            );
            return;
        }

        // An override supplied at the instantiation site shadows the default.
        // Overrides were evaluated in the caller's scope before we swapped
        // into this module's, so we just consume the precomputed Value here.
        if let Some(val) = self.param_overrides.remove(name) {
            let output_name = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}.{name}")
            };
            self.resolved_params.push(ResolvedParam {
                name: output_name,
                value: val.clone(),
            });
            self.param_values.insert(name.clone(), val);
            return;
        }

        if let Some(ref default) = p.default {
            match self.eval_expr(default) {
                Ok(val) => {
                    // Use prefixed name for output (e.g. "buf1.inv1.wp") so
                    // params from different module instances don't collide.
                    // The bare name is used in param_values for expression
                    // evaluation within the current module scope.
                    let output_name = if prefix.is_empty() {
                        name.clone()
                    } else {
                        format!("{prefix}.{name}")
                    };
                    self.resolved_params.push(ResolvedParam {
                        name: output_name,
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
        // A param without a default or override is allowed (forward-declared
        // for modules); attempts to use it in expressions will fail at
        // eval_expr time with "undefined param".
    }

    fn lower_let(&mut self, l: &LetDecl, prefix: &str) {
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
                let output_name = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}.{name}")
                };
                self.resolved_params.push(ResolvedParam {
                    name: output_name,
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

        let dev_type = match &device_type {
            Some(dt) => dt.clone(),
            None => {
                // The device_type might be a base model name (inheritance).
                // If so, look up the base model's device type.
                if let Some(&base_id) = self.model_by_name.get(&m.device_type.name) {
                    if let Some(base) = self.models.iter().find(|model| model.id == base_id) {
                        base.device_type.clone()
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

    fn lower_element_prefixed(&mut self, e: &ElementInst, prefix: &str) {
        let raw_name = &e.name.name;
        let name = if prefix.is_empty() {
            raw_name.clone()
        } else {
            format!("{prefix}.{raw_name}")
        };

        if self.element_by_name.contains_key(&name) {
            self.diags.push(
                Diagnostic::error(format!("duplicate element: `{name}`")).with_span(e.name.span),
            );
            return;
        }

        // Resolve element kind.  If the type name doesn't match a built-in
        // element, check whether it names a locally defined module and, if so,
        // handle it as a module instantiation instead.
        let kind = match element_kind_from_str(&e.element_type.name) {
            Some(k) => k,
            None => {
                if self.module_defs.contains_key(&e.element_type.name) {
                    // Treat as a local module instantiation.
                    self.lower_local_module_inst(e, prefix);
                    return;
                }
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
        let is_behavioral = matches!(kind, ElementKind::BehavioralSource { .. });
        let mut behavioral_mode: Option<BehavioralMode> = None;
        let mut behavioral_spec: Option<String> = None;

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

                    let from_net = self.resolve_net_ident_prefixed(from, prefix);
                    let pin_from = pins[positional_conn_idx].to_string();
                    connections.push(Connection {
                        terminal: pin_from,
                        net: from_net,
                    });
                    positional_conn_idx += 1;

                    if positional_conn_idx < pins.len() {
                        let to_net = self.resolve_net_ident_prefixed(to, prefix);
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
                    let from_net = self.resolve_net_ident_prefixed(from, prefix);
                    let to_net = self.resolve_net_ident_prefixed(to, prefix);
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
                            let net_id = self.resolve_net_name_prefixed(&net_name, prefix);
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
                    } else if is_behavioral && (param_name == "v" || param_name == "i") {
                        // Behavioral source mode and expression spec.
                        behavioral_mode = Some(if param_name == "v" {
                            BehavioralMode::Voltage
                        } else {
                            BehavioralMode::Current
                        });
                        behavioral_spec = Some(expr_to_spice_string(value));
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

        // Finalize behavioral source kind with the actual mode and spec.
        let kind = if is_behavioral {
            match (behavioral_mode, behavioral_spec) {
                (Some(mode), Some(spec)) => ElementKind::BehavioralSource { mode, spec },
                _ => {
                    self.diags.push(
                        Diagnostic::error(
                            "behavioral source requires a `v:` or `i:` argument specifying the expression"
                        )
                        .with_span(e.span),
                    );
                    return;
                }
            }
        } else {
            kind
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

    /// Resolve a net name with prefix and remapping applied.
    /// Used during module inlining to map port names to caller nets and
    /// prefix internal nets with the instance path.
    fn resolve_net_name_prefixed(&mut self, name: &str, prefix: &str) -> Id {
        // Global nets are never remapped or prefixed.
        if name == "gnd" {
            return self.intern_net("gnd", true);
        }
        // Check if this net is remapped (port binding).
        if let Some(remapped) = self.net_remap.get(name) {
            let remapped = remapped.clone();
            return self.intern_net(&remapped, false);
        }
        // Otherwise, prefix the net name for hierarchy.
        if prefix.is_empty() {
            self.intern_net(name, false)
        } else {
            let prefixed = format!("{prefix}.{name}");
            self.intern_net(&prefixed, false)
        }
    }

    /// Resolve an identifier to a net Id, applying prefix and remapping.
    fn resolve_net_ident_prefixed(&mut self, ident: &cirq_ast::Ident, prefix: &str) -> Id {
        self.resolve_net_name_prefixed(&ident.name, prefix)
    }

    // -------------------------------------------------------------------
    // Coupled transmission line lowering
    // -------------------------------------------------------------------

    fn lower_coupled_line_decl(&mut self, cl: &CoupledLineDecl, prefix: &str) {
        let elem_name = if prefix.is_empty() {
            cl.name.name.clone()
        } else {
            format!("{prefix}.{}", cl.name.name)
        };

        // Extract the known fields: in, out, gnd, model.
        let mut in_nets: Vec<String> = Vec::new();
        let mut out_nets: Vec<String> = Vec::new();
        let mut gnd_net: Option<String> = None;
        let mut model_name: Option<String> = None;
        let mut extra_params: Vec<(String, Value)> = Vec::new();

        for field in &cl.fields {
            let key = field.key.name.as_str();
            match key {
                "in" | "out" => {
                    // Expect a list expression: [a1, a2, ...]
                    let nets = match &field.value {
                        Expr::List { elements, .. } => {
                            let mut names = Vec::new();
                            for e in elements {
                                match e {
                                    Expr::Ident(id) => names.push(id.name.clone()),
                                    Expr::Gnd { .. } => names.push("gnd".to_owned()),
                                    _ => {
                                        self.diags.push(
                                            Diagnostic::error(format!(
                                                "coupled_line `{key}` list elements must be identifiers"
                                            ))
                                            .with_span(cl.span),
                                        );
                                        return;
                                    }
                                }
                            }
                            names
                        }
                        _ => {
                            self.diags.push(
                                Diagnostic::error(format!(
                                    "coupled_line `{key}` must be a list, e.g. [{key}: [a, b]]"
                                ))
                                .with_span(field.span),
                            );
                            return;
                        }
                    };
                    if key == "in" {
                        in_nets = nets;
                    } else {
                        out_nets = nets;
                    }
                }
                "gnd" => match &field.value {
                    Expr::Ident(id) => gnd_net = Some(id.name.clone()),
                    Expr::Gnd { .. } => gnd_net = Some("gnd".to_owned()),
                    _ => {
                        self.diags.push(
                            Diagnostic::error("coupled_line `gnd` must be a net name")
                                .with_span(field.span),
                        );
                        return;
                    }
                },
                "model" => match &field.value {
                    Expr::Ident(id) => model_name = Some(id.name.clone()),
                    _ => {
                        self.diags.push(
                            Diagnostic::error("coupled_line `model` must be an identifier")
                                .with_span(field.span),
                        );
                        return;
                    }
                },
                _ => {
                    // Any other field becomes a param.
                    match self.eval_expr(&field.value) {
                        Ok(val) => extra_params.push((key.to_owned(), val)),
                        Err(msg) => {
                            self.diags.push(
                                Diagnostic::error(format!(
                                    "cannot evaluate coupled_line param `{key}`: {msg}"
                                ))
                                .with_span(field.span),
                            );
                        }
                    }
                }
            }
        }

        // Validate required fields.
        if in_nets.is_empty() {
            self.diags.push(
                Diagnostic::error("coupled_line is missing `in` port list").with_span(cl.span),
            );
            return;
        }
        if out_nets.is_empty() {
            self.diags.push(
                Diagnostic::error("coupled_line is missing `out` port list").with_span(cl.span),
            );
            return;
        }
        if in_nets.len() != out_nets.len() {
            self.diags.push(
                Diagnostic::error(format!(
                    "coupled_line `in` ({}) and `out` ({}) lists must have the same length",
                    in_nets.len(),
                    out_nets.len()
                ))
                .with_span(cl.span),
            );
            return;
        }

        let width = in_nets.len();
        let gnd_name = gnd_net.unwrap_or_else(|| "gnd".to_owned());

        // Build connections: in0, in1, ..., gnd, out0, out1, ...
        let mut connections = Vec::new();
        for (i, net_name) in in_nets.iter().enumerate() {
            let net_id = self.resolve_net_name_prefixed(net_name, prefix);
            connections.push(Connection {
                terminal: format!("in{i}"),
                net: net_id,
            });
        }
        let gnd_id = self.resolve_net_name_prefixed(&gnd_name, prefix);
        connections.push(Connection {
            terminal: "gnd".to_owned(),
            net: gnd_id,
        });
        for (i, net_name) in out_nets.iter().enumerate() {
            let net_id = self.resolve_net_name_prefixed(net_name, prefix);
            connections.push(Connection {
                terminal: format!("out{i}"),
                net: net_id,
            });
        }

        // Build params.
        let mut params = extra_params;
        if let Some(m) = model_name {
            params.push(("model".to_owned(), Value::String(m)));
        }

        let id = self.alloc_element_id();
        self.element_by_name.insert(elem_name.clone(), id);
        self.elements.push(Element {
            id,
            name: elem_name,
            kind: ElementKind::CoupledLine { width },
            connections,
            params,
            model: None,
            source_spec: None,
        });
    }

    // -------------------------------------------------------------------
    // Module instantiation (subcircuit flattening)
    // -------------------------------------------------------------------

    /// Inline a module instantiation by flattening its body into the current circuit.
    ///
    /// The strategy:
    /// 1. Look up the module definition.
    /// 2. Detect instantiation cycles.
    /// 3. Map module ports to caller nets (via `net_remap`).
    /// 4. Recursively lower the module body with a hierarchical prefix.
    fn lower_module_inst(&mut self, mi: &ModuleInst, parent_prefix: &str) {
        // Build the module name from the qualified name segments.
        let module_name = mi
            .module_name
            .segments
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join(".");

        // Look up the module definition.
        let module_def = match self.module_defs.get(&module_name) {
            Some(m) => m.clone(),
            None => {
                self.diags.push(
                    Diagnostic::error(format!("unknown module: `{module_name}`"))
                        .with_span(mi.module_name.span),
                );
                return;
            }
        };

        // Cycle detection.
        if self.module_inst_stack.contains(&module_name) {
            self.diags.push(
                Diagnostic::error(format!(
                    "recursive module instantiation: `{module_name}` (cycle: {})",
                    self.module_inst_stack.join(" → ")
                ))
                .with_span(mi.span),
            );
            return;
        }

        // Build the hierarchical instance prefix.
        let inst_name = &mi.name.name;
        let inst_prefix = if parent_prefix.is_empty() {
            inst_name.clone()
        } else {
            format!("{parent_prefix}.{inst_name}")
        };

        // Build port-to-net remapping and param-overrides from the
        // instantiation arguments. The module's ports define formal net
        // parameters; declared `param` items inside the module body define
        // tunable values that the caller may override.
        let port_names: std::collections::HashSet<&str> = module_def
            .ports
            .iter()
            .map(|p| p.name.name.as_str())
            .collect();
        let param_names: std::collections::HashSet<&str> = module_def
            .body
            .iter()
            .filter_map(|item| match item {
                CircuitItem::Param(p) => Some(p.name.name.as_str()),
                _ => None,
            })
            .collect();

        let mut port_remap: HashMap<String, String> = HashMap::new();
        let mut overrides: HashMap<String, Value> = HashMap::new();
        let ports = &module_def.ports;

        for arg in &mi.args {
            match arg {
                Argument::Named { name, value } => {
                    let arg_name = name.name.as_str();
                    if port_names.contains(arg_name) {
                        // Named port binding: `in: caller_net`
                        if let Some(net_name) = expr_as_net_name(value) {
                            port_remap.insert(name.name.clone(), net_name);
                        } else if let Expr::Ident(ident) = value {
                            port_remap.insert(name.name.clone(), ident.name.clone());
                        } else {
                            self.diags.push(
                                Diagnostic::error(format!(
                                    "module port `{}` must be bound to a net name",
                                    name.name
                                ))
                                .with_span(name.span),
                            );
                        }
                    } else if param_names.contains(arg_name) {
                        // Named param override: `w: 2u`. Evaluate in the
                        // caller's scope (which is the current `param_values`
                        // because we haven't swapped scopes yet).
                        match self.eval_expr(value) {
                            Ok(val) => {
                                overrides.insert(name.name.clone(), val);
                            }
                            Err(msg) => {
                                self.diags.push(
                                    Diagnostic::error(format!(
                                        "cannot evaluate override for param `{}`: {msg}",
                                        name.name
                                    ))
                                    .with_span(name.span),
                                );
                            }
                        }
                    } else {
                        self.diags.push(
                            Diagnostic::error(format!(
                                "module `{module_name}` has no port or param named `{}`",
                                name.name
                            ))
                            .with_span(name.span),
                        );
                    }
                }
                Argument::Positional(expr) => {
                    // Positional port binding: maps to ports in declaration order.
                    let idx = port_remap.len();
                    if idx < ports.len() {
                        let port_name = &ports[idx].name.name;
                        if let Some(net_name) = expr_as_net_name(expr) {
                            port_remap.insert(port_name.clone(), net_name);
                        } else if let Expr::Ident(ident) = expr {
                            port_remap.insert(port_name.clone(), ident.name.clone());
                        } else {
                            self.diags.push(
                                Diagnostic::error(format!(
                                    "module port `{port_name}` must be bound to a net name"
                                ))
                                .with_span(mi.span),
                            );
                        }
                    } else {
                        self.diags.push(
                            Diagnostic::error(format!(
                                "too many arguments for module `{module_name}` (expected {})",
                                ports.len()
                            ))
                            .with_span(mi.span),
                        );
                    }
                }
                Argument::Connection { from, to } => {
                    // Connection-style: maps two ports positionally.
                    let idx = port_remap.len();
                    if idx < ports.len() {
                        port_remap.insert(ports[idx].name.name.clone(), from.name.clone());
                    }
                    let idx2 = port_remap.len();
                    if idx2 < ports.len() {
                        port_remap.insert(ports[idx2].name.name.clone(), to.name.clone());
                    }
                }
                Argument::NamedConnection { name, from, to } => {
                    // Named connection pair binding — less common for modules
                    // but handle it: `control: a -> b` maps to ports
                    // `control_pos` and `control_neg` (or similar).
                    port_remap.insert(format!("{}_pos", name.name), from.name.clone());
                    port_remap.insert(format!("{}_neg", name.name), to.name.clone());
                }
            }
        }

        // Save current net_remap and replace with the port bindings for
        // this instantiation level.
        let saved_remap = std::mem::replace(&mut self.net_remap, port_remap);

        // Save param scope — module-internal params must not leak to outer
        // scope or collide across multiple instantiations of the same module.
        let saved_params = self.param_values.clone();

        // Stash override map for the body; lower_param consumes it. Per-
        // instance, so save/restore the outer one (an enclosing instantiation
        // might still be unwinding).
        let saved_overrides = std::mem::replace(&mut self.param_overrides, overrides);

        // Push onto the instantiation stack for cycle detection.
        self.module_inst_stack.push(module_name.clone());

        // Recursively lower the module body with the instance prefix.
        // Params declared inside the module body will be prefixed by
        // lower_param/lower_let using inst_prefix.
        self.lower_circuit_body_prefixed(&module_def.body, &inst_prefix);

        // Restore state.
        self.module_inst_stack.pop();
        self.net_remap = saved_remap;
        self.param_overrides = saved_overrides;

        // Restore outer param scope.
        self.param_values = saved_params;
    }

    /// Handle a local module instantiation that was parsed as an `ElementInst`
    /// because the module name has no dots (single identifier).
    ///
    /// This synthesizes a `ModuleInst`-like lowering from the element's name,
    /// type (= module name), and arguments.
    fn lower_local_module_inst(&mut self, e: &ElementInst, prefix: &str) {
        let module_name = &e.element_type.name;

        let module_def = match self.module_defs.get(module_name) {
            Some(m) => m.clone(),
            None => {
                self.diags.push(
                    Diagnostic::error(format!("unknown module: `{module_name}`"))
                        .with_span(e.element_type.span),
                );
                return;
            }
        };

        // Cycle detection.
        if self.module_inst_stack.contains(module_name) {
            self.diags.push(
                Diagnostic::error(format!(
                    "recursive module instantiation: `{module_name}` (cycle: {})",
                    self.module_inst_stack.join(" → ")
                ))
                .with_span(e.span),
            );
            return;
        }

        // Build hierarchical instance prefix.
        let inst_name = &e.name.name;
        let inst_prefix = if prefix.is_empty() {
            inst_name.clone()
        } else {
            format!("{prefix}.{inst_name}")
        };

        // Build port-to-net remapping and param-overrides from the element's
        // arguments. See `lower_module_inst` for the equivalent logic.
        let port_names: std::collections::HashSet<&str> = module_def
            .ports
            .iter()
            .map(|p| p.name.name.as_str())
            .collect();
        let param_names: std::collections::HashSet<&str> = module_def
            .body
            .iter()
            .filter_map(|item| match item {
                CircuitItem::Param(p) => Some(p.name.name.as_str()),
                _ => None,
            })
            .collect();

        let mut port_remap: HashMap<String, String> = HashMap::new();
        let mut overrides: HashMap<String, Value> = HashMap::new();
        let ports = &module_def.ports;

        for arg in &e.args {
            match arg {
                Argument::Named { name, value } => {
                    let arg_name = name.name.as_str();
                    if port_names.contains(arg_name) {
                        if let Some(net_name) = expr_as_net_name(value) {
                            port_remap.insert(name.name.clone(), net_name);
                        } else if let Expr::Ident(ident) = value {
                            port_remap.insert(name.name.clone(), ident.name.clone());
                        } else {
                            self.diags.push(
                                Diagnostic::error(format!(
                                    "module port `{}` must be bound to a net name",
                                    name.name
                                ))
                                .with_span(name.span),
                            );
                        }
                    } else if param_names.contains(arg_name) {
                        match self.eval_expr(value) {
                            Ok(val) => {
                                overrides.insert(name.name.clone(), val);
                            }
                            Err(msg) => {
                                self.diags.push(
                                    Diagnostic::error(format!(
                                        "cannot evaluate override for param `{}`: {msg}",
                                        name.name
                                    ))
                                    .with_span(name.span),
                                );
                            }
                        }
                    } else {
                        self.diags.push(
                            Diagnostic::error(format!(
                                "module `{module_name}` has no port or param named `{}`",
                                name.name
                            ))
                            .with_span(name.span),
                        );
                    }
                }
                Argument::Positional(expr) => {
                    let idx = port_remap.len();
                    if idx < ports.len() {
                        let port_name = &ports[idx].name.name;
                        if let Some(net_name) = expr_as_net_name(expr) {
                            port_remap.insert(port_name.clone(), net_name);
                        } else if let Expr::Ident(ident) = expr {
                            port_remap.insert(port_name.clone(), ident.name.clone());
                        } else {
                            self.diags.push(
                                Diagnostic::error(format!(
                                    "module port `{port_name}` must be bound to a net name"
                                ))
                                .with_span(e.span),
                            );
                        }
                    } else {
                        self.diags.push(
                            Diagnostic::error(format!(
                                "too many arguments for module `{module_name}` (expected {})",
                                ports.len()
                            ))
                            .with_span(e.span),
                        );
                    }
                }
                Argument::Connection { from, to } => {
                    let idx = port_remap.len();
                    if idx < ports.len() {
                        port_remap.insert(ports[idx].name.name.clone(), from.name.clone());
                    }
                    let idx2 = port_remap.len();
                    if idx2 < ports.len() {
                        port_remap.insert(ports[idx2].name.name.clone(), to.name.clone());
                    }
                }
                Argument::NamedConnection { name, from, to } => {
                    port_remap.insert(format!("{}_pos", name.name), from.name.clone());
                    port_remap.insert(format!("{}_neg", name.name), to.name.clone());
                }
            }
        }

        // Save/restore net_remap, push/pop module stack, lower body.
        let saved_remap = std::mem::replace(&mut self.net_remap, port_remap);

        // Save param scope — module-internal params must not leak to outer
        // scope or collide across multiple instantiations of the same module.
        let saved_params = self.param_values.clone();

        // Stash override map for the body; lower_param consumes it.
        let saved_overrides = std::mem::replace(&mut self.param_overrides, overrides);

        self.module_inst_stack.push(module_name.clone());

        self.lower_circuit_body_prefixed(&module_def.body, &inst_prefix);

        self.module_inst_stack.pop();
        self.net_remap = saved_remap;
        self.param_overrides = saved_overrides;

        // Restore outer param scope.
        self.param_values = saved_params;
    }

    // -------------------------------------------------------------------
    // Options and temperature lowering
    // -------------------------------------------------------------------

    fn lower_options_decl(&mut self, o: &OptionsDecl) {
        for setting in &o.settings {
            let name = setting.name.name.clone();
            match self.eval_expr(&setting.value) {
                Ok(val) => {
                    // Overwrite if the same option is set multiple times.
                    if let Some(existing) = self.options.iter_mut().find(|p| p.0 == name) {
                        existing.1 = val;
                    } else {
                        self.options.push((name, val));
                    }
                }
                Err(msg) => {
                    self.diags.push(
                        Diagnostic::error(format!("cannot evaluate option `{name}`: {msg}"))
                            .with_span(setting.name.span),
                    );
                }
            }
        }
    }

    fn lower_temp_decl(&mut self, t: &TempDecl) {
        match self.eval_to_f64(&t.value) {
            Some(v) => self.temps.push(v),
            None => {
                self.diags
                    .push(Diagnostic::error("cannot evaluate temperature value").with_span(t.span));
            }
        }
    }

    fn lower_save_decl(&mut self, s: &SaveDecl) {
        for target in &s.targets {
            let spec = match target {
                SaveTarget::Voltage {
                    node, reference, ..
                } => {
                    if let Some(ref_node) = reference {
                        format!("v({},{})", node.name, ref_node.name)
                    } else {
                        format!("v({})", node.name)
                    }
                }
                SaveTarget::Current { element, .. } => {
                    format!("i({})", element.name)
                }
                SaveTarget::Name { name, .. } => name.name.clone(),
            };
            if !self.save.contains(&spec) {
                self.save.push(spec);
            }
        }
    }

    // -------------------------------------------------------------------
    // User-defined function lowering
    // -------------------------------------------------------------------

    fn lower_func_decl(&mut self, f: &FuncDecl) {
        let body_str = expr_to_spice_string(&f.body);
        self.funcs.push(FuncDef {
            name: f.name.name.clone(),
            args: f.params.iter().map(|p| p.name.clone()).collect(),
            body: body_str,
        });
    }

    // -------------------------------------------------------------------
    // Initial condition lowering
    // -------------------------------------------------------------------

    fn lower_ic_decl(&mut self, ic: &IcDecl) {
        for entry in &ic.entries {
            let net_id = self.intern_net(&entry.node.name, false);
            if let Some(val) = self.eval_to_f64(&entry.value) {
                self.initial_conditions.push((net_id, val));
            } else {
                self.diags.push(
                    Diagnostic::error(format!(
                        "cannot evaluate initial condition for `{}`",
                        entry.node.name
                    ))
                    .with_span(entry.span),
                );
            }
        }
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
            "sens" => Some(Analysis::Sens(self.lower_sens_analysis(&a.body))),
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
                    self.diags.push(
                        Diagnostic::error(format!(
                            "DC sweep source `{}` not found as an element",
                            source.name
                        ))
                        .with_span(source.span),
                    );
                    continue;
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

    fn lower_sens_analysis(&mut self, body: &[AnalysisItem]) -> cirq_ir::SensAnalysis {
        let mut output = String::new();
        let mut ac_mode = false;
        let mut scale = FrequencyScale::Decade;
        let mut points = 0u32;
        let mut fstart = 0.0;
        let mut fstop = 0.0;

        for item in body {
            if let AnalysisItem::Setting { name, value } = item {
                match name.name.as_str() {
                    "output" => {
                        output = self.get_setting_string(body, "output").unwrap_or_default();
                    }
                    "ac" => {
                        ac_mode = matches!(value, Expr::Bool { value: true, .. });
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
                    "points" => {
                        points = self.eval_to_f64(value).unwrap_or(0.0) as u32;
                    }
                    "fstart" | "start" => {
                        fstart = self.eval_to_f64(value).unwrap_or(0.0);
                    }
                    "fstop" | "stop" => {
                        fstop = self.eval_to_f64(value).unwrap_or(0.0);
                    }
                    _ => {}
                }
            }
        }

        let ac = if ac_mode || points > 0 || fstart != 0.0 || fstop != 0.0 {
            Some(cirq_ir::SensAcSpec {
                scale,
                points,
                fstart,
                fstop,
            })
        } else {
            None
        };

        cirq_ir::SensAnalysis { output, ac }
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

/// Convert a Cirq AST expression to a SPICE-compatible expression string.
///
/// This is used for behavioral source specs, where we need the expression
/// as a string rather than as an evaluated numeric value.
fn expr_to_spice_string(expr: &Expr) -> String {
    match expr {
        Expr::Number { value, .. } => format_f64(*value),
        Expr::Integer { value, .. } => value.to_string(),
        Expr::StringLit { value, .. } => value.clone(),
        Expr::Bool { value, .. } => {
            if *value {
                "1".to_string()
            } else {
                "0".to_string()
            }
        }
        Expr::Ident(id) => id.name.clone(),
        Expr::QualifiedName(qn) => qn
            .segments
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join("."),
        Expr::BinOp { op, lhs, rhs, .. } => {
            let l = expr_to_spice_string(lhs);
            let r = expr_to_spice_string(rhs);
            let op_str = match op {
                BinOp::Add => "+",
                BinOp::Sub => "-",
                BinOp::Mul => "*",
                BinOp::Div => "/",
                BinOp::Pow => "**",
                BinOp::Mod => "%",
                BinOp::Eq => "==",
                BinOp::Ne => "!=",
                BinOp::Lt => "<",
                BinOp::Gt => ">",
                BinOp::Le => "<=",
                BinOp::Ge => ">=",
                BinOp::And => "&&",
                BinOp::Or => "||",
            };
            format!("({l}{op_str}{r})")
        }
        Expr::UnaryOp { op, operand, .. } => {
            let o = expr_to_spice_string(operand);
            match op {
                UnaryOp::Neg => format!("(-{o})"),
                UnaryOp::Not => format!("(!{o})"),
            }
        }
        Expr::Call { func, args, .. } => {
            let arg_strs: Vec<String> = args.iter().map(expr_to_spice_string).collect();
            format!("{}({})", func.name, arg_strs.join(","))
        }
        Expr::Gnd { .. } => "0".to_string(),
        // Fallback for other expression types — produce a reasonable string.
        Expr::Range { start, end, .. } => {
            format!(
                "{}..{}",
                expr_to_spice_string(start),
                expr_to_spice_string(end)
            )
        }
        Expr::List { elements, .. } | Expr::Tuple { elements, .. } => {
            let strs: Vec<String> = elements.iter().map(expr_to_spice_string).collect();
            strs.join(",")
        }
        Expr::Block { entries, .. } => {
            let strs: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{}={}", k.name, expr_to_spice_string(v)))
                .collect();
            strs.join(",")
        }
    }
}

/// Format an f64 without unnecessary trailing zeros or scientific notation for
/// small integers, while preserving scientific notation for very large/small values.
fn format_f64(v: f64) -> String {
    if v == 0.0 {
        return "0".to_string();
    }
    // For values that are exact integers in a reasonable range, format without decimal.
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    // Otherwise use default formatting which picks a reasonable representation.
    format!("{v}")
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

    #[test]
    fn pmesfet_element_kind() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                model pm: pmesfet {
                    vto = 0.5
                }
                Z1: pmesfet(drain: d, gate: g, source: s, model: pm)
            }
            "#,
        );

        let z1 = circuit.elements.iter().find(|e| e.name == "Z1").unwrap();
        assert!(matches!(z1.kind, ElementKind::PMesfet));
        assert!(z1.model.is_some());

        // Verify the model was resolved as PMesfet device type.
        let model = circuit.models.iter().find(|m| m.name == "pm").unwrap();
        assert_eq!(model.device_type, cirq_ir::DeviceType::PMesfet);
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

    // -------------------------------------------------------------------
    // Gap 2.1: Module (subcircuit) flattening
    // -------------------------------------------------------------------

    #[test]
    fn simple_module_flattening() {
        // A local module used via element-inst syntax (single identifier).
        let circuit = compile_unwrap(
            r#"
            circuit test {
                module inverter {
                    port inp: in
                    port out: out
                    port vdd: inout
                    port vss: inout
                    R1: resistor(inp -> out, 1000)
                    R2: resistor(out -> vss, 2000)
                }

                inv1: inverter(inp: a, out: b, vdd: vdd, vss: gnd)
            }
            "#,
        );

        // The module body should be flattened: two resistors with prefixed names.
        assert_eq!(circuit.elements.len(), 2);

        let r1 = circuit
            .elements
            .iter()
            .find(|e| e.name == "inv1.R1")
            .expect("should have inv1.R1");
        assert!(matches!(r1.kind, ElementKind::Resistor));

        let r2 = circuit
            .elements
            .iter()
            .find(|e| e.name == "inv1.R2")
            .expect("should have inv1.R2");
        assert!(matches!(r2.kind, ElementKind::Resistor));

        // Port remapping: inv1.R1's `inp` port → caller net `a`, `out` port → caller net `b`.
        let r1_pos = r1.connections.iter().find(|c| c.terminal == "pos").unwrap();
        let r1_neg = r1.connections.iter().find(|c| c.terminal == "neg").unwrap();
        let net_a = circuit.nets.iter().find(|n| n.name == "a").unwrap();
        let net_b = circuit.nets.iter().find(|n| n.name == "b").unwrap();
        assert_eq!(
            r1_pos.net, net_a.id,
            "R1 pos should connect to caller net 'a'"
        );
        assert_eq!(
            r1_neg.net, net_b.id,
            "R1 neg should connect to caller net 'b'"
        );

        // inv1.R2: `out` → caller net `b`, `vss` → gnd
        let r2_pos = r2.connections.iter().find(|c| c.terminal == "pos").unwrap();
        let r2_neg = r2.connections.iter().find(|c| c.terminal == "neg").unwrap();
        assert_eq!(
            r2_pos.net, net_b.id,
            "R2 pos should connect to caller net 'b'"
        );
        assert_eq!(r2_neg.net, Id(0), "R2 neg should connect to gnd");
    }

    #[test]
    fn module_with_internal_nets() {
        // Internal nets that aren't ports should get hierarchical prefixed names.
        let circuit = compile_unwrap(
            r#"
            circuit test {
                module rc_filter {
                    port inp: in
                    port out: out
                    port gnd_port: inout
                    R1: resistor(inp -> mid, 1000)
                    C1: capacitor(mid -> gnd_port, 1e-12)
                }

                filt1: rc_filter(inp: sig_in, out: sig_out, gnd_port: gnd)
            }
            "#,
        );

        // `mid` is an internal net — should be prefixed as `filt1.mid`.
        let mid_net = circuit
            .nets
            .iter()
            .find(|n| n.name == "filt1.mid")
            .expect("internal net 'mid' should be prefixed as 'filt1.mid'");

        // R1's neg and C1's pos should both connect to filt1.mid.
        let r1 = circuit
            .elements
            .iter()
            .find(|e| e.name == "filt1.R1")
            .unwrap();
        let c1 = circuit
            .elements
            .iter()
            .find(|e| e.name == "filt1.C1")
            .unwrap();

        let r1_neg = r1.connections.iter().find(|c| c.terminal == "neg").unwrap();
        let c1_pos = c1.connections.iter().find(|c| c.terminal == "pos").unwrap();
        assert_eq!(r1_neg.net, mid_net.id);
        assert_eq!(c1_pos.net, mid_net.id);
    }

    #[test]
    fn multiple_module_instances() {
        // Two instances of the same module get distinct prefixed names and nets.
        let circuit = compile_unwrap(
            r#"
            circuit test {
                module buffer {
                    port inp: in
                    port out: out
                    R1: resistor(inp -> out, 100)
                }

                buf1: buffer(inp: a, out: b)
                buf2: buffer(inp: b, out: c)
            }
            "#,
        );

        assert_eq!(circuit.elements.len(), 2);

        let buf1_r1 = circuit
            .elements
            .iter()
            .find(|e| e.name == "buf1.R1")
            .expect("should have buf1.R1");
        let buf2_r1 = circuit
            .elements
            .iter()
            .find(|e| e.name == "buf2.R1")
            .expect("should have buf2.R1");

        // buf1.R1 connects a -> b, buf2.R1 connects b -> c.
        let net_a = circuit.nets.iter().find(|n| n.name == "a").unwrap();
        let net_b = circuit.nets.iter().find(|n| n.name == "b").unwrap();
        let net_c = circuit.nets.iter().find(|n| n.name == "c").unwrap();

        let b1_pos = buf1_r1
            .connections
            .iter()
            .find(|c| c.terminal == "pos")
            .unwrap();
        let b1_neg = buf1_r1
            .connections
            .iter()
            .find(|c| c.terminal == "neg")
            .unwrap();
        assert_eq!(b1_pos.net, net_a.id);
        assert_eq!(b1_neg.net, net_b.id);

        let b2_pos = buf2_r1
            .connections
            .iter()
            .find(|c| c.terminal == "pos")
            .unwrap();
        let b2_neg = buf2_r1
            .connections
            .iter()
            .find(|c| c.terminal == "neg")
            .unwrap();
        assert_eq!(b2_pos.net, net_b.id);
        assert_eq!(b2_neg.net, net_c.id);
    }

    #[test]
    fn nested_module_flattening() {
        // A module that instantiates another module — two levels of hierarchy.
        let circuit = compile_unwrap(
            r#"
            circuit test {
                module resistor_pair {
                    port inp: in
                    port out: out
                    R1: resistor(inp -> mid, 1000)
                    R2: resistor(mid -> out, 1000)
                }

                module double_pair {
                    port inp: in
                    port out: out
                    stage1: resistor_pair(inp: inp, out: link)
                    stage2: resistor_pair(inp: link, out: out)
                }

                top: double_pair(inp: a, out: b)
            }
            "#,
        );

        // Should flatten to 4 resistors with hierarchical names.
        assert_eq!(circuit.elements.len(), 4);

        let names: Vec<&str> = circuit.elements.iter().map(|e| e.name.as_str()).collect();
        assert!(
            names.contains(&"top.stage1.R1"),
            "expected top.stage1.R1, got {names:?}"
        );
        assert!(
            names.contains(&"top.stage1.R2"),
            "expected top.stage1.R2, got {names:?}"
        );
        assert!(
            names.contains(&"top.stage2.R1"),
            "expected top.stage2.R1, got {names:?}"
        );
        assert!(
            names.contains(&"top.stage2.R2"),
            "expected top.stage2.R2, got {names:?}"
        );

        // Verify net connectivity: stage1.R2 and stage2.R1 should share
        // the `link` net (prefixed as `top.link`).
        let s1_r2 = circuit
            .elements
            .iter()
            .find(|e| e.name == "top.stage1.R2")
            .unwrap();
        let s2_r1 = circuit
            .elements
            .iter()
            .find(|e| e.name == "top.stage2.R1")
            .unwrap();

        let s1_r2_neg = s1_r2
            .connections
            .iter()
            .find(|c| c.terminal == "neg")
            .unwrap();
        let s2_r1_pos = s2_r1
            .connections
            .iter()
            .find(|c| c.terminal == "pos")
            .unwrap();
        assert_eq!(
            s1_r2_neg.net, s2_r1_pos.net,
            "stage1.R2 neg and stage2.R1 pos should share the 'link' net"
        );
    }

    #[test]
    fn module_gnd_not_prefixed() {
        // `gnd` should always resolve to Id(0), never prefixed.
        let circuit = compile_unwrap(
            r#"
            circuit test {
                module grounded_r {
                    port inp: in
                    R1: resistor(inp -> gnd, 1000)
                }

                inst1: grounded_r(inp: a)
            }
            "#,
        );

        let r1 = circuit
            .elements
            .iter()
            .find(|e| e.name == "inst1.R1")
            .unwrap();
        let neg = r1.connections.iter().find(|c| c.terminal == "neg").unwrap();
        assert_eq!(neg.net, Id(0), "gnd inside module should map to Id(0)");
    }

    #[test]
    fn module_with_model_and_source() {
        // A module containing a model reference and source element.
        let circuit = compile_unwrap(
            r#"
            circuit test {
                model d1n4148: diode {
                    is = 2.52e-9
                }

                module rectifier {
                    port inp: in
                    port out: out
                    D1: diode(inp -> out, model: d1n4148)
                }

                rect1: rectifier(inp: ac_in, out: dc_out)
            }
            "#,
        );

        let d1 = circuit
            .elements
            .iter()
            .find(|e| e.name == "rect1.D1")
            .unwrap();
        assert!(matches!(d1.kind, ElementKind::Diode));
        assert!(d1.model.is_some(), "diode should reference model d1n4148");
    }

    #[test]
    fn unknown_module_error() {
        let result = compile(
            r#"
            circuit test {
                inv1: nonexistent(inp: a, out: b)
            }
            "#,
        );

        assert!(result.is_err());
        let diags = result.unwrap_err();
        let err = diags
            .iter()
            .find(|d| d.message.contains("unknown element type"));
        assert!(
            err.is_some(),
            "expected unknown element type, got: {diags:?}"
        );
    }

    // -------------------------------------------------------------------
    // Gap 3.1: Simulation options
    // -------------------------------------------------------------------

    #[test]
    fn options_block_lowering() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                options {
                    gmin: 1e-12
                    abstol: 1e-12
                    reltol: 1e-3
                    vntol: 1e-6
                }
            }
            "#,
        );

        assert_eq!(circuit.options.len(), 4);

        let gmin = circuit.options.iter().find(|o| o.0 == "gmin").unwrap();
        match &gmin.1 {
            Value::Real(v) => assert!((*v - 1e-12).abs() < 1e-20),
            _ => panic!("expected Real for gmin"),
        }

        let reltol = circuit.options.iter().find(|o| o.0 == "reltol").unwrap();
        match &reltol.1 {
            Value::Real(v) => assert!((*v - 1e-3).abs() < 1e-10),
            _ => panic!("expected Real for reltol"),
        }
    }

    // -------------------------------------------------------------------
    // Gap 3.3: Temperature
    // -------------------------------------------------------------------

    #[test]
    fn temp_decl_lowering() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                temp 85
            }
            "#,
        );

        assert_eq!(circuit.temps, vec![85.0]);
    }

    #[test]
    fn temp_with_expression() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                param t_corner = 125
                temp t_corner
            }
            "#,
        );

        assert_eq!(circuit.temps, vec![125.0]);
    }

    // -------------------------------------------------------------------
    // Gap 3.2: Save targets
    // -------------------------------------------------------------------

    #[test]
    fn save_block_lowering() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                R1: resistor(a -> gnd, 1000)
                save {
                    v(a)
                    i(R1)
                }
            }
            "#,
        );

        assert_eq!(circuit.save.len(), 2);
        assert!(circuit.save.contains(&"v(a)".to_string()));
        assert!(circuit.save.contains(&"i(R1)".to_string()));
    }

    #[test]
    fn save_differential_voltage() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                R1: resistor(a -> b, 1000)
                save {
                    v(a, b)
                }
            }
            "#,
        );

        assert_eq!(circuit.save.len(), 1);
        assert_eq!(circuit.save[0], "v(a,b)");
    }

    // -------------------------------------------------------------------
    // Gap 2.2: Behavioral sources
    // -------------------------------------------------------------------

    #[test]
    fn behavioral_voltage_source() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                b1: behavioral(out -> gnd, v: 3.3)
            }
            "#,
        );

        assert_eq!(circuit.elements.len(), 1);
        let b1 = &circuit.elements[0];
        assert_eq!(b1.name, "b1");
        match &b1.kind {
            ElementKind::BehavioralSource { mode, spec } => {
                assert_eq!(*mode, BehavioralMode::Voltage);
                assert_eq!(spec, "3.3");
            }
            other => panic!("expected BehavioralSource, got {other:?}"),
        }
        // Should have 2 connections: pos->out, neg->gnd.
        assert_eq!(b1.connections.len(), 2);
        assert_eq!(b1.connections[0].terminal, "pos");
        assert_eq!(b1.connections[1].terminal, "neg");
    }

    #[test]
    fn behavioral_current_source() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                b1: behavioral(out -> gnd, i: 0.001)
            }
            "#,
        );

        assert_eq!(circuit.elements.len(), 1);
        let b1 = &circuit.elements[0];
        assert_eq!(b1.name, "b1");
        match &b1.kind {
            ElementKind::BehavioralSource { mode, spec } => {
                assert_eq!(*mode, BehavioralMode::Current);
                assert_eq!(spec, "0.001");
            }
            other => panic!("expected BehavioralSource, got {other:?}"),
        }
        assert_eq!(b1.connections.len(), 2);
    }

    #[test]
    fn behavioral_source_with_expression() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                b1: behavioral(out -> gnd, v: sin(2 * 3.14159 * 1000 * time))
            }
            "#,
        );

        let b1 = &circuit.elements[0];
        match &b1.kind {
            ElementKind::BehavioralSource { mode, spec } => {
                assert_eq!(*mode, BehavioralMode::Voltage);
                // The spec should contain sin, multiplication operators, and time.
                assert!(spec.contains("sin"), "spec should contain sin: {spec}");
                assert!(spec.contains("time"), "spec should contain time: {spec}");
            }
            other => panic!("expected BehavioralSource, got {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // Gap 3.4: User-defined functions
    // -------------------------------------------------------------------

    #[test]
    fn func_decl_lowering() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                limit(x, lo, hi) = min(max(x, lo), hi)
                R1: resistor(a -> gnd, 1000)
            }
            "#,
        );

        assert_eq!(circuit.funcs.len(), 1);
        assert_eq!(circuit.funcs[0].name, "limit");
        assert_eq!(circuit.funcs[0].args, vec!["x", "lo", "hi"]);
        assert!(
            circuit.funcs[0].body.contains("min"),
            "body should contain min: {}",
            circuit.funcs[0].body
        );
        assert!(
            circuit.funcs[0].body.contains("max"),
            "body should contain max: {}",
            circuit.funcs[0].body
        );
    }

    #[test]
    fn func_decl_simple_expression() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                double(x) = x * 2
                R1: resistor(a -> gnd, 1000)
            }
            "#,
        );

        assert_eq!(circuit.funcs.len(), 1);
        assert_eq!(circuit.funcs[0].name, "double");
        assert_eq!(circuit.funcs[0].args, vec!["x"]);
        assert!(
            circuit.funcs[0].body.contains("*"),
            "body should contain multiplication: {}",
            circuit.funcs[0].body
        );
    }

    // -------------------------------------------------------------------
    // Gap 3.6: Initial conditions
    // -------------------------------------------------------------------

    #[test]
    fn ic_decl_lowering() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                R1: resistor(out -> gnd, 1000)
                ic {
                    v(out) = 1.5
                }
            }
            "#,
        );

        assert_eq!(circuit.initial_conditions.len(), 1);
        let (net_id, value) = &circuit.initial_conditions[0];
        // Find the net name for this id.
        let net = circuit.nets.iter().find(|n| n.id == *net_id).unwrap();
        assert_eq!(net.name, "out");
        assert!((value - 1.5).abs() < 1e-10);
    }

    #[test]
    fn ic_multiple_entries() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                R1: resistor(a -> b, 1000)
                R2: resistor(b -> gnd, 2000)
                ic {
                    v(a) = 3.3
                    v(b) = 1.5
                }
            }
            "#,
        );

        assert_eq!(circuit.initial_conditions.len(), 2);
    }

    #[test]
    fn coupled_line_basic() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                coupled_line P1 {
                    in: [a1, a2]
                    out: [b1, b2]
                    gnd: gnd
                    model: cpl_mod
                }
            }
            "#,
        );

        assert_eq!(circuit.elements.len(), 1);
        let elem = &circuit.elements[0];
        assert_eq!(elem.name, "P1");
        assert!(
            matches!(elem.kind, ElementKind::CoupledLine { width: 2 }),
            "expected CoupledLine {{ width: 2 }}, got {:?}",
            elem.kind
        );

        // Connections: in0, in1, gnd, out0, out1 = 5 total.
        assert_eq!(elem.connections.len(), 5);
        assert_eq!(elem.connections[0].terminal, "in0");
        assert_eq!(elem.connections[1].terminal, "in1");
        assert_eq!(elem.connections[2].terminal, "gnd");
        assert_eq!(elem.connections[3].terminal, "out0");
        assert_eq!(elem.connections[4].terminal, "out1");

        // Model param.
        let model_param = elem
            .params
            .iter()
            .find(|p| p.0 == "model")
            .expect("should have model param");
        assert!(matches!(&model_param.1, Value::String(s) if s == "cpl_mod"));
    }

    #[test]
    fn coupled_line_single_line() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                coupled_line P1 {
                    in: [a]
                    out: [b]
                    gnd: g
                    model: m
                }
            }
            "#,
        );

        assert_eq!(circuit.elements.len(), 1);
        let elem = &circuit.elements[0];
        assert!(matches!(elem.kind, ElementKind::CoupledLine { width: 1 }));
        // in0, gnd, out0 = 3 connections
        assert_eq!(elem.connections.len(), 3);
    }

    #[test]
    fn coupled_line_three_lines() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                coupled_line P1 {
                    in: [a1, a2, a3]
                    out: [b1, b2, b3]
                    gnd: gnd
                    model: cpl3
                }
            }
            "#,
        );

        let elem = &circuit.elements[0];
        assert!(matches!(elem.kind, ElementKind::CoupledLine { width: 3 }));
        // 3 in + 1 gnd + 3 out = 7
        assert_eq!(elem.connections.len(), 7);
    }

    #[test]
    fn coupled_line_mismatched_widths_errors() {
        let result = compile(
            r#"
            circuit test {
                coupled_line P1 {
                    in: [a1, a2]
                    out: [b1]
                    gnd: gnd
                    model: m
                }
            }
            "#,
        );
        assert!(result.is_err(), "mismatched in/out widths should error");
    }

    #[test]
    fn coupled_line_missing_in_errors() {
        let result = compile(
            r#"
            circuit test {
                coupled_line P1 {
                    out: [b1]
                    gnd: gnd
                    model: m
                }
            }
            "#,
        );
        assert!(result.is_err(), "missing `in` should error");
    }

    #[test]
    fn coupled_line_default_gnd() {
        let circuit = compile_unwrap(
            r#"
            circuit test {
                coupled_line P1 {
                    in: [a]
                    out: [b]
                    model: m
                }
            }
            "#,
        );

        // When gnd is omitted, it should default to "gnd".
        let elem = &circuit.elements[0];
        let gnd_conn = elem
            .connections
            .iter()
            .find(|c| c.terminal == "gnd")
            .expect("should have gnd connection");
        // Verify the gnd net exists by looking it up in the circuit nets.
        let gnd_net = &circuit.nets[gnd_conn.net.0 as usize];
        assert_eq!(
            gnd_net.name, "gnd",
            "gnd connection should point to ground net"
        );
    }

    // -------------------------------------------------------------------
    // Module param overrides at instantiation
    // -------------------------------------------------------------------

    fn find_param<'a>(circuit: &'a Circuit, name: &str) -> &'a ResolvedParam {
        circuit
            .params
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| {
                let names: Vec<&str> = circuit.params.iter().map(|p| p.name.as_str()).collect();
                panic!("param `{name}` not found; have: {names:?}")
            })
    }

    fn real_val(p: &ResolvedParam) -> f64 {
        match p.value {
            Value::Real(v) => v,
            Value::Integer(i) => i as f64,
            ref other => panic!("expected Real, got {other:?}"),
        }
    }

    #[test]
    fn module_param_override_basic() {
        // A single instance overrides one of the module's params.
        let circuit = compile_unwrap(
            r#"
            circuit test {
                module rdiv {
                    port inp: in
                    port out: out
                    param r = 1000
                    R1: resistor(inp -> out, r)
                }

                d1: rdiv(inp: a, out: b, r: 4700)
            }
            "#,
        );

        // The override should land in resolved_params under the prefixed name.
        let r = find_param(&circuit, "d1.r");
        assert_eq!(real_val(r), 4700.0);
    }

    #[test]
    fn module_param_override_two_instances_differ() {
        // Two instances of the same module take different override values.
        let circuit = compile_unwrap(
            r#"
            circuit test {
                module rdiv {
                    port inp: in
                    port out: out
                    param r = 1000
                    R1: resistor(inp -> out, r)
                }

                d1: rdiv(inp: a, out: b, r: 2200)
                d2: rdiv(inp: b, out: c, r: 4700)
            }
            "#,
        );

        assert_eq!(real_val(find_param(&circuit, "d1.r")), 2200.0);
        assert_eq!(real_val(find_param(&circuit, "d2.r")), 4700.0);
    }

    #[test]
    fn module_param_default_when_not_overridden() {
        // Instance with no override falls back to the param's default.
        let circuit = compile_unwrap(
            r#"
            circuit test {
                module rdiv {
                    port inp: in
                    port out: out
                    param r = 1000
                    R1: resistor(inp -> out, r)
                }

                d1: rdiv(inp: a, out: b, r: 4700)
                d2: rdiv(inp: b, out: c)
            }
            "#,
        );

        assert_eq!(real_val(find_param(&circuit, "d1.r")), 4700.0);
        assert_eq!(real_val(find_param(&circuit, "d2.r")), 1000.0);
    }

    #[test]
    fn module_param_override_evaluated_in_caller_scope() {
        // Override expressions resolve against the *caller's* params, not the
        // module's own scope.
        let circuit = compile_unwrap(
            r#"
            circuit test {
                param base = 1000
                param scale = 4.7

                module rdiv {
                    port inp: in
                    port out: out
                    param r = 100
                    R1: resistor(inp -> out, r)
                }

                d1: rdiv(inp: a, out: b, r: base * scale)
            }
            "#,
        );

        let r = find_param(&circuit, "d1.r");
        assert!((real_val(r) - 4700.0).abs() < 1e-9);
    }

    #[test]
    fn module_param_override_with_no_default() {
        // A module param without a default is fine as long as every instance
        // supplies an override.
        let circuit = compile_unwrap(
            r#"
            circuit test {
                module rdiv {
                    port inp: in
                    port out: out
                    param r
                    R1: resistor(inp -> out, r)
                }

                d1: rdiv(inp: a, out: b, r: 8200)
            }
            "#,
        );

        assert_eq!(real_val(find_param(&circuit, "d1.r")), 8200.0);
    }

    #[test]
    fn module_unknown_named_arg_errors() {
        // Naming something that's neither a port nor a param is a hard error.
        let result = compile(
            r#"
            circuit test {
                module rdiv {
                    port inp: in
                    port out: out
                    param r = 1000
                    R1: resistor(inp -> out, r)
                }

                d1: rdiv(inp: a, out: b, w: 4700)
            }
            "#,
        );

        let diags = result.expect_err("should fail with unknown port/param diagnostic");
        let found = diags
            .iter()
            .any(|d| d.message.contains("no port or param named `w`"));
        assert!(
            found,
            "expected unknown port/param diagnostic, got: {diags:?}"
        );
    }

    #[test]
    fn module_param_override_nested() {
        // Outer module overrides inner module's param via its own param.
        let circuit = compile_unwrap(
            r#"
            circuit test {
                module leaf {
                    port inp: in
                    port out: out
                    param r = 100
                    R1: resistor(inp -> out, r)
                }

                module wrap {
                    port inp: in
                    port out: out
                    param r_outer = 500
                    inner: leaf(inp: inp, out: out, r: r_outer)
                }

                w1: wrap(inp: a, out: b)
                w2: wrap(inp: b, out: c, r_outer: 2200)
            }
            "#,
        );

        assert_eq!(real_val(find_param(&circuit, "w1.inner.r")), 500.0);
        assert_eq!(real_val(find_param(&circuit, "w2.inner.r")), 2200.0);
    }
}
