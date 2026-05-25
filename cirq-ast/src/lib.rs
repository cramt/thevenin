//! Cirq AST — source-faithful abstract syntax tree for the Cirq language.
//!
//! This crate defines the AST types produced by lowering Tree-sitter's CST.
//! The AST preserves source spans and syntactic structure but does not resolve
//! names, evaluate parameters, or flatten hierarchy.

pub mod span;

use span::Span;

// ---------------------------------------------------------------------------
// Top-level
// ---------------------------------------------------------------------------

/// A complete Cirq source file.
#[derive(Debug, Clone)]
pub struct SourceFile {
    pub items: Vec<TopLevel>,
    pub span: Span,
}

/// A top-level item in a Cirq file.
#[derive(Debug, Clone)]
pub enum TopLevel {
    Circuit(Circuit),
    Module(ModuleDef),
    Import(Import),
    Export(ExportDecl),
    Model(ModelDef),
    Func(FuncDecl),
}

// ---------------------------------------------------------------------------
// Circuit
// ---------------------------------------------------------------------------

/// A `circuit` declaration — the top-level simulation unit.
#[derive(Debug, Clone)]
pub struct Circuit {
    pub name: Ident,
    pub body: Vec<CircuitItem>,
    pub attrs: Vec<Attribute>,
    pub span: Span,
}

/// Items that can appear inside a circuit body.
#[derive(Debug, Clone)]
pub enum CircuitItem {
    Param(ParamDecl),
    Let(LetDecl),
    Element(ElementInst),
    ModuleInst(ModuleInst),
    ModuleDef(ModuleDef),
    ModelDef(ModelDef),
    Analysis(AnalysisDecl),
    Global(GlobalDecl),
    Options(OptionsDecl),
    Temp(TempDecl),
    Save(SaveDecl),
    Func(FuncDecl),
    Ic(IcDecl),
    CoupledLine(CoupledLineDecl),
    Code(CodeDecl),
    Measure(MeasureDecl),
}

// ---------------------------------------------------------------------------
// Module
// ---------------------------------------------------------------------------

/// A `module` definition (subcircuit).
#[derive(Debug, Clone)]
pub struct ModuleDef {
    pub name: Ident,
    pub ports: Vec<PortDecl>,
    pub body: Vec<CircuitItem>,
    pub attrs: Vec<Attribute>,
    pub span: Span,
}

/// A port declaration within a module.
#[derive(Debug, Clone)]
pub struct PortDecl {
    pub name: Ident,
    pub direction: PortDirection,
    pub attrs: Vec<Attribute>,
    pub span: Span,
}

/// Port direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortDirection {
    In,
    Out,
    InOut,
}

// ---------------------------------------------------------------------------
// Elements
// ---------------------------------------------------------------------------

/// An element instantiation: `R1: resistor(a -> b, 10k)`
#[derive(Debug, Clone)]
pub struct ElementInst {
    pub name: Ident,
    pub element_type: Ident,
    pub args: Vec<Argument>,
    pub attrs: Vec<Attribute>,
    pub span: Span,
}

/// A module instantiation: `inv1: inverter(in: a, out: b, vdd: vdd, vss: gnd)`
#[derive(Debug, Clone)]
pub struct ModuleInst {
    pub name: Ident,
    pub module_name: QualifiedName,
    pub args: Vec<Argument>,
    pub attrs: Vec<Attribute>,
    pub span: Span,
}

/// An argument in an element or module instantiation.
#[derive(Debug, Clone)]
pub enum Argument {
    /// Positional argument: `10k`, `a -> b`
    Positional(Expr),
    /// Named argument: `resistance: 10k`, `model: nch`
    Named { name: Ident, value: Expr },
    /// Connection argument: `a -> b`
    Connection { from: Ident, to: Ident },
    /// Named connection: `control: a -> b`
    NamedConnection { name: Ident, from: Ident, to: Ident },
}

// ---------------------------------------------------------------------------
// Parameters and bindings
// ---------------------------------------------------------------------------

/// `param name = value` or `param name: type = value`
#[derive(Debug, Clone)]
pub struct ParamDecl {
    pub name: Ident,
    pub ty: Option<Ident>,
    pub default: Option<Expr>,
    pub attrs: Vec<Attribute>,
    pub span: Span,
}

/// `let name = expr`
#[derive(Debug, Clone)]
pub struct LetDecl {
    pub name: Ident,
    pub value: Expr,
    pub attrs: Vec<Attribute>,
    pub span: Span,
}

/// `global net_name`
#[derive(Debug, Clone)]
pub struct GlobalDecl {
    pub name: Ident,
    pub span: Span,
}

/// `options { gmin: 1e-12, abstol: 1e-12, ... }`
#[derive(Debug, Clone)]
pub struct OptionsDecl {
    pub settings: Vec<OptionSetting>,
    pub span: Span,
}

/// A single key-value setting inside an `options` block.
#[derive(Debug, Clone)]
pub struct OptionSetting {
    pub name: Ident,
    pub value: Expr,
    pub span: Span,
}

/// `temp 27`
#[derive(Debug, Clone)]
pub struct TempDecl {
    pub value: Expr,
    pub span: Span,
}

/// `save { v(out), i(R1), ... }`
#[derive(Debug, Clone)]
pub struct SaveDecl {
    pub targets: Vec<SaveTarget>,
    pub span: Span,
}

/// A single save target: `v(node)`, `v(n1, n2)`, `i(elem)`, or a bare name.
#[derive(Debug, Clone)]
pub enum SaveTarget {
    /// `v(node)` — node voltage
    Voltage {
        node: Ident,
        reference: Option<Ident>,
        span: Span,
    },
    /// `i(element)` — element current
    Current { element: Ident, span: Span },
    /// Bare identifier
    Name { name: Ident, span: Span },
}

// ---------------------------------------------------------------------------
// User-defined functions
// ---------------------------------------------------------------------------

/// `limit(x, lo, hi) = min(max(x, lo), hi)`
#[derive(Debug, Clone)]
pub struct FuncDecl {
    pub name: Ident,
    pub params: Vec<Ident>,
    pub body: Expr,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Initial conditions
// ---------------------------------------------------------------------------

/// `ic { v(out) = 1.5, v(mid) = 0.8 }`
#[derive(Debug, Clone)]
pub struct IcDecl {
    pub entries: Vec<IcEntry>,
    pub span: Span,
}

/// A single initial condition entry: `v(node) = value`.
#[derive(Debug, Clone)]
pub struct IcEntry {
    pub node: Ident,
    pub value: Expr,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Coupled transmission lines
// ---------------------------------------------------------------------------

/// `coupled_line P1 { in: [a1, a2], out: [b1, b2], gnd: gnd, model: cpl_mod }`
#[derive(Debug, Clone)]
pub struct CoupledLineDecl {
    pub name: Ident,
    pub fields: Vec<CoupledLineField>,
    pub span: Span,
}

/// A single key-value field inside a `coupled_line` block.
#[derive(Debug, Clone)]
pub struct CoupledLineField {
    pub key: Ident,
    pub value: Expr,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Code block
// ---------------------------------------------------------------------------

/// `code "lang" { ... }` — verbatim embedded language block passed through
/// without Cirq-level parsing. The `language` tag selects the interpreter
/// (e.g. `"control"` for SPICE control language).
#[derive(Debug, Clone)]
pub struct CodeDecl {
    pub language: String,
    pub lines: Vec<String>,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Measure
// ---------------------------------------------------------------------------

/// A native Cirq `measure` block, the source-language counterpart to SPICE's
/// `.meas` directive.
///
/// The body contains a single required `spec` field whose value is a string
/// literal carrying the measurement clauses in SPICE syntax (e.g.
/// `"TRIG v(out) VAL=0.5 RISE=1 TARG v(out) VAL=4.5 RISE=1"`). Reusing the
/// SPICE spec string here lets the IR lowering call straight into
/// `cirq_ir::MeasureSpec::parse` and gives a lossless round-trip with the
/// SPICE importer.
#[derive(Debug, Clone)]
pub struct MeasureDecl {
    /// The analysis kind this measurement applies to (`tran`, `ac`, `dc`, ...).
    pub analysis_kind: Ident,
    /// The measurement name (the value SPICE attaches to the result vector).
    pub name: String,
    /// Span of the string-literal token carrying the name.
    pub name_span: Span,
    /// The verbatim measurement clauses (no surrounding quotes).
    pub spec: String,
    /// Span of the string-literal token carrying the spec body.
    pub spec_span: Span,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

/// `model name: device_type { ... }`
#[derive(Debug, Clone)]
pub struct ModelDef {
    pub name: Ident,
    pub device_type: Ident,
    pub base: Option<Ident>,
    pub params: Vec<ModelParam>,
    pub attrs: Vec<Attribute>,
    pub span: Span,
}

/// A parameter assignment inside a model block.
#[derive(Debug, Clone)]
pub struct ModelParam {
    pub name: Ident,
    pub value: Expr,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Analysis
// ---------------------------------------------------------------------------

/// `analysis <kind> { ... }`
#[derive(Debug, Clone)]
pub struct AnalysisDecl {
    pub kind: Ident,
    pub body: Vec<AnalysisItem>,
    pub span: Span,
}

/// An item inside an analysis block.
#[derive(Debug, Clone)]
pub enum AnalysisItem {
    /// Key-value setting: `step: 1n`
    Setting { name: Ident, value: Expr },
    /// Sweep specification: `sweep V1: 0..5 step 0.1`
    Sweep {
        source: Ident,
        start: Expr,
        stop: Expr,
        step: Expr,
    },
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

/// An expression node.
#[derive(Debug, Clone)]
pub enum Expr {
    /// Numeric literal with optional SI suffix: `10k`, `3.14`, `100n`
    Number { value: f64, span: Span },
    /// Integer literal: `42`, `0xFF`
    Integer { value: i64, span: Span },
    /// String literal: `"hello"`
    StringLit { value: String, span: Span },
    /// Boolean literal: `true`, `false`
    Bool { value: bool, span: Span },
    /// Identifier reference: `vdd`, `r_load`
    Ident(Ident),
    /// Qualified name: `lib.model_name`
    QualifiedName(QualifiedName),
    /// Binary operation: `a + b`
    BinOp {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        span: Span,
    },
    /// Unary operation: `-x`, `!flag`
    UnaryOp {
        op: UnaryOp,
        operand: Box<Expr>,
        span: Span,
    },
    /// Function call: `sqrt(x)`
    Call {
        func: Ident,
        args: Vec<Expr>,
        span: Span,
    },
    /// Ternary conditional: `cond ? then_expr : else_expr`.
    ///
    /// Lowered to the same IR shape as the `if(cond, then, else)` builtin so
    /// downstream consumers don't need a separate branch. Right-associative so
    /// `a ? b : c ? d : e` parses as `a ? b : (c ? d : e)`.
    Ternary {
        cond: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
        span: Span,
    },
    /// Range expression: `0..5` (used in sweep specs)
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        span: Span,
    },
    /// Tuple/list for PWL points: `[(0, 0), (1u, 5)]`
    List { elements: Vec<Expr>, span: Span },
    /// Tuple: `(0, 5)`
    Tuple { elements: Vec<Expr>, span: Span },
    /// Waveform block: `pulse: { v1: 0, v2: 3.3, ... }`
    Block {
        entries: Vec<(Ident, Expr)>,
        span: Span,
    },
    /// The `gnd` keyword
    Gnd { span: Span },
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

// ---------------------------------------------------------------------------
// Imports
// ---------------------------------------------------------------------------

/// `import "path.cirq"` or `import "path.cirq" as name`
/// or `import { name1, name2 } from "path.cirq"`
#[derive(Debug, Clone)]
pub struct Import {
    pub path: String,
    pub alias: Option<Ident>,
    /// Named imports: `import { tt, ff } from "pdk.cirq"`.
    /// Empty means import all bare (non-exported) items.
    pub names: Vec<Ident>,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Exports
// ---------------------------------------------------------------------------

/// `export name { model ..., module ..., func ... }`
///
/// Groups top-level declarations under a name for selective import.
/// Analogous to SPICE `.lib section` / `.endl section`.
#[derive(Debug, Clone)]
pub struct ExportDecl {
    pub name: Ident,
    pub items: Vec<TopLevel>,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Attributes
// ---------------------------------------------------------------------------

/// `@name` or `@name(args...)`
#[derive(Debug, Clone)]
pub struct Attribute {
    pub name: Ident,
    pub args: Vec<Expr>,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Common types
// ---------------------------------------------------------------------------

/// A source identifier with its span.
#[derive(Debug, Clone)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

/// A qualified name: `module.member`
#[derive(Debug, Clone)]
pub struct QualifiedName {
    pub segments: Vec<Ident>,
    pub span: Span,
}
