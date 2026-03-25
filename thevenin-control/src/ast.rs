//! AST for `.control` block statements.

/// A single statement in a `.control` block.
#[derive(Debug, Clone)]
pub enum Statement {
    /// `let name = expr` or `let name[i] = expr`
    Let {
        name: String,
        expr: String,
    },
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
    Alter {
        spec: String,
        value: AlterValue,
    },
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
    /// Comment line (starts with * or $)
    Comment,
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
