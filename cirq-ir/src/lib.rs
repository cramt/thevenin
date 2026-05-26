//! Canonical Cirq IR — the semantic center of the Cirq toolchain.
//!
//! This IR is produced by lowering the Cirq AST (name resolution, parameter
//! evaluation, subcircuit flattening, validation). All downstream consumers
//! (simulator adapter, linting, formatting) should work from this representation.

pub mod control;
mod control_lower;

/// Unique identifier for IR nodes (nets, elements, modules, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Id(pub u32);

/// A fully resolved circuit ready for simulation or analysis.
#[derive(Debug, Clone)]
pub struct Circuit {
    pub name: String,
    pub nets: Vec<Net>,
    pub elements: Vec<Element>,
    pub models: Vec<Model>,
    pub analyses: Vec<Analysis>,
    pub params: Vec<ResolvedParam>,
    /// `.csparam` declarations. Semantically identical to `.param` for
    /// netlist resolution, but additionally seeded into the `.control` block
    /// interpreter's variable scope so control scripts can read them as
    /// ordinary `$name` variables. When a name collides with a `.param` of
    /// the same name, the `.csparam` value wins in the control scope —
    /// matching ngspice's behaviour.
    pub csparams: Vec<ResolvedParam>,
    /// Simulation options (e.g. GMIN, ABSTOL, RELTOL).
    pub options: Vec<(String, Value)>,
    /// Simulation temperatures in °C.
    ///
    /// Empty means use the default (27 °C). A single entry is the common case.
    /// Multiple entries request the simulation be run at each temperature
    /// (equivalent to SPICE `.temp 25 50 100`).
    pub temps: Vec<f64>,
    /// Output save targets (e.g. `v(out)`, `i(R1)`).
    pub save: Vec<String>,
    /// User-defined functions.
    pub funcs: Vec<FuncDef>,
    /// Initial node voltages (`.ic`).
    pub initial_conditions: Vec<(Id, f64)>,
    /// Suggested initial node voltages for convergence (`.nodeset`).
    pub nodeset: Vec<(Id, f64)>,
    /// Measurement specifications (`.meas`).
    pub measures: Vec<MeasureSpec>,
    /// Verbatim embedded code blocks — each entry is `(language, lines)`.
    /// `"control"` blocks are passed to the SPICE control-block interpreter.
    pub code_blocks: Vec<CodeBlock>,
    /// SPICE directives that have no typed IR representation yet, preserved
    /// verbatim so the Netlist round-trip is lossless. The simulator output
    /// formatter, for example, parses `.print` / `.plot` directives directly
    /// from `Item::Raw` strings — those land here.
    pub raw_directives: Vec<String>,
}

/// An embedded code block with a language tag.
///
/// `lines` is the verbatim source preserved for round-trip emission. For
/// blocks the IR knows how to type (`language == "control"`), `parsed`
/// holds the typed AST — interpreters consume that directly instead of
/// re-parsing `lines` on every invocation. `parsed` is `None` for blocks
/// in unrecognised languages, and may also be `None` for control blocks
/// that were constructed programmatically without parsing.
#[derive(Debug, Clone)]
pub struct CodeBlock {
    pub language: String,
    pub lines: Vec<String>,
    /// Parsed typed AST for blocks whose `language` the IR understands.
    /// Today that's only `language == "control"`. When `Some`, the
    /// statement list is the authoritative form; `lines` is kept in
    /// sync as the round-trip rendering.
    pub parsed: Option<Vec<control::Statement>>,
}

impl CodeBlock {
    /// Build a `CodeBlock` from raw lines, parsing `language == "control"`
    /// blocks into the typed AST. Other languages keep `parsed: None`.
    pub fn from_lines(language: impl Into<String>, lines: Vec<String>) -> Self {
        let language = language.into();
        let parsed = if language == "control" {
            control::parse_control_block(&lines).ok()
        } else {
            None
        };
        Self {
            language,
            lines,
            parsed,
        }
    }
}

/// A resolved electrical net.
#[derive(Debug, Clone)]
pub struct Net {
    pub id: Id,
    pub name: String,
    pub is_global: bool,
}

/// A resolved element instance.
#[derive(Debug, Clone)]
pub struct Element {
    pub id: Id,
    pub name: String,
    pub kind: ElementKind,
    pub connections: Vec<Connection>,
    pub params: Vec<(String, Value)>,
    pub model: Option<Id>,
    /// Source specification for voltage/current sources.
    /// `None` for non-source elements.
    pub source_spec: Option<SourceSpec>,
}

/// Connection between an element terminal and a net.
#[derive(Debug, Clone)]
pub struct Connection {
    pub terminal: String,
    pub net: Id,
}

/// Resolved element kinds (after model resolution).
///
/// `#[non_exhaustive]` so new device kinds (e.g. URC, HICUM) can land in
/// 1.x without a major bump. External callers must include a catch-all
/// arm; see `docs/api-stability.md`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ElementKind {
    Resistor,
    Capacitor,
    Inductor,
    Coupling,
    VoltageSource,
    CurrentSource,
    BehavioralSource {
        /// `Voltage` or `Current` — voltage or current mode.
        mode: BehavioralMode,
        /// The expression string, e.g. `"sin(2*pi*1k*time)"`.
        spec: String,
    },
    Diode,
    Npn,
    Pnp,
    Nmos,
    Pmos,
    NJfet,
    PJfet,
    NMesfet,
    PMesfet,
    Vcvs,
    Vccs,
    Ccvs,
    Cccs,
    /// Lossy transmission line (SPICE O element / LTRA model).
    TransmissionLine,
    /// Single lossy transmission line (SPICE Y element / TXL model).
    Txl,
    /// Ideal lossless transmission line (SPICE `T` element).
    /// Terminals appear in `connections` under the names `"port1_pos"`,
    /// `"port1_neg"`, `"port2_pos"`, `"port2_neg"`.
    Tline {
        /// Characteristic impedance Z0 in ohms.
        z0: f64,
        /// One-way propagation delay TD in seconds.
        td: f64,
        /// Optional initial conditions `(v1, i1, v2, i2)`.
        ic: Option<[f64; 4]>,
    },
    /// Coupled multiconductor transmission line (P element).
    /// Connections use terminal names `"in0"`, `"in1"`, ..., `"gnd"`,
    /// `"out0"`, `"out1"`, ... in the element's `connections` field.
    CoupledLine {
        /// Number of coupled lines (= number of in/out port pairs).
        width: usize,
    },
    /// XSPICE code model instance (A element).
    /// Scalar connections use terminal names `"c0"`, `"c1"`, ...
    /// The full structured connection list (scalar vs. array) is here.
    Xspice {
        connections: Vec<XspiceConnection>,
    },
    /// Voltage- or current-controlled switch (SPICE `S` / `W` elements).
    ///
    /// The switched terminals (`n+`, `n-`) live in `connections` under the
    /// names `"pos"` and `"neg"`. The control reference is carried inline
    /// in [`SwitchControl`]: voltage-controlled switches name two more
    /// connections (`"ctrl_pos"`, `"ctrl_neg"`) and current-controlled
    /// switches name the sensing voltage-source instance.
    Switch {
        kind: SwitchKind,
        control: SwitchControl,
    },
}

/// Behavioral source mode — voltage or current.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BehavioralMode {
    Voltage,
    Current,
}

/// Switch kind — voltage-controlled (`S` element) or current-controlled
/// (`W` element).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchKind {
    Voltage,
    Current,
}

/// Where a switch reads its control value from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchControl {
    /// `S` element: voltage difference between two nets carried on the
    /// element's `"ctrl_pos"` / `"ctrl_neg"` connections. The variants
    /// store the net ids directly so callers don't have to re-walk the
    /// connections vec.
    Nodes { pos: Id, neg: Id },
    /// `W` element: branch current through the named voltage source.
    Vsense { name: String },
}

/// A single XSPICE port connection at the IR level.
#[derive(Debug, Clone)]
pub enum XspiceConnection {
    /// A single scalar net.
    Scalar(Id),
    /// A bracketed array of nets.
    Array(Vec<Id>),
}

/// A resolved device model.
#[derive(Debug, Clone)]
pub struct Model {
    pub id: Id,
    pub name: String,
    pub device_type: DeviceType,
    pub params: Vec<(String, Value)>,
}

/// Device types for models.
///
/// SPICE has a large and growing menagerie of model kinds (BSIM variants,
/// XSPICE code models, transmission line models, switches, …). The typed
/// variants cover the well-known semiconductor kinds; everything else is
/// preserved verbatim in [`DeviceType::Other`] so the simulator's string-keyed
/// dispatch in `mna.rs` keeps working when the SPICE importer rebuilds a
/// model that has no first-class IR representation.
///
/// `#[non_exhaustive]` — new model kinds may be added in any 1.x release.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeviceType {
    Diode,
    Npn,
    Pnp,
    Nmos,
    Pmos,
    NJfet,
    PJfet,
    NMesfet,
    PMesfet,
    /// N-channel vertical-DMOS power MOSFET (`.model … VDMOS (…)` or `VDMOSN`).
    Vdmos,
    /// P-channel vertical-DMOS power MOSFET (`.model … VDMOSP (…)`).
    Pvdmos,
    /// Voltage-controlled switch (`.model … SW (…)`).
    VSwitch,
    /// Current-controlled switch (`.model … CSW (…)`).
    ISwitch,
    /// Any model kind that has no typed variant — held as the original SPICE
    /// kind string (e.g. `"TXL"`, `"LTRA"`, `"CPL"`, `"D_RAM"`, `"NHFET"`).
    Other(String),
}

/// A resolved parameter value.
///
/// `#[non_exhaustive]` — new typed variants (e.g. `Complex`, `Net`) may
/// be added in any 1.x release.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Value {
    Real(f64),
    Integer(i64),
    Bool(bool),
    String(String),
}

// ---------------------------------------------------------------------------
// Source specifications (voltage/current sources)
// ---------------------------------------------------------------------------

/// AC specification for a voltage/current source: magnitude and phase.
#[derive(Debug, Clone)]
pub struct AcSpec {
    pub mag: f64,
    /// Phase in degrees. Defaults to 0.0 when not specified.
    pub phase: f64,
}

/// Transient waveform for voltage/current sources.
///
/// `#[non_exhaustive]` — new waveform shapes may be added in any 1.x.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Waveform {
    /// `PULSE(v1 v2 [td [tr [tf [pw [per]]]]])`
    Pulse {
        v1: f64,
        v2: f64,
        td: Option<f64>,
        tr: Option<f64>,
        tf: Option<f64>,
        pw: Option<f64>,
        per: Option<f64>,
    },
    /// `SIN(v0 va [freq [td [theta [phi]]]])`
    Sin {
        v0: f64,
        va: f64,
        freq: Option<f64>,
        td: Option<f64>,
        theta: Option<f64>,
        phi: Option<f64>,
    },
    /// `EXP(v1 v2 [td1 [tau1 [td2 [tau2]]]])`
    Exp {
        v1: f64,
        v2: f64,
        td1: Option<f64>,
        tau1: Option<f64>,
        td2: Option<f64>,
        tau2: Option<f64>,
    },
    /// `PWL(t1 v1 t2 v2 ...)` — piecewise linear.
    Pwl(Vec<(f64, f64)>),
    /// `SFFM(v0 va [fc [fs [md]]])`
    Sffm {
        v0: f64,
        va: f64,
        fc: Option<f64>,
        fs: Option<f64>,
        md: Option<f64>,
    },
    /// `AM(va vo fc fs [td])`
    Am {
        va: f64,
        vo: f64,
        fc: f64,
        fs: f64,
        td: Option<f64>,
    },
}

/// Source specification for voltage/current sources.
///
/// Combines DC value, AC small-signal specification, and transient waveform.
/// All fields are independently optional.
#[derive(Debug, Clone, Default)]
pub struct SourceSpec {
    pub dc: Option<f64>,
    pub ac: Option<AcSpec>,
    pub waveform: Option<Waveform>,
}

/// A resolved parameter binding.
#[derive(Debug, Clone)]
pub struct ResolvedParam {
    pub name: String,
    pub value: Value,
}

/// A user-defined function.
#[derive(Debug, Clone)]
pub struct FuncDef {
    pub name: String,
    pub args: Vec<String>,
    /// The function body as a SPICE-compatible expression string.
    pub body: String,
}

/// A measurement specification (SPICE `.meas`).
///
/// Captures the measurement name, the analysis it applies to, the parsed
/// measurement expression (`expr`), and the verbatim source string (`spec`).
/// `expr` is the canonical form; `spec` is preserved for Netlist round-trip
/// and for diagnostics. When building a `MeasureSpec` programmatically, use
/// [`MeasureSpec::parse`] so the two stay in sync.
#[derive(Debug, Clone)]
pub struct MeasureSpec {
    /// Name of the measurement result (e.g. `"vout_max"`).
    pub name: String,
    /// Analysis type (`"tran"`, `"dc"`, `"ac"`).
    pub analysis_type: String,
    /// The measurement expression verbatim (e.g. `"MAX v(out)"`).
    pub spec: String,
    /// Parsed typed form of `spec`. `None` if parsing failed; the
    /// downstream evaluator will then skip the measurement and report
    /// the original spec string in diagnostics.
    pub expr: Option<MeasureExpr>,
    /// Optional conditional clause (`IF '<expr>'`). When set, the
    /// measurement is only recorded if the arithmetic expression
    /// (resolved against earlier measurement results) evaluates to a
    /// nonzero, finite value. Used to skip dependent measurements
    /// whose predecessor failed.
    pub condition: Option<MeasArith>,
    /// Optional output file (`FILE=<path>`). When set, the measurement
    /// result is appended to the named file as a single line in
    /// `name = value` format, in addition to landing in the
    /// `measurements` plot. Created lazily on first write.
    pub file: Option<String>,
}

impl MeasureSpec {
    /// Build a `MeasureSpec` from `name`/`analysis_type`/`spec`, attempting to
    /// parse `spec` into the typed [`MeasureExpr`] form. A parse failure is
    /// recorded as `expr: None` rather than an error — the simulator's
    /// measurement evaluator treats that as a skipped measurement.
    pub fn parse(
        name: impl Into<String>,
        analysis_type: impl Into<String>,
        spec: impl Into<String>,
    ) -> Self {
        let spec = spec.into();
        // Strip optional IF '<cond>' and FILE=<path> clauses before
        // delegating the rest to `parse_measure_expr`. Both clauses are
        // independent of the measurement kind so they live on the wrapper.
        let (core_spec, condition, file) = strip_meas_clauses(&spec);
        let expr = parse_measure_expr(&core_spec).ok();
        Self {
            name: name.into(),
            analysis_type: analysis_type.into(),
            spec,
            expr,
            condition,
            file,
        }
    }
}

/// Extract optional `IF '<arith>'` and `FILE=<path>` clauses from a
/// `.meas` spec string. Returns the stripped core spec and the parsed
/// clauses. Anything unrecognised is left in the core.
fn strip_meas_clauses(spec: &str) -> (String, Option<MeasArith>, Option<String>) {
    let mut core = spec.to_string();
    let mut condition: Option<MeasArith> = None;
    let mut file: Option<String> = None;

    // FILE=<path> — single token, no quoting in ngspice form.
    if let Some(idx) = find_keyword(&core, "FILE=") {
        let (head, rest) = core.split_at(idx);
        let after_eq = &rest[5..]; // skip "FILE="
        let (path, tail) = match after_eq.find(char::is_whitespace) {
            Some(end) => (&after_eq[..end], &after_eq[end..]),
            None => (after_eq, ""),
        };
        file = Some(path.trim_matches(|c| c == '\'' || c == '"').to_string());
        core = format!("{}{}", head.trim_end(), tail);
    }

    // IF '<arith>' or IF=<arith>. The quoted form is the canonical HSPICE
    // shape; we also accept IF=<arith> for symmetry with PARAM=.
    if let Some(idx) = find_keyword_word(&core, "IF") {
        let (head, rest) = core.split_at(idx);
        let after_if = &rest[2..]; // skip "IF"
        let mut s = after_if.trim_start();
        // Optional '=' between IF and the expression.
        if let Some(stripped) = s.strip_prefix('=') {
            s = stripped.trim_start();
        }
        // Quoted form '...' — pluck the inside.
        let (expr_str, tail) = if let Some(stripped) = s.strip_prefix('\'') {
            match stripped.find('\'') {
                Some(end) => (&stripped[..end], &stripped[end + 1..]),
                None => (stripped, ""), // unterminated — take rest
            }
        } else if let Some(stripped) = s.strip_prefix('"') {
            match stripped.find('"') {
                Some(end) => (&stripped[..end], &stripped[end + 1..]),
                None => (stripped, ""),
            }
        } else {
            // Unquoted — take to end of line.
            (s, "")
        };
        condition = measure_parse::parse_arith(expr_str).ok();
        core = format!("{}{}", head.trim_end(), tail);
    }

    (core.trim().to_string(), condition, file)
}

/// Find a case-insensitive substring at a word boundary (preceded by start
/// or whitespace). Used for `FILE=` etc.
fn find_keyword(s: &str, key: &str) -> Option<usize> {
    let upper = s.to_uppercase();
    let mut from = 0usize;
    while let Some(rel) = upper[from..].find(key) {
        let idx = from + rel;
        let prev_ok = idx == 0 || s.as_bytes()[idx - 1].is_ascii_whitespace();
        if prev_ok {
            return Some(idx);
        }
        from = idx + 1;
    }
    None
}

/// Find a standalone keyword (whitespace on both sides or at start/end).
/// Used for `IF` so we don't match `MID-IFx` style identifiers.
fn find_keyword_word(s: &str, key: &str) -> Option<usize> {
    let upper = s.to_uppercase();
    let bytes = s.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = upper[from..].find(key) {
        let idx = from + rel;
        let end = idx + key.len();
        let prev_ok = idx == 0 || bytes[idx - 1].is_ascii_whitespace();
        let next_ok = end >= bytes.len()
            || bytes[end].is_ascii_whitespace()
            || bytes[end] == b'='
            || bytes[end] == b'\''
            || bytes[end] == b'"';
        if prev_ok && next_ok {
            return Some(idx);
        }
        from = idx + 1;
    }
    None
}

/// Typed form of a `.meas` expression.
///
/// Parsed by [`parse_measure_expr`] from the verbatim spec string. Each
/// variant maps to one of the supported `.meas` keywords.
///
/// `#[non_exhaustive]` — new clauses (ERROR, IF, file-PARAM) may land in
/// any 1.x.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum MeasureExpr {
    /// `MAX | MIN | AVG | RMS | PP` over a vector, optionally bounded.
    Aggregate {
        kind: AggregateKind,
        vec: String,
        from: Option<f64>,
        to: Option<f64>,
    },
    /// `INTEG` — trapezoidal integral, optionally bounded.
    Integ {
        vec: String,
        from: Option<f64>,
        to: Option<f64>,
    },
    /// `FIND` — value of a vector at a sweep point or at a crossing.
    Find { vec: String, at: FindAt },
    /// `WHEN` — sweep value at which a crossing occurs.
    When(CrossingSpec),
    /// `TRIG ... TARG ...` — delay between two crossings or fixed points.
    TrigTarg {
        trig: TrigTargClause,
        targ: TrigTargClause,
    },
    /// `DERIV` — numerical derivative at a sweep point or crossing.
    Deriv { vec: String, at: FindAt },
    /// `PARAM=<expr>` — a constant or arithmetic expression over earlier
    /// measurement results. The expression is parsed but its identifier
    /// resolution is deferred to evaluation time, where the surrounding
    /// `measurements` plot is available for lookup.
    Param(MeasArith),
    /// `ERR{1|2|3} expected_value calc_value [MINVAL=...] [IGNORE=...]`
    /// — compares two signals over the sweep and returns an error
    /// metric. ERR1 = max relative error, ERR2 = max absolute error,
    /// ERR3 = RMS relative error.
    ///
    /// `expected` may be either a constant (e.g. `1.5`) or another
    /// vector (e.g. `v(ref)`). `actual` is always a vector name. The
    /// per-sample relative error uses `max(|expected|, minval)` as the
    /// denominator to avoid blowing up near zero.
    Error {
        kind: ErrorKind,
        expected: ErrorReference,
        actual: String,
        /// Floor for the denominator in relative-error modes (ngspice
        /// `MINVAL=`). Defaults to `1.0e-6` when omitted, matching
        /// ngspice's default.
        minval: Option<f64>,
        /// Samples below this threshold are excluded from the
        /// comparison (ngspice `IGNORE=`/`IGNOR=`).
        ignore: Option<f64>,
    },
}

/// Which error metric `ERR{1|2|3}` computes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// `ERR` / `ERR1`: max over samples of `|actual - expected| /
    /// max(|expected|, MINVAL)`.
    Relative,
    /// `ERR2`: max over samples of `|actual - expected|`.
    Absolute,
    /// `ERR3`: RMS of `|actual - expected| / max(|expected|, MINVAL)`.
    Rms,
}

/// The "expected" side of an `ERR` measurement — either a literal
/// constant or another vector to compare against.
#[derive(Debug, Clone)]
pub enum ErrorReference {
    /// Literal numeric value (e.g. `1.5`).
    Constant(f64),
    /// Vector name resolved at evaluation time (e.g. `v(ref)`).
    Vector(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregateKind {
    Max,
    Min,
    Avg,
    Rms,
    Pp,
}

#[derive(Debug, Clone)]
pub enum FindAt {
    /// `AT=<value>` — fixed sweep point.
    Sweep(f64),
    /// `AT=LAST` — the last sample of the sweep.
    SweepLast,
    /// `WHEN ...` — at the matched crossing of another signal.
    Crossing(CrossingSpec),
}

#[derive(Debug, Clone)]
pub struct CrossingSpec {
    pub signal: String,
    pub threshold: Threshold,
    pub crossing: CrossingKind,
    pub from: Option<f64>,
    pub to: Option<f64>,
}

#[derive(Debug, Clone)]
pub enum Threshold {
    Constant(f64),
    Vector(String),
}

/// Which crossing to report: rising/falling/either, indexed by `occurrence`
/// (1-based) or by `Last` (the final matching crossing in the bounded
/// range).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossingKind {
    Rise(CrossingPick),
    Fall(CrossingPick),
    Cross(CrossingPick),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossingPick {
    /// 1-based occurrence (`RISE=2` → second rising crossing).
    Nth(u32),
    /// `RISE=LAST` / `FALL=LAST` / `CROSS=LAST`.
    Last,
}

#[derive(Debug, Clone)]
pub enum TrigTargClause {
    /// `AT=<value>` — fixed sweep point.
    At(f64),
    /// Signal crossing with optional trigger-delay (`TD=<value>`, honored
    /// on TRIG; ignored on TARG).
    Signal {
        signal: String,
        val: f64,
        crossing: CrossingKind,
        td: Option<f64>,
    },
}

/// Arithmetic over constants and references to other measurement results.
/// Identifiers are resolved at evaluation time against the running
/// `measurements` plot.
#[derive(Debug, Clone)]
pub enum MeasArith {
    Const(f64),
    Ref(String),
    Neg(Box<MeasArith>),
    BinOp(Box<MeasArith>, ArithOp, Box<MeasArith>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
}

/// Errors from parsing a `.meas` spec string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeasParseError {
    /// The spec is empty after tokenization.
    Empty,
    /// The leading keyword was not recognised.
    UnknownKeyword(String),
    /// A clause was structurally invalid (missing val, etc.).
    InvalidClause(String),
    /// Numeric value could not be parsed (e.g. bad SI suffix).
    InvalidNumber(String),
    /// PARAM= arithmetic expression was malformed.
    InvalidArith(String),
}

impl core::fmt::Display for MeasParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => write!(f, "empty measurement spec"),
            Self::UnknownKeyword(k) => write!(f, "unknown measurement keyword `{k}`"),
            Self::InvalidClause(m) => write!(f, "invalid measurement clause: {m}"),
            Self::InvalidNumber(s) => write!(f, "invalid number `{s}`"),
            Self::InvalidArith(m) => write!(f, "invalid PARAM= expression: {m}"),
        }
    }
}

impl std::error::Error for MeasParseError {}

/// Parse a `.meas` spec string into a typed [`MeasureExpr`].
pub fn parse_measure_expr(spec: &str) -> Result<MeasureExpr, MeasParseError> {
    measure_parse::parse(spec)
}

mod measure_parse {
    use super::*;

    pub(super) fn parse(spec: &str) -> Result<MeasureExpr, MeasParseError> {
        // PARAM= is a special form: it's not space-separated like the others,
        // and the expression can contain `=` of its own (e.g. PARAM='a==b').
        // Allow either `PARAM=<expr>` or `PARAM <expr>`.
        let trimmed = spec.trim();
        if let Some(rest) = strip_keyword_eq(trimmed, "PARAM") {
            return Ok(MeasureExpr::Param(parse_arith(rest)?));
        }

        let tokens = tokenize(spec);
        if tokens.is_empty() {
            return Err(MeasParseError::Empty);
        }
        let keyword = tokens[0].to_uppercase();
        let rest = &tokens[1..];

        match keyword.as_str() {
            "MAX" => agg(AggregateKind::Max, rest),
            "MIN" => agg(AggregateKind::Min, rest),
            "AVG" => agg(AggregateKind::Avg, rest),
            "RMS" => agg(AggregateKind::Rms, rest),
            "PP" => agg(AggregateKind::Pp, rest),
            "INTEG" | "INTEGRAL" => integ(rest),
            "FIND" => find(rest),
            "WHEN" => Ok(MeasureExpr::When(parse_crossing_spec(rest)?)),
            "TRIG" => trig_targ(rest),
            "DERIV" => deriv(rest),
            "ERR" | "ERR1" => err(ErrorKind::Relative, rest),
            "ERR2" => err(ErrorKind::Absolute, rest),
            "ERR3" => err(ErrorKind::Rms, rest),
            other => Err(MeasParseError::UnknownKeyword(other.to_string())),
        }
    }

    fn err(kind: ErrorKind, tokens: &[String]) -> Result<MeasureExpr, MeasParseError> {
        if tokens.len() < 2 {
            return Err(MeasParseError::InvalidClause(
                "ERR needs <expected> <actual>".into(),
            ));
        }
        // `expected` may be a literal number or a vector name.
        let expected_tok = &tokens[0];
        let expected = match parse_si_value(expected_tok) {
            Some(v) => ErrorReference::Constant(v),
            None => ErrorReference::Vector(expected_tok.clone()),
        };
        let actual = tokens[1].clone();
        let mut minval: Option<f64> = None;
        let mut ignore: Option<f64> = None;
        for t in &tokens[2..] {
            let up = t.to_uppercase();
            if let Some(v) = up.strip_prefix("MINVAL=") {
                minval = parse_si_value(v);
            } else if let Some(v) = up.strip_prefix("IGNORE=").or(up.strip_prefix("IGNOR=")) {
                ignore = parse_si_value(v);
            }
        }
        Ok(MeasureExpr::Error {
            kind,
            expected,
            actual,
            minval,
            ignore,
        })
    }

    /// Match `<KEY>` followed by `=` (any whitespace allowed), case-insensitive.
    /// Returns the slice after the `=` if found.
    fn strip_keyword_eq<'a>(s: &'a str, key: &str) -> Option<&'a str> {
        let s = s.trim_start();
        if s.len() < key.len() {
            return None;
        }
        let (head, tail) = s.split_at(key.len());
        if !head.eq_ignore_ascii_case(key) {
            return None;
        }
        let after = tail.trim_start();
        after.strip_prefix('=').map(str::trim_start)
    }

    // ---- aggregate / integ / deriv / find ----

    fn agg(kind: AggregateKind, tokens: &[String]) -> Result<MeasureExpr, MeasParseError> {
        let vr = parse_vec_ref_and_range(tokens)?;
        Ok(MeasureExpr::Aggregate {
            kind,
            vec: vr.vec,
            from: vr.from,
            to: vr.to,
        })
    }

    fn integ(tokens: &[String]) -> Result<MeasureExpr, MeasParseError> {
        let vr = parse_vec_ref_and_range(tokens)?;
        Ok(MeasureExpr::Integ {
            vec: vr.vec,
            from: vr.from,
            to: vr.to,
        })
    }

    fn deriv(tokens: &[String]) -> Result<MeasureExpr, MeasParseError> {
        if tokens.is_empty() {
            return Err(MeasParseError::InvalidClause("DERIV needs a vector".into()));
        }
        let vec = tokens[0].clone();
        let at = parse_find_at(&tokens[1..])?;
        Ok(MeasureExpr::Deriv { vec, at })
    }

    fn find(tokens: &[String]) -> Result<MeasureExpr, MeasParseError> {
        if tokens.is_empty() {
            return Err(MeasParseError::InvalidClause("FIND needs a vector".into()));
        }
        let vec = tokens[0].clone();
        let at = parse_find_at(&tokens[1..])?;
        Ok(MeasureExpr::Find { vec, at })
    }

    fn parse_find_at(tokens: &[String]) -> Result<FindAt, MeasParseError> {
        // Look for AT= first; otherwise expect a WHEN clause.
        for token in tokens {
            let upper = token.to_uppercase();
            if let Some(v) = upper.strip_prefix("AT=") {
                if v == "LAST" {
                    return Ok(FindAt::SweepLast);
                }
                let val = parse_si_value(v)
                    .ok_or_else(|| MeasParseError::InvalidNumber(v.to_string()))?;
                return Ok(FindAt::Sweep(val));
            }
        }
        let when_pos = tokens
            .iter()
            .position(|t| t.eq_ignore_ascii_case("WHEN"))
            .ok_or_else(|| MeasParseError::InvalidClause("expected AT= or WHEN clause".into()))?;
        let spec = parse_crossing_spec(&tokens[when_pos + 1..])?;
        Ok(FindAt::Crossing(spec))
    }

    // ---- crossing spec ----

    fn parse_crossing_spec(tokens: &[String]) -> Result<CrossingSpec, MeasParseError> {
        if tokens.is_empty() {
            return Err(MeasParseError::InvalidClause(
                "crossing spec needs a signal=threshold".into(),
            ));
        }
        // First token: "v(out)=0.5" or "v(out)=v(ref)".
        let (signal, thresh_str) = tokens[0].split_once('=').ok_or_else(|| {
            MeasParseError::InvalidClause(format!(
                "expected `<signal>=<threshold>`, got `{}`",
                tokens[0]
            ))
        })?;
        if signal.is_empty() || thresh_str.is_empty() {
            return Err(MeasParseError::InvalidClause(
                "empty side in crossing spec".into(),
            ));
        }
        let threshold = if let Some(val) = parse_si_value(thresh_str) {
            Threshold::Constant(val)
        } else {
            Threshold::Vector(thresh_str.to_string())
        };

        let mut crossing = CrossingKind::Cross(CrossingPick::Nth(1));
        let mut from = None;
        let mut to = None;

        for token in &tokens[1..] {
            let upper = token.to_uppercase();
            if let Some(rest) = upper.strip_prefix("RISE=") {
                crossing = CrossingKind::Rise(parse_pick(rest)?);
            } else if let Some(rest) = upper.strip_prefix("FALL=") {
                crossing = CrossingKind::Fall(parse_pick(rest)?);
            } else if let Some(rest) = upper.strip_prefix("CROSS=") {
                crossing = CrossingKind::Cross(parse_pick(rest)?);
            } else if let Some(v) = upper.strip_prefix("FROM=") {
                from = parse_si_value(v);
            } else if let Some(v) = upper.strip_prefix("TO=") {
                to = parse_si_value(v);
            }
        }

        Ok(CrossingSpec {
            signal: signal.to_string(),
            threshold,
            crossing,
            from,
            to,
        })
    }

    fn parse_pick(s: &str) -> Result<CrossingPick, MeasParseError> {
        if s == "LAST" {
            return Ok(CrossingPick::Last);
        }
        s.parse::<u32>()
            .map(|n| CrossingPick::Nth(n.max(1)))
            .map_err(|_| MeasParseError::InvalidNumber(s.to_string()))
    }

    // ---- TRIG / TARG ----

    fn trig_targ(tokens: &[String]) -> Result<MeasureExpr, MeasParseError> {
        let (trig, consumed) = parse_trig_targ_clause(tokens, /* allow_td */ true)?;
        let rest = &tokens[consumed..];
        let targ_pos = rest
            .iter()
            .position(|t| t.eq_ignore_ascii_case("TARG"))
            .ok_or_else(|| MeasParseError::InvalidClause("TRIG without matching TARG".into()))?;
        let (targ, _) = parse_trig_targ_clause(&rest[targ_pos + 1..], /* allow_td */ false)?;
        Ok(MeasureExpr::TrigTarg { trig, targ })
    }

    fn parse_trig_targ_clause(
        tokens: &[String],
        allow_td: bool,
    ) -> Result<(TrigTargClause, usize), MeasParseError> {
        if tokens.is_empty() {
            return Err(MeasParseError::InvalidClause(
                "expected TRIG/TARG signal or AT=".into(),
            ));
        }
        let upper0 = tokens[0].to_uppercase();
        if let Some(v) = upper0.strip_prefix("AT=") {
            let val =
                parse_si_value(v).ok_or_else(|| MeasParseError::InvalidNumber(v.to_string()))?;
            return Ok((TrigTargClause::At(val), 1));
        }

        let signal = tokens[0].clone();
        let mut val: Option<f64> = None;
        let mut crossing = CrossingKind::Cross(CrossingPick::Nth(1));
        let mut td: Option<f64> = None;
        let mut consumed = 1;

        for token in &tokens[1..] {
            if token.eq_ignore_ascii_case("TARG") {
                break;
            }
            consumed += 1;
            let upper = token.to_uppercase();
            if let Some(v) = upper.strip_prefix("VAL=") {
                val = parse_si_value(v);
            } else if let Some(rest) = upper.strip_prefix("RISE=") {
                crossing = CrossingKind::Rise(parse_pick(rest)?);
            } else if let Some(rest) = upper.strip_prefix("FALL=") {
                crossing = CrossingKind::Fall(parse_pick(rest)?);
            } else if let Some(rest) = upper.strip_prefix("CROSS=") {
                crossing = CrossingKind::Cross(parse_pick(rest)?);
            } else if let Some(v) = upper.strip_prefix("TD=") {
                td = parse_si_value(v);
            }
        }

        let val = val.ok_or_else(|| {
            MeasParseError::InvalidClause(format!("TRIG/TARG clause for `{signal}` missing VAL="))
        })?;

        Ok((
            TrigTargClause::Signal {
                signal,
                val,
                crossing,
                td: if allow_td { td } else { None },
            },
            consumed,
        ))
    }

    // ---- shared helpers ----

    struct VecRefAndRange {
        vec: String,
        from: Option<f64>,
        to: Option<f64>,
    }

    fn parse_vec_ref_and_range(tokens: &[String]) -> Result<VecRefAndRange, MeasParseError> {
        if tokens.is_empty() {
            return Err(MeasParseError::InvalidClause(
                "aggregate needs a vector".into(),
            ));
        }
        let vec = tokens[0].clone();
        let mut from = None;
        let mut to = None;
        let mut i = 1;
        while i < tokens.len() {
            let upper = tokens[i].to_uppercase();
            if let Some(v) = upper.strip_prefix("FROM=") {
                from = parse_si_value(v);
                i += 1;
            } else if let Some(v) = upper.strip_prefix("TO=") {
                to = parse_si_value(v);
                i += 1;
            } else if upper == "FROM" && i + 1 < tokens.len() {
                from = parse_si_value(&tokens[i + 1]);
                i += 2;
            } else if upper == "TO" && i + 1 < tokens.len() {
                to = parse_si_value(&tokens[i + 1]);
                i += 2;
            } else {
                break;
            }
        }
        Ok(VecRefAndRange { vec, from, to })
    }

    fn tokenize(spec: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut depth = 0u32;
        for ch in spec.chars() {
            match ch {
                '(' => {
                    depth += 1;
                    cur.push(ch);
                }
                ')' => {
                    depth = depth.saturating_sub(1);
                    cur.push(ch);
                }
                ' ' | '\t' if depth == 0 => {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                }
                _ => cur.push(ch),
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }

    pub(super) fn parse_si_value(s: &str) -> Option<f64> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        if let Ok(v) = s.parse::<f64>() {
            return Some(v);
        }
        let suffixes: &[(&str, f64)] = &[
            ("meg", 1e6),
            ("t", 1e12),
            ("g", 1e9),
            ("k", 1e3),
            ("m", 1e-3),
            ("u", 1e-6),
            ("n", 1e-9),
            ("p", 1e-12),
            ("f", 1e-15),
            ("a", 1e-18),
        ];
        let lower = s.to_lowercase();
        for &(suffix, mult) in suffixes {
            if let Some(num_str) = lower.strip_suffix(suffix)
                && let Ok(v) = num_str.parse::<f64>()
            {
                return Some(v * mult);
            }
        }
        None
    }

    // ---- PARAM arithmetic parser ----
    //
    // Grammar:
    //   expr   ::= term (('+' | '-') term)*
    //   term   ::= factor (('*' | '/') factor)*
    //   factor ::= ('-' | '+') factor | primary
    //   primary ::= number | identifier | '(' expr ')'

    pub(super) fn parse_arith(input: &str) -> Result<MeasArith, MeasParseError> {
        let mut p = ArithParser::new(input);
        let e = p.parse_expr()?;
        p.skip_ws();
        if !p.eof() {
            return Err(MeasParseError::InvalidArith(format!(
                "trailing input at position {}",
                p.pos
            )));
        }
        Ok(e)
    }

    struct ArithParser<'a> {
        src: &'a [u8],
        pos: usize,
    }

    impl<'a> ArithParser<'a> {
        fn new(input: &'a str) -> Self {
            Self {
                src: input.as_bytes(),
                pos: 0,
            }
        }

        fn eof(&self) -> bool {
            self.pos >= self.src.len()
        }

        fn skip_ws(&mut self) {
            while self.pos < self.src.len()
                && (self.src[self.pos] == b' ' || self.src[self.pos] == b'\t')
            {
                self.pos += 1;
            }
        }

        fn peek(&self) -> Option<u8> {
            self.src.get(self.pos).copied()
        }

        fn consume(&mut self, c: u8) -> bool {
            self.skip_ws();
            if self.peek() == Some(c) {
                self.pos += 1;
                true
            } else {
                false
            }
        }

        fn parse_expr(&mut self) -> Result<MeasArith, MeasParseError> {
            let mut lhs = self.parse_term()?;
            loop {
                self.skip_ws();
                let op = match self.peek() {
                    Some(b'+') => ArithOp::Add,
                    Some(b'-') => ArithOp::Sub,
                    _ => break,
                };
                self.pos += 1;
                let rhs = self.parse_term()?;
                lhs = MeasArith::BinOp(Box::new(lhs), op, Box::new(rhs));
            }
            Ok(lhs)
        }

        fn parse_term(&mut self) -> Result<MeasArith, MeasParseError> {
            let mut lhs = self.parse_factor()?;
            loop {
                self.skip_ws();
                let op = match self.peek() {
                    Some(b'*') => ArithOp::Mul,
                    Some(b'/') => ArithOp::Div,
                    _ => break,
                };
                self.pos += 1;
                let rhs = self.parse_factor()?;
                lhs = MeasArith::BinOp(Box::new(lhs), op, Box::new(rhs));
            }
            Ok(lhs)
        }

        fn parse_factor(&mut self) -> Result<MeasArith, MeasParseError> {
            self.skip_ws();
            match self.peek() {
                Some(b'-') => {
                    self.pos += 1;
                    Ok(MeasArith::Neg(Box::new(self.parse_factor()?)))
                }
                Some(b'+') => {
                    self.pos += 1;
                    self.parse_factor()
                }
                _ => self.parse_primary(),
            }
        }

        fn parse_primary(&mut self) -> Result<MeasArith, MeasParseError> {
            self.skip_ws();
            if self.consume(b'(') {
                let e = self.parse_expr()?;
                if !self.consume(b')') {
                    return Err(MeasParseError::InvalidArith("missing `)`".into()));
                }
                return Ok(e);
            }
            let start = self.pos;
            let first = self
                .peek()
                .ok_or_else(|| MeasParseError::InvalidArith("unexpected end of input".into()))?;
            if first.is_ascii_digit() || first == b'.' {
                // Number literal (with optional SI suffix in trailing alpha run).
                while let Some(b) = self.peek() {
                    if b.is_ascii_digit()
                        || b == b'.'
                        || b == b'e'
                        || b == b'E'
                        || b == b'+'
                        || b == b'-'
                    {
                        // For exponent signs, only consume if preceded by e/E
                        if (b == b'+' || b == b'-')
                            && (self.pos == start
                                || (self.src[self.pos - 1] != b'e'
                                    && self.src[self.pos - 1] != b'E'))
                        {
                            break;
                        }
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                // Optional SI suffix (letters immediately following the number).
                let num_end = self.pos;
                while let Some(b) = self.peek() {
                    if b.is_ascii_alphabetic() {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                let slice = std::str::from_utf8(&self.src[start..self.pos])
                    .map_err(|_| MeasParseError::InvalidArith("non-utf8 input".into()))?;
                if let Some(v) = parse_si_value(slice) {
                    Ok(MeasArith::Const(v))
                } else {
                    // Didn't parse as SI — back up to num_end and try plain f64.
                    let bare = std::str::from_utf8(&self.src[start..num_end]).unwrap_or("");
                    let v = bare
                        .parse::<f64>()
                        .map_err(|_| MeasParseError::InvalidNumber(slice.to_string()))?;
                    self.pos = num_end;
                    Ok(MeasArith::Const(v))
                }
            } else if first.is_ascii_alphabetic() || first == b'_' {
                while let Some(b) = self.peek() {
                    if b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'(' {
                        // Stop at a function-call boundary; we don't support calls here.
                        if b == b'(' {
                            break;
                        }
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                let slice = std::str::from_utf8(&self.src[start..self.pos]).unwrap_or("");
                Ok(MeasArith::Ref(slice.to_string()))
            } else {
                Err(MeasParseError::InvalidArith(format!(
                    "unexpected character `{}` at position {}",
                    first as char, self.pos
                )))
            }
        }
    }
}

/// A resolved analysis command.
///
/// `#[non_exhaustive]` — new analyses (e.g. `.disto`) may land in any
/// 1.x release without a major bump. External callers must include a
/// catch-all arm.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Analysis {
    Op,
    Dc(DcAnalysis),
    Ac(AcAnalysis),
    Tran(TranAnalysis),
    Noise(NoiseAnalysis),
    Pz(PzAnalysis),
    Sens(SensAnalysis),
    Tf(TfAnalysis),
    /// `.four <freq> <vec> [<vec> ...]` — Fourier post-processing of a
    /// transient simulation.
    Four(FourAnalysis),
    /// `.fft <vec> [<vec> ...] [opts]` — windowed FFT of transient output.
    Fft(FftAnalysis),
}

#[derive(Debug, Clone)]
pub struct DcAnalysis {
    pub sweeps: Vec<DcSweep>,
}

#[derive(Debug, Clone)]
pub struct DcSweep {
    pub source: Id,
    pub start: f64,
    pub stop: f64,
    pub step: f64,
}

#[derive(Debug, Clone)]
pub struct AcAnalysis {
    pub start: f64,
    pub stop: f64,
    pub points: u32,
    pub scale: FrequencyScale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrequencyScale {
    Decade,
    Octave,
    Linear,
}

#[derive(Debug, Clone)]
pub struct TranAnalysis {
    pub step: f64,
    pub stop: f64,
    pub start: f64,
    pub uic: bool,
    /// Maximum internal timestep. `None` means the solver picks automatically.
    pub tmax: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct NoiseAnalysis {
    pub output_net: Id,
    pub reference_net: Id,
    pub source: Id,
    pub start: f64,
    pub stop: f64,
    pub points: u32,
    pub scale: FrequencyScale,
}

#[derive(Debug, Clone)]
pub struct PzAnalysis {
    pub input_pos: Id,
    pub input_neg: Id,
    pub output_pos: Id,
    pub output_neg: Id,
    pub transfer: TransferType,
    pub analysis_type: PzType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferType {
    Voltage,
    Current,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PzType {
    Poles,
    Zeros,
    Both,
}

#[derive(Debug, Clone)]
pub struct SensAnalysis {
    /// The output expression being differentiated (e.g. `"v(out)"`, `"v(5,4)"`,
    /// `"ix(...)"`). Single SPICE token preserved verbatim.
    pub output: String,
    /// AC sensitivity sweep parameters. `None` for DC sensitivity.
    pub ac: Option<SensAcSpec>,
}

#[derive(Debug, Clone)]
pub struct SensAcSpec {
    pub scale: FrequencyScale,
    pub points: u32,
    pub fstart: f64,
    pub fstop: f64,
}

#[derive(Debug, Clone)]
pub struct TfAnalysis {
    pub output: String,
    pub source: Id,
}

/// `.four <freq> <vec> [<vec> ...]` — Fourier post-processing.
///
/// Operates on the result of the preceding `.tran` simulation: it isolates
/// the final `1/fundamental` window of the transient response and computes
/// the DC component plus harmonic magnitudes/phases up to `num_harmonics`
/// (ngspice's default is 9, controllable via the `nfreqs` option).
#[derive(Debug, Clone)]
pub struct FourAnalysis {
    /// Fundamental frequency in Hz.
    pub fundamental: f64,
    /// Vector expressions to analyse (e.g. `"v(out)"`, `"i(vsense)"`).
    pub vectors: Vec<String>,
    /// Number of harmonics to report (DC excluded). Defaults to 9.
    pub num_harmonics: usize,
}

/// Window function for `.fft`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FftWindow {
    Rectangular,
    Hann,
    Hamming,
    Blackman,
    Bartlett,
}

/// Whether `.fft` outputs magnitudes (real) or complex spectra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FftFormat {
    /// Magnitudes only.
    Magnitude,
    /// Complex re/im pairs.
    Complex,
}

/// `.fft <vec> [<vec> ...] [start=... stop=... npoints=... window=... format=...]`.
#[derive(Debug, Clone)]
pub struct FftAnalysis {
    pub vectors: Vec<String>,
    /// Window start time in seconds. `None` → use the transient `tstart`.
    pub start: Option<f64>,
    /// Window stop time in seconds. `None` → use the transient `tstop`.
    pub stop: Option<f64>,
    /// Requested point count. Rounded up to the next power of two for the
    /// radix-2 FFT. Defaults to 1024.
    pub npoints: usize,
    pub window: FftWindow,
    pub format: FftFormat,
}

#[cfg(test)]
mod measure_tests {
    use super::*;

    fn parse(spec: &str) -> MeasureExpr {
        parse_measure_expr(spec).expect("should parse")
    }

    #[test]
    fn parses_max() {
        match parse("MAX v(out)") {
            MeasureExpr::Aggregate {
                kind,
                vec,
                from,
                to,
            } => {
                assert_eq!(kind, AggregateKind::Max);
                assert_eq!(vec, "v(out)");
                assert!(from.is_none() && to.is_none());
            }
            other => panic!("expected Aggregate, got {other:?}"),
        }
    }

    #[test]
    fn parses_max_with_range() {
        match parse("MAX v(out) FROM=1u TO=5u") {
            MeasureExpr::Aggregate { from, to, .. } => {
                assert!((from.unwrap() - 1e-6).abs() < 1e-18);
                assert!((to.unwrap() - 5e-6).abs() < 1e-18);
            }
            other => panic!("expected Aggregate, got {other:?}"),
        }
    }

    #[test]
    fn parses_find_at() {
        match parse("FIND v(out) AT=1.5") {
            MeasureExpr::Find {
                vec,
                at: FindAt::Sweep(t),
            } => {
                assert_eq!(vec, "v(out)");
                assert!((t - 1.5).abs() < 1e-15);
            }
            other => panic!("expected Find(Sweep), got {other:?}"),
        }
    }

    #[test]
    fn parses_find_at_last() {
        match parse("FIND v(out) AT=LAST") {
            MeasureExpr::Find {
                at: FindAt::SweepLast,
                ..
            } => {}
            other => panic!("expected Find(SweepLast), got {other:?}"),
        }
    }

    #[test]
    fn parses_when_rise_last() {
        // LAST as the occurrence qualifier on a crossing.
        match parse("WHEN v(out)=0.5 RISE=LAST") {
            MeasureExpr::When(spec) => {
                assert_eq!(spec.signal, "v(out)");
                assert!(matches!(spec.threshold, Threshold::Constant(_)));
                assert_eq!(spec.crossing, CrossingKind::Rise(CrossingPick::Last));
            }
            other => panic!("expected When, got {other:?}"),
        }
    }

    #[test]
    fn parses_when_two_signals() {
        match parse("WHEN v(a)=v(b)") {
            MeasureExpr::When(spec) => {
                assert_eq!(spec.signal, "v(a)");
                match spec.threshold {
                    Threshold::Vector(v) => assert_eq!(v, "v(b)"),
                    _ => panic!("expected Vector threshold"),
                }
            }
            other => panic!("expected When, got {other:?}"),
        }
    }

    #[test]
    fn parses_trig_targ_with_td() {
        match parse("TRIG v(in) VAL=0.5 RISE=1 TD=1n TARG v(out) VAL=0.5 RISE=1") {
            MeasureExpr::TrigTarg { trig, targ } => {
                match trig {
                    TrigTargClause::Signal {
                        signal,
                        val,
                        crossing,
                        td,
                    } => {
                        assert_eq!(signal, "v(in)");
                        assert!((val - 0.5).abs() < 1e-12);
                        assert_eq!(crossing, CrossingKind::Rise(CrossingPick::Nth(1)));
                        assert!((td.unwrap() - 1e-9).abs() < 1e-18);
                    }
                    _ => panic!("expected Signal TRIG"),
                }
                // TD on TARG must be dropped.
                match targ {
                    TrigTargClause::Signal { td, .. } => assert!(td.is_none()),
                    _ => panic!("expected Signal TARG"),
                }
            }
            other => panic!("expected TrigTarg, got {other:?}"),
        }
    }

    #[test]
    fn parses_trig_at_targ_signal() {
        match parse("TRIG AT=1.0 TARG v(out) VAL=0.5 RISE=1") {
            MeasureExpr::TrigTarg { trig, targ } => {
                assert!(matches!(trig, TrigTargClause::At(_)));
                assert!(matches!(targ, TrigTargClause::Signal { .. }));
            }
            other => panic!("expected TrigTarg, got {other:?}"),
        }
    }

    #[test]
    fn parses_param_constant() {
        match parse("PARAM=42") {
            MeasureExpr::Param(MeasArith::Const(v)) => assert!((v - 42.0).abs() < 1e-15),
            other => panic!("expected Param(Const), got {other:?}"),
        }
    }

    #[test]
    fn parses_param_ref() {
        match parse("PARAM=vout_max") {
            MeasureExpr::Param(MeasArith::Ref(name)) => assert_eq!(name, "vout_max"),
            other => panic!("expected Param(Ref), got {other:?}"),
        }
    }

    #[test]
    fn parses_param_arith() {
        // (vout_max - vout_min) * 2
        let e = parse("PARAM=(vout_max - vout_min) * 2");
        let s = format!("{e:?}");
        assert!(s.contains("Mul"));
        assert!(s.contains("Sub"));
        assert!(s.contains("vout_max"));
        assert!(s.contains("vout_min"));
    }

    #[test]
    fn parses_param_with_si_constant() {
        match parse("PARAM=1k") {
            MeasureExpr::Param(MeasArith::Const(v)) => assert!((v - 1000.0).abs() < 1e-9),
            other => panic!("expected Param(Const), got {other:?}"),
        }
    }

    #[test]
    fn parses_deriv_when() {
        match parse("DERIV v(out) WHEN v(clk)=0.5 RISE=1") {
            MeasureExpr::Deriv {
                vec,
                at: FindAt::Crossing(_),
            } => assert_eq!(vec, "v(out)"),
            other => panic!("expected Deriv(Crossing), got {other:?}"),
        }
    }

    #[test]
    fn unknown_keyword_errors() {
        assert!(parse_measure_expr("BOGUS v(out)").is_err());
    }

    #[test]
    fn trig_without_targ_errors() {
        assert!(parse_measure_expr("TRIG v(out) VAL=0.5").is_err());
    }

    #[test]
    fn measure_spec_parse_helper() {
        let m = MeasureSpec::parse("vmax", "tran", "MAX v(out)");
        assert!(m.expr.is_some());
        assert_eq!(m.spec, "MAX v(out)");
        let bad = MeasureSpec::parse("x", "tran", "BOGUS");
        assert!(bad.expr.is_none());
    }
}
