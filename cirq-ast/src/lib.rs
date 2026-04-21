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
    Model(ModelDef),
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
#[derive(Debug, Clone)]
pub struct Import {
    pub path: String,
    pub alias: Option<Ident>,
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
