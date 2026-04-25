//! CST-to-AST lowering -- walks a Tree-sitter concrete syntax tree and produces
//! [`cirq_ast`] types.

use cirq_ast::{
    AnalysisDecl, AnalysisItem, Argument, Attribute, BinOp, Circuit, CircuitItem, CoupledLineDecl,
    CoupledLineField, ElementInst, Expr, FuncDecl, GlobalDecl, IcDecl, IcEntry, Ident, Import,
    LetDecl, ModelDef, ModelParam, ModuleDef, ModuleInst, OptionSetting, OptionsDecl, ParamDecl,
    PortDecl, PortDirection, QualifiedName, SaveDecl, SaveTarget, SourceFile, TempDecl, TopLevel,
    UnaryOp, span::Span,
};

use crate::diagnostics::{Diagnostic, Severity};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Lower a Tree-sitter parse tree into a [`SourceFile`].
///
/// Diagnostics (errors/warnings) are collected into the returned vec. The
/// lowering is best-effort: even when there are ERROR nodes the function will
/// try to produce as much AST as possible.
pub fn lower(tree: &tree_sitter::Tree, source: &str) -> (SourceFile, Vec<Diagnostic>) {
    let mut ctx = Ctx::new(source);
    let root = tree.root_node();

    let mut items = Vec::new();
    let mut cursor = root.walk();

    for child in root.named_children(&mut cursor) {
        match child.kind() {
            "circuit_decl" => {
                if let Some(c) = ctx.lower_circuit(child) {
                    items.push(TopLevel::Circuit(c));
                }
            }
            "module_decl" => {
                if let Some(m) = ctx.lower_module(child) {
                    items.push(TopLevel::Module(m));
                }
            }
            "import_decl" => {
                if let Some(i) = ctx.lower_import(child) {
                    items.push(TopLevel::Import(i));
                }
            }
            "model_decl" => {
                if let Some(m) = ctx.lower_model(child) {
                    items.push(TopLevel::Model(m));
                }
            }
            "func_decl" => {
                if let Some(f) = ctx.lower_func(child) {
                    items.push(TopLevel::Func(f));
                }
            }
            "line_comment" | "block_comment" => {}
            "ERROR" => {
                ctx.error_at(child, "syntax error");
            }
            other => {
                ctx.error_at(child, format!("unexpected top-level node: {other}"));
            }
        }
    }

    let sf = SourceFile {
        items,
        span: span_of(root),
    };
    (sf, ctx.diags)
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

struct Ctx<'src> {
    source: &'src str,
    diags: Vec<Diagnostic>,
}

impl<'src> Ctx<'src> {
    fn new(source: &'src str) -> Self {
        Self {
            source,
            diags: Vec::new(),
        }
    }

    fn text(&self, node: tree_sitter::Node) -> &'src str {
        node.utf8_text(self.source.as_bytes())
            .unwrap_or("<invalid utf8>")
    }

    fn error_at(&mut self, node: tree_sitter::Node, msg: impl Into<String>) {
        self.diags.push(Diagnostic {
            severity: Severity::Error,
            message: msg.into(),
            span: Some(span_of(node)),
            notes: Vec::new(),
        });
    }

    fn ident(&self, node: tree_sitter::Node) -> Ident {
        Ident {
            name: self.text(node).to_owned(),
            span: span_of(node),
        }
    }

    /// Get a required field from a node. Reports a diagnostic and returns
    /// `None` if the field is missing.
    fn required_field<'tree>(
        &mut self,
        parent: tree_sitter::Node<'tree>,
        name: &str,
    ) -> Option<tree_sitter::Node<'tree>> {
        match parent.child_by_field_name(name) {
            Some(n) => Some(n),
            None => {
                self.error_at(parent, format!("missing field `{name}`"));
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Circuit
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn lower_circuit(&mut self, node: tree_sitter::Node) -> Option<Circuit> {
        let name_node = self.required_field(node, "name")?;
        let name = self.ident(name_node);
        let mut body = Vec::new();
        let mut attrs = Vec::new();
        let mut cursor = node.walk();

        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "attribute" => {
                    if let Some(a) = self.lower_attribute(child) {
                        attrs.push(a);
                    }
                }
                _ => {
                    if let Some(item) = self.lower_circuit_item(child) {
                        body.push(item);
                    }
                }
            }
        }

        Some(Circuit {
            name,
            body,
            attrs,
            span: span_of(node),
        })
    }

    fn lower_circuit_item(&mut self, node: tree_sitter::Node) -> Option<CircuitItem> {
        match node.kind() {
            "param_decl" => self.lower_param(node).map(CircuitItem::Param),
            "let_decl" => self.lower_let(node).map(CircuitItem::Let),
            "element_inst" => self.lower_element(node).map(CircuitItem::Element),
            "module_inst" => self.lower_module_inst(node).map(CircuitItem::ModuleInst),
            "module_decl" => self.lower_module(node).map(CircuitItem::ModuleDef),
            "model_decl" => self.lower_model(node).map(CircuitItem::ModelDef),
            "analysis_decl" => self.lower_analysis(node).map(CircuitItem::Analysis),
            "global_decl" => self.lower_global(node).map(CircuitItem::Global),
            "options_decl" => self.lower_options(node).map(CircuitItem::Options),
            "temp_decl" => self.lower_temp(node).map(CircuitItem::Temp),
            "save_decl" => self.lower_save(node).map(CircuitItem::Save),
            "func_decl" => self.lower_func(node).map(CircuitItem::Func),
            "ic_decl" => self.lower_ic(node).map(CircuitItem::Ic),
            "coupled_line_decl" => self.lower_coupled_line(node).map(CircuitItem::CoupledLine),
            "line_comment" | "block_comment" => None,
            "ERROR" => {
                self.error_at(node, "syntax error in circuit body");
                None
            }
            // identifier, port_direction, etc. are structural children we skip
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Module
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn lower_module(&mut self, node: tree_sitter::Node) -> Option<ModuleDef> {
        let name_node = self.required_field(node, "name")?;
        let name = self.ident(name_node);
        let mut ports = Vec::new();
        let mut body = Vec::new();
        let mut attrs = Vec::new();
        let mut cursor = node.walk();

        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "attribute" => {
                    if let Some(a) = self.lower_attribute(child) {
                        attrs.push(a);
                    }
                }
                "port_decl" => {
                    if let Some(p) = self.lower_port(child) {
                        ports.push(p);
                    }
                }
                _ => {
                    if let Some(item) = self.lower_circuit_item(child) {
                        body.push(item);
                    }
                }
            }
        }

        Some(ModuleDef {
            name,
            ports,
            body,
            attrs,
            span: span_of(node),
        })
    }

    fn lower_port(&mut self, node: tree_sitter::Node) -> Option<PortDecl> {
        let name_node = self.required_field(node, "name")?;
        let name = self.ident(name_node);

        let dir_node = self.required_field(node, "direction")?;
        let dir_text = self.text(dir_node);
        let direction = match dir_text {
            "in" => PortDirection::In,
            "out" => PortDirection::Out,
            "inout" => PortDirection::InOut,
            _ => {
                self.error_at(dir_node, format!("unknown port direction: {dir_text}"));
                return None;
            }
        };

        let mut attrs = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "attribute"
                && let Some(a) = self.lower_attribute(child)
            {
                attrs.push(a);
            }
        }

        Some(PortDecl {
            name,
            direction,
            attrs,
            span: span_of(node),
        })
    }
}

// ---------------------------------------------------------------------------
// Elements & Module Instantiation
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn lower_element(&mut self, node: tree_sitter::Node) -> Option<ElementInst> {
        let name_node = self.required_field(node, "name")?;
        let name = self.ident(name_node);
        let type_node = self.required_field(node, "type")?;
        let element_type = self.ident(type_node);
        let args = self.lower_argument_list(node);
        let mut attrs = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "attribute"
                && let Some(a) = self.lower_attribute(child)
            {
                attrs.push(a);
            }
        }

        Some(ElementInst {
            name,
            element_type,
            args,
            attrs,
            span: span_of(node),
        })
    }

    fn lower_module_inst(&mut self, node: tree_sitter::Node) -> Option<ModuleInst> {
        let name_node = self.required_field(node, "name")?;
        let name = self.ident(name_node);
        let module_node = self.required_field(node, "module")?;
        let module_name = self.lower_qualified_name(module_node)?;
        let args = self.lower_argument_list(node);
        let mut attrs = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "attribute"
                && let Some(a) = self.lower_attribute(child)
            {
                attrs.push(a);
            }
        }

        Some(ModuleInst {
            name,
            module_name,
            args,
            attrs,
            span: span_of(node),
        })
    }

    fn lower_argument_list(&mut self, parent: tree_sitter::Node) -> Vec<Argument> {
        let mut args = Vec::new();
        let mut cursor = parent.walk();
        for child in parent.named_children(&mut cursor) {
            if child.kind() == "argument_list" {
                let mut inner_cursor = child.walk();
                for arg_node in child.named_children(&mut inner_cursor) {
                    if arg_node.kind() == "argument"
                        && let Some(a) = self.lower_argument(arg_node)
                    {
                        args.push(a);
                    }
                }
            }
        }
        args
    }

    fn lower_argument(&mut self, node: tree_sitter::Node) -> Option<Argument> {
        // An argument node has a single named child which is one of:
        // named_argument, connection, named_connection, or an expression
        let mut cursor = node.walk();
        let child = node.named_children(&mut cursor).next()?;

        match child.kind() {
            "named_argument" => {
                let name_node = self.required_field(child, "name")?;
                let name = self.ident(name_node);
                let value_node = self.required_field(child, "value")?;
                let value = self.lower_expr(value_node)?;
                Some(Argument::Named { name, value })
            }
            "connection" => {
                let from = self.lower_net_ref(child, "from")?;
                let to = self.lower_net_ref(child, "to")?;
                Some(Argument::Connection { from, to })
            }
            "named_connection" => {
                let name_node = self.required_field(child, "name")?;
                let name = self.ident(name_node);
                let from = self.lower_net_ref(child, "from")?;
                let to = self.lower_net_ref(child, "to")?;
                Some(Argument::NamedConnection { name, from, to })
            }
            _ => {
                // Positional expression argument
                let expr = self.lower_expr(child)?;
                Some(Argument::Positional(expr))
            }
        }
    }

    fn lower_net_ref(&mut self, parent: tree_sitter::Node, field_name: &str) -> Option<Ident> {
        let node = self.required_field(parent, field_name)?;
        match node.kind() {
            "gnd" => Some(Ident {
                name: "gnd".to_owned(),
                span: span_of(node),
            }),
            "identifier" => Some(self.ident(node)),
            _ => {
                let kind = node.kind().to_owned();
                self.error_at(node, format!("expected net reference, got {kind}"));
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Parameters, let, global
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn lower_param(&mut self, node: tree_sitter::Node) -> Option<ParamDecl> {
        let name_node = self.required_field(node, "name")?;
        let name = self.ident(name_node);
        let ty = node.child_by_field_name("type").map(|n| self.ident(n));
        let default = node
            .child_by_field_name("value")
            .and_then(|n| self.lower_expr(n));

        let mut attrs = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "attribute"
                && let Some(a) = self.lower_attribute(child)
            {
                attrs.push(a);
            }
        }

        Some(ParamDecl {
            name,
            ty,
            default,
            attrs,
            span: span_of(node),
        })
    }

    fn lower_let(&mut self, node: tree_sitter::Node) -> Option<LetDecl> {
        let name_node = self.required_field(node, "name")?;
        let name = self.ident(name_node);
        let value_node = self.required_field(node, "value")?;
        let value = self.lower_expr(value_node)?;

        let mut attrs = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "attribute"
                && let Some(a) = self.lower_attribute(child)
            {
                attrs.push(a);
            }
        }

        Some(LetDecl {
            name,
            value,
            attrs,
            span: span_of(node),
        })
    }

    fn lower_global(&mut self, node: tree_sitter::Node) -> Option<GlobalDecl> {
        let name_node = self.required_field(node, "name")?;
        let name = self.ident(name_node);
        Some(GlobalDecl {
            name,
            span: span_of(node),
        })
    }

    fn lower_options(&mut self, node: tree_sitter::Node) -> Option<OptionsDecl> {
        let mut settings = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "options_setting"
                && let Some(setting) = self.lower_option_setting(child)
            {
                settings.push(setting);
            }
        }
        Some(OptionsDecl {
            settings,
            span: span_of(node),
        })
    }

    fn lower_option_setting(&mut self, node: tree_sitter::Node) -> Option<OptionSetting> {
        let name_node = self.required_field(node, "name")?;
        let name = self.ident(name_node);
        let value_node = self.required_field(node, "value")?;
        let value = self.lower_expr(value_node)?;
        Some(OptionSetting {
            name,
            value,
            span: span_of(node),
        })
    }

    fn lower_temp(&mut self, node: tree_sitter::Node) -> Option<TempDecl> {
        let value_node = self.required_field(node, "value")?;
        let value = self.lower_expr(value_node)?;
        Some(TempDecl {
            value,
            span: span_of(node),
        })
    }

    fn lower_save(&mut self, node: tree_sitter::Node) -> Option<SaveDecl> {
        let mut targets = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "save_target"
                && let Some(target) = self.lower_save_target(child)
            {
                targets.push(target);
            }
        }
        Some(SaveDecl {
            targets,
            span: span_of(node),
        })
    }

    fn lower_save_target(&mut self, node: tree_sitter::Node) -> Option<SaveTarget> {
        // Check for a "type" field (v or i function form).
        if let Some(type_node) = node.child_by_field_name("type") {
            let type_text = self.text(type_node);
            match type_text {
                "v" => {
                    let node_field = node.child_by_field_name("node")?;
                    let node_ident = self.ident(node_field);
                    let reference = node.child_by_field_name("node2").map(|n| self.ident(n));
                    Some(SaveTarget::Voltage {
                        node: node_ident,
                        reference,
                        span: span_of(node),
                    })
                }
                "i" => {
                    let elem_field = node.child_by_field_name("element")?;
                    let element = self.ident(elem_field);
                    Some(SaveTarget::Current {
                        element,
                        span: span_of(node),
                    })
                }
                _ => None,
            }
        } else if let Some(name_node) = node.child_by_field_name("name") {
            let name = self.ident(name_node);
            Some(SaveTarget::Name {
                name,
                span: span_of(node),
            })
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// User-defined functions
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn lower_func(&mut self, node: tree_sitter::Node) -> Option<FuncDecl> {
        let name_node = self.required_field(node, "name")?;
        let name = self.ident(name_node);
        let body_node = self.required_field(node, "body")?;
        let body = self.lower_expr(body_node)?;

        let mut params = Vec::new();

        // Collect parameters from the func_params child.
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "func_params" {
                let mut param_cursor = child.walk();
                for param_child in child.named_children(&mut param_cursor) {
                    if param_child.kind() == "identifier" {
                        params.push(self.ident(param_child));
                    }
                }
            }
        }

        Some(FuncDecl {
            name,
            params,
            body,
            span: span_of(node),
        })
    }
}

// ---------------------------------------------------------------------------
// Initial conditions
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn lower_ic(&mut self, node: tree_sitter::Node) -> Option<IcDecl> {
        let mut entries = Vec::new();
        let mut cursor = node.walk();

        for child in node.named_children(&mut cursor) {
            if child.kind() == "ic_entry"
                && let Some(entry) = self.lower_ic_entry(child)
            {
                entries.push(entry);
            }
        }

        Some(IcDecl {
            entries,
            span: span_of(node),
        })
    }

    fn lower_ic_entry(&mut self, node: tree_sitter::Node) -> Option<IcEntry> {
        let node_ident = self.required_field(node, "node")?;
        let node_name = self.ident(node_ident);
        let value_node = self.required_field(node, "value")?;
        let value = self.lower_expr(value_node)?;

        Some(IcEntry {
            node: node_name,
            value,
            span: span_of(node),
        })
    }
}

// ---------------------------------------------------------------------------
// Coupled transmission lines
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn lower_coupled_line(&mut self, node: tree_sitter::Node) -> Option<CoupledLineDecl> {
        let name_node = self.required_field(node, "name")?;
        let name = self.ident(name_node);
        let mut fields = Vec::new();
        let mut cursor = node.walk();

        for child in node.named_children(&mut cursor) {
            if child.kind() == "coupled_line_field"
                && let Some(f) = self.lower_coupled_line_field(child)
            {
                fields.push(f);
            }
        }

        Some(CoupledLineDecl {
            name,
            fields,
            span: span_of(node),
        })
    }

    fn lower_coupled_line_field(&mut self, node: tree_sitter::Node) -> Option<CoupledLineField> {
        let key_node = self.required_field(node, "key")?;
        let key = self.ident(key_node);
        let value_node = self.required_field(node, "value")?;
        let value = self.lower_expr(value_node)?;
        Some(CoupledLineField {
            key,
            value,
            span: span_of(node),
        })
    }
}

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn lower_model(&mut self, node: tree_sitter::Node) -> Option<ModelDef> {
        let name_node = self.required_field(node, "name")?;
        let name = self.ident(name_node);
        let dt_node = self.required_field(node, "device_type")?;
        let device_type = self.ident(dt_node);
        // The grammar uses device_type for both the device kind and model
        // inheritance. The AST has a separate `base` field, but the grammar
        // doesn't distinguish. We leave base as None.
        let base = None;

        let mut params = Vec::new();
        let mut attrs = Vec::new();
        let mut cursor = node.walk();

        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "model_param" => {
                    if let Some(p) = self.lower_model_param(child) {
                        params.push(p);
                    }
                }
                "attribute" => {
                    if let Some(a) = self.lower_attribute(child) {
                        attrs.push(a);
                    }
                }
                _ => {}
            }
        }

        Some(ModelDef {
            name,
            device_type,
            base,
            params,
            attrs,
            span: span_of(node),
        })
    }

    fn lower_model_param(&mut self, node: tree_sitter::Node) -> Option<ModelParam> {
        let name_node = self.required_field(node, "name")?;
        let name = self.ident(name_node);
        let value_node = self.required_field(node, "value")?;
        let value = self.lower_expr(value_node)?;
        Some(ModelParam {
            name,
            value,
            span: span_of(node),
        })
    }
}

// ---------------------------------------------------------------------------
// Analysis
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn lower_analysis(&mut self, node: tree_sitter::Node) -> Option<AnalysisDecl> {
        let kind_node = self.required_field(node, "kind")?;
        let kind = self.ident(kind_node);
        let mut body = Vec::new();
        let mut cursor = node.walk();

        for child in node.named_children(&mut cursor) {
            match child.kind() {
                "analysis_setting" => {
                    if let Some(s) = self.lower_analysis_setting(child) {
                        body.push(s);
                    }
                }
                "sweep_spec" => {
                    if let Some(s) = self.lower_sweep_spec(child) {
                        body.push(s);
                    }
                }
                _ => {}
            }
        }

        Some(AnalysisDecl {
            kind,
            body,
            span: span_of(node),
        })
    }

    fn lower_analysis_setting(&mut self, node: tree_sitter::Node) -> Option<AnalysisItem> {
        let name_node = self.required_field(node, "name")?;
        let name = self.ident(name_node);
        let value_node = self.required_field(node, "value")?;
        let value = self.lower_expr(value_node)?;
        Some(AnalysisItem::Setting { name, value })
    }

    fn lower_sweep_spec(&mut self, node: tree_sitter::Node) -> Option<AnalysisItem> {
        let source_node = self.required_field(node, "source")?;
        let source = self.ident(source_node);
        let start_node = self.required_field(node, "start")?;
        let start = self.lower_expr(start_node)?;
        let stop_node = self.required_field(node, "stop")?;
        let stop = self.lower_expr(stop_node)?;
        let step_node = self.required_field(node, "step")?;
        let step = self.lower_expr(step_node)?;
        Some(AnalysisItem::Sweep {
            source,
            start,
            stop,
            step,
        })
    }
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn lower_import(&mut self, node: tree_sitter::Node) -> Option<Import> {
        let path_node = self.required_field(node, "path")?;
        let raw = self.text(path_node);
        // Strip surrounding quotes
        let path = raw
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(raw)
            .to_owned();
        let alias = node.child_by_field_name("alias").map(|n| self.ident(n));
        Some(Import {
            path,
            alias,
            span: span_of(node),
        })
    }
}

// ---------------------------------------------------------------------------
// Attributes
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn lower_attribute(&mut self, node: tree_sitter::Node) -> Option<Attribute> {
        let name_node = self.required_field(node, "name")?;
        let name = self.ident(name_node);
        let mut args = Vec::new();

        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "argument_list" {
                let mut inner_cursor = child.walk();
                for arg_node in child.named_children(&mut inner_cursor) {
                    if arg_node.kind() == "argument" {
                        let mut arg_cursor = arg_node.walk();
                        if let Some(expr_node) = arg_node.named_children(&mut arg_cursor).next()
                            && let Some(e) = self.lower_expr(expr_node)
                        {
                            args.push(e);
                        }
                    }
                }
            }
        }

        Some(Attribute {
            name,
            args,
            span: span_of(node),
        })
    }
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

impl Ctx<'_> {
    fn lower_expr(&mut self, node: tree_sitter::Node) -> Option<Expr> {
        match node.kind() {
            "number_literal" => self.lower_number(node),
            "string_literal" => self.lower_string(node),
            "boolean_literal" => self.lower_boolean(node),
            "identifier" => Some(Expr::Ident(self.ident(node))),
            "qualified_name" => {
                let qn = self.lower_qualified_name(node)?;
                Some(Expr::QualifiedName(qn))
            }
            "gnd" => Some(Expr::Gnd {
                span: span_of(node),
            }),
            "binary_expression" => self.lower_binary_expr(node),
            "unary_expression" => self.lower_unary_expr(node),
            "call_expression" => self.lower_call_expr(node),
            "paren_expression" => self.lower_paren_expr(node),
            "list_literal" => self.lower_list(node),
            "block_literal" => self.lower_block(node),
            "ERROR" => {
                self.error_at(node, "syntax error in expression");
                None
            }
            other => {
                self.error_at(node, format!("unexpected expression kind: {other}"));
                None
            }
        }
    }

    fn lower_number(&mut self, node: tree_sitter::Node) -> Option<Expr> {
        let text = self.text(node);
        match parse_number(text) {
            Some(v) => Some(Expr::Number {
                value: v,
                span: span_of(node),
            }),
            None => {
                let owned = text.to_owned();
                self.error_at(node, format!("invalid number literal: {owned}"));
                None
            }
        }
    }

    fn lower_string(&self, node: tree_sitter::Node) -> Option<Expr> {
        let raw = self.text(node);
        let value = raw
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(raw)
            .to_owned();
        Some(Expr::StringLit {
            value,
            span: span_of(node),
        })
    }

    fn lower_boolean(&self, node: tree_sitter::Node) -> Option<Expr> {
        let text = self.text(node);
        let value = text == "true";
        Some(Expr::Bool {
            value,
            span: span_of(node),
        })
    }

    fn lower_binary_expr(&mut self, node: tree_sitter::Node) -> Option<Expr> {
        let lhs_node = self.required_field(node, "left")?;
        let rhs_node = self.required_field(node, "right")?;

        // Read operator text before recursive lowering so we don't fight the
        // borrow checker. The operator node's text is trivially copyable.
        let op_node = self.required_field(node, "operator")?;
        let op_text = self.text(op_node).to_owned();

        let lhs = self.lower_expr(lhs_node)?;
        let rhs = self.lower_expr(rhs_node)?;

        let op = match op_text.as_str() {
            "+" => BinOp::Add,
            "-" => BinOp::Sub,
            "*" => BinOp::Mul,
            "/" => BinOp::Div,
            "%" => BinOp::Mod,
            "**" => BinOp::Pow,
            "==" => BinOp::Eq,
            "!=" => BinOp::Ne,
            "<" => BinOp::Lt,
            ">" => BinOp::Gt,
            "<=" => BinOp::Le,
            ">=" => BinOp::Ge,
            "&&" => BinOp::And,
            "||" => BinOp::Or,
            _ => {
                self.error_at(node, format!("unknown binary operator: {op_text}"));
                return None;
            }
        };

        Some(Expr::BinOp {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
            span: span_of(node),
        })
    }

    fn lower_unary_expr(&mut self, node: tree_sitter::Node) -> Option<Expr> {
        let op_node = self.required_field(node, "operator")?;
        let op_text = self.text(op_node).to_owned();
        let operand_node = self.required_field(node, "operand")?;
        let operand = self.lower_expr(operand_node)?;

        let op = match op_text.as_str() {
            "-" => UnaryOp::Neg,
            "!" => UnaryOp::Not,
            _ => {
                self.error_at(node, format!("unknown unary operator: {op_text}"));
                return None;
            }
        };

        Some(Expr::UnaryOp {
            op,
            operand: Box::new(operand),
            span: span_of(node),
        })
    }

    fn lower_call_expr(&mut self, node: tree_sitter::Node) -> Option<Expr> {
        let func_node = self.required_field(node, "function")?;
        let func_id = func_node.id();
        let func = self.ident(func_node);

        let mut args = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.id() != func_id
                && let Some(e) = self.lower_expr(child)
            {
                args.push(e);
            }
        }

        Some(Expr::Call {
            func,
            args,
            span: span_of(node),
        })
    }

    fn lower_paren_expr(&mut self, node: tree_sitter::Node) -> Option<Expr> {
        let mut exprs = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if let Some(e) = self.lower_expr(child) {
                exprs.push(e);
            }
        }

        match exprs.len() {
            0 => {
                self.error_at(node, "empty parenthesized expression");
                None
            }
            1 => Some(exprs.into_iter().next().unwrap()),
            _ => Some(Expr::Tuple {
                elements: exprs,
                span: span_of(node),
            }),
        }
    }

    fn lower_list(&mut self, node: tree_sitter::Node) -> Option<Expr> {
        let mut elements = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if let Some(e) = self.lower_expr(child) {
                elements.push(e);
            }
        }
        Some(Expr::List {
            elements,
            span: span_of(node),
        })
    }

    fn lower_block(&mut self, node: tree_sitter::Node) -> Option<Expr> {
        let mut entries = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "block_entry"
                && let Some((k, v)) = self.lower_block_entry(child)
            {
                entries.push((k, v));
            }
        }
        Some(Expr::Block {
            entries,
            span: span_of(node),
        })
    }

    fn lower_block_entry(&mut self, node: tree_sitter::Node) -> Option<(Ident, Expr)> {
        let key_node = self.required_field(node, "key")?;
        let key = self.ident(key_node);
        let value_node = self.required_field(node, "value")?;
        let value = self.lower_expr(value_node)?;
        Some((key, value))
    }

    fn lower_qualified_name(&mut self, node: tree_sitter::Node) -> Option<QualifiedName> {
        let mut segments = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "identifier" {
                segments.push(self.ident(child));
            }
        }
        if segments.is_empty() {
            self.error_at(node, "empty qualified name");
            return None;
        }
        Some(QualifiedName {
            segments,
            span: span_of(node),
        })
    }
}

// ---------------------------------------------------------------------------
// Number parsing
// ---------------------------------------------------------------------------

/// Parse a Cirq number literal, handling SI suffixes and alternative bases.
fn parse_number(text: &str) -> Option<f64> {
    let text = text.replace('_', "");

    // Hex
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16).ok().map(|v| v as f64);
    }

    // Binary
    if let Some(bin) = text.strip_prefix("0b").or_else(|| text.strip_prefix("0B")) {
        return u64::from_str_radix(bin, 2).ok().map(|v| v as f64);
    }

    // SI suffixes -- check multi-char first to avoid "Meg" matching "M"
    let suffixes: &[(&str, f64)] = &[
        ("Meg", 1e6),
        ("T", 1e12),
        ("G", 1e9),
        ("M", 1e6),
        ("k", 1e3),
        ("m", 1e-3),
        ("u", 1e-6),
        ("n", 1e-9),
        ("p", 1e-12),
        ("f", 1e-15),
    ];

    for &(suffix, mult) in suffixes {
        if let Some(num_part) = text.strip_suffix(suffix)
            && !num_part.is_empty()
        {
            return num_part.parse::<f64>().ok().map(|v| v * mult);
        }
    }

    // Plain number
    text.parse::<f64>().ok()
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

fn span_of(node: tree_sitter::Node) -> Span {
    Span::new(node.start_byte() as u32, node.end_byte() as u32)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_and_lower(source: &str) -> (SourceFile, Vec<Diagnostic>) {
        let tree = crate::parser::parse(source).expect("tree-sitter parse failed");
        lower(&tree, source)
    }

    #[test]
    fn empty_circuit() {
        let (sf, diags) = parse_and_lower("circuit empty {}");
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        assert_eq!(sf.items.len(), 1);
        match &sf.items[0] {
            TopLevel::Circuit(c) => assert_eq!(c.name.name, "empty"),
            other => panic!("expected Circuit, got {other:?}"),
        }
    }

    #[test]
    fn simple_voltage_divider() {
        let src = r#"
circuit voltage_divider {
    V1: vsource(vdd -> gnd, dc: 5)
    R1: resistor(in -> mid, 10k)
    R2: resistor(mid -> gnd, 2k)
    analysis op {}
}
"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

        let c = match &sf.items[0] {
            TopLevel::Circuit(c) => c,
            other => panic!("expected Circuit, got {other:?}"),
        };
        assert_eq!(c.name.name, "voltage_divider");
        assert_eq!(c.body.len(), 4);

        match &c.body[0] {
            CircuitItem::Element(e) => {
                assert_eq!(e.name.name, "V1");
                assert_eq!(e.element_type.name, "vsource");
                assert_eq!(e.args.len(), 2);
            }
            other => panic!("expected Element, got {other:?}"),
        }
    }

    #[test]
    fn module_with_ports() {
        let src = r#"
module inverter {
    port in: in
    port out: out
    port vdd: inout
    port vss: inout
    param wp = 2u
    param wn = 1u
}
"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

        let m = match &sf.items[0] {
            TopLevel::Module(m) => m,
            other => panic!("expected Module, got {other:?}"),
        };
        assert_eq!(m.name.name, "inverter");
        assert_eq!(m.ports.len(), 4);
        assert_eq!(m.body.len(), 2);
        assert_eq!(m.ports[0].direction, PortDirection::In);
        assert_eq!(m.ports[1].direction, PortDirection::Out);
        assert_eq!(m.ports[2].direction, PortDirection::InOut);
    }

    #[test]
    fn number_parsing() {
        /// Assert f64 values are close enough (relative tolerance for FP math).
        fn approx(text: &str, expected: f64) {
            let actual = parse_number(text).unwrap_or_else(|| panic!("failed to parse: {text}"));
            let tol = expected.abs() * 1e-12 + 1e-30;
            assert!(
                (actual - expected).abs() < tol,
                "{text}: expected {expected}, got {actual}"
            );
        }

        approx("42", 42.0);
        approx("3.14", 3.14);
        approx("10k", 10_000.0);
        approx("100n", 100e-9);
        approx("4.7u", 4.7e-6);
        approx("1Meg", 1e6);
        approx("1T", 1e12);
        approx("1G", 1e9);
        approx("1f", 1e-15);
        approx("10p", 10e-12);
        approx("330m", 0.33);
        approx("0xFF", 255.0);
        approx("0b1010", 10.0);
        approx("1_000_000", 1_000_000.0);
        approx("1.0e-3", 0.001);
        approx(".001", 0.001);
    }

    #[test]
    fn model_definition() {
        let src = r#"
model d1n4148: diode {
    is = 2.52n
    rs = 0.568
    n = 1.752
}
"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

        let m = match &sf.items[0] {
            TopLevel::Model(m) => m,
            other => panic!("expected Model, got {other:?}"),
        };
        assert_eq!(m.name.name, "d1n4148");
        assert_eq!(m.device_type.name, "diode");
        assert_eq!(m.params.len(), 3);
        assert_eq!(m.params[0].name.name, "is");
    }

    #[test]
    fn import_with_alias() {
        let src = r#"import "models/cmos.cirq" as cmos"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

        let i = match &sf.items[0] {
            TopLevel::Import(i) => i,
            other => panic!("expected Import, got {other:?}"),
        };
        assert_eq!(i.path, "models/cmos.cirq");
        assert_eq!(i.alias.as_ref().unwrap().name, "cmos");
    }

    #[test]
    fn import_without_alias() {
        let src = r#"import "models/cmos.cirq""#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

        let i = match &sf.items[0] {
            TopLevel::Import(i) => i,
            other => panic!("expected Import, got {other:?}"),
        };
        assert_eq!(i.path, "models/cmos.cirq");
        assert!(i.alias.is_none());
    }

    #[test]
    fn analysis_dc_sweep() {
        let src = r#"
circuit test {
    analysis dc {
        sweep V1: 0..5 step 0.1
    }
}
"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

        let c = match &sf.items[0] {
            TopLevel::Circuit(c) => c,
            other => panic!("expected Circuit, got {other:?}"),
        };
        match &c.body[0] {
            CircuitItem::Analysis(a) => {
                assert_eq!(a.kind.name, "dc");
                assert_eq!(a.body.len(), 1);
                match &a.body[0] {
                    AnalysisItem::Sweep {
                        source,
                        start,
                        stop,
                        step,
                    } => {
                        assert_eq!(source.name, "V1");
                        assert!(matches!(start, Expr::Number { value, .. } if *value == 0.0));
                        assert!(matches!(stop, Expr::Number { value, .. } if *value == 5.0));
                        assert!(matches!(step, Expr::Number { value, .. } if *value == 0.1));
                    }
                    other => panic!("expected Sweep, got {other:?}"),
                }
            }
            other => panic!("expected Analysis, got {other:?}"),
        }
    }

    #[test]
    fn binary_expression_precedence() {
        let src = r#"
circuit test {
    param x = 1 + 2 * 3
}
"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

        let c = match &sf.items[0] {
            TopLevel::Circuit(c) => c,
            other => panic!("expected Circuit, got {other:?}"),
        };
        match &c.body[0] {
            CircuitItem::Param(p) => match &p.default {
                Some(Expr::BinOp { op, lhs, rhs, .. }) => {
                    assert_eq!(*op, BinOp::Add);
                    assert!(matches!(lhs.as_ref(), Expr::Number { value, .. } if *value == 1.0));
                    match rhs.as_ref() {
                        Expr::BinOp { op, lhs, rhs, .. } => {
                            assert_eq!(*op, BinOp::Mul);
                            assert!(
                                matches!(lhs.as_ref(), Expr::Number { value, .. } if *value == 2.0)
                            );
                            assert!(
                                matches!(rhs.as_ref(), Expr::Number { value, .. } if *value == 3.0)
                            );
                        }
                        other => panic!("expected inner BinOp, got {other:?}"),
                    }
                }
                other => panic!("expected BinOp, got {other:?}"),
            },
            other => panic!("expected Param, got {other:?}"),
        }
    }

    #[test]
    fn unary_negation() {
        let src = r#"
circuit test {
    param x = -1
}
"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

        let c = match &sf.items[0] {
            TopLevel::Circuit(c) => c,
            other => panic!("expected Circuit, got {other:?}"),
        };
        match &c.body[0] {
            CircuitItem::Param(p) => match &p.default {
                Some(Expr::UnaryOp { op, operand, .. }) => {
                    assert_eq!(*op, UnaryOp::Neg);
                    assert!(
                        matches!(operand.as_ref(), Expr::Number { value, .. } if *value == 1.0)
                    );
                }
                other => panic!("expected UnaryOp, got {other:?}"),
            },
            other => panic!("expected Param, got {other:?}"),
        }
    }

    #[test]
    fn function_call() {
        let src = r#"
circuit test {
    let x = sqrt(2)
    let y = atan2(1, 0)
}
"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

        let c = match &sf.items[0] {
            TopLevel::Circuit(c) => c,
            other => panic!("expected Circuit, got {other:?}"),
        };
        match &c.body[0] {
            CircuitItem::Let(l) => match &l.value {
                Expr::Call { func, args, .. } => {
                    assert_eq!(func.name, "sqrt");
                    assert_eq!(args.len(), 1);
                }
                other => panic!("expected Call, got {other:?}"),
            },
            other => panic!("expected Let, got {other:?}"),
        }
        match &c.body[1] {
            CircuitItem::Let(l) => match &l.value {
                Expr::Call { func, args, .. } => {
                    assert_eq!(func.name, "atan2");
                    assert_eq!(args.len(), 2);
                }
                other => panic!("expected Call, got {other:?}"),
            },
            other => panic!("expected Let, got {other:?}"),
        }
    }

    #[test]
    fn global_declarations() {
        let src = r#"
circuit top {
    global vdd
    global vss
}
"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

        let c = match &sf.items[0] {
            TopLevel::Circuit(c) => c,
            other => panic!("expected Circuit, got {other:?}"),
        };
        assert_eq!(c.body.len(), 2);
        match &c.body[0] {
            CircuitItem::Global(g) => assert_eq!(g.name.name, "vdd"),
            other => panic!("expected Global, got {other:?}"),
        }
    }

    #[test]
    fn connection_with_gnd() {
        let src = r#"
circuit test {
    R1: resistor(a -> gnd, 10k)
}
"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

        let c = match &sf.items[0] {
            TopLevel::Circuit(c) => c,
            other => panic!("expected Circuit, got {other:?}"),
        };
        match &c.body[0] {
            CircuitItem::Element(e) => {
                assert_eq!(e.args.len(), 2);
                match &e.args[0] {
                    Argument::Connection { from, to } => {
                        assert_eq!(from.name, "a");
                        assert_eq!(to.name, "gnd");
                    }
                    other => panic!("expected Connection, got {other:?}"),
                }
            }
            other => panic!("expected Element, got {other:?}"),
        }
    }

    #[test]
    fn named_connection() {
        let src = r#"
circuit test {
    E1: vcvs(out_p -> out_n, control: ctrl_p -> ctrl_n, gain: 10)
}
"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

        let c = match &sf.items[0] {
            TopLevel::Circuit(c) => c,
            other => panic!("expected Circuit, got {other:?}"),
        };
        match &c.body[0] {
            CircuitItem::Element(e) => {
                assert_eq!(e.args.len(), 3);
                match &e.args[1] {
                    Argument::NamedConnection { name, from, to } => {
                        assert_eq!(name.name, "control");
                        assert_eq!(from.name, "ctrl_p");
                        assert_eq!(to.name, "ctrl_n");
                    }
                    other => panic!("expected NamedConnection, got {other:?}"),
                }
            }
            other => panic!("expected Element, got {other:?}"),
        }
    }

    #[test]
    fn block_literal_waveform() {
        let src = r#"
circuit test {
    V1: vsource(in -> gnd,
        pulse: { v1: 0, v2: 1.8, delay: 0 }
    )
}
"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

        let c = match &sf.items[0] {
            TopLevel::Circuit(c) => c,
            other => panic!("expected Circuit, got {other:?}"),
        };
        match &c.body[0] {
            CircuitItem::Element(e) => {
                assert_eq!(e.args.len(), 2);
                match &e.args[1] {
                    Argument::Named { name, value } => {
                        assert_eq!(name.name, "pulse");
                        match value {
                            Expr::Block { entries, .. } => {
                                assert_eq!(entries.len(), 3);
                                assert_eq!(entries[0].0.name, "v1");
                            }
                            other => panic!("expected Block, got {other:?}"),
                        }
                    }
                    other => panic!("expected Named, got {other:?}"),
                }
            }
            other => panic!("expected Element, got {other:?}"),
        }
    }

    #[test]
    fn list_literal_pwl() {
        let src = r#"
circuit test {
    V1: vsource(a -> gnd,
        pwl: [(0, 0), (1u, 0), (2u, 5), (10u, 5)]
    )
}
"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

        let c = match &sf.items[0] {
            TopLevel::Circuit(c) => c,
            other => panic!("expected Circuit, got {other:?}"),
        };
        match &c.body[0] {
            CircuitItem::Element(e) => match &e.args[1] {
                Argument::Named { name, value } => {
                    assert_eq!(name.name, "pwl");
                    match value {
                        Expr::List { elements, .. } => {
                            assert_eq!(elements.len(), 4);
                            assert!(matches!(
                                &elements[0],
                                Expr::Tuple { elements, .. } if elements.len() == 2
                            ));
                        }
                        other => panic!("expected List, got {other:?}"),
                    }
                }
                other => panic!("expected Named, got {other:?}"),
            },
            other => panic!("expected Element, got {other:?}"),
        }
    }

    #[test]
    fn attribute_on_param() {
        let src = r#"
circuit test {
    @range(0, 1)
    param coupling = 0.5
}
"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

        let c = match &sf.items[0] {
            TopLevel::Circuit(c) => c,
            other => panic!("expected Circuit, got {other:?}"),
        };
        match &c.body[0] {
            CircuitItem::Param(p) => {
                assert_eq!(p.attrs.len(), 1);
                assert_eq!(p.attrs[0].name.name, "range");
                assert_eq!(p.attrs[0].args.len(), 2);
            }
            other => panic!("expected Param, got {other:?}"),
        }
    }

    #[test]
    fn module_instantiation_qualified_name() {
        let src = r#"
circuit top {
    inv1: lib.inverter(in: a, out: b, vdd: vdd, vss: gnd)
}
"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

        let c = match &sf.items[0] {
            TopLevel::Circuit(c) => c,
            other => panic!("expected Circuit, got {other:?}"),
        };
        match &c.body[0] {
            CircuitItem::ModuleInst(mi) => {
                assert_eq!(mi.name.name, "inv1");
                assert_eq!(mi.module_name.segments.len(), 2);
                assert_eq!(mi.module_name.segments[0].name, "lib");
                assert_eq!(mi.module_name.segments[1].name, "inverter");
                assert_eq!(mi.args.len(), 4);
            }
            other => panic!("expected ModuleInst, got {other:?}"),
        }
    }

    #[test]
    fn error_recovery_produces_diagnostics() {
        let src = r#"
circuit broken {
    R1: resistor(a ->
}

circuit good {
    param x = 1
}
"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(!diags.is_empty(), "expected diagnostics for broken circuit");
        // Should still find circuits
        assert!(sf.items.len() >= 2);
        let found_good = sf
            .items
            .iter()
            .any(|item| matches!(item, TopLevel::Circuit(c) if c.name.name == "good"));
        assert!(
            found_good,
            "expected to find 'good' circuit after error recovery"
        );
    }

    #[test]
    fn typed_parameter() {
        let src = r#"
circuit test {
    param mode: string = "fast"
    param count: int = 10
}
"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

        let c = match &sf.items[0] {
            TopLevel::Circuit(c) => c,
            other => panic!("expected Circuit, got {other:?}"),
        };
        match &c.body[0] {
            CircuitItem::Param(p) => {
                assert_eq!(p.name.name, "mode");
                assert_eq!(p.ty.as_ref().unwrap().name, "string");
                assert!(
                    matches!(&p.default, Some(Expr::StringLit { value, .. }) if value == "fast")
                );
            }
            other => panic!("expected Param, got {other:?}"),
        }
    }

    #[test]
    fn complete_cmos_inverter() {
        let src = r#"
circuit cmos_inverter {
    model nch: nmos {
        level = 1
        vto = 0.7
        kp = 110u
        lambda = 0.04
    }

    model pch: pmos {
        level = 1
        vto = -0.7
        kp = 55u
        lambda = 0.04
    }

    param vdd_voltage = 1.8

    Vdd: vsource(vdd -> gnd, dc: vdd_voltage)
    Vin: vsource(in -> gnd,
        pulse: { v1: 0, v2: 1.8, delay: 0, rise: 1n, fall: 1n, width: 5n, period: 10n }
    )

    M1: pmos(vdd -> out, gate: in, bulk: vdd, model: pch, w: 2u, l: 180n)
    M2: nmos(out -> gnd, gate: in, bulk: gnd, model: nch, w: 1u, l: 180n)

    analysis tran {
        step: 100p
        stop: 20n
    }
}
"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

        let c = match &sf.items[0] {
            TopLevel::Circuit(c) => c,
            other => panic!("expected Circuit, got {other:?}"),
        };
        assert_eq!(c.name.name, "cmos_inverter");
        // 2 models + 1 param + 4 elements + 1 analysis = 8
        assert_eq!(c.body.len(), 8);
    }

    #[test]
    fn boolean_in_analysis() {
        let src = r#"
circuit test {
    analysis tran {
        step: 10n
        stop: 1u
        uic: true
    }
}
"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

        let c = match &sf.items[0] {
            TopLevel::Circuit(c) => c,
            other => panic!("expected Circuit, got {other:?}"),
        };
        match &c.body[0] {
            CircuitItem::Analysis(a) => {
                assert_eq!(a.body.len(), 3);
                match &a.body[2] {
                    AnalysisItem::Setting { name, value } => {
                        assert_eq!(name.name, "uic");
                        assert!(matches!(value, Expr::Bool { value: true, .. }));
                    }
                    other => panic!("expected Setting, got {other:?}"),
                }
            }
            other => panic!("expected Analysis, got {other:?}"),
        }
    }

    #[test]
    fn exponentiation_right_associative() {
        let src = r#"
circuit test {
    param x = 2 ** 3 ** 4
}
"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

        let c = match &sf.items[0] {
            TopLevel::Circuit(c) => c,
            other => panic!("expected Circuit, got {other:?}"),
        };
        match &c.body[0] {
            CircuitItem::Param(p) => match &p.default {
                Some(Expr::BinOp { op, lhs, rhs, .. }) => {
                    assert_eq!(*op, BinOp::Pow);
                    assert!(matches!(lhs.as_ref(), Expr::Number { value, .. } if *value == 2.0));
                    match rhs.as_ref() {
                        Expr::BinOp { op, lhs, rhs, .. } => {
                            assert_eq!(*op, BinOp::Pow);
                            assert!(
                                matches!(lhs.as_ref(), Expr::Number { value, .. } if *value == 3.0)
                            );
                            assert!(
                                matches!(rhs.as_ref(), Expr::Number { value, .. } if *value == 4.0)
                            );
                        }
                        other => panic!("expected inner BinOp(**), got {other:?}"),
                    }
                }
                other => panic!("expected BinOp, got {other:?}"),
            },
            other => panic!("expected Param, got {other:?}"),
        }
    }

    #[test]
    fn multiple_imports() {
        let src = r#"
import "models/nmos.cirq" as nmos_lib
import "models/pmos.cirq" as pmos_lib
"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        assert_eq!(sf.items.len(), 2);

        let i0 = match &sf.items[0] {
            TopLevel::Import(i) => i,
            other => panic!("expected Import, got {other:?}"),
        };
        assert_eq!(i0.path, "models/nmos.cirq");
        assert_eq!(i0.alias.as_ref().unwrap().name, "nmos_lib");

        let i1 = match &sf.items[1] {
            TopLevel::Import(i) => i,
            other => panic!("expected Import, got {other:?}"),
        };
        assert_eq!(i1.path, "models/pmos.cirq");
        assert_eq!(i1.alias.as_ref().unwrap().name, "pmos_lib");
    }

    #[test]
    fn coupled_line_declaration() {
        let src = r#"
circuit cpl_test {
    coupled_line P1 {
        in: [a1, a2]
        out: [b1, b2]
        gnd: gnd
        model: cpl_mod
    }
}
"#;
        let (sf, diags) = parse_and_lower(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");

        let c = match &sf.items[0] {
            TopLevel::Circuit(c) => c,
            other => panic!("expected Circuit, got {other:?}"),
        };
        assert_eq!(c.body.len(), 1);
        match &c.body[0] {
            CircuitItem::CoupledLine(cl) => {
                assert_eq!(cl.name.name, "P1");
                assert_eq!(cl.fields.len(), 4);

                // in: [a1, a2]
                assert_eq!(cl.fields[0].key.name, "in");
                match &cl.fields[0].value {
                    Expr::List { elements, .. } => {
                        assert_eq!(elements.len(), 2);
                        assert!(matches!(&elements[0], Expr::Ident(id) if id.name == "a1"));
                        assert!(matches!(&elements[1], Expr::Ident(id) if id.name == "a2"));
                    }
                    other => panic!("expected List, got {other:?}"),
                }

                // out: [b1, b2]
                assert_eq!(cl.fields[1].key.name, "out");

                // gnd: gnd
                assert_eq!(cl.fields[2].key.name, "gnd");
                assert!(matches!(&cl.fields[2].value, Expr::Gnd { .. }));

                // model: cpl_mod
                assert_eq!(cl.fields[3].key.name, "model");
                assert!(matches!(&cl.fields[3].value, Expr::Ident(id) if id.name == "cpl_mod"));
            }
            other => panic!("expected CoupledLine, got {other:?}"),
        }
    }
}
