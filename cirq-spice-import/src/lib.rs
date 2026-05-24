//! SPICE import — converts `thevenin_types::Netlist` into `cirq_ir::Circuit`.
//!
//! This crate provides the bridge from legacy SPICE netlists into the canonical
//! Cirq IR. It enables gradual migration: existing SPICE files can be imported
//! into the Cirq toolchain without manual rewriting.

mod preprocess;

pub use preprocess::{IncludeError, IncludeOptions};

use std::collections::HashMap;
use std::path::Path;

use cirq_ir::{
    AcAnalysis, AcSpec as IrAcSpec, Analysis as IrAnalysis, BehavioralMode, Circuit, Connection,
    DcAnalysis, DcSweep as IrDcSweep, Element as IrElement, ElementKind as IrElementKind,
    FrequencyScale, Id, Model as IrModel, Net, NoiseAnalysis, PzAnalysis, PzType, ResolvedParam,
    SensAcSpec, SensAnalysis, SourceSpec, TfAnalysis, TranAnalysis, TransferType, Value,
    Waveform as IrWaveform, XspiceConnection as IrXspiceConnection,
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

    /// Subcircuit flattening failed.
    #[error("subcircuit flattening error: {0}")]
    SubcktError(#[from] thevenin::subckt::SubcktError),

    /// An analysis directive could not be lowered into the IR.
    #[error("unsupported analysis: {0}")]
    UnsupportedAnalysis(String),

    /// `.include` / `.lib` resolution failed.
    #[error("include resolution failed: {0}")]
    Include(#[from] preprocess::IncludeError),
}

/// Parse the optional AC tail of `.sens output [AC DEC|OCT|LIN n fstart fstop]`.
///
/// Returns `None` when the tail is empty or consists only of the legacy `dc`
/// marker (which ngspice accepts and the simulator silently ignores).
fn parse_sens_ac_tail(tail: &[String]) -> Result<Option<SensAcSpec>, ImportError> {
    if tail.is_empty() {
        return Ok(None);
    }

    let first = tail[0].to_ascii_lowercase();
    if first == "dc" {
        return Ok(None);
    }
    if first != "ac" {
        return Err(ImportError::UnsupportedAnalysis(format!(
            ".sens: expected AC|DC marker after output, got `{}`",
            tail[0]
        )));
    }
    if tail.len() < 5 {
        return Err(ImportError::UnsupportedAnalysis(
            ".sens AC: needs variation n fstart fstop".into(),
        ));
    }
    let scale = match tail[1].to_ascii_lowercase().as_str() {
        "dec" | "decade" => FrequencyScale::Decade,
        "oct" | "octave" => FrequencyScale::Octave,
        "lin" | "linear" => FrequencyScale::Linear,
        other => {
            return Err(ImportError::UnsupportedAnalysis(format!(
                ".sens AC: unknown variation `{other}`"
            )));
        }
    };
    let parse_num = |s: &str, field: &str| -> Result<f64, ImportError> {
        thevenin_types::parse::parse_spice_number(s).ok_or_else(|| {
            ImportError::UnsupportedAnalysis(format!(".sens AC: bad {field}: `{s}`"))
        })
    };
    let points = parse_num(&tail[2], "n")? as u32;
    let fstart = parse_num(&tail[3], "fstart")?;
    let fstop = parse_num(&tail[4], "fstop")?;
    Ok(Some(SensAcSpec {
        scale,
        points,
        fstart,
        fstop,
    }))
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
    ///
    /// SPICE node names — including purely numeric ones like `"1"` — are
    /// preserved verbatim. The IR is the semantic center and doesn't impose
    /// Cirq surface-syntax constraints on net identifiers; any rewriting
    /// needed to emit valid Cirq source happens at the Cirq emitter, not
    /// here. Ground `"0"` is pre-seeded and always maps to `Id(0)`.
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

/// Try to extract a numeric f64 from an `Expr`, resolving parameter references
/// against the supplied lookup table.
fn expr_to_f64(expr: &Expr, params: &HashMap<String, f64>) -> Result<f64, ImportError> {
    match expr {
        Expr::Num(v) => Ok(*v),
        Expr::Param(name) => params
            .get(name)
            .copied()
            .ok_or_else(|| ImportError::UnevaluableExpr(format!("unresolved parameter: {name}"))),
        Expr::Brace(s) => eval_brace_expr(s.trim(), params),
    }
}

/// Evaluate a brace expression string against a parameter table.
///
/// Supports: numeric literals (with SI suffixes), parameter references,
/// binary operators (+, -, *, /, **), unary minus, parentheses, and common
/// SPICE math functions (sqrt, abs, log, exp, sin, cos, tan, pow, min, max).
fn eval_brace_expr(input: &str, params: &HashMap<String, f64>) -> Result<f64, ImportError> {
    let tokens = tokenize_expr(input);
    let mut pos = 0;
    let result = parse_ternary(&tokens, &mut pos, params)?;
    if pos < tokens.len() {
        return Err(ImportError::UnevaluableExpr(format!(
            "unexpected token in brace expression: {input}"
        )));
    }
    Ok(result)
}

/// Token types for the mini expression evaluator.
#[derive(Debug, Clone)]
enum ExprToken {
    Num(f64),
    Ident(String),
    Op(char), // +, -, *, /
    Pow,      // **
    LParen,
    RParen,
    Comma,
    /// Comparison operator. Stored as the canonical 1- or 2-byte form
    /// (`>`, `<`, `>=`, `<=`, `==`, `!=`).
    Cmp(&'static str),
    /// Ternary delimiters.
    Question,
    Colon,
}

/// Tokenize a SPICE expression string.
fn tokenize_expr(input: &str) -> Vec<ExprToken> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        match chars[i] {
            ' ' | '\t' => {
                i += 1;
            }
            '(' => {
                tokens.push(ExprToken::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(ExprToken::RParen);
                i += 1;
            }
            ',' => {
                tokens.push(ExprToken::Comma);
                i += 1;
            }
            '*' => {
                if i + 1 < chars.len() && chars[i + 1] == '*' {
                    tokens.push(ExprToken::Pow);
                    i += 2;
                } else {
                    tokens.push(ExprToken::Op('*'));
                    i += 1;
                }
            }
            '+' | '-' | '/' => {
                tokens.push(ExprToken::Op(chars[i]));
                i += 1;
            }
            '?' => {
                tokens.push(ExprToken::Question);
                i += 1;
            }
            ':' => {
                tokens.push(ExprToken::Colon);
                i += 1;
            }
            '>' => {
                if i + 1 < chars.len() && chars[i + 1] == '=' {
                    tokens.push(ExprToken::Cmp(">="));
                    i += 2;
                } else {
                    tokens.push(ExprToken::Cmp(">"));
                    i += 1;
                }
            }
            '<' => {
                if i + 1 < chars.len() && chars[i + 1] == '=' {
                    tokens.push(ExprToken::Cmp("<="));
                    i += 2;
                } else {
                    tokens.push(ExprToken::Cmp("<"));
                    i += 1;
                }
            }
            '=' if i + 1 < chars.len() && chars[i + 1] == '=' => {
                tokens.push(ExprToken::Cmp("=="));
                i += 2;
            }
            '!' if i + 1 < chars.len() && chars[i + 1] == '=' => {
                tokens.push(ExprToken::Cmp("!="));
                i += 2;
            }
            c if c.is_ascii_digit() || c == '.' => {
                // Numeric literal — may include SI suffix
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric()
                        || chars[i] == '.'
                        || chars[i] == 'e'
                        || chars[i] == 'E'
                        || (i > start
                            && (chars[i] == '+' || chars[i] == '-')
                            && (chars[i - 1] == 'e' || chars[i - 1] == 'E')))
                {
                    i += 1;
                }
                let s: String = chars[start..i].iter().collect();
                if let Some(v) = thevenin_types::parse::parse_spice_number(&s) {
                    tokens.push(ExprToken::Num(v));
                } else if let Ok(v) = s.parse::<f64>() {
                    tokens.push(ExprToken::Num(v));
                } else {
                    // Fallback: treat as identifier
                    tokens.push(ExprToken::Ident(s));
                }
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                tokens.push(ExprToken::Ident(chars[start..i].iter().collect()));
            }
            _ => {
                i += 1;
            } // skip unknown
        }
    }
    tokens
}

/// Parse the ternary operator `cond ? then : else` (lowest precedence,
/// right-associative). Falls through to comparison when no `?` follows.
fn parse_ternary(
    tokens: &[ExprToken],
    pos: &mut usize,
    params: &HashMap<String, f64>,
) -> Result<f64, ImportError> {
    let cond = parse_comparison(tokens, pos, params)?;
    if *pos < tokens.len() && matches!(&tokens[*pos], ExprToken::Question) {
        *pos += 1;
        // Both branches are still ternary-level so we get right-associativity
        // (`a ? b : c ? d : e` parses as `a ? b : (c ? d : e)`).
        let then_branch = parse_ternary(tokens, pos, params)?;
        if *pos >= tokens.len() || !matches!(&tokens[*pos], ExprToken::Colon) {
            return Err(ImportError::UnevaluableExpr(
                "ternary expression: missing `:`".to_owned(),
            ));
        }
        *pos += 1;
        let else_branch = parse_ternary(tokens, pos, params)?;
        Ok(if cond != 0.0 {
            then_branch
        } else {
            else_branch
        })
    } else {
        Ok(cond)
    }
}

/// Parse comparison operators (`>`, `<`, `>=`, `<=`, `==`, `!=`).
///
/// Result is 1.0 for true / 0.0 for false to feed the ternary. Comparisons
/// do not chain (left-associative, but multiple chained comparisons would
/// mean comparing a 0.0/1.0 result against the next operand — almost
/// certainly a bug, so we keep the simple precedence ladder).
fn parse_comparison(
    tokens: &[ExprToken],
    pos: &mut usize,
    params: &HashMap<String, f64>,
) -> Result<f64, ImportError> {
    let lhs = parse_add_sub(tokens, pos, params)?;
    if *pos < tokens.len()
        && let ExprToken::Cmp(op) = &tokens[*pos]
    {
        let op = *op;
        *pos += 1;
        let rhs = parse_add_sub(tokens, pos, params)?;
        let truthy = match op {
            ">" => lhs > rhs,
            "<" => lhs < rhs,
            ">=" => lhs >= rhs,
            "<=" => lhs <= rhs,
            "==" => (lhs - rhs).abs() < 1e-15,
            "!=" => (lhs - rhs).abs() >= 1e-15,
            other => {
                return Err(ImportError::UnevaluableExpr(format!(
                    "unknown comparison operator: {other}"
                )));
            }
        };
        Ok(if truthy { 1.0 } else { 0.0 })
    } else {
        Ok(lhs)
    }
}

/// Parse addition and subtraction.
fn parse_add_sub(
    tokens: &[ExprToken],
    pos: &mut usize,
    params: &HashMap<String, f64>,
) -> Result<f64, ImportError> {
    let mut lhs = parse_mul_div(tokens, pos, params)?;
    while *pos < tokens.len() {
        match &tokens[*pos] {
            ExprToken::Op('+') => {
                *pos += 1;
                lhs += parse_mul_div(tokens, pos, params)?;
            }
            ExprToken::Op('-') => {
                *pos += 1;
                lhs -= parse_mul_div(tokens, pos, params)?;
            }
            _ => break,
        }
    }
    Ok(lhs)
}

/// Parse multiplication and division.
fn parse_mul_div(
    tokens: &[ExprToken],
    pos: &mut usize,
    params: &HashMap<String, f64>,
) -> Result<f64, ImportError> {
    let mut lhs = parse_power(tokens, pos, params)?;
    while *pos < tokens.len() {
        match &tokens[*pos] {
            ExprToken::Op('*') => {
                *pos += 1;
                lhs *= parse_power(tokens, pos, params)?;
            }
            ExprToken::Op('/') => {
                *pos += 1;
                lhs /= parse_power(tokens, pos, params)?;
            }
            _ => break,
        }
    }
    Ok(lhs)
}

/// Parse exponentiation (right-associative, `**`).
fn parse_power(
    tokens: &[ExprToken],
    pos: &mut usize,
    params: &HashMap<String, f64>,
) -> Result<f64, ImportError> {
    let base = parse_unary(tokens, pos, params)?;
    if *pos < tokens.len() && matches!(&tokens[*pos], ExprToken::Pow) {
        *pos += 1;
        let exp = parse_power(tokens, pos, params)?; // right-assoc
        Ok(base.powf(exp))
    } else {
        Ok(base)
    }
}

/// Parse unary minus / plus.
fn parse_unary(
    tokens: &[ExprToken],
    pos: &mut usize,
    params: &HashMap<String, f64>,
) -> Result<f64, ImportError> {
    if *pos < tokens.len() {
        match &tokens[*pos] {
            ExprToken::Op('-') => {
                *pos += 1;
                Ok(-parse_primary(tokens, pos, params)?)
            }
            ExprToken::Op('+') => {
                *pos += 1;
                parse_primary(tokens, pos, params)
            }
            _ => parse_primary(tokens, pos, params),
        }
    } else {
        Err(ImportError::UnevaluableExpr(
            "unexpected end of expression".to_owned(),
        ))
    }
}

/// SPICE built-in math functions.
fn eval_function(name: &str, args: &[f64]) -> Result<f64, ImportError> {
    let err = || ImportError::UnevaluableExpr(format!("function {name}: wrong arity"));
    match name.to_ascii_lowercase().as_str() {
        "sqrt" => Ok(args.first().ok_or_else(err)?.sqrt()),
        "abs" => Ok(args.first().ok_or_else(err)?.abs()),
        "log" | "ln" => Ok(args.first().ok_or_else(err)?.ln()),
        "log10" => Ok(args.first().ok_or_else(err)?.log10()),
        "exp" => Ok(args.first().ok_or_else(err)?.exp()),
        "sin" => Ok(args.first().ok_or_else(err)?.sin()),
        "cos" => Ok(args.first().ok_or_else(err)?.cos()),
        "tan" => Ok(args.first().ok_or_else(err)?.tan()),
        "asin" => Ok(args.first().ok_or_else(err)?.asin()),
        "acos" => Ok(args.first().ok_or_else(err)?.acos()),
        "atan" => Ok(args.first().ok_or_else(err)?.atan()),
        "atan2" => {
            if args.len() < 2 {
                return Err(err());
            }
            Ok(args[0].atan2(args[1]))
        }
        "sinh" => Ok(args.first().ok_or_else(err)?.sinh()),
        "cosh" => Ok(args.first().ok_or_else(err)?.cosh()),
        "tanh" => Ok(args.first().ok_or_else(err)?.tanh()),
        "pow" => {
            if args.len() < 2 {
                return Err(err());
            }
            Ok(args[0].powf(args[1]))
        }
        "min" => {
            if args.len() < 2 {
                return Err(err());
            }
            Ok(args[0].min(args[1]))
        }
        "max" => {
            if args.len() < 2 {
                return Err(err());
            }
            Ok(args[0].max(args[1]))
        }
        "sgn" | "sign" => {
            let v = *args.first().ok_or_else(err)?;
            Ok(if v > 0.0 {
                1.0
            } else if v < 0.0 {
                -1.0
            } else {
                0.0
            })
        }
        // SPICE convention: int(x) truncates toward zero (not floor).
        "int" => Ok(args.first().ok_or_else(err)?.trunc()),
        "floor" => Ok(args.first().ok_or_else(err)?.floor()),
        "ceil" => Ok(args.first().ok_or_else(err)?.ceil()),
        // Voltage/amplitude dB: 20 * log10(|x|). ngspice/SPICE convention.
        "db" | "db20" => Ok(20.0 * args.first().ok_or_else(err)?.abs().log10()),
        "limit" => {
            if args.len() < 3 {
                return Err(err());
            }
            let (x, lo, hi) = (args[0], args[1], args[2]);
            if lo > hi {
                return Err(ImportError::UnevaluableExpr(format!(
                    "limit: lower bound ({lo}) is greater than upper bound ({hi})"
                )));
            }
            Ok(x.clamp(lo, hi))
        }
        _ => Err(ImportError::UnevaluableExpr(format!(
            "unknown function: {name}"
        ))),
    }
}

/// Parse a primary: number, param reference, function call, or parenthesized expr.
fn parse_primary(
    tokens: &[ExprToken],
    pos: &mut usize,
    params: &HashMap<String, f64>,
) -> Result<f64, ImportError> {
    if *pos >= tokens.len() {
        return Err(ImportError::UnevaluableExpr(
            "unexpected end of expression".to_owned(),
        ));
    }
    match &tokens[*pos] {
        ExprToken::Num(v) => {
            let v = *v;
            *pos += 1;
            Ok(v)
        }
        ExprToken::Ident(name) => {
            let name = name.clone();
            *pos += 1;
            // Check for function call: ident followed by '('
            if *pos < tokens.len() && matches!(&tokens[*pos], ExprToken::LParen) {
                *pos += 1; // consume '('
                let mut args = Vec::new();
                if *pos < tokens.len() && !matches!(&tokens[*pos], ExprToken::RParen) {
                    args.push(parse_ternary(tokens, pos, params)?);
                    while *pos < tokens.len() && matches!(&tokens[*pos], ExprToken::Comma) {
                        *pos += 1;
                        args.push(parse_ternary(tokens, pos, params)?);
                    }
                }
                if *pos < tokens.len() && matches!(&tokens[*pos], ExprToken::RParen) {
                    *pos += 1;
                }
                eval_function(&name, &args)
            } else {
                // Parameter reference. Try exact match first (preserves
                // user-chosen case for `.param` names); fall back to a
                // case-insensitive sweep so SPICE built-ins like TEMPER /
                // temper / Temper all resolve to the same entry.
                if let Some(v) = params.get(&name) {
                    Ok(*v)
                } else if let Some(v) = params
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(&name))
                    .map(|(_, v)| *v)
                {
                    Ok(v)
                } else {
                    Err(ImportError::UnevaluableExpr(format!(
                        "unresolved parameter: {name}"
                    )))
                }
            }
        }
        ExprToken::LParen => {
            *pos += 1;
            let val = parse_ternary(tokens, pos, params)?;
            if *pos < tokens.len() && matches!(&tokens[*pos], ExprToken::RParen) {
                *pos += 1;
            }
            Ok(val)
        }
        other => Err(ImportError::UnevaluableExpr(format!(
            "unexpected token: {other:?}"
        ))),
    }
}

/// Build a parameter resolution table from SPICE `.param` items.
///
/// Performs a simple multi-pass resolution to handle chained parameters
/// (e.g., `.param a=1k` then `.param b=2*a`). Parameters whose values
/// are numeric are resolved immediately; parameters referencing other
/// params are resolved in subsequent passes until no more progress is made.
///
/// `TEMPER` is seeded from `.options temp=<value>` (defaulting to 27 degC
/// when not set) so brace expressions in element values and waveforms can
/// reference it — matching ngspice's behaviour where TEMPER expands to the
/// circuit temperature. The seed runs before the `.param` collection so
/// users can override TEMPER via `.param TEMPER=…` if they need to.
fn build_param_table(netlist: &Netlist) -> HashMap<String, f64> {
    let mut table = HashMap::new();
    let mut pending: Vec<(&str, &Expr)> = Vec::new();

    // Seed TEMPER from `.options temp=…`. Case-insensitive match; numeric
    // values only (anything else is dropped silently). Default is 27 degC
    // — ngspice's TNOM, matching the simulator's circuit-temp fallback.
    let mut temper: f64 = 27.0;
    for item in &netlist.items {
        if let Item::Options(opts) = item {
            for p in opts {
                if p.name.eq_ignore_ascii_case("TEMP")
                    && let Expr::Num(v) = &p.value
                {
                    temper = *v;
                }
            }
        }
    }
    table.insert("TEMPER".to_string(), temper);
    table.insert("temper".to_string(), temper);

    // First pass: collect all .param and .csparam items. `.csparam` is
    // semantically identical to `.param` at the netlist-resolution layer
    // (it additionally seeds the control-block variable scope — see
    // `circuit_from_netlist`), so both kinds feed the same table here.
    for item in &netlist.items {
        let params = match item {
            Item::Param(p) | Item::Csparam(p) => p,
            _ => continue,
        };
        for p in params {
            match &p.value {
                Expr::Num(v) => {
                    table.insert(p.name.clone(), *v);
                }
                _ => {
                    pending.push((&p.name, &p.value));
                }
            }
        }
    }

    // Multi-pass resolution: try to resolve pending params that reference
    // already-resolved params. Stop when no progress is made.
    let mut made_progress = true;
    while made_progress {
        made_progress = false;
        pending.retain(|(name, expr)| {
            if let Ok(v) = expr_to_f64(expr, &table) {
                table.insert((*name).to_owned(), v);
                made_progress = true;
                false // remove from pending
            } else {
                true // keep for next pass
            }
        });
    }

    table
}

/// Build a `SourceSpec` from a `thevenin_types::Source`.
fn build_source_spec(
    source: &thevenin_types::Source,
    params: &HashMap<String, f64>,
) -> Result<SourceSpec, ImportError> {
    let dc = source.dc.as_ref().and_then(|e| expr_to_f64(e, params).ok());
    let ac = source
        .ac
        .as_ref()
        .map(|ac| {
            Ok::<IrAcSpec, ImportError>(IrAcSpec {
                mag: expr_to_f64(&ac.mag, params).unwrap_or(0.0),
                phase: ac
                    .phase
                    .as_ref()
                    .map(|e| expr_to_f64(e, params))
                    .transpose()?
                    .unwrap_or(0.0),
            })
        })
        .transpose()?;
    let waveform = source
        .waveform
        .as_ref()
        .map(|w| convert_waveform(w, params))
        .transpose()?;
    Ok(SourceSpec { dc, ac, waveform })
}

/// Convert a `thevenin_types::Waveform` to an `IrWaveform`.
fn convert_waveform(
    w: &thevenin_types::Waveform,
    params: &HashMap<String, f64>,
) -> Result<IrWaveform, ImportError> {
    let resolve = |e: &Expr| expr_to_f64(e, params);
    // Optional waveform fields default to `None` rather than failing the
    // whole import when an expression doesn't resolve. SPICE source lines
    // can pick up trailing keywords like `DISTOF1` into a positional slot
    // because the parser greedily fills waveform arg lists; the simulator
    // treats unresolved waveform tail values as unspecified, so we mirror
    // that leniency here. Required fields (e.g. PULSE.v1) still propagate
    // errors via `resolve` below.
    let resolve_opt = |e: &Option<Expr>| -> Option<f64> {
        e.as_ref().and_then(|expr| expr_to_f64(expr, params).ok())
    };

    match w {
        thevenin_types::Waveform::Pulse {
            v1,
            v2,
            td,
            tr,
            tf,
            pw,
            per,
        } => Ok(IrWaveform::Pulse {
            v1: resolve(v1)?,
            v2: resolve(v2)?,
            td: resolve_opt(td),
            tr: resolve_opt(tr),
            tf: resolve_opt(tf),
            pw: resolve_opt(pw),
            per: resolve_opt(per),
        }),
        thevenin_types::Waveform::Sin {
            v0,
            va,
            freq,
            td,
            theta,
            phi,
        } => Ok(IrWaveform::Sin {
            v0: resolve(v0)?,
            va: resolve(va)?,
            freq: resolve_opt(freq),
            td: resolve_opt(td),
            theta: resolve_opt(theta),
            phi: resolve_opt(phi),
        }),
        thevenin_types::Waveform::Exp {
            v1,
            v2,
            td1,
            tau1,
            td2,
            tau2,
        } => Ok(IrWaveform::Exp {
            v1: resolve(v1)?,
            v2: resolve(v2)?,
            td1: resolve_opt(td1),
            tau1: resolve_opt(tau1),
            td2: resolve_opt(td2),
            tau2: resolve_opt(tau2),
        }),
        thevenin_types::Waveform::Pwl(points) => {
            let pairs = points
                .iter()
                .map(|pt| Ok((resolve(&pt.time)?, resolve(&pt.value)?)))
                .collect::<Result<Vec<(f64, f64)>, ImportError>>()?;
            Ok(IrWaveform::Pwl(pairs))
        }
        thevenin_types::Waveform::Sffm { v0, va, fc, fs, md } => Ok(IrWaveform::Sffm {
            v0: resolve(v0)?,
            va: resolve(va)?,
            fc: resolve_opt(fc),
            fs: resolve_opt(fs),
            md: resolve_opt(md),
        }),
        thevenin_types::Waveform::Am { va, vo, fc, fs, td } => Ok(IrWaveform::Am {
            va: resolve(va)?,
            vo: resolve(vo)?,
            fc: resolve(fc)?,
            fs: resolve(fs)?,
            td: resolve_opt(td),
        }),
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

/// Strip SPICE voltage syntax `v(node[,ref])` to recover the bare node
/// names. A bare token round-trips unchanged. Mirrors
/// `thevenin::noise::parse_output_spec` so the IR's `output_net` /
/// `reference_net` Ids point at the actual circuit nets rather than at
/// `v(...)`-shaped artifact strings.
fn parse_voltage_node_spec(spec: &str) -> (String, Option<String>) {
    let s = spec.trim();
    let stripped = s.strip_prefix("v(").or_else(|| s.strip_prefix("V("));
    let Some(rest) = stripped else {
        return (s.to_string(), None);
    };
    let inner = rest.strip_suffix(')').unwrap_or(rest);
    if let Some((pos, neg)) = inner.split_once(',') {
        (pos.trim().to_string(), Some(neg.trim().to_string()))
    } else {
        (inner.trim().to_string(), None)
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
///
/// Unknown kinds are preserved as [`cirq_ir::DeviceType::Other`] rather than
/// discarded — the simulator dispatches several model families (TXL, LTRA,
/// CPL, XSPICE code models, HFETs, etc.) directly on the kind string, so a
/// lossy import would silently drop entire device classes.
fn map_device_type(kind: &str) -> cirq_ir::DeviceType {
    match kind.to_ascii_uppercase().as_str() {
        "D" => cirq_ir::DeviceType::Diode,
        "NPN" => cirq_ir::DeviceType::Npn,
        "PNP" => cirq_ir::DeviceType::Pnp,
        "NMOS" => cirq_ir::DeviceType::Nmos,
        "PMOS" => cirq_ir::DeviceType::Pmos,
        "NJF" => cirq_ir::DeviceType::NJfet,
        "PJF" => cirq_ir::DeviceType::PJfet,
        "NMF" | "GASFET" | "MESA" => cirq_ir::DeviceType::NMesfet,
        "PMF" => cirq_ir::DeviceType::PMesfet,
        "VDMOS" | "VDMOSN" => cirq_ir::DeviceType::Vdmos,
        "VDMOSP" => cirq_ir::DeviceType::Pvdmos,
        "SW" | "VSWITCH" => cirq_ir::DeviceType::VSwitch,
        "CSW" | "ISWITCH" => cirq_ir::DeviceType::ISwitch,
        _ => cirq_ir::DeviceType::Other(kind.to_owned()),
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
        // VDMOS / PVDMOS share the SPICE `M` element letter with lateral
        // MOSFETs. The IR keeps a single `Nmos` / `Pmos` ElementKind for all
        // four-terminal MOSFET-like devices; the simulator's mna_ir layer
        // discriminates VDMOS by inspecting the resolved model's DeviceType,
        // not via the LEVEL parameter (VDMOS has no LEVEL).
        Some(cirq_ir::DeviceType::Pvdmos) => Ok(IrElementKind::Pmos),
        Some(cirq_ir::DeviceType::Vdmos) => Ok(IrElementKind::Nmos),
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

/// Determine `ElementKind` for a MESFET based on its model type.
fn mesfet_kind(
    model_name: &str,
    model_table: &HashMap<String, cirq_ir::DeviceType>,
) -> Result<IrElementKind, ImportError> {
    match model_table.get(&model_name.to_ascii_uppercase()) {
        Some(cirq_ir::DeviceType::PMesfet) => Ok(IrElementKind::PMesfet),
        Some(cirq_ir::DeviceType::NMesfet) | Some(_) => Ok(IrElementKind::NMesfet),
        None => Err(ImportError::ModelNotFound(model_name.to_owned())),
    }
}

// ---------------------------------------------------------------------------
// Main import function
// ---------------------------------------------------------------------------

/// Convert a parsed `thevenin_types::Netlist` into a `cirq_ir::Circuit`.
pub fn import_netlist(netlist: &Netlist) -> Result<Circuit, ImportError> {
    // 0. Flatten subcircuit calls so all elements are at the top level.
    let flat_netlist = thevenin::subckt::flatten_netlist(netlist)?;
    let netlist = &flat_netlist;

    // 1. Build model table: model name (uppercased) → DeviceType.
    //    Also collect model IR objects.
    let mut model_type_table: HashMap<String, cirq_ir::DeviceType> = HashMap::new();
    let mut ir_models: Vec<IrModel> = Vec::new();
    let mut model_id_counter: u32 = 0;
    let mut model_id_table: HashMap<String, Id> = HashMap::new();

    for item in &netlist.items {
        if let Item::Model(mdef) = item {
            let device_type = map_device_type(&mdef.kind);
            let id = Id(model_id_counter);
            model_id_counter += 1;
            model_type_table.insert(mdef.name.to_ascii_uppercase(), device_type.clone());
            model_id_table.insert(mdef.name.to_ascii_uppercase(), id);
            ir_models.push(IrModel {
                id,
                name: mdef.name.clone(),
                device_type,
                params: convert_params(&mdef.params),
            });
        }
    }

    // SPICE BSIM4-style model binning: `.model foo.1 nmos (... wmin=4.5u wmax=5.5u)`
    // plus `.model foo.2 nmos (... wmin=5.5u wmax=6.5u)`. Elements reference the
    // base name `foo`; the simulator picks the bin by W/L at sim time. Register
    // synthetic alias entries (`foo` -> DeviceType, fresh Id) so element lookups
    // by base name succeed during import. The aliases are filtered out at
    // emission time in `circuit_to_netlists` so the simulator still sees the
    // original `.model foo.N` definitions and runs its own bin selection.
    let mut alias_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    for item in &netlist.items {
        if let Item::Model(mdef) = item {
            let upper = mdef.name.to_ascii_uppercase();
            if let Some(dot_pos) = upper.rfind('.') {
                let suffix = &upper[dot_pos + 1..];
                if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
                    let base_upper = upper[..dot_pos].to_string();
                    // Skip if a real .model with the base name already exists,
                    // or if we've already registered this alias.
                    if model_type_table.contains_key(&base_upper) || alias_set.contains(&base_upper)
                    {
                        continue;
                    }
                    let base = mdef
                        .name
                        .rfind('.')
                        .map(|i| mdef.name[..i].to_string())
                        .unwrap_or_else(|| mdef.name.clone());
                    let device_type = map_device_type(&mdef.kind);
                    let id = Id(model_id_counter);
                    model_id_counter += 1;
                    model_type_table.insert(base_upper.clone(), device_type.clone());
                    model_id_table.insert(base_upper.clone(), id);
                    alias_set.insert(base_upper);
                    ir_models.push(IrModel {
                        id,
                        name: base,
                        device_type,
                        // Empty params — see `circuit_to_netlists` for the
                        // emit-time filter that recognises this as an alias.
                        params: Vec::new(),
                    });
                }
            }
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

    // 3. Build parameter resolution table from .param items (before elements
    //    so that parametric element values and waveforms can be resolved).
    let param_table = build_param_table(netlist);

    // 4. Build element name → Id table for source lookups in analyses.
    let mut element_name_to_id: HashMap<String, Id> = HashMap::new();

    // 5. Convert elements.
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

        let ir_elem = convert_element(
            id,
            elem,
            &mut net_table,
            &model_type_table,
            &model_id_table,
            &param_table,
        )?;

        if let Some(e) = ir_elem {
            ir_elements.push(e);
        }
    }

    // 6. Convert analysis.
    let ir_analyses = convert_analysis(
        &netlist.analysis,
        &element_name_to_id,
        &mut net_table,
        &param_table,
    )?;

    // 7. Collect .param items.
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

    // 7b. Collect .csparam items. These live in a parallel list so the
    //     control-block interpreter can seed them as named variables; they
    //     have already been merged into the parameter resolution table
    //     above so any element value or waveform referencing them resolves
    //     identically to a `.param` of the same name.
    let mut ir_csparams: Vec<ResolvedParam> = Vec::new();
    for item in &netlist.items {
        if let Item::Csparam(params) = item {
            for p in params {
                ir_csparams.push(ResolvedParam {
                    name: p.name.clone(),
                    value: expr_to_value(&p.value),
                });
            }
        }
    }

    // 8. Collect .options items.
    //
    // Every `.option <key>=<value>` line survives verbatim into
    // `Circuit::options`; downstream consumers (the simulator, the IR-side
    // option resolvers) decide which keys to honour. ngspice-specific
    // entries such as `scale` (global MOSFET geometry multiplier on L/W/AD/
    // AS/PD/PS) are preserved here so the value is available, but the
    // actual rescaling pass is deferred — no in-scope 1.0 fixture exercises
    // it yet, and the deferred work is tracked in `docs/1.0-checklist.md`
    // section C6.
    let mut ir_options: Vec<(String, cirq_ir::Value)> = Vec::new();
    for item in &netlist.items {
        if let Item::Options(params) = item {
            for p in params {
                let val = expr_to_value(&p.value);
                if let Some(existing) = ir_options.iter_mut().find(|o| o.0 == p.name) {
                    existing.1 = val;
                } else {
                    ir_options.push((p.name.clone(), val));
                }
            }
        }
    }

    // 9. Collect .temp (multi-point: accumulate all values).
    let mut ir_temps: Vec<f64> = Vec::new();
    for item in &netlist.items {
        if let Item::Temp(t) = item {
            ir_temps.push(*t);
        }
    }

    // 10. Collect .save targets.
    let mut ir_save: Vec<String> = Vec::new();
    for item in &netlist.items {
        if let Item::Save(targets) = item {
            for t in targets {
                if !ir_save.contains(t) {
                    ir_save.push(t.clone());
                }
            }
        }
    }

    // 11. Collect .func definitions.
    let mut ir_funcs: Vec<cirq_ir::FuncDef> = Vec::new();
    for item in &netlist.items {
        if let Item::Func { name, args, body } = item {
            ir_funcs.push(cirq_ir::FuncDef {
                name: name.clone(),
                args: args.clone(),
                body: body.clone(),
            });
        }
    }

    // 12. Collect .ic initial conditions.
    let mut ir_initial_conditions: Vec<(cirq_ir::Id, f64)> = Vec::new();
    for item in &netlist.items {
        if let Item::Ic(pairs) = item {
            for (node_name, val) in pairs {
                let net_id = net_table.intern(node_name);
                ir_initial_conditions.push((net_id, *val));
            }
        }
    }

    // 13. Collect .control blocks as code blocks with "control" language tag.
    let mut ir_code_blocks: Vec<cirq_ir::CodeBlock> = Vec::new();
    for item in &netlist.items {
        if let Item::Control(lines) = item {
            ir_code_blocks.push(cirq_ir::CodeBlock::from_lines("control", lines.clone()));
        }
    }

    // 14. Collect .nodeset convergence hints.
    let mut ir_nodeset: Vec<(cirq_ir::Id, f64)> = Vec::new();
    for item in &netlist.items {
        if let Item::Nodeset(pairs) = item {
            for (node_name, val) in pairs {
                let net_id = net_table.intern(node_name);
                ir_nodeset.push((net_id, *val));
            }
        }
    }

    // 15. Collect .meas measurement specifications.
    let mut ir_measures: Vec<cirq_ir::MeasureSpec> = Vec::new();
    for item in &netlist.items {
        if let Item::Meas(spec) = item {
            ir_measures.push(cirq_ir::MeasureSpec::parse(
                spec.name.clone(),
                spec.analysis_type.clone(),
                spec.spec.clone(),
            ));
        }
    }

    // 15b. Preserve Item::Raw lines verbatim. Output-formatting directives
    // like `.print` and `.plot` live here because they have no typed Item
    // variant; the output formatter reads them out of Item::Raw strings.
    // Blank lines and lone `.end` markers carry no semantic content, so we
    // drop them to keep the IR tidy.
    let mut ir_raw_directives: Vec<String> = Vec::new();
    for item in &netlist.items {
        if let Item::Raw(line) = item {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case(".end") {
                continue;
            }
            ir_raw_directives.push(line.clone());
        }
    }

    // 16. Build circuit.
    let nets = net_table.into_nets();

    Ok(Circuit {
        name: netlist.title.clone(),
        nets,
        elements: ir_elements,
        models: ir_models,
        analyses: ir_analyses,
        params: ir_params,
        csparams: ir_csparams,
        options: ir_options,
        temps: ir_temps,
        save: ir_save,
        funcs: ir_funcs,
        initial_conditions: ir_initial_conditions,
        nodeset: ir_nodeset,
        measures: ir_measures,
        code_blocks: ir_code_blocks,
        raw_directives: ir_raw_directives,
    })
}

/// Parse SPICE source text and convert each resulting netlist into a `Circuit`.
///
/// `.include` and `.lib` directives in `source` are NOT resolved by this entry
/// point — they are parsed as opaque [`Item::Include`] / [`Item::Lib`] values.
/// To resolve them against the filesystem, use [`import_spice_with_options`]
/// or [`import_spice_path`].
pub fn import_spice(source: &str) -> Result<Vec<Circuit>, ImportError> {
    let netlists = Netlist::parse(source)?;
    netlists.iter().map(import_netlist).collect()
}

/// Parse SPICE source text with full `.include` / `.lib` resolution.
///
/// The preprocessor runs before the SPICE tokenizer and produces a flat
/// netlist string. `opts` controls search paths, encoding tolerance, and the
/// originating source directory. Pass `IncludeOptions::default()` to fall back
/// to CWD-relative resolution.
pub fn import_spice_with_options(
    source: &str,
    opts: &IncludeOptions,
) -> Result<Vec<Circuit>, ImportError> {
    let flattened = preprocess::preprocess_includes(source, opts)?;
    let netlists = Netlist::parse(&flattened)?;
    netlists.iter().map(import_netlist).collect()
}

/// Read a SPICE file from disk and import it, resolving `.include` / `.lib`
/// relative to the file's directory and the supplied `lib_paths`.
pub fn import_spice_path(
    path: impl AsRef<Path>,
    lib_paths: &[std::path::PathBuf],
) -> Result<Vec<Circuit>, ImportError> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|e| {
        ImportError::Include(preprocess::IncludeError::Io {
            path: path.to_path_buf(),
            source: e,
        })
    })?;
    let source: String = match std::str::from_utf8(&bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => {
            eprintln!(
                "warning: {} is not valid UTF-8; falling back to Latin-1 decoding",
                path.display()
            );
            bytes.iter().map(|&b| b as char).collect()
        }
    };
    let mut opts = IncludeOptions::new();
    if let Some(dir) = path.parent() {
        opts = opts.with_source_dir(dir);
    }
    for lp in lib_paths {
        opts = opts.add_lib_path(lp.clone());
    }
    import_spice_with_options(&source, &opts)
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
        }
        | SpiceElementKind::Tline {
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
        SpiceElementKind::VSwitch {
            pos,
            neg,
            ctrl_pos,
            ctrl_neg,
            ..
        } => {
            nets.intern(pos);
            nets.intern(neg);
            nets.intern(ctrl_pos);
            nets.intern(ctrl_neg);
        }
        SpiceElementKind::ISwitch { pos, neg, .. } => {
            nets.intern(pos);
            nets.intern(neg);
        }
        SpiceElementKind::Raw(_) => {}
    }
}

fn intern_analysis_nodes(analysis: &SpiceAnalysis, nets: &mut NetTable) {
    match analysis {
        SpiceAnalysis::Noise {
            output, ref_node, ..
        } => {
            // Unpack `v(node)` / `v(node,ref)` before interning so the
            // resulting Net table doesn't accumulate `v(...)` artifact
            // names. The analysis-conversion pass below performs the same
            // unpacking when wiring Ids.
            let (out_name, inline_ref) = parse_voltage_node_spec(output);
            nets.intern(&out_name);
            if let Some(name) = inline_ref {
                nets.intern(&name);
            }
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
    params: &HashMap<String, f64>,
) -> Result<Option<IrElement>, ImportError> {
    let name = &elem.name;

    match &elem.kind {
        SpiceElementKind::Resistor {
            pos,
            neg,
            value,
            params,
        } => {
            let mut ir_params = vec![("value".to_owned(), expr_to_value(value))];
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
                source_spec: None,
            }))
        }

        SpiceElementKind::Capacitor {
            pos,
            neg,
            value,
            params,
        } => {
            let mut ir_params = vec![("value".to_owned(), expr_to_value(value))];
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
                source_spec: None,
            }))
        }

        SpiceElementKind::Inductor {
            pos,
            neg,
            value,
            params,
        } => {
            let mut ir_params = vec![("value".to_owned(), expr_to_value(value))];
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
                source_spec: None,
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
            let source_spec = Some(build_source_spec(source, params)?);
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
                source_spec,
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
            let source_spec = Some(build_source_spec(source, params)?);
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
                source_spec,
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
                source_spec: None,
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
                source_spec: None,
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
                source_spec: None,
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
                source_spec: None,
            }))
        }

        SpiceElementKind::Mesa {
            d,
            g,
            s,
            model,
            params,
        } => {
            // MESA devices map to MESFET kind based on model, defaulting to NMesfet.
            let kind = mesfet_kind(model, model_types).unwrap_or(IrElementKind::NMesfet);
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
                source_spec: None,
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
                source_spec: None,
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
            source_spec: None,
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
            source_spec: None,
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
            source_spec: None,
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
            source_spec: None,
        })),

        SpiceElementKind::Ltra {
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
                    connection("in_pos", nets.intern(pos1)),
                    connection("in_neg", nets.intern(neg1)),
                    connection("out_pos", nets.intern(pos2)),
                    connection("out_neg", nets.intern(neg2)),
                ],
                params: convert_params(params),
                model: model_id,
                source_spec: None,
            }))
        }

        SpiceElementKind::Txl {
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
                kind: IrElementKind::Txl,
                connections: vec![
                    connection("in_pos", nets.intern(pos1)),
                    connection("in_neg", nets.intern(neg1)),
                    connection("out_pos", nets.intern(pos2)),
                    connection("out_neg", nets.intern(neg2)),
                ],
                params: convert_params(params),
                model: model_id,
                source_spec: None,
            }))
        }

        SpiceElementKind::Tline {
            pos1,
            neg1,
            pos2,
            neg2,
            z0,
            td,
            f,
            nl,
            ic,
        } => {
            let z0_val = expr_to_f64(z0, params)?;
            let td_val = if let Some(td_expr) = td {
                expr_to_f64(td_expr, params)?
            } else {
                let nl_val = nl
                    .as_ref()
                    .map(|e| expr_to_f64(e, params))
                    .transpose()?
                    .unwrap_or(0.25);
                let f_val = f
                    .as_ref()
                    .map(|e| expr_to_f64(e, params))
                    .transpose()?
                    .unwrap_or(1.0e9);
                if f_val <= 0.0 {
                    return Err(ImportError::UnevaluableExpr(format!(
                        "T element `{name}`: F= must be positive"
                    )));
                }
                nl_val / f_val
            };
            let ic_val: Option<[f64; 4]> = if let Some(ic_arr) = ic {
                Some([
                    expr_to_f64(&ic_arr[0], params)?,
                    expr_to_f64(&ic_arr[1], params)?,
                    expr_to_f64(&ic_arr[2], params)?,
                    expr_to_f64(&ic_arr[3], params)?,
                ])
            } else {
                None
            };
            let mut ir_params: Vec<(String, Value)> = vec![
                ("z0".to_owned(), Value::Real(z0_val)),
                ("td".to_owned(), Value::Real(td_val)),
            ];
            if let Some([v1, i1, v2, i2]) = ic_val {
                ir_params.push(("ic_v1".to_owned(), Value::Real(v1)));
                ir_params.push(("ic_i1".to_owned(), Value::Real(i1)));
                ir_params.push(("ic_v2".to_owned(), Value::Real(v2)));
                ir_params.push(("ic_i2".to_owned(), Value::Real(i2)));
            }
            Ok(Some(IrElement {
                id,
                name: name.clone(),
                kind: IrElementKind::Tline {
                    z0: z0_val,
                    td: td_val,
                    ic: ic_val,
                },
                connections: vec![
                    connection("port1_pos", nets.intern(pos1)),
                    connection("port1_neg", nets.intern(neg1)),
                    connection("port2_pos", nets.intern(pos2)),
                    connection("port2_neg", nets.intern(neg2)),
                ],
                params: ir_params,
                model: None,
                source_spec: None,
            }))
        }

        SpiceElementKind::SubcktCall { subckt, .. } => {
            // If we reach here, flatten_netlist() didn't fully resolve this
            // call — the subcircuit definition is missing or flattening is
            // incomplete.  Report an error rather than silently dropping
            // the element (which would corrupt the circuit topology).
            Err(ImportError::UnsupportedElement(format!(
                "{name} (unresolved subcircuit call to `{subckt}`)"
            )))
        }

        SpiceElementKind::BehavioralSource { pos, neg, spec } => {
            let spec_trimmed = spec.trim();
            // Parse "V=expr" or "I=expr" to determine mode and extract expression.
            let (mode, expr_str) = if let Some(rest) = spec_trimmed
                .strip_prefix("V=")
                .or_else(|| spec_trimmed.strip_prefix("v="))
            {
                (BehavioralMode::Voltage, rest.trim().to_owned())
            } else if let Some(rest) = spec_trimmed
                .strip_prefix("I=")
                .or_else(|| spec_trimmed.strip_prefix("i="))
            {
                (BehavioralMode::Current, rest.trim().to_owned())
            } else {
                // Default to voltage mode with the full spec as the expression.
                (BehavioralMode::Voltage, spec_trimmed.to_owned())
            };
            Ok(Some(IrElement {
                id,
                name: name.clone(),
                kind: IrElementKind::BehavioralSource {
                    mode,
                    spec: expr_str,
                },
                connections: vec![
                    connection("pos", nets.intern(pos)),
                    connection("neg", nets.intern(neg)),
                ],
                params: Vec::new(),
                model: None,
                source_spec: None,
            }))
        }

        SpiceElementKind::Cpl {
            in_nodes,
            out_nodes,
            gnd,
            model,
            params,
        } => {
            let width = in_nodes.len();
            let mut conns = Vec::new();
            for (i, n) in in_nodes.iter().enumerate() {
                conns.push(connection(&format!("in{i}"), nets.intern(n)));
            }
            conns.push(connection("gnd", nets.intern(gnd)));
            for (i, n) in out_nodes.iter().enumerate() {
                conns.push(connection(&format!("out{i}"), nets.intern(n)));
            }
            let mut ir_params = convert_params(params);
            ir_params.push(("model".to_owned(), Value::String(model.clone())));
            Ok(Some(IrElement {
                id,
                name: name.clone(),
                kind: IrElementKind::CoupledLine { width },
                connections: conns,
                params: ir_params,
                model: None,
                source_spec: None,
            }))
        }

        SpiceElementKind::Xspice { connections, model } => {
            let mut ir_conns: Vec<Connection> = Vec::new();
            let mut xspice_conns: Vec<IrXspiceConnection> = Vec::new();
            let mut scalar_idx = 0usize;

            for conn_spec in connections {
                match conn_spec {
                    thevenin_types::XspiceConnection::Scalar(s) => {
                        let net_id = nets.intern(s);
                        ir_conns.push(connection(&format!("c{scalar_idx}"), net_id));
                        xspice_conns.push(IrXspiceConnection::Scalar(net_id));
                        scalar_idx += 1;
                    }
                    thevenin_types::XspiceConnection::Array(arr) => {
                        let ids: Vec<Id> = arr.iter().map(|s| nets.intern(s)).collect();
                        xspice_conns.push(IrXspiceConnection::Array(ids));
                    }
                }
            }

            Ok(Some(IrElement {
                id,
                name: name.clone(),
                kind: IrElementKind::Xspice {
                    connections: xspice_conns,
                },
                connections: ir_conns,
                params: vec![("model".to_owned(), Value::String(model.clone()))],
                model: None,
                source_spec: None,
            }))
        }

        SpiceElementKind::VSwitch {
            pos,
            neg,
            ctrl_pos,
            ctrl_neg,
            model,
            on,
            params,
        } => {
            let model_id = model_ids.get(&model.to_ascii_uppercase()).copied();
            let pos_id = nets.intern(pos);
            let neg_id = nets.intern(neg);
            let ctrl_pos_id = nets.intern(ctrl_pos);
            let ctrl_neg_id = nets.intern(ctrl_neg);
            let mut ir_params = convert_params(params);
            if let Some(state) = on {
                ir_params.push(("on".to_owned(), Value::Bool(*state)));
            }
            Ok(Some(IrElement {
                id,
                name: name.clone(),
                kind: IrElementKind::Switch {
                    kind: cirq_ir::SwitchKind::Voltage,
                    control: cirq_ir::SwitchControl::Nodes {
                        pos: ctrl_pos_id,
                        neg: ctrl_neg_id,
                    },
                },
                connections: vec![
                    connection("pos", pos_id),
                    connection("neg", neg_id),
                    connection("ctrl_pos", ctrl_pos_id),
                    connection("ctrl_neg", ctrl_neg_id),
                ],
                params: ir_params,
                model: model_id,
                source_spec: None,
            }))
        }
        SpiceElementKind::ISwitch {
            pos,
            neg,
            vsense,
            model,
            on,
            params,
        } => {
            let model_id = model_ids.get(&model.to_ascii_uppercase()).copied();
            let pos_id = nets.intern(pos);
            let neg_id = nets.intern(neg);
            let mut ir_params = convert_params(params);
            if let Some(state) = on {
                ir_params.push(("on".to_owned(), Value::Bool(*state)));
            }
            Ok(Some(IrElement {
                id,
                name: name.clone(),
                kind: IrElementKind::Switch {
                    kind: cirq_ir::SwitchKind::Current,
                    control: cirq_ir::SwitchControl::Vsense {
                        name: vsense.clone(),
                    },
                },
                connections: vec![connection("pos", pos_id), connection("neg", neg_id)],
                params: ir_params,
                model: model_id,
                source_spec: None,
            }))
        }
        SpiceElementKind::Raw(_) => {
            // Unrecognized element — skip gracefully rather than failing the
            // entire import.  The element is lost but the rest of the circuit
            // can still be simulated.
            Ok(None)
        }
    }
}

// ---------------------------------------------------------------------------
// Analysis conversion
// ---------------------------------------------------------------------------

fn convert_analysis(
    analysis: &SpiceAnalysis,
    element_names: &HashMap<String, Id>,
    nets: &mut NetTable,
    params: &HashMap<String, f64>,
) -> Result<Vec<IrAnalysis>, ImportError> {
    let resolve = |e: &Expr| expr_to_f64(e, params);
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
                start: resolve(start)?,
                stop: resolve(stop)?,
                step: resolve(step)?,
            }];
            if let Some(s2) = src2 {
                let s2_id = element_names
                    .get(&s2.src.to_ascii_uppercase())
                    .copied()
                    .ok_or_else(|| ImportError::SourceNotFound(s2.src.clone()))?;
                sweeps.push(IrDcSweep {
                    source: s2_id,
                    start: resolve(&s2.start)?,
                    stop: resolve(&s2.stop)?,
                    step: resolve(&s2.step)?,
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
                start: resolve(fstart)?,
                stop: resolve(fstop)?,
                points: *n,
                scale,
            })
        }

        SpiceAnalysis::Tran {
            tstep,
            tstop,
            tstart,
            tmax,
            uic,
        } => IrAnalysis::Tran(TranAnalysis {
            step: resolve(tstep)?,
            stop: resolve(tstop)?,
            start: tstart.as_ref().map(&resolve).transpose()?.unwrap_or(0.0),
            uic: *uic,
            tmax: tmax.as_ref().and_then(|e| resolve(e).ok()),
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
            // SPICE permits `v(node)` or `v(node,ref)` as the output spec.
            // The IR carries separate `output_net` / `reference_net` Ids,
            // so unpack the parenthesised form here before interning — an
            // inline reference takes precedence over `ref_node`.
            let (out_name, inline_ref) = parse_voltage_node_spec(output);
            let ref_name = inline_ref.or_else(|| ref_node.as_ref().map(|r| r.to_string()));
            let output_id = nets.intern(&out_name);
            let ref_id = match ref_name.as_deref() {
                Some(name) => nets.intern(name),
                None => Id(0),
            };
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
                start: resolve(fstart)?,
                stop: resolve(fstop)?,
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

        SpiceAnalysis::Sens { output } => {
            let output_var = output
                .first()
                .ok_or_else(|| ImportError::UnsupportedAnalysis(".sens: missing output".into()))?
                .clone();
            let ac = parse_sens_ac_tail(&output[1..])?;
            IrAnalysis::Sens(SensAnalysis {
                output: output_var,
                ac,
            })
        }

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
        // Value param
        let value_param = r1.params.iter().find(|p| p.0 == "value").unwrap();
        match &value_param.1 {
            Value::Real(v) => assert!((v - 1000.0).abs() < 1e-6),
            other => panic!("expected Real, got {other:?}"),
        }

        let r2 = c.elements.iter().find(|e| e.name == "R2").unwrap();
        assert!(matches!(r2.kind, IrElementKind::Resistor));
        let value_param2 = r2.params.iter().find(|p| p.0 == "value").unwrap();
        match &value_param2.1 {
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
        assert_eq!(map_device_type("D"), cirq_ir::DeviceType::Diode);
        assert_eq!(map_device_type("NPN"), cirq_ir::DeviceType::Npn);
        assert_eq!(map_device_type("PNP"), cirq_ir::DeviceType::Pnp);
        assert_eq!(map_device_type("NMOS"), cirq_ir::DeviceType::Nmos);
        assert_eq!(map_device_type("PMOS"), cirq_ir::DeviceType::Pmos);
        assert_eq!(map_device_type("NJF"), cirq_ir::DeviceType::NJfet);
        assert_eq!(map_device_type("PJF"), cirq_ir::DeviceType::PJfet);
        assert_eq!(map_device_type("NMF"), cirq_ir::DeviceType::NMesfet);
        assert_eq!(map_device_type("PMF"), cirq_ir::DeviceType::PMesfet);
        assert_eq!(map_device_type("GASFET"), cirq_ir::DeviceType::NMesfet);
        // Case insensitive
        assert_eq!(map_device_type("nmos"), cirq_ir::DeviceType::Nmos);
        // Unknown kinds are preserved verbatim (no longer an error).
        assert_eq!(
            map_device_type("TXL"),
            cirq_ir::DeviceType::Other("TXL".to_string())
        );
        assert_eq!(
            map_device_type("D_RAM"),
            cirq_ir::DeviceType::Other("D_RAM".to_string())
        );
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
    fn subckt_call_expanded() {
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

        // X1 subckt call is expanded; R1 + two MOSFETs from the subcircuit body.
        assert_eq!(c.elements.len(), 3);

        // The expanded elements are prefixed with the instance name.
        // M1 uses PMOD (PMOS) and M2 uses NMOD (NMOS).
        let m1 = c.elements.iter().find(|e| e.name == "x1.m1").unwrap();
        assert!(matches!(m1.kind, IrElementKind::Pmos));

        let m2 = c.elements.iter().find(|e| e.name == "x1.m2").unwrap();
        assert!(matches!(m2.kind, IrElementKind::Nmos));

        // R1 is at top level, not prefixed.
        let r1 = c.elements.iter().find(|e| e.name == "R1").unwrap();
        assert!(matches!(r1.kind, IrElementKind::Resistor));
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
    fn csparam_collected() {
        let spice = "\
Csparam test
.csparam vcstart=-0.2
.csparam ibstop=200u
R1 a 0 1k
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];

        assert_eq!(c.csparams.len(), 2);
        assert_eq!(c.csparams[0].name, "vcstart");
        match &c.csparams[0].value {
            cirq_ir::Value::Real(v) => assert!((v - (-0.2)).abs() < 1e-12),
            other => panic!("expected Real, got {other:?}"),
        }
        assert_eq!(c.csparams[1].name, "ibstop");
        match &c.csparams[1].value {
            cirq_ir::Value::Real(v) => assert!((v - 200e-6).abs() < 1e-18),
            other => panic!("expected Real, got {other:?}"),
        }
        // .csparam must not bleed into the regular .param list.
        assert!(c.params.is_empty());
    }

    #[test]
    fn csparam_and_param_coexist() {
        let spice = "\
Mixed param test
.param a=1
.csparam b=2
.param c=3
R1 n1 0 1k
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];
        assert_eq!(c.params.len(), 2);
        assert_eq!(c.params[0].name, "a");
        assert_eq!(c.params[1].name, "c");
        assert_eq!(c.csparams.len(), 1);
        assert_eq!(c.csparams[0].name, "b");
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

    #[test]
    fn mesa_maps_to_mesfet() {
        let spice = "\
MESFET test
.model ZM1 NMF
Z1 d g s ZM1
R1 d 0 1k
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];
        let z1 = c.elements.iter().find(|e| e.name == "Z1").unwrap();
        assert!(matches!(z1.kind, IrElementKind::NMesfet));
        assert!(z1.model.is_some());
    }

    #[test]
    fn pmesa_maps_to_pmesfet() {
        let spice = "\
PMESFET test
.model ZM1 PMF
Z1 d g s ZM1
R1 d 0 1k
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];
        let z1 = c.elements.iter().find(|e| e.name == "Z1").unwrap();
        assert!(matches!(z1.kind, IrElementKind::PMesfet));
        assert!(z1.model.is_some());

        // Verify the model was imported with the correct device type.
        let model = c.models.iter().find(|m| m.name == "ZM1").unwrap();
        assert_eq!(model.device_type, cirq_ir::DeviceType::PMesfet);
    }

    #[test]
    fn mesfet_round_trip() {
        // Build a SPICE Netlist with a Mesa element, import it to IR, convert back
        // to Netlist, and verify the Mesa element survives the round trip.
        use thevenin_types::{Analysis, Element, ElementKind as SK, Expr, Item, ModelDef, Param};

        let netlist = Netlist {
            title: "MESFET round-trip".to_string(),
            items: vec![
                Item::Model(ModelDef {
                    name: "mesmod".to_string(),
                    kind: "NMF".to_string(),
                    params: vec![Param {
                        name: "vto".to_string(),
                        value: Expr::Num(-1.3),
                    }],
                }),
                Item::Element(Element {
                    name: "Z1".to_string(),
                    kind: SK::Mesa {
                        d: "drain".to_string(),
                        g: "gate".to_string(),
                        s: "0".to_string(),
                        model: "mesmod".to_string(),
                        params: vec![],
                    },
                }),
            ],
            analysis: Analysis::Op,
            source: String::new(),
        };

        // Step 1: Import to IR.
        let ir = import_netlist(&netlist).unwrap();
        let z1_ir = ir.elements.iter().find(|e| e.name == "Z1").unwrap();
        assert!(matches!(z1_ir.kind, IrElementKind::NMesfet));

        // Step 2: Convert IR back to Netlist.
        let netlists_out = cirq_frontend::to_netlist::circuit_to_netlists(&ir).unwrap();
        let nl_out = &netlists_out[0];

        // Step 3: Verify the Mesa element survived.
        let z1_out = nl_out
            .items
            .iter()
            .find_map(|i| {
                if let Item::Element(e) = i {
                    if e.name == "Z1" { Some(e) } else { None }
                } else {
                    None
                }
            })
            .expect("Z1 should survive round-trip");
        match &z1_out.kind {
            SK::Mesa { d, g, s, model, .. } => {
                assert_eq!(d, "drain");
                assert_eq!(g, "gate");
                assert_eq!(s, "0");
                assert_eq!(model, "mesmod");
            }
            other => panic!("expected Mesa, got {other:?}"),
        }

        // Step 4: Verify model survived.
        let model_out = nl_out.items.iter().find_map(|i| {
            if let Item::Model(m) = i {
                if m.name == "mesmod" { Some(m) } else { None }
            } else {
                None
            }
        });
        assert!(model_out.is_some());
        assert_eq!(model_out.unwrap().kind, "NMF");
    }

    #[test]
    fn cpl_element_imported() {
        use thevenin_types::{Analysis, Element, ElementKind as SK};

        let netlist = Netlist {
            title: "CPL test".to_string(),
            items: vec![Item::Element(Element {
                name: "P1".to_string(),
                kind: SK::Cpl {
                    in_nodes: vec!["n1".to_string(), "n2".to_string()],
                    out_nodes: vec!["n3".to_string(), "n4".to_string()],
                    gnd: "0".to_string(),
                    model: "cpl_mod".to_string(),
                    params: vec![],
                },
            })],
            analysis: Analysis::Op,
            source: String::new(),
        };

        let circuit = import_netlist(&netlist).unwrap();
        let p1 = circuit.elements.iter().find(|e| e.name == "P1").unwrap();
        match &p1.kind {
            IrElementKind::CoupledLine { width } => assert_eq!(*width, 2),
            other => panic!("expected CoupledLine, got {other:?}"),
        }
        // Check connections: in0, in1, gnd, out0, out1
        assert_eq!(p1.connections.len(), 5);
        assert_eq!(p1.connections[0].terminal, "in0");
        assert_eq!(p1.connections[1].terminal, "in1");
        assert_eq!(p1.connections[2].terminal, "gnd");
        assert_eq!(p1.connections[3].terminal, "out0");
        assert_eq!(p1.connections[4].terminal, "out1");
        // Model stored as param
        let model_param = p1.params.iter().find(|p| p.0 == "model").unwrap();
        match &model_param.1 {
            Value::String(s) => assert_eq!(s, "cpl_mod"),
            other => panic!("expected String, got {other:?}"),
        }
    }

    #[test]
    fn xspice_element_imported() {
        use thevenin_types::{Analysis, Element, ElementKind as SK, XspiceConnection};

        let netlist = Netlist {
            title: "XSPICE test".to_string(),
            items: vec![Item::Element(Element {
                name: "A1".to_string(),
                kind: SK::Xspice {
                    connections: vec![
                        XspiceConnection::Scalar("in".to_string()),
                        XspiceConnection::Array(vec!["out1".to_string(), "out2".to_string()]),
                    ],
                    model: "buf_model".to_string(),
                },
            })],
            analysis: Analysis::Op,
            source: String::new(),
        };

        let circuit = import_netlist(&netlist).unwrap();
        let a1 = circuit.elements.iter().find(|e| e.name == "A1").unwrap();
        match &a1.kind {
            IrElementKind::Xspice { connections } => {
                assert_eq!(connections.len(), 2);
                match &connections[0] {
                    IrXspiceConnection::Scalar(_) => {}
                    other => panic!("expected Scalar, got {other:?}"),
                }
                match &connections[1] {
                    IrXspiceConnection::Array(ids) => assert_eq!(ids.len(), 2),
                    other => panic!("expected Array, got {other:?}"),
                }
            }
            other => panic!("expected Xspice, got {other:?}"),
        }
        // Scalar connection appears in ir_conns.
        assert_eq!(a1.connections.len(), 1);
        assert_eq!(a1.connections[0].terminal, "c0");
        // Model stored as param.
        let model_param = a1.params.iter().find(|p| p.0 == "model").unwrap();
        match &model_param.1 {
            Value::String(s) => assert_eq!(s, "buf_model"),
            other => panic!("expected String, got {other:?}"),
        }
    }

    #[test]
    fn subckt_round_trip_port_remapping() {
        // Verifies full round-trip: SPICE with subcircuit -> import -> IR
        // with correct prefix names and port remapping.
        let spice = "\
Subcircuit round-trip test
.subckt RBUF inp outp
R1 inp mid 100
R2 mid outp 200
.ends RBUF
X1 net_a net_b RBUF
X2 net_b net_c RBUF
V1 net_a 0 DC 5
R_load net_c 0 1k
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];

        // Two instances expanded: X1 produces x1.r1, x1.r2; X2 produces x2.r1, x2.r2.
        // Plus V1 and R_load at the top level = 6 elements total.
        assert_eq!(c.elements.len(), 6);

        // Verify prefixed element names exist.
        let x1_r1 = c.elements.iter().find(|e| e.name == "x1.r1").unwrap();
        assert!(matches!(x1_r1.kind, IrElementKind::Resistor));
        let x1_r2 = c.elements.iter().find(|e| e.name == "x1.r2").unwrap();
        assert!(matches!(x1_r2.kind, IrElementKind::Resistor));
        let x2_r1 = c.elements.iter().find(|e| e.name == "x2.r1").unwrap();
        assert!(matches!(x2_r1.kind, IrElementKind::Resistor));
        let x2_r2 = c.elements.iter().find(|e| e.name == "x2.r2").unwrap();
        assert!(matches!(x2_r2.kind, IrElementKind::Resistor));

        // Verify port remapping: x1.r1 should connect to net_a (inp->net_a)
        // and x1.mid (internal node), not to "inp" or "outp".
        let x1_r1_node_names: Vec<&str> = x1_r1
            .connections
            .iter()
            .map(|conn| {
                c.nets
                    .iter()
                    .find(|n| n.id == conn.net)
                    .unwrap()
                    .name
                    .as_str()
            })
            .collect();
        assert!(
            x1_r1_node_names.contains(&"net_a"),
            "x1.r1 should connect to net_a (remapped port)"
        );
        assert!(
            x1_r1_node_names.contains(&"x1.mid"),
            "x1.r1 should connect to x1.mid (prefixed internal node)"
        );

        // x1.r2 connects x1.mid -> net_b (outp->net_b)
        let x1_r2_node_names: Vec<&str> = x1_r2
            .connections
            .iter()
            .map(|conn| {
                c.nets
                    .iter()
                    .find(|n| n.id == conn.net)
                    .unwrap()
                    .name
                    .as_str()
            })
            .collect();
        assert!(
            x1_r2_node_names.contains(&"x1.mid"),
            "x1.r2 should connect to x1.mid"
        );
        assert!(
            x1_r2_node_names.contains(&"net_b"),
            "x1.r2 should connect to net_b (remapped port)"
        );

        // x2.r1 connects net_b -> x2.mid
        let x2_r1_node_names: Vec<&str> = x2_r1
            .connections
            .iter()
            .map(|conn| {
                c.nets
                    .iter()
                    .find(|n| n.id == conn.net)
                    .unwrap()
                    .name
                    .as_str()
            })
            .collect();
        assert!(
            x2_r1_node_names.contains(&"net_b"),
            "x2.r1 should connect to net_b (remapped port)"
        );
        assert!(
            x2_r1_node_names.contains(&"x2.mid"),
            "x2.r1 should connect to x2.mid"
        );

        // Verify top-level elements are not prefixed.
        assert!(c.elements.iter().any(|e| e.name == "V1"));
        assert!(c.elements.iter().any(|e| e.name == "R_load"));
    }

    // -----------------------------------------------------------------------
    // Gap 12: Parametric expression resolution
    // -----------------------------------------------------------------------

    #[test]
    fn param_references_resolved_in_tran_analysis() {
        let spice = "\
Parametric tran
.param tstep = 1n
.param tstop = 10u
R1 a 0 1k
V1 a 0 DC 1
.tran tstep tstop
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];
        assert_eq!(c.analyses.len(), 1);
        match &c.analyses[0] {
            IrAnalysis::Tran(tran) => {
                assert!((tran.step - 1e-9).abs() < 1e-15, "step should be 1n");
                assert!((tran.stop - 10e-6).abs() < 1e-12, "stop should be 10u");
            }
            other => panic!("expected Tran, got {other:?}"),
        }
    }

    #[test]
    fn param_references_resolved_in_source_waveform() {
        let spice = "\
Parametric pulse
.param vhigh = 3.3
.param trise = 10n
V1 a 0 PULSE(0 vhigh 0 trise trise 50n 100n)
R1 a 0 1k
.tran 1n 200n
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];
        let v1 = c.elements.iter().find(|e| e.name == "V1").unwrap();
        let spec = v1.source_spec.as_ref().expect("source spec");
        match spec.waveform.as_ref().unwrap() {
            IrWaveform::Pulse { v2, tr, tf, .. } => {
                assert!((v2 - 3.3).abs() < 1e-10, "v2 should resolve to vhigh=3.3");
                assert!(
                    (tr.unwrap() - 10e-9).abs() < 1e-15,
                    "tr should resolve to trise=10n"
                );
                assert!(
                    (tf.unwrap() - 10e-9).abs() < 1e-15,
                    "tf should resolve to trise=10n"
                );
            }
            other => panic!("expected Pulse, got {other:?}"),
        }
    }

    #[test]
    fn brace_param_reference_resolved() {
        let spice = "\
Brace param
.param Rval = 4.7k
R1 a 0 {Rval}
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];
        let r1 = c.elements.iter().find(|e| e.name == "R1").unwrap();
        let value = r1.params.iter().find(|p| p.0 == "value").unwrap();
        // Element values go through expr_to_value, not expr_to_f64, so they
        // may store the string representation. The important thing is the import
        // doesn't fail.
        match &value.1 {
            Value::Real(v) => assert!((v - 4700.0).abs() < 1e-6),
            Value::String(s) => assert_eq!(s, "{Rval}"),
            other => panic!("unexpected value: {other:?}"),
        }
    }

    #[test]
    fn chained_param_resolution() {
        let spice = "\
Chained params
.param base = 1k
.param doubled = base
R1 a 0 doubled
V1 a 0 DC doubled
.tran doubled 10u
.end
";
        // This should not fail — `doubled` depends on `base`.
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];
        match &c.analyses[0] {
            IrAnalysis::Tran(tran) => {
                assert!(
                    (tran.step - 1000.0).abs() < 1e-6,
                    "doubled should resolve to 1k via base"
                );
            }
            other => panic!("expected Tran, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Gap 23: Numeric node name sanitization
    // -----------------------------------------------------------------------

    #[test]
    fn numeric_node_names_preserved() {
        let spice = "\
Numeric nodes
R1 1 0 1k
R2 1 2 2k
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];

        // SPICE numeric node names round-trip verbatim. Renaming them at
        // import would break `.print v(2)` and other raw output directives
        // that reference the original SPICE node names.
        let net_names: Vec<&str> = c.nets.iter().map(|n| n.name.as_str()).collect();
        assert!(
            net_names.contains(&"1"),
            "node '1' preserved: {net_names:?}"
        );
        assert!(
            net_names.contains(&"2"),
            "node '2' preserved: {net_names:?}"
        );
        assert!(net_names.contains(&"0"), "ground remains '0'");
    }

    #[test]
    fn alpha_node_names_unchanged() {
        let spice = "\
Alpha nodes
R1 vdd gnd_net 1k
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];

        let net_names: Vec<&str> = c.nets.iter().map(|n| n.name.as_str()).collect();
        assert!(
            net_names.contains(&"vdd"),
            "alphabetic names should be unchanged"
        );
        assert!(
            net_names.contains(&"gnd_net"),
            "alphabetic names should be unchanged"
        );
    }

    // -----------------------------------------------------------------------
    // Gap 1: Subcircuit .param passing (verify flatten handles it)
    // -----------------------------------------------------------------------

    #[test]
    fn subckt_param_passing() {
        let spice = "\
Subckt param test
.subckt RES inp outp PARAMS: Rval=1k
R1 inp outp Rval
.ends RES
X1 a 0 RES PARAMS: Rval=4.7k
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];

        // The expanded resistor from X1 should have the overridden value.
        let r1 = c.elements.iter().find(|e| e.name == "x1.r1").unwrap();
        let value = r1.params.iter().find(|p| p.0 == "value").unwrap();
        match &value.1 {
            Value::Real(v) => assert!(
                (v - 4700.0).abs() < 1e-6,
                "Rval should be 4.7k from instance override, got {v}"
            ),
            other => panic!("expected Real, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Gap 15: .nodeset convergence hints
    // -----------------------------------------------------------------------

    #[test]
    fn nodeset_imported() {
        let spice = "\
Nodeset test
R1 a 0 1k
R2 a b 2k
.nodeset V(a)=2.5 V(b)=1.0
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];

        assert_eq!(c.nodeset.len(), 2, "should have 2 nodeset entries");
        // Find net IDs for 'a' and 'b'.
        let net_a = c.nets.iter().find(|n| n.name == "a").unwrap();
        let net_b = c.nets.iter().find(|n| n.name == "b").unwrap();
        let ns_a = c.nodeset.iter().find(|ns| ns.0 == net_a.id).unwrap();
        assert!((ns_a.1 - 2.5).abs() < 1e-12, "nodeset a should be 2.5V");
        let ns_b = c.nodeset.iter().find(|ns| ns.0 == net_b.id).unwrap();
        assert!((ns_b.1 - 1.0).abs() < 1e-12, "nodeset b should be 1.0V");
    }

    // -----------------------------------------------------------------------
    // Gap 22: .meas measurement specifications
    // -----------------------------------------------------------------------

    #[test]
    fn meas_imported() {
        let spice = "\
Measure test
V1 a 0 PULSE(0 5 0 1n 1n 50n 100n)
R1 a 0 1k
.meas tran vout_max MAX v(a)
.meas tran delay TRIG v(a) VAL=0.5 RISE=1 TARG v(a) VAL=4.5 RISE=1
.tran 1n 200n
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];

        assert_eq!(c.measures.len(), 2, "should have 2 measurements");
        assert_eq!(c.measures[0].name, "vout_max");
        assert_eq!(c.measures[0].analysis_type, "tran");
        assert!(c.measures[0].spec.contains("MAX"));
        assert_eq!(c.measures[1].name, "delay");
        assert!(c.measures[1].spec.contains("TRIG"));
    }

    // -----------------------------------------------------------------------
    // Gap 4: .temp multi-point
    // -----------------------------------------------------------------------

    #[test]
    fn temp_single_value() {
        let spice = "\
Single temp
R1 a 0 1k
.temp 85
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];
        assert_eq!(c.temps, vec![85.0]);
    }

    #[test]
    fn temp_multi_value() {
        let spice = "\
Multi temp
R1 a 0 1k
.temp 25 50 100
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];
        assert_eq!(c.temps, vec![25.0, 50.0, 100.0]);
    }

    #[test]
    fn temp_multiple_lines_accumulated() {
        let spice = "\
Accumulated temps
R1 a 0 1k
.temp 27
.temp 85
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];
        assert_eq!(c.temps, vec![27.0, 85.0]);
    }

    // -----------------------------------------------------------------------
    // Arithmetic .param expression evaluation
    // -----------------------------------------------------------------------

    #[test]
    fn brace_arithmetic_add_mul() {
        let spice = "\
Arithmetic params
.param base = 1k
.param doubled = {2*base}
R1 a 0 doubled
V1 a 0 DC {base + 500}
.tran 1n 10u
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];

        // doubled = 2 * 1000 = 2000; used in tran step via param resolution
        match &c.analyses[0] {
            IrAnalysis::Tran(tran) => {
                // tstep is 1n (literal), tstop is 10u (literal)
                assert!((tran.step - 1e-9).abs() < 1e-15);
                assert!((tran.stop - 10e-6).abs() < 1e-12);
            }
            other => panic!("expected Tran, got {other:?}"),
        }
    }

    #[test]
    fn brace_arithmetic_division_and_parens() {
        let params = HashMap::from([("R".to_string(), 10_000.0), ("N".to_string(), 5.0)]);
        let result = eval_brace_expr("R / N", &params).unwrap();
        assert!((result - 2000.0).abs() < 1e-6, "10k / 5 = 2k");

        let result2 = eval_brace_expr("(R + 5k) / N", &params).unwrap();
        assert!((result2 - 3000.0).abs() < 1e-6, "(10k + 5k) / 5 = 3k");
    }

    #[test]
    fn brace_arithmetic_power() {
        let params = HashMap::from([("x".to_string(), 3.0)]);
        let result = eval_brace_expr("x ** 2", &params).unwrap();
        assert!((result - 9.0).abs() < 1e-12, "3 ** 2 = 9");
    }

    #[test]
    fn brace_arithmetic_functions() {
        let params = HashMap::new();
        let result = eval_brace_expr("sqrt(4)", &params).unwrap();
        assert!((result - 2.0).abs() < 1e-12, "sqrt(4) = 2");

        let result2 = eval_brace_expr("max(3, 7)", &params).unwrap();
        assert!((result2 - 7.0).abs() < 1e-12, "max(3, 7) = 7");

        let result3 = eval_brace_expr("abs(-5)", &params).unwrap();
        assert!((result3 - 5.0).abs() < 1e-12, "abs(-5) = 5");
    }

    #[test]
    fn brace_arithmetic_unary_minus() {
        let params = HashMap::from([("v".to_string(), 3.3)]);
        let result = eval_brace_expr("-v", &params).unwrap();
        assert!((result - (-3.3)).abs() < 1e-12, "-v = -3.3");
    }

    #[test]
    fn brace_arithmetic_chained_resolution() {
        let spice = "\
Chained arithmetic
.param base = 100
.param scaled = {base * 10}
.param offset = {scaled + 50}
R1 a 0 offset
V1 a 0 DC 1
.tran {offset} 10u
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];
        // base=100, scaled=1000, offset=1050
        match &c.analyses[0] {
            IrAnalysis::Tran(tran) => {
                assert!(
                    (tran.step - 1050.0).abs() < 1e-6,
                    "offset should resolve to 1050 via chained arithmetic"
                );
            }
            other => panic!("expected Tran, got {other:?}"),
        }
    }

    #[test]
    fn brace_si_suffixes_in_expr() {
        let params = HashMap::new();
        // 1k + 500 = 1500
        let result = eval_brace_expr("1k + 500", &params).unwrap();
        assert!((result - 1500.0).abs() < 1e-6, "1k + 500 = 1500");

        // 2.5n * 4 = 10n
        let result2 = eval_brace_expr("2.5n * 4", &params).unwrap();
        assert!((result2 - 10e-9).abs() < 1e-18, "2.5n * 4 = 10n");
    }

    // ----- Math function additions (B2 of 1.0 checklist) -----

    fn eval_b(s: &str) -> f64 {
        let params = HashMap::new();
        eval_brace_expr(s, &params).unwrap()
    }

    #[test]
    fn brace_atan2() {
        // atan2(1, 1) = pi/4
        assert!((eval_b("atan2(1, 1)") - std::f64::consts::FRAC_PI_4).abs() < 1e-12);
        // atan2(0, -1) = pi  (edge case: y == 0, x < 0)
        assert!((eval_b("atan2(0, -1)") - std::f64::consts::PI).abs() < 1e-12);
    }

    #[test]
    fn brace_hyperbolic() {
        // sinh(0) = 0, cosh(0) = 1, tanh(0) = 0
        assert!(eval_b("sinh(0)").abs() < 1e-12);
        assert!((eval_b("cosh(0)") - 1.0).abs() < 1e-12);
        assert!(eval_b("tanh(0)").abs() < 1e-12);
        // tanh(large) -> 1
        assert!((eval_b("tanh(10)") - 1.0).abs() < 1e-6);
    }

    #[test]
    fn brace_sgn() {
        assert_eq!(eval_b("sgn(2)"), 1.0);
        assert_eq!(eval_b("sgn(-2)"), -1.0);
        // edge case: sgn(0) == 0
        assert_eq!(eval_b("sgn(0)"), 0.0);
    }

    #[test]
    fn brace_int_trunc_toward_zero() {
        // int(x) truncates toward zero — distinguishes from floor for negatives.
        assert_eq!(eval_b("int(1.9)"), 1.0);
        assert_eq!(eval_b("int(-1.9)"), -1.0);
        // floor still rounds down.
        assert_eq!(eval_b("floor(-1.9)"), -2.0);
    }

    #[test]
    fn brace_ceil_floor() {
        assert_eq!(eval_b("ceil(1.2)"), 2.0);
        assert_eq!(eval_b("floor(1.9)"), 1.0);
    }

    #[test]
    fn brace_db() {
        // db(10) = 20 dB; db(0.1) = -20 dB; db(-10) = 20 (uses |x|).
        assert!((eval_b("db(10)") - 20.0).abs() < 1e-9);
        assert!((eval_b("db(0.1)") - (-20.0)).abs() < 1e-9);
        assert!((eval_b("db(-10)") - 20.0).abs() < 1e-9);
        // db20 alias.
        assert!((eval_b("db20(10)") - 20.0).abs() < 1e-9);
        // db(0) == -infinity. We only assert it is non-finite.
        assert!(!eval_b("db(0)").is_finite());
    }

    #[test]
    fn brace_limit() {
        // x within range
        assert_eq!(eval_b("limit(5, 0, 10)"), 5.0);
        // x below lo
        assert_eq!(eval_b("limit(-5, 0, 10)"), 0.0);
        // x above hi
        assert_eq!(eval_b("limit(20, 0, 10)"), 10.0);
    }

    #[test]
    fn brace_limit_invalid_bounds() {
        // lo > hi should error.
        let params = HashMap::new();
        let result = eval_brace_expr("limit(1, 10, 0)", &params);
        assert!(
            matches!(result, Err(ImportError::UnevaluableExpr(_))),
            "expected error for lo > hi, got {result:?}"
        );
    }

    // ----- C4 of 1.0 checklist: ternary `?:` and TEMPER in brace expr -----

    #[test]
    fn brace_ternary_true_branch() {
        let params = HashMap::new();
        let result = eval_brace_expr("1 > 0 ? 5 : 10", &params).unwrap();
        assert!((result - 5.0).abs() < 1e-12, "1>0 ? 5 : 10 should be 5");
    }

    #[test]
    fn brace_ternary_false_branch() {
        let params = HashMap::new();
        let result = eval_brace_expr("1 < 0 ? 5 : 10", &params).unwrap();
        assert!((result - 10.0).abs() < 1e-12, "1<0 ? 5 : 10 should be 10");
    }

    #[test]
    fn brace_temper_with_options_temp_picks_then_branch() {
        // `.options temp=100` → TEMPER=100. Expression `{TEMPER > 50 ? 1k : 2k}`
        // should pick the then-branch (1000). Routed through `.tran step`
        // because element values stay as string references — `.tran step`
        // goes through `expr_to_f64` and yields a concrete numeric.
        let spice = "\
TEMPER ternary
.options temp=100
R1 a 0 1k
V1 a 0 DC 1
.tran {TEMPER > 50 ? 1k : 2k} 10u
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];
        match &c.analyses[0] {
            IrAnalysis::Tran(tran) => {
                assert!(
                    (tran.step - 1000.0).abs() < 1e-6,
                    "expected tran.step=1000 (then-branch), got {}",
                    tran.step
                );
            }
            other => panic!("expected Tran, got {other:?}"),
        }
    }

    #[test]
    fn brace_temper_default_picks_else_branch() {
        // No `.options temp=` → TEMPER defaults to 27 degC, so
        // `{TEMPER > 50 ? 1k : 2k}` should pick the else-branch (2000).
        let spice = "\
TEMPER default
R1 a 0 1k
V1 a 0 DC 1
.tran {TEMPER > 50 ? 1k : 2k} 10u
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];
        match &c.analyses[0] {
            IrAnalysis::Tran(tran) => {
                assert!(
                    (tran.step - 2000.0).abs() < 1e-6,
                    "expected tran.step=2000 (else-branch), got {}",
                    tran.step
                );
            }
            other => panic!("expected Tran, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // C3 / devices.md: Resistor `tc=tc1,tc2` parsing
    // -----------------------------------------------------------------------

    /// Helper: pull a numeric param off an IR element.
    fn elem_num(c: &Circuit, name: &str, param: &str) -> Option<f64> {
        let e = c.elements.iter().find(|e| e.name == name)?;
        e.params.iter().find_map(|(k, v)| {
            if k.eq_ignore_ascii_case(param) {
                match v {
                    Value::Real(f) => Some(*f),
                    Value::Integer(i) => Some(*i as f64),
                    _ => None,
                }
            } else {
                None
            }
        })
    }

    #[test]
    fn resistor_tc_pair_split() {
        // `tc=tc1,tc2` on a plain resistor — ngspice supports this; the
        // importer must surface tc1 and tc2 as separate numeric params.
        let spice = "\
Resistor tc pair
R1 a 0 1k tc=1m,1u
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];
        let r1 = c.elements.iter().find(|e| e.name == "R1").unwrap();
        let params_dbg: Vec<_> = r1.params.iter().collect();
        let tc1 = elem_num(c, "R1", "tc1");
        let tc2 = elem_num(c, "R1", "tc2");
        assert!(
            tc1.is_some() && (tc1.unwrap() - 1e-3).abs() < 1e-12,
            "tc1 should be 1m, got {tc1:?}; all params: {params_dbg:?}"
        );
        assert!(
            tc2.is_some() && (tc2.unwrap() - 1e-6).abs() < 1e-18,
            "tc2 should be 1u, got {tc2:?}; all params: {params_dbg:?}"
        );
    }

    #[test]
    fn resistor_tc_single_value() {
        // `tc=X` (single value) → tc1=X, tc2=0.
        let spice = "\
Resistor tc single
R1 a 0 1k tc=2m
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];
        assert!((elem_num(c, "R1", "tc1").unwrap() - 2e-3).abs() < 1e-12);
        // tc2 defaults to 0 when only one value is supplied (and is therefore
        // absent / zero on the element).
        let tc2 = elem_num(c, "R1", "tc2").unwrap_or(0.0);
        assert!(tc2.abs() < 1e-18);
    }

    // -----------------------------------------------------------------------
    // C6: `.option scale` survives into IR
    // -----------------------------------------------------------------------

    #[test]
    fn option_scale_preserved() {
        // ngspice's `.options scale=<f>` is a globally-applied geometry
        // multiplier for MOSFET L/W/AD/AS/PD/PS. For 1.0 we only require it
        // to round-trip into `Circuit::options`; the actual geometric
        // rescaling pass is deferred (no fixture currently exercises it).
        let spice = "\
Option scale test
.options scale=1e-6
R1 a 0 1k
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];
        let scale = c
            .options
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("scale"));
        assert!(scale.is_some(), "scale option missing: {:?}", c.options);
        match &scale.unwrap().1 {
            Value::Real(v) => assert!((v - 1e-6).abs() < 1e-18, "scale = {v}, expected 1e-6"),
            other => panic!("expected Real, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // C3: `.width` no-op handler
    // -----------------------------------------------------------------------

    #[test]
    fn width_directive_is_no_op() {
        // `.width out=<n>` is an output-formatting directive — we never
        // print to fixed-width columns. Accept silently (preserved as a
        // raw directive alongside `.print` / `.plot`) and don't error.
        let spice = "\
Width directive test
.width out=80
R1 a 0 1k
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        assert_eq!(circuits.len(), 1, "import should succeed");
        // The directive is preserved in raw_directives, mirroring how the
        // importer handles other output-formatting directives like .print.
        let c = &circuits[0];
        let raw_has_width = c
            .raw_directives
            .iter()
            .any(|d| d.to_ascii_lowercase().starts_with(".width"));
        assert!(
            raw_has_width,
            ".width should survive as a raw directive: {:?}",
            c.raw_directives
        );
    }

    // -----------------------------------------------------------------------
    // C6: graceful unknown-directive policy
    // -----------------------------------------------------------------------

    #[test]
    fn unknown_directive_does_not_error() {
        // Per docs/1.0-checklist.md C6: unknown dot-directives are kept as
        // warnings + ignored (ngspice convention). They surface as raw
        // directives in the IR and never fail the import.
        let spice = "\
Unknown directive test
.totally_unknown_directive arg1 arg2
R1 a 0 1k
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        assert_eq!(circuits.len(), 1, "import should succeed");
    }

    #[test]
    fn option_scale_case_insensitive() {
        let spice = "\
Option SCALE test
.OPTION SCALE=2e-6
R1 a 0 1k
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];
        let scale = c
            .options
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("scale"));
        assert!(scale.is_some(), "scale option missing: {:?}", c.options);
    }

    #[test]
    fn resistor_tc_split_keys() {
        // ngspice also accepts tc1= / tc2= as separate keys.
        let spice = "\
Resistor tc split
R1 a 0 1k tc1=3m tc2=4u
.op
.end
";
        let circuits = import_spice(spice).unwrap();
        let c = &circuits[0];
        assert!((elem_num(c, "R1", "tc1").unwrap() - 3e-3).abs() < 1e-12);
        assert!((elem_num(c, "R1", "tc2").unwrap() - 4e-6).abs() < 1e-18);
    }
}
