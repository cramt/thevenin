//! Expression evaluator for SPICE parameter expressions.
//!
//! Supports arithmetic, comparison, boolean, ternary operators and
//! standard math functions matching ngspice's expression syntax.

use std::collections::BTreeMap;

use thevenin_types::Expr;
use thiserror::Error;

use crate::mna::MnaError;

#[derive(Error, Debug, Clone)]
pub enum ExprError {
    #[error("unknown variable: {0}")]
    UnknownVariable(String),
    #[error("unknown function: {0}")]
    UnknownFunction(String),
    #[error("wrong number of arguments for {name}: expected {expected}, got {got}")]
    WrongArgCount {
        name: String,
        expected: usize,
        got: usize,
    },
    #[error("parse error in expression: {0}")]
    ParseError(String),
}

/// Context for expression evaluation: parameters and user-defined functions.
#[derive(Debug, Clone, Default)]
pub struct EvalContext {
    /// Parameter values (case-insensitive, stored uppercase).
    pub params: BTreeMap<String, f64>,
    /// User-defined functions: name -> (arg_names, body_expression).
    pub funcs: BTreeMap<String, (Vec<String>, String)>,
}

impl EvalContext {
    /// Evaluate a `thevenin_types::Expr` to a numeric value.
    pub fn eval_expr(&self, expr: &Expr) -> Result<f64, ExprError> {
        match expr {
            Expr::Num(v) => Ok(*v),
            Expr::Param(name) => {
                let key = name.to_uppercase();
                self.params
                    .get(&key)
                    .copied()
                    .ok_or_else(|| ExprError::UnknownVariable(name.clone()))
            }
            Expr::Brace(s) => self.eval_str(s),
        }
    }

    /// Evaluate an expression string to a numeric value.
    pub fn eval_str(&self, s: &str) -> Result<f64, ExprError> {
        let tokens = tokenize(s)?;
        let mut pos = 0;
        let result = parse_ternary(&tokens, &mut pos, self)?;
        if pos < tokens.len() {
            return Err(ExprError::ParseError(format!(
                "unexpected token at position {pos}: {s}"
            )));
        }
        Ok(result)
    }

    /// Evaluate a `thevenin_types::Expr`, returning MnaError on failure.
    pub fn eval_expr_mna(&self, expr: &Expr, context: &str) -> Result<f64, MnaError> {
        self.eval_expr(expr).map_err(|e| MnaError::ExprError {
            element: context.to_string(),
            detail: e.to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Num(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Power,
    Lparen,
    Rparen,
    Comma,
    Question,
    Colon,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Not,
}

fn tokenize(input: &str) -> Result<Vec<Token>, ExprError> {
    let bytes = input.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];

        // Skip whitespace
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // Number: digit or decimal point followed by digit
        if b.is_ascii_digit() || (b == b'.' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit())
        {
            let start = i;
            // Consume digits and dots
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            // Scientific notation
            if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
                i += 1;
                if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
                    i += 1;
                }
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
            // SPICE SI suffix
            let num_end = i;
            let suffix_start = i;
            if i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                // Collect suffix
                let suf_start = i;
                while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                    i += 1;
                }
                let suffix = &input[suf_start..i];
                let multiplier = spice_suffix(suffix);
                if let Some(mult) = multiplier {
                    let base: f64 = input[start..num_end].parse().map_err(|_| {
                        ExprError::ParseError(format!("bad number: {}", &input[start..i]))
                    })?;
                    tokens.push(Token::Num(base * mult));
                    continue;
                }
                // Not a known suffix — rewind
                i = suffix_start;
            }
            let s = &input[start..num_end];
            let v: f64 = s
                .parse()
                .map_err(|_| ExprError::ParseError(format!("bad number: {s}")))?;
            tokens.push(Token::Num(v));
            continue;
        }

        // Identifier (starts with letter or underscore)
        if b.is_ascii_alphabetic() || b == b'_' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.')
            {
                i += 1;
            }
            let name = &input[start..i];
            // Check for named constants
            match name.to_uppercase().as_str() {
                "PI" => tokens.push(Token::Num(std::f64::consts::PI)),
                "E" if !matches!(tokens.last(), Some(Token::Num(_))) => {
                    tokens.push(Token::Num(std::f64::consts::E));
                }
                "TRUE" | "YES" => tokens.push(Token::Num(1.0)),
                "FALSE" | "NO" => tokens.push(Token::Num(0.0)),
                _ => tokens.push(Token::Ident(name.to_string())),
            }
            continue;
        }

        // Two-character operators
        if i + 1 < bytes.len() {
            let two = &input[i..i + 2];
            match two {
                "**" => {
                    tokens.push(Token::Power);
                    i += 2;
                    continue;
                }
                "==" => {
                    tokens.push(Token::Eq);
                    i += 2;
                    continue;
                }
                "!=" => {
                    tokens.push(Token::Ne);
                    i += 2;
                    continue;
                }
                "<=" => {
                    tokens.push(Token::Le);
                    i += 2;
                    continue;
                }
                ">=" => {
                    tokens.push(Token::Ge);
                    i += 2;
                    continue;
                }
                "&&" => {
                    tokens.push(Token::And);
                    i += 2;
                    continue;
                }
                "||" => {
                    tokens.push(Token::Or);
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }

        // Single-character operators
        match b {
            b'+' => tokens.push(Token::Plus),
            b'-' => tokens.push(Token::Minus),
            b'*' => tokens.push(Token::Star),
            b'/' => tokens.push(Token::Slash),
            b'%' => tokens.push(Token::Percent),
            b'^' => tokens.push(Token::Power),
            b'(' => tokens.push(Token::Lparen),
            b')' => tokens.push(Token::Rparen),
            b',' => tokens.push(Token::Comma),
            b'?' => tokens.push(Token::Question),
            b':' => tokens.push(Token::Colon),
            b'<' => tokens.push(Token::Lt),
            b'>' => tokens.push(Token::Gt),
            b'!' => tokens.push(Token::Not),
            b'~' => tokens.push(Token::Not), // bitwise not treated as logical not
            _ => {
                return Err(ExprError::ParseError(format!(
                    "unexpected character: '{}'",
                    b as char
                )));
            }
        }
        i += 1;
    }

    Ok(tokens)
}

fn spice_suffix(s: &str) -> Option<f64> {
    let su = s.to_uppercase();
    match su.as_str() {
        "T" => Some(1e12),
        "G" => Some(1e9),
        "MEG" => Some(1e6),
        "K" => Some(1e3),
        "M" | "MIL" => {
            // M alone is milli in SPICE, MIL is 25.4e-6
            if su == "MIL" {
                Some(25.4e-6)
            } else {
                Some(1e-3)
            }
        }
        "U" => Some(1e-6),
        "N" => Some(1e-9),
        "P" => Some(1e-12),
        "F" => Some(1e-15),
        "A" => Some(1e-18),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Recursive descent parser/evaluator
// ---------------------------------------------------------------------------

fn peek(tokens: &[Token], pos: usize) -> Option<&Token> {
    tokens.get(pos)
}

fn is_unary_context(tokens: &[Token], pos: usize) -> bool {
    if pos == 0 {
        return true;
    }
    matches!(
        tokens[pos - 1],
        Token::Plus
            | Token::Minus
            | Token::Star
            | Token::Slash
            | Token::Percent
            | Token::Power
            | Token::Lparen
            | Token::Comma
            | Token::Question
            | Token::Colon
            | Token::Eq
            | Token::Ne
            | Token::Lt
            | Token::Le
            | Token::Gt
            | Token::Ge
            | Token::And
            | Token::Or
            | Token::Not
    )
}

// Precedence 1 (lowest): ternary ? :
//
// ngspice's expression engine evaluates `cond ? then : else` lazily — only
// the selected branch is evaluated. We do the same: evaluate `cond`, then
// evaluate exactly one of `then` / `else` while token-skipping the other.
//
// This matters in practice for guard patterns like
//   `vpb > 0 ? sqrt(vpb) : 0`
//   `legacy_mode ? legacy_param : modern_value`
// where the unselected branch would otherwise produce NaN, an error from
// an unresolved parameter, or a wrong-arity function call.
fn parse_ternary(tokens: &[Token], pos: &mut usize, ctx: &EvalContext) -> Result<f64, ExprError> {
    let cond = parse_or(tokens, pos, ctx)?;
    if !matches!(peek(tokens, *pos), Some(Token::Question)) {
        return Ok(cond);
    }
    *pos += 1;
    let cond_truthy = cond != 0.0;
    let value = if cond_truthy {
        let then_val = parse_ternary(tokens, pos, ctx)?;
        expect_colon(tokens, pos)?;
        skip_ternary(tokens, pos);
        then_val
    } else {
        skip_ternary(tokens, pos);
        expect_colon(tokens, pos)?;
        parse_ternary(tokens, pos, ctx)?
    };
    Ok(value)
}

fn expect_colon(tokens: &[Token], pos: &mut usize) -> Result<(), ExprError> {
    if !matches!(peek(tokens, *pos), Some(Token::Colon)) {
        return Err(ExprError::ParseError(
            "expected ':' in ternary expression".into(),
        ));
    }
    *pos += 1;
    Ok(())
}

/// Advance `pos` past a single ternary-precedence expression *without*
/// evaluating it. Used to skip the unselected branch of a ternary so
/// unresolved parameters, unknown functions, and other evaluation errors
/// in the dead branch never reach the caller.
///
/// Stops at the first top-level `:`, `,`, or `)` — i.e. tokens that can
/// only legally terminate an expression at this level. `(` and `?` open
/// matched nesting; the corresponding `)` / `:` close that nesting before
/// they would terminate the outer expression.
fn skip_ternary(tokens: &[Token], pos: &mut usize) {
    let mut paren_depth: i32 = 0;
    let mut ternary_depth: i32 = 0;
    while *pos < tokens.len() {
        match &tokens[*pos] {
            Token::Lparen => paren_depth += 1,
            Token::Rparen => {
                if paren_depth == 0 {
                    return;
                }
                paren_depth -= 1;
            }
            Token::Comma => {
                if paren_depth == 0 {
                    return;
                }
            }
            Token::Question => ternary_depth += 1,
            Token::Colon => {
                if paren_depth == 0 && ternary_depth == 0 {
                    return;
                }
                if ternary_depth > 0 {
                    ternary_depth -= 1;
                }
            }
            _ => {}
        }
        *pos += 1;
    }
}

// Precedence 2: ||
fn parse_or(tokens: &[Token], pos: &mut usize, ctx: &EvalContext) -> Result<f64, ExprError> {
    let mut left = parse_and(tokens, pos, ctx)?;
    while matches!(peek(tokens, *pos), Some(Token::Or)) {
        *pos += 1;
        let right = parse_and(tokens, pos, ctx)?;
        left = if left != 0.0 || right != 0.0 {
            1.0
        } else {
            0.0
        };
    }
    Ok(left)
}

// Precedence 3: &&
fn parse_and(tokens: &[Token], pos: &mut usize, ctx: &EvalContext) -> Result<f64, ExprError> {
    let mut left = parse_equality(tokens, pos, ctx)?;
    while matches!(peek(tokens, *pos), Some(Token::And)) {
        *pos += 1;
        let right = parse_equality(tokens, pos, ctx)?;
        left = if left != 0.0 && right != 0.0 {
            1.0
        } else {
            0.0
        };
    }
    Ok(left)
}

// Precedence 4: == !=
fn parse_equality(tokens: &[Token], pos: &mut usize, ctx: &EvalContext) -> Result<f64, ExprError> {
    let mut left = parse_comparison(tokens, pos, ctx)?;
    loop {
        match peek(tokens, *pos) {
            Some(Token::Eq) => {
                *pos += 1;
                let right = parse_comparison(tokens, pos, ctx)?;
                left = if (left - right).abs() < f64::EPSILON {
                    1.0
                } else {
                    0.0
                };
            }
            Some(Token::Ne) => {
                *pos += 1;
                let right = parse_comparison(tokens, pos, ctx)?;
                left = if (left - right).abs() >= f64::EPSILON {
                    1.0
                } else {
                    0.0
                };
            }
            _ => break,
        }
    }
    Ok(left)
}

// Precedence 5: < <= > >=
fn parse_comparison(
    tokens: &[Token],
    pos: &mut usize,
    ctx: &EvalContext,
) -> Result<f64, ExprError> {
    let mut left = parse_additive(tokens, pos, ctx)?;
    loop {
        match peek(tokens, *pos) {
            Some(Token::Lt) => {
                *pos += 1;
                let right = parse_additive(tokens, pos, ctx)?;
                left = if left < right { 1.0 } else { 0.0 };
            }
            Some(Token::Le) => {
                *pos += 1;
                let right = parse_additive(tokens, pos, ctx)?;
                left = if left <= right { 1.0 } else { 0.0 };
            }
            Some(Token::Gt) => {
                *pos += 1;
                let right = parse_additive(tokens, pos, ctx)?;
                left = if left > right { 1.0 } else { 0.0 };
            }
            Some(Token::Ge) => {
                *pos += 1;
                let right = parse_additive(tokens, pos, ctx)?;
                left = if left >= right { 1.0 } else { 0.0 };
            }
            _ => break,
        }
    }
    Ok(left)
}

// Precedence 6: + -
fn parse_additive(tokens: &[Token], pos: &mut usize, ctx: &EvalContext) -> Result<f64, ExprError> {
    let mut left = parse_multiplicative(tokens, pos, ctx)?;
    loop {
        match peek(tokens, *pos) {
            Some(Token::Plus) if !is_unary_context(tokens, *pos) => {
                *pos += 1;
                let right = parse_multiplicative(tokens, pos, ctx)?;
                left += right;
            }
            Some(Token::Minus) if !is_unary_context(tokens, *pos) => {
                *pos += 1;
                let right = parse_multiplicative(tokens, pos, ctx)?;
                left -= right;
            }
            _ => break,
        }
    }
    Ok(left)
}

// Precedence 7: * / %
fn parse_multiplicative(
    tokens: &[Token],
    pos: &mut usize,
    ctx: &EvalContext,
) -> Result<f64, ExprError> {
    let mut left = parse_power(tokens, pos, ctx)?;
    loop {
        match peek(tokens, *pos) {
            Some(Token::Star) => {
                *pos += 1;
                let right = parse_power(tokens, pos, ctx)?;
                left *= right;
            }
            Some(Token::Slash) => {
                *pos += 1;
                let right = parse_power(tokens, pos, ctx)?;
                left /= right;
            }
            Some(Token::Percent) => {
                *pos += 1;
                let right = parse_power(tokens, pos, ctx)?;
                left %= right;
            }
            _ => break,
        }
    }
    Ok(left)
}

// Precedence 8: ** ^ (right-associative)
fn parse_power(tokens: &[Token], pos: &mut usize, ctx: &EvalContext) -> Result<f64, ExprError> {
    let base = parse_unary(tokens, pos, ctx)?;
    if matches!(peek(tokens, *pos), Some(Token::Power)) {
        *pos += 1;
        let exp = parse_power(tokens, pos, ctx)?; // right-associative
        Ok(base.powf(exp))
    } else {
        Ok(base)
    }
}

// Precedence 9: unary + - !
fn parse_unary(tokens: &[Token], pos: &mut usize, ctx: &EvalContext) -> Result<f64, ExprError> {
    match peek(tokens, *pos) {
        Some(Token::Plus) if is_unary_context(tokens, *pos) => {
            *pos += 1;
            parse_unary(tokens, pos, ctx)
        }
        Some(Token::Minus) if is_unary_context(tokens, *pos) => {
            *pos += 1;
            let val = parse_unary(tokens, pos, ctx)?;
            Ok(-val)
        }
        Some(Token::Not) => {
            *pos += 1;
            let val = parse_unary(tokens, pos, ctx)?;
            Ok(if val == 0.0 { 1.0 } else { 0.0 })
        }
        _ => parse_primary(tokens, pos, ctx),
    }
}

// Precedence 10: atoms, function calls, parenthesized expressions
fn parse_primary(tokens: &[Token], pos: &mut usize, ctx: &EvalContext) -> Result<f64, ExprError> {
    match peek(tokens, *pos) {
        Some(Token::Num(v)) => {
            let v = *v;
            *pos += 1;
            Ok(v)
        }
        Some(Token::Lparen) => {
            *pos += 1;
            let val = parse_ternary(tokens, pos, ctx)?;
            if !matches!(peek(tokens, *pos), Some(Token::Rparen)) {
                return Err(ExprError::ParseError("expected ')'".into()));
            }
            *pos += 1;
            Ok(val)
        }
        Some(Token::Ident(name)) => {
            let name = name.clone();
            *pos += 1;
            // Function call?
            if matches!(peek(tokens, *pos), Some(Token::Lparen)) {
                *pos += 1;
                let mut args = Vec::new();
                if !matches!(peek(tokens, *pos), Some(Token::Rparen)) {
                    args.push(parse_ternary(tokens, pos, ctx)?);
                    while matches!(peek(tokens, *pos), Some(Token::Comma)) {
                        *pos += 1;
                        args.push(parse_ternary(tokens, pos, ctx)?);
                    }
                }
                if !matches!(peek(tokens, *pos), Some(Token::Rparen)) {
                    return Err(ExprError::ParseError(
                        "expected ')' after function args".into(),
                    ));
                }
                *pos += 1;
                eval_function(&name, &args, ctx)
            } else {
                // Variable lookup
                let key = name.to_uppercase();
                ctx.params
                    .get(&key)
                    .copied()
                    .ok_or(ExprError::UnknownVariable(name))
            }
        }
        Some(tok) => Err(ExprError::ParseError(format!("unexpected token: {tok:?}"))),
        None => Err(ExprError::ParseError("unexpected end of expression".into())),
    }
}

// ---------------------------------------------------------------------------
// Built-in functions
// ---------------------------------------------------------------------------

fn eval_function(name: &str, args: &[f64], ctx: &EvalContext) -> Result<f64, ExprError> {
    let name_upper = name.to_uppercase();

    // Check user-defined functions first
    if let Some((param_names, body)) = ctx.funcs.get(&name_upper) {
        if args.len() != param_names.len() {
            return Err(ExprError::WrongArgCount {
                name: name.to_string(),
                expected: param_names.len(),
                got: args.len(),
            });
        }
        // Create a child context with function parameters
        let mut child_ctx = ctx.clone();
        for (pname, &val) in param_names.iter().zip(args.iter()) {
            child_ctx.params.insert(pname.to_uppercase(), val);
        }
        return child_ctx.eval_str(body);
    }

    // Built-in functions
    match name_upper.as_str() {
        // Trigonometric
        "SIN" => require_args(name, args, 1).map(|_| args[0].sin()),
        "COS" => require_args(name, args, 1).map(|_| args[0].cos()),
        "TAN" => require_args(name, args, 1).map(|_| args[0].tan()),
        "ASIN" => require_args(name, args, 1).map(|_| args[0].asin()),
        "ACOS" => require_args(name, args, 1).map(|_| args[0].acos()),
        "ATAN" | "ARCTAN" => require_args(name, args, 1).map(|_| args[0].atan()),
        "ATAN2" => require_args(name, args, 2).map(|_| args[0].atan2(args[1])),

        // Hyperbolic
        "SINH" => require_args(name, args, 1).map(|_| args[0].sinh()),
        "COSH" => require_args(name, args, 1).map(|_| args[0].cosh()),
        "TANH" => require_args(name, args, 1).map(|_| args[0].tanh()),
        "ASINH" => require_args(name, args, 1).map(|_| args[0].asinh()),
        "ACOSH" => require_args(name, args, 1).map(|_| args[0].acosh()),
        "ATANH" => require_args(name, args, 1).map(|_| args[0].atanh()),

        // Exponential / logarithmic
        "EXP" => require_args(name, args, 1).map(|_| args[0].exp()),
        "LOG" | "LN" => require_args(name, args, 1).map(|_| args[0].ln()),
        "LOG10" => require_args(name, args, 1).map(|_| args[0].log10()),
        "SQRT" => require_args(name, args, 1).map(|_| args[0].sqrt()),
        "SQR" => require_args(name, args, 1).map(|_| args[0] * args[0]),

        // Power
        "POW" | "PWR" => require_args(name, args, 2).map(|_| args[0].abs().powf(args[1])),

        // Rounding / integer
        "ABS" => require_args(name, args, 1).map(|_| args[0].abs()),
        "SGN" | "SIGN" => require_args(name, args, 1).map(|_| {
            if args[0] > 0.0 {
                1.0
            } else if args[0] < 0.0 {
                -1.0
            } else {
                0.0
            }
        }),
        "INT" => require_args(name, args, 1).map(|_| args[0].trunc()),
        "NINT" => require_args(name, args, 1).map(|_| args[0].round()),
        "FLOOR" => require_args(name, args, 1).map(|_| args[0].floor()),
        "CEIL" | "CEILING" => require_args(name, args, 1).map(|_| args[0].ceil()),

        // Min/max
        "MIN" => require_args(name, args, 2).map(|_| args[0].min(args[1])),
        "MAX" => require_args(name, args, 2).map(|_| args[0].max(args[1])),

        // Decibel (voltage/amplitude). ngspice/SPICE convention: 20 * log10(|x|).
        "DB" | "DB20" => require_args(name, args, 1).map(|_| 20.0 * args[0].abs().log10()),

        // Clamp x to [lo, hi]. Errors if lo > hi.
        "LIMIT" => {
            require_args(name, args, 3)?;
            let (x, lo, hi) = (args[0], args[1], args[2]);
            if lo > hi {
                return Err(ExprError::ParseError(format!(
                    "limit: lower bound ({lo}) is greater than upper bound ({hi})"
                )));
            }
            Ok(x.clamp(lo, hi))
        }

        // Step/ramp functions (B-source)
        "U" => require_args(name, args, 1).map(|_| if args[0] > 0.0 { 1.0 } else { 0.0 }),
        "U2" => require_args(name, args, 1).map(|_| {
            if args[0] <= 0.0 {
                0.0
            } else if args[0] < 1.0 {
                args[0]
            } else {
                1.0
            }
        }),
        "URAMP" => require_args(name, args, 1).map(|_| if args[0] > 0.0 { args[0] } else { 0.0 }),

        // Predicate functions (return 0 or 1)
        "EQ0" => require_args(name, args, 1).map(|_| if args[0] == 0.0 { 1.0 } else { 0.0 }),
        "NE0" => require_args(name, args, 1).map(|_| if args[0] != 0.0 { 1.0 } else { 0.0 }),
        "GT0" => require_args(name, args, 1).map(|_| if args[0] > 0.0 { 1.0 } else { 0.0 }),
        "LT0" => require_args(name, args, 1).map(|_| if args[0] < 0.0 { 1.0 } else { 0.0 }),
        "GE0" => require_args(name, args, 1).map(|_| if args[0] >= 0.0 { 1.0 } else { 0.0 }),
        "LE0" => require_args(name, args, 1).map(|_| if args[0] <= 0.0 { 1.0 } else { 0.0 }),

        // PWL function: pwl(x, x1, y1, x2, y2, ...)
        "PWL" => {
            if args.len() < 3 || args.len().is_multiple_of(2) {
                return Err(ExprError::WrongArgCount {
                    name: name.to_string(),
                    expected: 3, // at least x, x1, y1
                    got: args.len(),
                });
            }
            let x = args[0];
            let pairs: Vec<(f64, f64)> = args[1..].chunks(2).map(|c| (c[0], c[1])).collect();
            Ok(pwl_interp(x, &pairs))
        }

        _ => Err(ExprError::UnknownFunction(name.to_string())),
    }
}

fn require_args(name: &str, args: &[f64], expected: usize) -> Result<(), ExprError> {
    if args.len() != expected {
        Err(ExprError::WrongArgCount {
            name: name.to_string(),
            expected,
            got: args.len(),
        })
    } else {
        Ok(())
    }
}

/// Piecewise-linear interpolation.
fn pwl_interp(x: f64, pairs: &[(f64, f64)]) -> f64 {
    if pairs.is_empty() {
        return 0.0;
    }
    if x <= pairs[0].0 {
        return pairs[0].1;
    }
    if x >= pairs[pairs.len() - 1].0 {
        return pairs[pairs.len() - 1].1;
    }
    for i in 1..pairs.len() {
        if x <= pairs[i].0 {
            let (x0, y0) = pairs[i - 1];
            let (x1, y1) = pairs[i];
            let t = (x - x0) / (x1 - x0);
            return y0 + t * (y1 - y0);
        }
    }
    pairs[pairs.len() - 1].1
}

// ---------------------------------------------------------------------------
// Netlist expression resolution
// ---------------------------------------------------------------------------

/// Build an `EvalContext` from `.param` and `.func` items in a netlist.
pub fn build_context(items: &[thevenin_types::Item]) -> EvalContext {
    let mut ctx = EvalContext::default();
    collect_context(items, &mut ctx);
    ctx
}

/// Collect .param and .func definitions into a context, resolving params in order.
fn collect_context(items: &[thevenin_types::Item], ctx: &mut EvalContext) {
    for item in items {
        match item {
            thevenin_types::Item::Param(params) => {
                for p in params {
                    if let Ok(val) = ctx.eval_expr(&p.value) {
                        ctx.params.insert(p.name.to_uppercase(), val);
                    }
                }
            }
            thevenin_types::Item::Func { name, args, body } => {
                ctx.funcs.insert(
                    name.to_uppercase(),
                    (
                        args.iter().map(|a| a.to_uppercase()).collect(),
                        body.clone(),
                    ),
                );
            }
            _ => {}
        }
    }
}

/// Resolve all `Expr::Param` and `Expr::Brace` in a netlist to `Expr::Num`.
/// Also converts constant B-source expressions to V/I sources.
pub fn resolve_netlist_exprs(
    netlist: &mut thevenin_types::Netlist,
) -> Result<EvalContext, ExprError> {
    let ctx = build_context(&netlist.items);
    resolve_items(&mut netlist.items, &ctx)?;
    resolve_bsources(&mut netlist.items, &ctx)?;
    resolve_analysis(&mut netlist.analysis, &ctx)?;
    Ok(ctx)
}

fn resolve_items(items: &mut [thevenin_types::Item], ctx: &EvalContext) -> Result<(), ExprError> {
    for item in items.iter_mut() {
        match item {
            thevenin_types::Item::Element(el) => resolve_element(&mut el.kind, ctx)?,
            thevenin_types::Item::Subckt(_) => {
                // Don't resolve inside subcircuit definitions — the subcircuit
                // expander handles parameter substitution with instance params.
            }
            _ => {}
        }
    }
    Ok(())
}

fn try_resolve_expr(expr: &mut Expr, ctx: &EvalContext) {
    if let Ok(val) = ctx.eval_expr(expr) {
        *expr = Expr::Num(val);
    }
}

fn resolve_source(source: &mut thevenin_types::Source, ctx: &EvalContext) -> Result<(), ExprError> {
    if let Some(dc) = &mut source.dc {
        try_resolve_expr(dc, ctx);
    }
    if let Some(ac) = &mut source.ac {
        try_resolve_expr(&mut ac.mag, ctx);
        if let Some(phase) = &mut ac.phase {
            try_resolve_expr(phase, ctx);
        }
    }
    if let Some(wf) = &mut source.waveform {
        resolve_waveform(wf, ctx);
    }
    Ok(())
}

fn resolve_waveform(wf: &mut thevenin_types::Waveform, ctx: &EvalContext) {
    match wf {
        thevenin_types::Waveform::Pulse {
            v1,
            v2,
            td,
            tr,
            tf,
            pw,
            per,
        } => {
            try_resolve_expr(v1, ctx);
            try_resolve_expr(v2, ctx);
            for e in [td, tr, tf, pw, per].into_iter().flatten() {
                try_resolve_expr(e, ctx);
            }
        }
        thevenin_types::Waveform::Sin {
            v0,
            va,
            freq,
            td,
            theta,
            phi,
        } => {
            try_resolve_expr(v0, ctx);
            try_resolve_expr(va, ctx);
            for e in [freq, td, theta, phi].into_iter().flatten() {
                try_resolve_expr(e, ctx);
            }
        }
        thevenin_types::Waveform::Exp {
            v1,
            v2,
            td1,
            tau1,
            td2,
            tau2,
        } => {
            try_resolve_expr(v1, ctx);
            try_resolve_expr(v2, ctx);
            for e in [td1, tau1, td2, tau2].into_iter().flatten() {
                try_resolve_expr(e, ctx);
            }
        }
        thevenin_types::Waveform::Pwl(points) => {
            for p in points {
                try_resolve_expr(&mut p.time, ctx);
                try_resolve_expr(&mut p.value, ctx);
            }
        }
        thevenin_types::Waveform::Sffm { v0, va, fc, fs, md } => {
            try_resolve_expr(v0, ctx);
            try_resolve_expr(va, ctx);
            for e in [fc, fs, md].into_iter().flatten() {
                try_resolve_expr(e, ctx);
            }
        }
        thevenin_types::Waveform::Am { va, vo, fc, fs, td } => {
            try_resolve_expr(va, ctx);
            try_resolve_expr(vo, ctx);
            try_resolve_expr(fc, ctx);
            try_resolve_expr(fs, ctx);
            if let Some(e) = td {
                try_resolve_expr(e, ctx);
            }
        }
    }
}

fn resolve_params(params: &mut [thevenin_types::Param], ctx: &EvalContext) {
    for p in params {
        try_resolve_expr(&mut p.value, ctx);
    }
}

fn resolve_element(
    kind: &mut thevenin_types::ElementKind,
    ctx: &EvalContext,
) -> Result<(), ExprError> {
    use thevenin_types::ElementKind;
    match kind {
        ElementKind::Resistor { value, params, .. } => {
            try_resolve_expr(value, ctx);
            resolve_params(params, ctx);
        }
        ElementKind::Capacitor { value, params, .. } => {
            try_resolve_expr(value, ctx);
            resolve_params(params, ctx);
        }
        ElementKind::Inductor { value, params, .. } => {
            try_resolve_expr(value, ctx);
            resolve_params(params, ctx);
        }
        ElementKind::VoltageSource { source, .. } => {
            resolve_source(source, ctx)?;
        }
        ElementKind::CurrentSource { source, .. } => {
            resolve_source(source, ctx)?;
        }
        ElementKind::Diode { params, .. } => {
            resolve_params(params, ctx);
        }
        ElementKind::Bjt { params, .. } => {
            resolve_params(params, ctx);
        }
        ElementKind::Mosfet { params, .. } => {
            resolve_params(params, ctx);
        }
        ElementKind::Jfet { params, .. } => {
            resolve_params(params, ctx);
        }
        ElementKind::Mesa { params, .. } => {
            resolve_params(params, ctx);
        }
        ElementKind::MutualCoupling { coupling, .. } => {
            try_resolve_expr(coupling, ctx);
        }
        ElementKind::Vcvs { gain, .. } => {
            try_resolve_expr(gain, ctx);
        }
        ElementKind::Cccs { gain, .. } => {
            try_resolve_expr(gain, ctx);
        }
        ElementKind::Vccs { gm, .. } => {
            try_resolve_expr(gm, ctx);
        }
        ElementKind::Ccvs { rm, .. } => {
            try_resolve_expr(rm, ctx);
        }
        ElementKind::SubcktCall { params, .. } => {
            resolve_params(params, ctx);
        }
        ElementKind::Ltra { params, .. } => {
            resolve_params(params, ctx);
        }
        ElementKind::Txl { params, .. } => {
            resolve_params(params, ctx);
        }
        ElementKind::Tline {
            z0, td, f, nl, ic, ..
        } => {
            try_resolve_expr(z0, ctx);
            if let Some(e) = td {
                try_resolve_expr(e, ctx);
            }
            if let Some(e) = f {
                try_resolve_expr(e, ctx);
            }
            if let Some(e) = nl {
                try_resolve_expr(e, ctx);
            }
            if let Some(arr) = ic {
                for e in arr.iter_mut() {
                    try_resolve_expr(e, ctx);
                }
            }
        }
        ElementKind::Cpl { params, .. } => {
            resolve_params(params, ctx);
        }
        ElementKind::VSwitch { params, .. } | ElementKind::ISwitch { params, .. } => {
            resolve_params(params, ctx);
        }
        ElementKind::Urc { length, lumps, .. } => {
            try_resolve_expr(length, ctx);
            if let Some(l) = lumps {
                try_resolve_expr(l, ctx);
            }
        }
        ElementKind::Xspice { .. } | ElementKind::BehavioralSource { .. } | ElementKind::Raw(_) => {
        }
    }
    Ok(())
}

/// Convert constant B-source expressions to V/I sources.
fn resolve_bsources(
    items: &mut [thevenin_types::Item],
    ctx: &EvalContext,
) -> Result<(), ExprError> {
    for item in items.iter_mut() {
        if let thevenin_types::Item::Element(el) = item
            && let thevenin_types::ElementKind::BehavioralSource { pos, neg, spec } = &el.kind
        {
            // Parse spec: "V=expr" or "I=expr" or "V = expr" etc.
            let spec_trimmed = spec.trim();
            let (is_voltage, expr_str) = if let Some(rest) = spec_trimmed
                .strip_prefix("V=")
                .or_else(|| spec_trimmed.strip_prefix("v="))
            {
                (true, rest.trim())
            } else if let Some(rest) = spec_trimmed
                .strip_prefix("V =")
                .or_else(|| spec_trimmed.strip_prefix("v ="))
            {
                (true, rest.trim())
            } else if let Some(rest) = spec_trimmed
                .strip_prefix("I=")
                .or_else(|| spec_trimmed.strip_prefix("i="))
            {
                (false, rest.trim())
            } else if let Some(rest) = spec_trimmed
                .strip_prefix("I =")
                .or_else(|| spec_trimmed.strip_prefix("i ="))
            {
                (false, rest.trim())
            } else {
                continue;
            };

            // Strip surrounding quotes/braces from expression
            let expr_clean =
                if let Some(inner) = expr_str.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
                    inner.trim()
                } else if let Some(inner) = expr_str
                    .strip_prefix('\'')
                    .and_then(|s| s.strip_suffix('\''))
                {
                    inner.trim()
                } else {
                    expr_str
                };

            // Try to evaluate as a constant expression
            if let Ok(val) = ctx.eval_str(expr_clean) {
                let pos = pos.clone();
                let neg = neg.clone();
                let source = thevenin_types::Source {
                    dc: Some(Expr::Num(val)),
                    ac: None,
                    waveform: None,
                };
                if is_voltage {
                    el.kind = thevenin_types::ElementKind::VoltageSource { pos, neg, source };
                } else {
                    el.kind = thevenin_types::ElementKind::CurrentSource { pos, neg, source };
                }
            }
            // If evaluation fails (has circuit variable references), leave as B-source
        }
    }
    Ok(())
}

fn resolve_analysis(
    analysis: &mut thevenin_types::Analysis,
    ctx: &EvalContext,
) -> Result<(), ExprError> {
    use thevenin_types::Analysis;
    match analysis {
        Analysis::Op => {}
        Analysis::Dc {
            start,
            stop,
            step,
            src2,
            ..
        } => {
            try_resolve_expr(start, ctx);
            try_resolve_expr(stop, ctx);
            try_resolve_expr(step, ctx);
            if let Some(s2) = src2 {
                try_resolve_expr(&mut s2.start, ctx);
                try_resolve_expr(&mut s2.stop, ctx);
                try_resolve_expr(&mut s2.step, ctx);
            }
        }
        Analysis::Tran {
            tstep,
            tstop,
            tstart,
            tmax,
            ..
        } => {
            try_resolve_expr(tstep, ctx);
            try_resolve_expr(tstop, ctx);
            if let Some(e) = tstart {
                try_resolve_expr(e, ctx);
            }
            if let Some(e) = tmax {
                try_resolve_expr(e, ctx);
            }
        }
        Analysis::Ac { fstart, fstop, .. } => {
            try_resolve_expr(fstart, ctx);
            try_resolve_expr(fstop, ctx);
        }
        Analysis::Noise { fstart, fstop, .. } => {
            try_resolve_expr(fstart, ctx);
            try_resolve_expr(fstop, ctx);
        }
        Analysis::Tf { .. } | Analysis::Sens { .. } | Analysis::Pz { .. } => {}
        Analysis::Four { fundamental, .. } => {
            try_resolve_expr(fundamental, ctx);
        }
        Analysis::Fft {
            start,
            stop,
            npoints,
            ..
        } => {
            if let Some(e) = start {
                try_resolve_expr(e, ctx);
            }
            if let Some(e) = stop {
                try_resolve_expr(e, ctx);
            }
            if let Some(e) = npoints {
                try_resolve_expr(e, ctx);
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// B-source expression helpers
// ---------------------------------------------------------------------------

/// Substitute `v(node)` and `v(n1,n2)` voltage references in a B-source expression
/// with their actual numeric values from `node_voltages`.
/// Node "0" (ground) is always 0.0.
pub fn substitute_v_refs(
    expr: &str,
    node_voltages: &std::collections::BTreeMap<String, f64>,
) -> String {
    let mut result = String::with_capacity(expr.len() + 32);
    let mut pos = 0;

    while pos < expr.len() {
        // Case-insensitive search for 'v('
        let remaining = &expr[pos..];
        let found = remaining.char_indices().find(|&(i, c)| {
            (c == 'v' || c == 'V') && remaining[i + c.len_utf8()..].starts_with('(')
        });

        let (rel, ch) = match found {
            Some(x) => x,
            None => {
                result.push_str(&expr[pos..]);
                break;
            }
        };

        let v_start = pos + rel;

        // Check word boundary: char before 'v' must not be alphanumeric/underscore
        let is_word_start = if v_start == 0 {
            true
        } else {
            let prev = expr[..v_start].chars().last().unwrap_or(' ');
            !prev.is_alphanumeric() && prev != '_'
        };

        if is_word_start {
            let after_paren = v_start + 2; // skip 'v('
            // Find matching ')'
            if let Some(rel_close) = expr[after_paren..].find(')') {
                let close = after_paren + rel_close;
                let content = &expr[after_paren..close];
                let parts: Vec<&str> = content.split(',').collect();

                let lookup = |name: &str| -> f64 {
                    let n = name.trim();
                    if n == "0" {
                        return 0.0;
                    }
                    node_voltages
                        .get(n)
                        .or_else(|| node_voltages.get(&n.to_lowercase()))
                        .or_else(|| node_voltages.get(&n.to_uppercase()))
                        .copied()
                        .unwrap_or(0.0)
                };

                let v = match parts.len() {
                    1 => lookup(parts[0]),
                    2 => lookup(parts[0]) - lookup(parts[1]),
                    _ => {
                        // Unknown, copy verbatim
                        result.push_str(&expr[pos..close + 1]);
                        pos = close + 1;
                        continue;
                    }
                };

                result.push_str(&expr[pos..v_start]);
                result.push_str(&format!("({v:.15e})"));
                pos = close + 1;
                continue;
            }
        }

        // Not a v() call at word boundary, copy char and advance
        result.push_str(&expr[pos..v_start + ch.len_utf8()]);
        pos = v_start + ch.len_utf8();
    }

    result
}

/// Evaluate a B-source expression with node voltages substituted.
pub fn evaluate_bsrc_expr(
    expr: &str,
    node_voltages: &std::collections::BTreeMap<String, f64>,
) -> Result<f64, ExprError> {
    evaluate_bsrc_expr_with_ctx(expr, node_voltages, SimContext::default())
}

/// Simulation-context bindings for behavioural and parameter expressions.
///
/// These are the ngspice "magic" identifiers that get bound at evaluation
/// time rather than at parse / parameter-resolution time. Callers from the
/// simulator (transient, AC, DC) populate the relevant fields; unused fields
/// stay `None` and the corresponding identifier is undefined.
#[derive(Debug, Clone, Copy, Default)]
pub struct SimContext {
    /// Current simulation time in seconds (transient only).
    pub time: Option<f64>,
    /// Current frequency in Hz (AC and noise analyses).
    pub freq: Option<f64>,
    /// Current circuit temperature in degrees Celsius.
    pub temper: Option<f64>,
}

impl SimContext {
    pub fn at_time(time: f64, temper: f64) -> Self {
        Self {
            time: Some(time),
            freq: None,
            temper: Some(temper),
        }
    }

    pub fn at_freq(freq: f64, temper: f64) -> Self {
        Self {
            time: None,
            freq: Some(freq),
            temper: Some(temper),
        }
    }

    pub fn at_temper(temper: f64) -> Self {
        Self {
            time: None,
            freq: None,
            temper: Some(temper),
        }
    }
}

/// Evaluate a B-source expression with node voltages and simulation-context
/// constants (`time`, `freq` / `hertz`, `temper`) bound for lookup.
///
/// The sim-context constants are stored in the `EvalContext.params` table
/// keyed by their uppercase form, since `parse_primary` looks up identifiers
/// case-insensitively via `to_uppercase()`. `freq` and `hertz` are aliases
/// of the same Hz value, matching ngspice's behaviour.
pub fn evaluate_bsrc_expr_with_ctx(
    expr: &str,
    node_voltages: &std::collections::BTreeMap<String, f64>,
    sim: SimContext,
) -> Result<f64, ExprError> {
    let substituted = substitute_v_refs(expr, node_voltages);
    let mut ctx = EvalContext::default();
    if let Some(t) = sim.time {
        ctx.params.insert("TIME".to_string(), t);
    }
    if let Some(f) = sim.freq {
        ctx.params.insert("FREQ".to_string(), f);
        ctx.params.insert("HERTZ".to_string(), f);
    }
    if let Some(tc) = sim.temper {
        ctx.params.insert("TEMPER".to_string(), tc);
    }
    ctx.eval_str(&substituted)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(s: &str) -> f64 {
        let ctx = EvalContext::default();
        ctx.eval_str(s).unwrap()
    }

    fn eval_ctx(s: &str, params: &[(&str, f64)]) -> f64 {
        let mut ctx = EvalContext::default();
        for &(k, v) in params {
            ctx.params.insert(k.to_uppercase(), v);
        }
        ctx.eval_str(s).unwrap()
    }

    #[test]
    fn basic_arithmetic() {
        assert_eq!(eval("1+2"), 3.0);
        assert_eq!(eval("1+2*3"), 7.0);
        assert_eq!(eval("(1+2)*3"), 9.0);
        assert_eq!(eval("10/4"), 2.5);
        assert_eq!(eval("10%3"), 1.0);
    }

    #[test]
    fn double_minus() {
        assert_eq!(eval("2--3"), 5.0);
        assert_eq!(eval("1--1"), 2.0);
    }

    #[test]
    fn unary() {
        assert_eq!(eval("-3"), -3.0);
        assert_eq!(eval("+3"), 3.0);
        assert_eq!(eval("-(1+2)"), -3.0);
    }

    #[test]
    fn power() {
        assert_eq!(eval("2**3"), 8.0);
        assert_eq!(eval("2^3"), 8.0);
        // Right-associative: 2^3^2 = 2^(3^2) = 2^9 = 512
        assert_eq!(eval("2^3^2"), 512.0);
    }

    #[test]
    fn comparison() {
        assert_eq!(eval("1<2"), 1.0);
        assert_eq!(eval("2<1"), 0.0);
        assert_eq!(eval("1<=1"), 1.0);
        assert_eq!(eval("1==1"), 1.0);
        assert_eq!(eval("1!=2"), 1.0);
    }

    #[test]
    fn boolean_ops() {
        assert_eq!(eval("1&&1"), 1.0);
        assert_eq!(eval("1&&0"), 0.0);
        assert_eq!(eval("0||1"), 1.0);
        assert_eq!(eval("!0"), 1.0);
        assert_eq!(eval("!1"), 0.0);
    }

    #[test]
    fn ternary() {
        assert_eq!(eval("1 ? 10 : 20"), 10.0);
        assert_eq!(eval("0 ? 10 : 20"), 20.0);
    }

    // ----- ternary short-circuit + grammar tests -------------------------
    //
    // These pin down the behaviour the SPICE expression engine promises to
    // users writing `cond ? then : else` guards. Two things matter:
    //
    //   1. *Selection* — exactly one branch is evaluated; its value flows
    //      out, the other's value never does.
    //   2. *Short-circuit* — errors / NaNs / Infs in the unselected branch
    //      never reach the caller, just like in ngspice.
    //
    // Each test names the property it pins down; together they cover the
    // patterns the agent reviewer (and ngspice docs) call out as the real
    // motivation for short-circuit ternary.

    fn try_eval_ctx(s: &str, params: &[(&str, f64)]) -> Result<f64, ExprError> {
        let mut ctx = EvalContext::default();
        for &(k, v) in params {
            ctx.params.insert(k.to_uppercase(), v);
        }
        ctx.eval_str(s)
    }

    #[test]
    fn ternary_skips_unresolved_param_in_else_branch() {
        // `legacy_mode ? legacy_value : modern_value` — only modern is
        // resolved, legacy_value isn't even defined. Because the condition
        // is false, the else-branch is evaluated and `legacy_value` is
        // never touched.
        let v = try_eval_ctx(
            "legacy_mode ? legacy_value : modern_value",
            &[("legacy_mode", 0.0), ("modern_value", 42.0)],
        )
        .expect("else-branch selected, then-branch reference must not error");
        assert_eq!(v, 42.0);
    }

    #[test]
    fn ternary_skips_unresolved_param_in_then_branch() {
        let v = try_eval_ctx(
            "use_legacy ? legacy_only : 7",
            &[("use_legacy", 1.0), ("legacy_only", 99.0)],
        )
        .expect("then-branch resolves cleanly");
        assert_eq!(v, 99.0);
        // Symmetric: the else can hold an unresolved name when then is selected.
        let v = try_eval_ctx("use_legacy ? 11 : unresolved_else", &[("use_legacy", 1.0)])
            .expect("else-branch with unresolved name is skipped");
        assert_eq!(v, 11.0);
    }

    #[test]
    fn ternary_skips_unknown_function_in_dead_branch() {
        // `nonexistent_func` would be an UnknownFunction error on eval.
        // Since the then-branch is taken, the dead else is never run.
        let v = try_eval_ctx("1 ? 42 : nonexistent_func(1, 2)", &[])
            .expect("dead-branch unknown function must not propagate");
        assert_eq!(v, 42.0);
    }

    #[test]
    fn ternary_skips_wrong_arity_in_dead_branch() {
        // `sin(1, 2)` is a real function called with the wrong number of
        // args — would be a WrongArgCount error on eval. Skipped here.
        let v = try_eval_ctx("0 ? sin(1, 2) : 5", &[])
            .expect("dead-branch arity mismatch must not propagate");
        assert_eq!(v, 5.0);
    }

    #[test]
    fn ternary_does_propagate_error_in_selected_branch() {
        // Sanity check the symmetric case: errors in the *selected* branch
        // must still propagate. Without this, the "skip" logic could be
        // hiding real user mistakes.
        let err = try_eval_ctx("1 ? unresolved : 0", &[]).unwrap_err();
        assert!(matches!(err, ExprError::UnknownVariable(ref n) if n == "unresolved"));

        let err = try_eval_ctx("0 ? 0 : unresolved", &[]).unwrap_err();
        assert!(matches!(err, ExprError::UnknownVariable(ref n) if n == "unresolved"));
    }

    #[test]
    fn ternary_classic_safe_sqrt_guard() {
        // The canonical motivation for short-circuit: a guard that keeps
        // `sqrt` away from negative inputs. With eager evaluation, the
        // dead `sqrt(x)` call would produce a NaN that *should* never
        // matter — but does, if a future Rust `sqrt` ever returns Err on
        // negatives (or if the guard expression sits inside arithmetic
        // that propagates NaN downstream).
        assert_eq!(
            eval_ctx("x > 0 ? sqrt(x) : 0", &[("x", 4.0)]),
            2.0,
            "positive x → sqrt branch taken"
        );
        assert_eq!(
            eval_ctx("x > 0 ? sqrt(x) : 0", &[("x", -1.0)]),
            0.0,
            "negative x → guard branch taken, sqrt(-1) never reached"
        );
    }

    #[test]
    fn ternary_right_associative() {
        // `a ? b : c ? d : e` parses as `a ? b : (c ? d : e)`.
        assert_eq!(eval("0 ? 10 : 1 ? 20 : 30"), 20.0);
        assert_eq!(eval("0 ? 10 : 0 ? 20 : 30"), 30.0);
        assert_eq!(eval("1 ? 10 : 0 ? 20 : 30"), 10.0);
        // Right-associativity means the chain *also* short-circuits past
        // unresolved names deeper in the chain.
        let v = try_eval_ctx("0 ? 10 : 0 ? unresolved : 30", &[])
            .expect("right-associative chain must skip the unresolved middle branch");
        assert_eq!(v, 30.0);
    }

    #[test]
    fn ternary_nested_in_then_branch() {
        // `a ? (b ? c : d) : e` — the inner ternary lives entirely inside
        // the outer then-branch.
        assert_eq!(eval("1 ? (1 ? 10 : 20) : 30"), 10.0);
        assert_eq!(eval("1 ? (0 ? 10 : 20) : 30"), 20.0);
        assert_eq!(eval("0 ? (1 ? 10 : 20) : 30"), 30.0);
        // And the inner else of the inner ternary can be unresolved when
        // the outer takes its else.
        let v = try_eval_ctx("0 ? (1 ? 10 : unresolved) : 30", &[])
            .expect("outer else skips the entire inner ternary");
        assert_eq!(v, 30.0);
    }

    #[test]
    fn ternary_inside_arithmetic() {
        // Ternary's precedence is below arithmetic, so `1 + 0 ? a : b`
        // is `(1 + 0) ? a : b` = `1 ? a : b`. Parens are needed to nest
        // it inside an arithmetic expression.
        assert_eq!(eval("1 + (1 ? 10 : 20)"), 11.0);
        assert_eq!(eval("(0 ? 10 : 20) * 3"), 60.0);
        // And the dead branch is still skipped when wrapped in arithmetic.
        let v = try_eval_ctx("(0 ? unresolved : 5) + 1", &[])
            .expect("dead branch inside parenthesised ternary is skipped");
        assert_eq!(v, 6.0);
    }

    #[test]
    fn ternary_inside_function_args() {
        // `min(a ? b : c, d)` — ternary fills one function argument; the
        // comma terminates the ternary's else-branch.
        assert_eq!(eval("min(1 ? 3 : 7, 5)"), 3.0);
        assert_eq!(eval("min(0 ? 3 : 7, 5)"), 5.0);
        assert_eq!(eval("max(1 ? 3 : 7, 5)"), 5.0);
        // Skipping a dead branch must not eat the comma that separates
        // function arguments.
        let v = try_eval_ctx("min(0 ? unresolved : 4, 9)", &[])
            .expect("ternary skip inside function args must respect commas");
        assert_eq!(v, 4.0);
    }

    #[test]
    fn ternary_precedence_below_or() {
        // `a || b ? c : d` parses as `(a || b) ? c : d`, not
        // `a || (b ? c : d)`. Comparison ladder runs first.
        assert_eq!(eval("0 || 1 ? 10 : 20"), 10.0);
        assert_eq!(eval("0 || 0 ? 10 : 20"), 20.0);
        // Same for &&.
        assert_eq!(eval("1 && 1 ? 10 : 20"), 10.0);
        assert_eq!(eval("1 && 0 ? 10 : 20"), 20.0);
    }

    #[test]
    fn ternary_with_comparison_condition() {
        assert_eq!(eval_ctx("x < 0 ? -x : x", &[("x", -3.5)]), 3.5);
        assert_eq!(eval_ctx("x < 0 ? -x : x", &[("x", 4.0)]), 4.0);
        // Chained: clamp x to [0, 10].
        assert_eq!(eval_ctx("x < 0 ? 0 : x > 10 ? 10 : x", &[("x", -5.0)]), 0.0);
        assert_eq!(eval_ctx("x < 0 ? 0 : x > 10 ? 10 : x", &[("x", 5.0)]), 5.0);
        assert_eq!(
            eval_ctx("x < 0 ? 0 : x > 10 ? 10 : x", &[("x", 99.0)]),
            10.0
        );
    }

    #[test]
    fn ternary_missing_colon_is_a_parse_error() {
        // Eager parsing of the then-branch then expects a `:`. Without it
        // we must give a clear ParseError rather than evaluating wrongly.
        let err = try_eval_ctx("1 ? 10", &[]).unwrap_err();
        assert!(
            matches!(err, ExprError::ParseError(msg) if msg.contains(':')),
            "missing colon should be a ParseError mentioning ':'"
        );
    }

    #[test]
    fn math_functions() {
        let eps = 1e-12;
        assert!((eval("sin(0)") - 0.0).abs() < eps);
        assert!((eval("cos(0)") - 1.0).abs() < eps);
        assert!((eval("exp(1)") - std::f64::consts::E).abs() < eps);
        assert!((eval("log(1)") - 0.0).abs() < eps);
        assert!((eval("sqrt(4)") - 2.0).abs() < eps);
        assert!((eval("abs(-5)") - 5.0).abs() < eps);
    }

    #[test]
    fn rounding_functions() {
        assert_eq!(eval("floor(1.7)"), 1.0);
        assert_eq!(eval("ceil(1.2)"), 2.0);
        assert_eq!(eval("int(1.9)"), 1.0);
        assert_eq!(eval("nint(1.5)"), 2.0);
        assert_eq!(eval("nint(2.5)"), 3.0); // round half away from zero
    }

    #[test]
    fn parameters() {
        assert_eq!(eval_ctx("x+1", &[("x", 5.0)]), 6.0);
        assert_eq!(eval_ctx("x*y", &[("x", 3.0), ("y", 4.0)]), 12.0);
    }

    #[test]
    fn user_function() {
        let mut ctx = EvalContext::default();
        ctx.funcs.insert(
            "DOUBLE".to_string(),
            (vec!["X".to_string()], "x*2".to_string()),
        );
        assert_eq!(ctx.eval_str("double(5)").unwrap(), 10.0);
    }

    #[test]
    fn spice_suffixes_in_expr() {
        assert_eq!(eval("1k"), 1000.0);
        assert_eq!(eval("1k + 500"), 1500.0);
        assert_eq!(eval("2.5n"), 2.5e-9);
    }

    #[test]
    fn step_functions() {
        assert_eq!(eval("u(1)"), 1.0);
        assert_eq!(eval("u(-1)"), 0.0);
        assert_eq!(eval("uramp(2)"), 2.0);
        assert_eq!(eval("uramp(-1)"), 0.0);
    }

    #[test]
    fn predicate_functions() {
        assert_eq!(eval("eq0(0)"), 1.0);
        assert_eq!(eval("eq0(1)"), 0.0);
        assert_eq!(eval("gt0(1)"), 1.0);
        assert_eq!(eval("lt0(-1)"), 1.0);
    }

    // ----- Math function additions (B2 of 1.0 checklist) -----
    // The resolver already has atan, atan2, asin, acos, sinh, cosh, tanh,
    // sgn, ceil, floor and int(trunc). New built-ins: db, db20, limit.

    #[test]
    fn math_inverse_trig() {
        let eps = 1e-12;
        // asin(1) = pi/2, acos(0) = pi/2, atan(1) = pi/4
        assert!((eval("asin(1)") - std::f64::consts::FRAC_PI_2).abs() < eps);
        assert!((eval("acos(0)") - std::f64::consts::FRAC_PI_2).abs() < eps);
        assert!((eval("atan(1)") - std::f64::consts::FRAC_PI_4).abs() < eps);
        // atan2(1, 1) = pi/4
        assert!((eval("atan2(1, 1)") - std::f64::consts::FRAC_PI_4).abs() < eps);
    }

    #[test]
    fn math_hyperbolic() {
        let eps = 1e-12;
        assert!(eval("sinh(0)").abs() < eps);
        assert!((eval("cosh(0)") - 1.0).abs() < eps);
        assert!(eval("tanh(0)").abs() < eps);
    }

    #[test]
    fn math_sgn() {
        assert_eq!(eval("sgn(5)"), 1.0);
        assert_eq!(eval("sgn(-5)"), -1.0);
        // edge: sgn(0) == 0
        assert_eq!(eval("sgn(0)"), 0.0);
        // sign alias
        assert_eq!(eval("sign(-3)"), -1.0);
    }

    #[test]
    fn math_int_trunc_toward_zero() {
        // int(x) truncates toward zero — both positive and negative.
        assert_eq!(eval("int(1.9)"), 1.0);
        assert_eq!(eval("int(-1.9)"), -1.0);
        assert_eq!(eval("floor(-1.9)"), -2.0);
        assert_eq!(eval("ceil(-1.9)"), -1.0);
    }

    #[test]
    fn math_db_basic() {
        let eps = 1e-9;
        // db(10) = 20 dB (amplitude convention).
        assert!((eval("db(10)") - 20.0).abs() < eps);
        // db(0.1) = -20 dB.
        assert!((eval("db(0.1)") - (-20.0)).abs() < eps);
        // db(-10) = 20 (uses |x|).
        assert!((eval("db(-10)") - 20.0).abs() < eps);
        // db20 is an alias.
        assert!((eval("db20(100)") - 40.0).abs() < eps);
    }

    #[test]
    fn math_db_at_zero_is_non_finite() {
        // db(0) is log10(0) * 20 == -infinity.
        let v = eval("db(0)");
        assert!(!v.is_finite(), "db(0) should be non-finite, got {v}");
    }

    #[test]
    fn math_limit_in_range() {
        assert_eq!(eval("limit(5, 0, 10)"), 5.0);
    }

    #[test]
    fn math_limit_below() {
        assert_eq!(eval("limit(-5, 0, 10)"), 0.0);
    }

    #[test]
    fn math_limit_above() {
        assert_eq!(eval("limit(50, 0, 10)"), 10.0);
    }

    #[test]
    fn math_limit_invalid_bounds_errors() {
        let ctx = EvalContext::default();
        // lo > hi must error.
        let err = ctx.eval_str("limit(1, 10, 0)").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("limit"),
            "error message should mention limit: {msg}"
        );
    }

    #[test]
    fn behavioral_resistor_expr() {
        let mut voltages: std::collections::BTreeMap<String, f64> =
            std::collections::BTreeMap::new();
        voltages.insert("0".to_string(), 0.0);
        voltages.insert("1".to_string(), 100.0);
        voltages.insert("3".to_string(), 0.0);
        voltages.insert("9".to_string(), 0.0);

        let result = evaluate_bsrc_expr("v(1,3)/(1k + v(9))", &voltages).unwrap();
        assert!((result - 0.1).abs() < 1e-10, "expected 0.1, got {result}");
    }
}
