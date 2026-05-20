//! AST for `.control` block statements.

/// A single statement in a `.control` block.
#[derive(Debug, Clone)]
pub enum Statement {
    /// `let name = expr` or `let name[i] = expr`
    Let { name: String, expr: String },
    /// `echo "text" $var $&vec ...`
    Echo(Vec<EchoFragment>),
    /// `if cond ... else ... end`
    If {
        cond: String,
        body: Vec<Statement>,
        else_body: Vec<Statement>,
    },
    /// `foreach var val1 val2 ... end`
    Foreach {
        var: String,
        values: Vec<String>,
        body: Vec<Statement>,
    },
    /// `quit [exitcode]`
    Quit(Option<i32>),
    /// `set key = value` or `set key`
    Set(Vec<(String, Option<String>)>),
    /// `setplot plotname`
    Setplot(String),
    /// `define name(args) body`
    Define {
        name: String,
        args: Vec<String>,
        body: String,
    },
    /// `compose name values expr1 expr2 ...`
    Compose {
        name: String,
        value_exprs: Vec<String>,
    },
    /// `alter @device[param] = value` or `alter @device[param] = [ v1 v2 ... ]`
    Alter { spec: String, value: AlterValue },
    /// `strcmp result a b`
    Strcmp {
        result: String,
        a: String,
        b: String,
    },
    /// `print expr1 expr2 ...`
    Print {
        exprs: Vec<String>,
        file: Option<String>,
    },
    /// Simulation commands: op, dc, ac, tran, sens, noise, pz, tf
    RunAnalysis(String),
    /// `eprint ...` — print element info (treated as echo for now)
    Eprint(Vec<String>),
    /// `stop when <condition>` — register a pause condition for the next
    /// transient run. Currently only `stop when time = <value>` is supported;
    /// the value is parsed as a SPICE number with optional SI suffix.
    StopWhen(StopCondition),
    /// `resume` — resume a previously paused transient simulation.
    Resume,
    /// Comment line (starts with * or $)
    Comment,
}

/// A pause condition registered by `stop when`.
///
/// ngspice supports several condition kinds (`time =`, `<expr>` comparisons,
/// `node v(...) > x`, etc.); thevenin currently implements only the
/// time-equals form needed by `regression/misc/resume-1.cir`. Other forms
/// are parsed leniently — the executor errors out if asked to honour an
/// unsupported kind.
#[derive(Debug, Clone)]
pub enum StopCondition {
    /// `stop when time = <value>` — pause the next transient run at the
    /// first integration point at or past `t_pause`.
    TimeEq(f64),
}

/// A value for the `alter` command.
#[derive(Debug, Clone)]
pub enum AlterValue {
    Scalar(f64),
    Vector(Vec<f64>),
}

/// Fragment of an `echo` command.
#[derive(Debug, Clone)]
pub enum EchoFragment {
    /// Literal text.
    Literal(String),
    /// `$varname` — substitute string variable.
    VarRef(String),
    /// `$&varname` — substitute vector's scalar value as string.
    VecScalar(String),
}
