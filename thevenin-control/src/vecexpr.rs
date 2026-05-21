//! Vector expression evaluator for `.control` `let` / `if` / `print` commands.
//!
//! Like ngspice's `ft_evaluate()`, operates on vectors (arrays of f64).
//! Scalars are single-element vectors. Binary ops broadcast the shorter operand.

use crate::context::SimContext;

/// A vector value — the result of evaluating an expression.
///
/// When `imag` is non-empty, the vector is complex-valued (real + j*imag).
/// When `imag` is empty, the vector is real-valued.
#[derive(Debug, Clone)]
pub struct VecVal {
    pub data: Vec<f64>,
    pub imag: Vec<f64>,
}

impl VecVal {
    pub fn scalar(v: f64) -> Self {
        Self {
            data: vec![v],
            imag: vec![],
        }
    }

    pub fn complex_scalar(re: f64, im: f64) -> Self {
        Self {
            data: vec![re],
            imag: vec![im],
        }
    }

    pub fn is_complex(&self) -> bool {
        !self.imag.is_empty()
    }

    pub fn is_scalar(&self) -> bool {
        self.data.len() == 1
    }

    pub fn as_scalar(&self) -> f64 {
        if self.data.is_empty() {
            0.0
        } else {
            self.data[self.data.len() - 1]
        }
    }

    pub fn is_truthy(&self) -> bool {
        self.as_scalar() != 0.0
    }

    /// Get imaginary part at index, or 0.0 if real-only.
    fn im(&self, i: usize) -> f64 {
        if i < self.imag.len() {
            self.imag[i]
        } else {
            0.0
        }
    }

    /// Length of the vector (max of real and imag lengths).
    fn len(&self) -> usize {
        self.data.len().max(self.imag.len())
    }

    /// Get real part at index, broadcasting scalars.
    fn re(&self, i: usize) -> f64 {
        if self.data.len() == 1 {
            self.data[0]
        } else if i < self.data.len() {
            self.data[i]
        } else {
            0.0
        }
    }

    /// Get imaginary part at index, broadcasting scalars.
    fn im_broadcast(&self, i: usize) -> f64 {
        if self.imag.len() == 1 {
            self.imag[0]
        } else if i < self.imag.len() {
            self.imag[i]
        } else {
            0.0
        }
    }

    /// Create a real-only vector.
    fn real(data: Vec<f64>) -> Self {
        Self { data, imag: vec![] }
    }
}

/// Evaluate a vector expression string in the given context.
pub fn eval_vec_expr(expr: &str, ctx: &SimContext) -> Result<VecVal, String> {
    let tokens = tokenize(expr)?;
    if tokens.is_empty() {
        return Err("empty expression".to_string());
    }
    let mut pos = 0;
    let result = parse_or(&tokens, &mut pos, ctx)?;
    Ok(result)
}

/// Evaluate a condition expression (returns bool).
pub fn eval_condition(expr: &str, ctx: &SimContext) -> Result<bool, String> {
    let val = eval_vec_expr(expr, ctx)?;
    Ok(val.is_truthy())
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Num(f64),
    Ident(String),
    /// Quoted string — treated as a plot-qualified vector name.
    Str(String),
    /// `@device[param]` device parameter query.
    DeviceParam(String),
    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    // Comparison
    Gt,
    Lt,
    Ge,
    Le,
    /// Single `=` — treated as equality (ngspice compat).
    SingleEq,
    // Parens, brackets, comma
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
}

fn tokenize(s: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut chars = s.chars().peekable();

    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' => {
                chars.next();
            }
            '+' => {
                chars.next();
                tokens.push(Token::Plus);
            }
            '-' => {
                chars.next();
                // Distinguish unary minus from subtraction
                // If previous token is a number, ident, or rparen, this is subtraction
                let is_binary = matches!(
                    tokens.last(),
                    Some(
                        Token::Num(_)
                            | Token::Ident(_)
                            | Token::Str(_)
                            | Token::RParen
                            | Token::RBracket
                    )
                );
                if is_binary {
                    tokens.push(Token::Minus);
                } else {
                    // Unary minus — read the next number/ident
                    // Push as a special unary-minus token
                    tokens.push(Token::Minus);
                }
            }
            '*' => {
                chars.next();
                tokens.push(Token::Star);
            }
            '/' => {
                chars.next();
                tokens.push(Token::Slash);
            }
            '^' => {
                chars.next();
                tokens.push(Token::Caret);
            }
            '>' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    tokens.push(Token::Ge);
                } else {
                    tokens.push(Token::Gt);
                }
            }
            '<' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    tokens.push(Token::Le);
                } else if chars.peek() == Some(&'>') {
                    // <> is "not equal"
                    chars.next();
                    tokens.push(Token::Ident("ne".to_string()));
                } else {
                    tokens.push(Token::Lt);
                }
            }
            '=' => {
                chars.next();
                tokens.push(Token::SingleEq);
            }
            '(' => {
                chars.next();
                tokens.push(Token::LParen);
            }
            ')' => {
                chars.next();
                tokens.push(Token::RParen);
            }
            ',' => {
                chars.next();
                tokens.push(Token::Comma);
            }
            '[' => {
                chars.next();
                tokens.push(Token::LBracket);
            }
            ']' => {
                chars.next();
                tokens.push(Token::RBracket);
            }
            '{' => {
                // {plotname}.vecname — plot-qualified vector reference
                chars.next();
                let mut plot = String::new();
                while let Some(&c) = chars.peek() {
                    if c == '}' {
                        chars.next();
                        break;
                    }
                    plot.push(c);
                    chars.next();
                }
                // Expect `.vecname` after `}`
                if chars.peek() == Some(&'.') {
                    chars.next();
                    let mut vec_name = String::new();
                    while let Some(&c) = chars.peek() {
                        if c.is_alphanumeric() || c == '_' || c == '#' || c == '.' {
                            vec_name.push(c);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    // Encode as "plotname.vecname" string token
                    tokens.push(Token::Str(format!("{plot}.{vec_name}")));
                } else {
                    // Just {value} — treat as string
                    tokens.push(Token::Str(plot));
                }
            }
            '"' => {
                chars.next();
                let mut s = String::new();
                while let Some(&c) = chars.peek() {
                    if c == '"' {
                        chars.next();
                        break;
                    }
                    s.push(c);
                    chars.next();
                }
                tokens.push(Token::Str(s));
            }
            '@' => {
                // @device[param] — stop reading after first `]` so that
                // @v1[dc][2] becomes DeviceParam("@v1[dc]"), LBracket, Num(2), RBracket
                let mut s = String::new();
                while let Some(&c) = chars.peek() {
                    if c == ' '
                        || c == '\t'
                        || c == '+'
                        || c == '-'
                        || c == '*'
                        || c == '/'
                        || c == ')'
                        || c == ','
                        || c == '>'
                        || c == '<'
                    {
                        break;
                    }
                    s.push(c);
                    chars.next();
                    if c == ']' {
                        break; // End of device param — any further [idx] is postfix indexing
                    }
                }
                tokens.push(Token::DeviceParam(s));
            }
            c if c.is_ascii_digit() || c == '.' => {
                let mut num_str = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_alphanumeric()
                        || c == '.'
                        || c == 'e'
                        || c == 'E'
                        || ((c == '+' || c == '-') && num_str.ends_with(['e', 'E']))
                    {
                        num_str.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                let v = parse_number_with_suffix(&num_str)?;
                tokens.push(Token::Num(v));
            }
            c if c.is_alphabetic() || c == '_' || c == '#' => {
                let mut ident = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' || c == '#' || c == '.' {
                        ident.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(Token::Ident(ident));
            }
            '$' => {
                // Variable reference $var or $&var — should have been interpolated already,
                // but handle $curplot etc.
                chars.next();
                if chars.peek() == Some(&'&') {
                    chars.next();
                }
                let mut name = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                // This will be resolved later as an ident
                tokens.push(Token::Ident(name));
            }
            _ => {
                chars.next(); // skip unknown
            }
        }
    }
    Ok(tokens)
}

fn parse_number_with_suffix(s: &str) -> Result<f64, String> {
    if let Ok(v) = s.parse::<f64>() {
        return Ok(v);
    }
    // Strip trailing unit suffixes (V, A, W, Hz, Ohm)
    let s = strip_unit_suffix_str(s);
    if let Ok(v) = s.parse::<f64>() {
        return Ok(v);
    }
    let lower = s.to_lowercase();
    if let Some(rest) = lower.strip_suffix("meg") {
        return rest
            .parse::<f64>()
            .map(|v| v * 1e6)
            .map_err(|_| format!("bad number: {s}"));
    }
    let (num, mult) = match lower.chars().last() {
        Some('t') => (&s[..s.len() - 1], 1e12),
        Some('g') => (&s[..s.len() - 1], 1e9),
        Some('k') => (&s[..s.len() - 1], 1e3),
        Some('m') => (&s[..s.len() - 1], 1e-3),
        Some('u') => (&s[..s.len() - 1], 1e-6),
        Some('n') => (&s[..s.len() - 1], 1e-9),
        Some('p') => (&s[..s.len() - 1], 1e-12),
        Some('f') => (&s[..s.len() - 1], 1e-15),
        Some('a') => (&s[..s.len() - 1], 1e-18),
        _ => return Err(format!("bad number: {s}")),
    };
    num.parse::<f64>()
        .map(|v| v * mult)
        .map_err(|_| format!("bad number: {s}"))
}

// ---------------------------------------------------------------------------
// Recursive descent parser — produces VecVal
// ---------------------------------------------------------------------------

fn parse_or(tokens: &[Token], pos: &mut usize, ctx: &SimContext) -> Result<VecVal, String> {
    let mut left = parse_and(tokens, pos, ctx)?;
    while *pos < tokens.len() {
        if let Token::Ident(s) = &tokens[*pos]
            && s.eq_ignore_ascii_case("or")
        {
            *pos += 1;
            let right = parse_and(tokens, pos, ctx)?;
            left = vec_binop(
                &left,
                &right,
                |a, b| {
                    if a != 0.0 || b != 0.0 { 1.0 } else { 0.0 }
                },
            );
            continue;
        }
        break;
    }
    Ok(left)
}

fn parse_and(tokens: &[Token], pos: &mut usize, ctx: &SimContext) -> Result<VecVal, String> {
    let mut left = parse_comparison(tokens, pos, ctx)?;
    while *pos < tokens.len() {
        if let Token::Ident(s) = &tokens[*pos]
            && s.eq_ignore_ascii_case("and")
        {
            *pos += 1;
            let right = parse_comparison(tokens, pos, ctx)?;
            left = vec_binop(
                &left,
                &right,
                |a, b| {
                    if a != 0.0 && b != 0.0 { 1.0 } else { 0.0 }
                },
            );
            continue;
        }
        break;
    }
    Ok(left)
}

fn parse_comparison(tokens: &[Token], pos: &mut usize, ctx: &SimContext) -> Result<VecVal, String> {
    let mut left = parse_add(tokens, pos, ctx)?;
    while *pos < tokens.len() {
        enum CmpOp {
            Gt,
            Lt,
            Ge,
            Le,
            Eq,
            Ne,
        }
        let op = match &tokens[*pos] {
            Token::Gt => Some(CmpOp::Gt),
            Token::Lt => Some(CmpOp::Lt),
            Token::Ge => Some(CmpOp::Ge),
            Token::Le => Some(CmpOp::Le),
            Token::SingleEq => Some(CmpOp::Eq),
            // ngspice-style word forms — `time le tstop` is equivalent to
            // `time <= tstop`. resume-1.cir's golden trace uses these.
            Token::Ident(s) if s.eq_ignore_ascii_case("gt") => Some(CmpOp::Gt),
            Token::Ident(s) if s.eq_ignore_ascii_case("lt") => Some(CmpOp::Lt),
            Token::Ident(s) if s.eq_ignore_ascii_case("ge") => Some(CmpOp::Ge),
            Token::Ident(s) if s.eq_ignore_ascii_case("le") => Some(CmpOp::Le),
            Token::Ident(s) if s.eq_ignore_ascii_case("eq") => Some(CmpOp::Eq),
            Token::Ident(s) if s.eq_ignore_ascii_case("ne") => Some(CmpOp::Ne),
            _ => None,
        };
        if let Some(op) = op {
            *pos += 1;
            let right = parse_add(tokens, pos, ctx)?;
            left = match op {
                CmpOp::Gt => vec_binop(&left, &right, |a, b| if a > b { 1.0 } else { 0.0 }),
                CmpOp::Lt => vec_binop(&left, &right, |a, b| if a < b { 1.0 } else { 0.0 }),
                CmpOp::Ge => vec_binop(&left, &right, |a, b| if a >= b { 1.0 } else { 0.0 }),
                CmpOp::Le => vec_binop(&left, &right, |a, b| if a <= b { 1.0 } else { 0.0 }),
                CmpOp::Eq => {
                    vec_binop(
                        &left,
                        &right,
                        |a, b| if (a - b).abs() < 1e-15 { 1.0 } else { 0.0 },
                    )
                }
                CmpOp::Ne => vec_binop(&left, &right, |a, b| {
                    if (a - b).abs() >= 1e-15 { 1.0 } else { 0.0 }
                }),
            };
        } else {
            break;
        }
    }
    Ok(left)
}

fn parse_add(tokens: &[Token], pos: &mut usize, ctx: &SimContext) -> Result<VecVal, String> {
    let mut left = parse_mul(tokens, pos, ctx)?;
    while *pos < tokens.len() {
        match &tokens[*pos] {
            Token::Plus => {
                *pos += 1;
                let right = parse_mul(tokens, pos, ctx)?;
                left = vec_complex_binop(&left, &right, |ar, ai, br, bi| (ar + br, ai + bi));
            }
            Token::Minus => {
                *pos += 1;
                let right = parse_mul(tokens, pos, ctx)?;
                left = vec_complex_binop(&left, &right, |ar, ai, br, bi| (ar - br, ai - bi));
            }
            _ => break,
        }
    }
    Ok(left)
}

fn parse_mul(tokens: &[Token], pos: &mut usize, ctx: &SimContext) -> Result<VecVal, String> {
    let mut left = parse_power(tokens, pos, ctx)?;
    while *pos < tokens.len() {
        match &tokens[*pos] {
            Token::Star => {
                *pos += 1;
                let right = parse_power(tokens, pos, ctx)?;
                left = vec_complex_binop(&left, &right, |ar, ai, br, bi| {
                    (ar * br - ai * bi, ar * bi + ai * br)
                });
            }
            Token::Slash => {
                *pos += 1;
                let right = parse_power(tokens, pos, ctx)?;
                left = vec_complex_binop(&left, &right, |ar, ai, br, bi| {
                    let denom = br * br + bi * bi;
                    if denom != 0.0 {
                        ((ar * br + ai * bi) / denom, (ai * br - ar * bi) / denom)
                    } else {
                        (f64::INFINITY, 0.0)
                    }
                });
            }
            _ => break,
        }
    }
    Ok(left)
}

/// Power operator `^` — higher precedence than `*` and `/`.
fn parse_power(tokens: &[Token], pos: &mut usize, ctx: &SimContext) -> Result<VecVal, String> {
    let mut left = parse_unary(tokens, pos, ctx)?;
    while *pos < tokens.len() {
        if tokens[*pos] == Token::Caret {
            *pos += 1;
            let right = parse_unary(tokens, pos, ctx)?;
            left = vec_complex_binop(&left, &right, |ar, ai, br, _bi| {
                // Complex power: (ar + j*ai)^br
                // For real exponent (bi≈0): z^n = |z|^n * e^{j*n*θ}
                if ai == 0.0 {
                    // Both operands real: standard powf
                    (ar.powf(br), 0.0)
                } else {
                    let mag = (ar * ar + ai * ai).sqrt();
                    let theta = ai.atan2(ar);
                    let new_mag = mag.powf(br);
                    let new_theta = br * theta;
                    (new_mag * new_theta.cos(), new_mag * new_theta.sin())
                }
            });
        } else {
            break;
        }
    }
    Ok(left)
}

fn parse_unary(tokens: &[Token], pos: &mut usize, ctx: &SimContext) -> Result<VecVal, String> {
    if *pos >= tokens.len() {
        return Err("unexpected end of expression".to_string());
    }
    match &tokens[*pos] {
        Token::Minus => {
            *pos += 1;
            let val = parse_primary(tokens, pos, ctx)?;
            let data = val.data.iter().map(|v| -v).collect();
            let imag = if val.is_complex() {
                val.imag.iter().map(|v| -v).collect()
            } else {
                vec![]
            };
            Ok(VecVal { data, imag })
        }
        Token::Ident(s) if s.eq_ignore_ascii_case("not") => {
            *pos += 1;
            let val = parse_primary(tokens, pos, ctx)?;
            Ok(VecVal::real(
                val.data
                    .iter()
                    .map(|v| if *v == 0.0 { 1.0 } else { 0.0 })
                    .collect(),
            ))
        }
        _ => parse_primary(tokens, pos, ctx),
    }
}

fn parse_primary(tokens: &[Token], pos: &mut usize, ctx: &SimContext) -> Result<VecVal, String> {
    let mut val = parse_primary_base(tokens, pos, ctx)?;
    // Apply postfix indexing: vec[idx] — extract a single element from a vector
    while *pos < tokens.len() && tokens[*pos] == Token::LBracket {
        *pos += 1;
        let index = parse_or(tokens, pos, ctx)?;
        if *pos < tokens.len() && tokens[*pos] == Token::RBracket {
            *pos += 1;
        }
        let idx = index.as_scalar() as usize;
        val = if idx < val.data.len() {
            VecVal::scalar(val.data[idx])
        } else {
            VecVal::scalar(0.0)
        };
    }
    Ok(val)
}

fn parse_primary_base(
    tokens: &[Token],
    pos: &mut usize,
    ctx: &SimContext,
) -> Result<VecVal, String> {
    if *pos >= tokens.len() {
        return Err("unexpected end of expression".to_string());
    }

    match &tokens[*pos] {
        Token::Num(v) => {
            let v = *v;
            *pos += 1;
            Ok(VecVal::scalar(v))
        }
        Token::LParen => {
            *pos += 1;
            let val = parse_or(tokens, pos, ctx)?;
            // Check for complex literal: (re, im) → complex number
            if *pos < tokens.len() && tokens[*pos] == Token::Comma {
                *pos += 1; // skip comma
                let imag_val = parse_or(tokens, pos, ctx)?;
                if *pos < tokens.len() && tokens[*pos] == Token::RParen {
                    *pos += 1;
                }
                // Build complex VecVal from (real, imag) pair
                let re = val.as_scalar();
                let im = imag_val.as_scalar();
                return Ok(VecVal {
                    data: vec![re],
                    imag: vec![im],
                });
            }
            if *pos < tokens.len() && tokens[*pos] == Token::RParen {
                *pos += 1;
            }
            Ok(val)
        }
        Token::Str(s) => {
            // Quoted string — resolve as plot-qualified vector: "plotname"
            // In `let val = "temp-sweep"`, this resolves the sweep variable from the named plot
            let s = s.clone();
            *pos += 1;
            // Try as "plotname.vecname" or just "plotname" (returns sweep vec)
            if let Some(dot_pos) = s.find('.') {
                let plot = &s[..dot_pos];
                let vec = &s[dot_pos + 1..];
                if let Some(v) = ctx.find_vector_in_plot(plot, vec) {
                    return Ok(simvec_to_vecval(v));
                }
            }
            // Try as a plot name — return the sweep (first) vector
            let lower = s.to_lowercase();
            for plot in &ctx.plots {
                if plot.name.to_lowercase() == lower
                    && let Some(v) = plot.vecs.first()
                {
                    return Ok(simvec_to_vecval(v));
                }
            }
            // Try as a regular vector name
            if let Some(v) = ctx.find_vector(&s) {
                return Ok(simvec_to_vecval(v));
            }
            // Try resolving as plot name matching plot type (e.g., "temp-sweep" → the DC temp sweep vector)
            // ngspice names DC temp sweep plot as "temp-sweep"
            for plot in &ctx.plots {
                if (plot.name.to_lowercase().contains(&lower.replace('-', "_"))
                    || plot.name.to_lowercase().contains(&lower))
                    && let Some(v) = plot.vecs.first()
                {
                    return Ok(simvec_to_vecval(v));
                }
            }
            Err(format!("cannot resolve \"{s}\""))
        }
        Token::DeviceParam(s) => {
            let s = s.clone();
            *pos += 1;
            // First try as a named vector (e.g., after alter or DC sweep)
            if let Some(v) = ctx.find_vector(&s) {
                return Ok(simvec_to_vecval(v));
            }
            // Try vector-valued device parameter (e.g., @v1[pulse])
            if let Some(vec) = resolve_device_param_vec(&s, ctx) {
                return Ok(vec);
            }
            // Fall back to scalar device/instance parameter from netlist
            if let Some(val) = resolve_device_param(&s, ctx) {
                Ok(VecVal::scalar(val))
            } else {
                Err(format!("cannot resolve device parameter: {s}"))
            }
        }
        Token::Ident(name) => {
            let name = name.clone();
            *pos += 1;

            // Check for function call: name(args)
            if *pos < tokens.len() && tokens[*pos] == Token::LParen {
                // Special case: v(...) and i(...) are vector name lookups, not function calls
                let lower = name.to_lowercase();
                if lower == "v"
                    || lower == "i"
                    || lower == "vm"
                    || lower == "vp"
                    || lower == "vr"
                    || lower == "vi"
                    || lower == "vdb"
                {
                    // Reconstruct the full vector name: v(node), i(src), etc.
                    *pos += 1;
                    let mut inner = String::new();
                    let mut depth = 1;
                    while *pos < tokens.len() && depth > 0 {
                        match &tokens[*pos] {
                            Token::LParen => {
                                inner.push('(');
                                depth += 1;
                            }
                            Token::RParen => {
                                depth -= 1;
                                if depth > 0 {
                                    inner.push(')');
                                }
                            }
                            Token::Num(v) => inner.push_str(&format_num_for_vecname(*v)),
                            Token::Ident(s) => inner.push_str(s),
                            Token::Comma => inner.push(','),
                            Token::Plus => inner.push('+'),
                            Token::Minus => inner.push('-'),
                            Token::Star => inner.push('*'),
                            Token::Slash => inner.push('/'),
                            _ => {}
                        }
                        *pos += 1;
                    }
                    let vec_name = format!("{name}({inner})");
                    if let Some(v) = ctx.find_vector(&vec_name) {
                        return Ok(simvec_to_vecval(v));
                    }
                    // Also try without lowercase normalization
                    let vec_name_lower = vec_name.to_lowercase();
                    if let Some(v) = ctx.find_vector(&vec_name_lower) {
                        return Ok(simvec_to_vecval(v));
                    }
                    return Err(format!("undefined vector: {vec_name}"));
                }

                // Regular function call
                *pos += 1;
                let mut args = Vec::new();
                if *pos < tokens.len() && tokens[*pos] != Token::RParen {
                    args.push(parse_or(tokens, pos, ctx)?);
                    while *pos < tokens.len() && tokens[*pos] == Token::Comma {
                        *pos += 1;
                        args.push(parse_or(tokens, pos, ctx)?);
                    }
                }
                if *pos < tokens.len() && tokens[*pos] == Token::RParen {
                    *pos += 1;
                }
                return eval_function(&name, &args, ctx);
            }

            // Built-in constants
            match name.to_lowercase().as_str() {
                "pi" => return Ok(VecVal::scalar(std::f64::consts::PI)),
                "e" => return Ok(VecVal::scalar(std::f64::consts::E)),
                "i" => return Ok(VecVal::complex_scalar(0.0, 1.0)),
                "true" | "yes" => return Ok(VecVal::scalar(1.0)),
                "false" | "no" => return Ok(VecVal::scalar(0.0)),
                _ => {}
            }

            // Try as vector name
            if let Some(v) = ctx.find_vector(&name) {
                return Ok(simvec_to_vecval(v));
            }

            // Try as plot-qualified name: "plotname.vecname"
            if let Some(dot_pos) = name.find('.') {
                let plot = &name[..dot_pos];
                let vec_name = &name[dot_pos + 1..];
                if let Some(v) = ctx.find_vector_in_plot(plot, vec_name) {
                    return Ok(simvec_to_vecval(v));
                }
            }

            // Try as user-defined function with no args (like a variable)
            // Check if it looks like a number (e.g., after variable interpolation)
            if let Ok(v) = name.parse::<f64>() {
                return Ok(VecVal::scalar(v));
            }

            // Unknown — return 0 (matching ngspice behavior for undefined vectors)
            Err(format!("undefined vector or variable: {name}"))
        }
        other => Err(format!("unexpected token: {other:?}")),
    }
}

fn eval_function(name: &str, args: &[VecVal], ctx: &SimContext) -> Result<VecVal, String> {
    let lower = name.to_lowercase();

    // Check user-defined functions first
    if let Some((param_names, body)) = ctx.functions.get(&lower).cloned() {
        if args.len() != param_names.len() {
            return Err(format!(
                "{name}: expected {} args, got {}",
                param_names.len(),
                args.len()
            ));
        }
        // Substitute parameters into body using word-boundary-aware replacement.
        // This prevents replacing "a" inside "abs" — only standalone identifiers
        // matching the parameter name are replaced.
        let mut expanded = body.clone();
        for (pname, arg) in param_names.iter().zip(args.iter()) {
            expanded = replace_word(&expanded, pname, &arg.as_scalar().to_string());
        }
        return eval_vec_expr(&expanded, ctx);
    }

    match lower.as_str() {
        "abs" => {
            require_args(name, args, 1)?;
            let a = &args[0];
            if a.is_complex() {
                let data: Vec<f64> = (0..a.len())
                    .map(|i| (a.re(i) * a.re(i) + a.im(i) * a.im(i)).sqrt())
                    .collect();
                Ok(VecVal::real(data))
            } else {
                Ok(VecVal::real(a.data.iter().map(|v| v.abs()).collect()))
            }
        }
        "sqrt" => {
            require_args(name, args, 1)?;
            Ok(VecVal::real(
                args[0].data.iter().map(|v| v.sqrt()).collect(),
            ))
        }
        "exp" => {
            require_args(name, args, 1)?;
            Ok(VecVal::real(args[0].data.iter().map(|v| v.exp()).collect()))
        }
        "log" | "ln" => {
            require_args(name, args, 1)?;
            Ok(VecVal::real(args[0].data.iter().map(|v| v.ln()).collect()))
        }
        "log10" => {
            require_args(name, args, 1)?;
            Ok(VecVal::real(
                args[0].data.iter().map(|v| v.log10()).collect(),
            ))
        }
        "sin" => {
            require_args(name, args, 1)?;
            Ok(VecVal::real(args[0].data.iter().map(|v| v.sin()).collect()))
        }
        "cos" => {
            require_args(name, args, 1)?;
            Ok(VecVal::real(args[0].data.iter().map(|v| v.cos()).collect()))
        }
        "vecmax" => {
            require_args(name, args, 1)?;
            let max = args[0]
                .data
                .iter()
                .copied()
                .fold(f64::NEG_INFINITY, f64::max);
            Ok(VecVal::scalar(max))
        }
        "vecmin" => {
            require_args(name, args, 1)?;
            let min = args[0].data.iter().copied().fold(f64::INFINITY, f64::min);
            Ok(VecVal::scalar(min))
        }
        "length" | "vector_length" => {
            require_args(name, args, 1)?;
            Ok(VecVal::scalar(args[0].data.len() as f64))
        }
        "max" => {
            require_args(name, args, 2)?;
            Ok(vec_binop(&args[0], &args[1], f64::max))
        }
        "min" => {
            require_args(name, args, 2)?;
            Ok(vec_binop(&args[0], &args[1], f64::min))
        }
        "v" | "i" | "vm" | "vp" | "vr" | "vi" | "vdb" => {
            // These are handled as vector name lookups in parse_primary, not here
            Err(format!(
                "{name}() should be resolved as vector lookup, not function"
            ))
        }
        "vector" => {
            // vector(n) — create a vector [0, 1, ..., n-1]
            require_args(name, args, 1)?;
            let n = args[0].as_scalar() as usize;
            Ok(VecVal::real((0..n).map(|i| i as f64).collect()))
        }
        "unitvec" => {
            require_args(name, args, 1)?;
            let n = args[0].as_scalar() as usize;
            Ok(VecVal::real(vec![1.0; n]))
        }
        "mean" | "avg" => {
            require_args(name, args, 1)?;
            let sum: f64 = args[0].data.iter().sum();
            let n = args[0].data.len() as f64;
            Ok(VecVal::scalar(if n > 0.0 { sum / n } else { 0.0 }))
        }
        "ceil" | "ceiling" => {
            require_args(name, args, 1)?;
            Ok(VecVal::real(
                args[0].data.iter().map(|v| v.ceil()).collect(),
            ))
        }
        "floor" | "int" => {
            require_args(name, args, 1)?;
            Ok(VecVal::real(
                args[0].data.iter().map(|v| v.floor()).collect(),
            ))
        }
        "nint" | "round" => {
            require_args(name, args, 1)?;
            Ok(VecVal::real(
                args[0].data.iter().map(|v| v.round()).collect(),
            ))
        }
        "tan" => {
            require_args(name, args, 1)?;
            Ok(VecVal::real(args[0].data.iter().map(|v| v.tan()).collect()))
        }
        "atan" => {
            require_args(name, args, 1)?;
            Ok(VecVal::real(
                args[0].data.iter().map(|v| v.atan()).collect(),
            ))
        }
        // pole(n) / zero(n) — look up PZ analysis result vectors
        "pole" | "zero" => {
            require_args(name, args, 1)?;
            let idx = args[0].as_scalar() as usize;
            let vec_name = format!("{}({})", lower, idx);
            if let Some(v) = ctx.find_vector(&vec_name) {
                Ok(simvec_to_vecval(v))
            } else {
                Err(format!("undefined vector: {vec_name}"))
            }
        }
        _ => Err(format!("unknown function: {name}")),
    }
}

fn require_args(name: &str, args: &[VecVal], expected: usize) -> Result<(), String> {
    if args.len() != expected {
        Err(format!(
            "{name}: expected {expected} args, got {}",
            args.len()
        ))
    } else {
        Ok(())
    }
}

/// Resolve a `@device[param]` query from the netlist.
///
/// Searches model definitions and element instance parameters for the given
/// device/model name and parameter. Handles:
/// - `@modelname[param]` — look up model definition parameter
/// - `@instname[param]` — look up instance parameter (e.g., `@v1[dc]`)
fn resolve_device_param(spec: &str, ctx: &SimContext) -> Option<f64> {
    // Parse @device[param]
    let spec = spec.strip_prefix('@')?;
    let bracket = spec.find('[')?;
    let end = spec.find(']')?;
    let device = &spec[..bracket];
    let param = &spec[bracket + 1..end];
    let param_upper = param.to_uppercase();

    // Search resolved model parameters (TEMPER-evaluated) first
    if let Some(params) = ctx.resolved_models.get(&device.to_uppercase()) {
        for p in params {
            if p.name.to_uppercase() == param_upper
                && let thevenin_types::Expr::Num(v) = &p.value
            {
                return Some(*v);
            }
        }
    }

    // Fall back to original model definitions (walking the Circuit directly).
    // A `SimContext` without a driving Circuit (test-only `new(netlist)`
    // construction) skips this fallback; the `resolved_models` lookup above
    // is the only path that works for those contexts and they don't have
    // @device[param] usage in practice.
    let circuit = ctx.circuit()?;
    for model in &circuit.models {
        if model.name.eq_ignore_ascii_case(device) {
            for (name, value) in &model.params {
                if name.to_uppercase() == param_upper
                    && let Some(v) = value_as_real(value)
                {
                    return Some(v);
                }
            }
        }
    }

    // Search element instance parameters (e.g., @v1[dc], @r1[resistance]).
    for element in &circuit.elements {
        if element.name.eq_ignore_ascii_case(device) {
            return resolve_element_param_ir(element, param);
        }
    }

    None
}

/// Coerce a Cirq IR `Value` to `f64`, or `None` for non-numeric variants.
fn value_as_real(value: &cirq_ir::Value) -> Option<f64> {
    match value {
        cirq_ir::Value::Real(v) => Some(*v),
        cirq_ir::Value::Integer(v) => Some(*v as f64),
        cirq_ir::Value::Bool(_) | cirq_ir::Value::String(_) => None,
    }
}

/// Mirror of [`resolve_element_param`] over a Cirq IR `Element`.
fn resolve_element_param_ir(element: &cirq_ir::Element, param: &str) -> Option<f64> {
    let param_lower = param.to_lowercase();
    match element.kind {
        cirq_ir::ElementKind::VoltageSource | cirq_ir::ElementKind::CurrentSource => {
            if param_lower == "dc"
                && let Some(spec) = &element.source_spec
            {
                return spec.dc;
            }
            None
        }
        cirq_ir::ElementKind::Resistor => {
            if matches!(param_lower.as_str(), "resistance" | "r") {
                // SPICE importer normalises the resistance param to "value".
                for (name, value) in &element.params {
                    if name.eq_ignore_ascii_case("value") || name.eq_ignore_ascii_case("resistance")
                    {
                        return value_as_real(value);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Resolve a vector-valued `@device[param]` query (e.g., `@v1[pulse]`).
fn resolve_device_param_vec(spec: &str, ctx: &SimContext) -> Option<VecVal> {
    let spec = spec.strip_prefix('@')?;
    let bracket = spec.find('[')?;
    let end = spec.find(']')?;
    let device = &spec[..bracket];
    let param = &spec[bracket + 1..end];

    let circuit = ctx.circuit()?;
    for element in &circuit.elements {
        if element.name.eq_ignore_ascii_case(device) {
            return resolve_element_param_vec_ir(element, param);
        }
    }
    None
}

/// Mirror of [`resolve_element_param_vec`] over a Cirq IR `Element`.
fn resolve_element_param_vec_ir(element: &cirq_ir::Element, param: &str) -> Option<VecVal> {
    let param_lower = param.to_lowercase();
    let spec = element.source_spec.as_ref()?;
    match element.kind {
        cirq_ir::ElementKind::VoltageSource | cirq_ir::ElementKind::CurrentSource => {
            if param_lower == "pulse"
                && let Some(cirq_ir::Waveform::Pulse {
                    v1,
                    v2,
                    td,
                    tr,
                    tf,
                    pw,
                    per,
                }) = &spec.waveform
            {
                let vals = vec![
                    *v1,
                    *v2,
                    td.unwrap_or(0.0),
                    tr.unwrap_or(0.0),
                    tf.unwrap_or(0.0),
                    pw.unwrap_or(0.0),
                    per.unwrap_or(0.0),
                ];
                return Some(VecVal::real(vals));
            }
            None
        }
        _ => None,
    }
}


/// Strip SPICE unit suffixes from a number string (V, A, W, Hz, Ohm, s).
///
/// `s` (seconds) is stripped so time literals like `1ms`/`200us` parse as
/// `1m`/`200u` (then resolve via SI prefix). Stripping is safe because the
/// tokenizer only reaches this function for tokens that started with a digit
/// or `.` — bare `s` is an identifier and never hits this path.
fn strip_unit_suffix_str(s: &str) -> &str {
    let lower = s.to_lowercase();
    for suffix in &["hz", "ohm", "ohms"] {
        if lower.ends_with(suffix) {
            return &s[..s.len() - suffix.len()];
        }
    }
    for &unit in &['v', 'a', 'w', 's'] {
        if lower.ends_with(unit) {
            return &s[..s.len() - 1];
        }
    }
    s
}

/// Replace a word (identifier) in a string, respecting word boundaries.
///
/// Only replaces `word` when it appears as a standalone identifier — not when
/// it's part of a larger word like "abs" containing "a" or "err" containing "e".
pub fn replace_word(s: &str, word: &str, replacement: &str) -> String {
    if word.is_empty() {
        return s.to_string();
    }
    let mut result = String::with_capacity(s.len());
    let mut i = 0;
    let bytes = s.as_bytes();
    let word_bytes = word.as_bytes();
    let word_len = word_bytes.len();

    while i < bytes.len() {
        if i + word_len <= bytes.len() && bytes[i..i + word_len].eq_ignore_ascii_case(word_bytes) {
            // Check word boundary before
            let before_ok = i == 0 || !is_ident_char(bytes[i - 1]);
            // Check word boundary after
            let after_ok = i + word_len >= bytes.len() || !is_ident_char(bytes[i + word_len]);
            if before_ok && after_ok {
                result.push_str(replacement);
                i += word_len;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Extract real-valued data from a SimVector, using complex magnitudes if needed.
///
/// Noise spectrum vectors (`onoise_spectrum`, `inoise_spectrum`) are stored as
/// V²/Hz (power spectral density) for batch output compatibility, but `.control`
/// scripts expect V/√Hz (amplitude spectral density) — matching ngspice's
/// interactive convention.  This function applies the sqrt conversion automatically.
#[allow(dead_code)]
fn vec_to_real(v: &thevenin_types::SimVector) -> Vec<f64> {
    simvec_to_vecval(v).data
}

/// Convert a SimVector to a VecVal, preserving complex data when present.
fn simvec_to_vecval(v: &thevenin_types::SimVector) -> VecVal {
    let (data, imag): (Vec<f64>, Vec<f64>) = if let Some(complex) = v.data.try_complex() {
        if !complex.is_empty() {
            (
                complex.iter().map(|c| c.re).collect(),
                complex.iter().map(|c| c.im).collect(),
            )
        } else {
            (vec![0.0], vec![])
        }
    } else if let Some(real) = v.data.try_real() {
        if !real.is_empty() {
            (real.to_vec(), vec![])
        } else {
            (vec![0.0], vec![])
        }
    } else {
        (vec![0.0], vec![])
    };
    // Noise spectrum vectors: convert V²/Hz → V/√Hz for .control access.
    let name_lower = v.name.to_lowercase();
    let data = if name_lower.ends_with("noise_spectrum") {
        data.iter().map(|x| x.sqrt()).collect()
    } else {
        data
    };
    VecVal { data, imag }
}

/// Format a number for inclusion in a vector name (e.g., node number).
fn format_num_for_vecname(v: f64) -> String {
    if v == v.floor() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// Complex-aware element-wise binary operation with broadcasting.
///
/// The callback receives (a_re, a_im, b_re, b_im) and returns (re, im).
/// When neither operand is complex, imaginary parts are zero and the result
/// is real-only (no imag allocation).
fn vec_complex_binop(
    a: &VecVal,
    b: &VecVal,
    f: impl Fn(f64, f64, f64, f64) -> (f64, f64),
) -> VecVal {
    let either_complex = a.is_complex() || b.is_complex();
    let len = a.len().max(b.len());
    let mut data = Vec::with_capacity(len);
    let mut imag = if either_complex {
        Vec::with_capacity(len)
    } else {
        vec![]
    };
    for i in 0..len {
        let (ar, ai) = (a.re(i), a.im_broadcast(i));
        let (br, bi) = (b.re(i), b.im_broadcast(i));
        let (re, im) = f(ar, ai, br, bi);
        data.push(re);
        if either_complex {
            imag.push(im);
        }
    }
    VecVal { data, imag }
}

/// Element-wise binary operation with broadcasting (real-only).
fn vec_binop(a: &VecVal, b: &VecVal, f: impl Fn(f64, f64) -> f64) -> VecVal {
    if a.is_scalar() && b.is_scalar() {
        return VecVal::scalar(f(a.data[0], b.data[0]));
    }
    let len = a.data.len().max(b.data.len());
    let data: Vec<f64> = (0..len)
        .map(|i| {
            let av = if a.data.len() == 1 {
                a.data[0]
            } else {
                a.data.get(i).copied().unwrap_or(0.0)
            };
            let bv = if b.data.len() == 1 {
                b.data[0]
            } else {
                b.data.get(i).copied().unwrap_or(0.0)
            };
            f(av, bv)
        })
        .collect();
    VecVal::real(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_ctx() -> SimContext {
        SimContext::new(thevenin_types::Netlist {
            title: String::new(),
            items: Vec::new(),
            analysis: thevenin_types::Analysis::Op,
            source: String::new(),
        })
    }

    #[test]
    fn test_scalar_arithmetic() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("2 + 3", &ctx).unwrap();
        assert_eq!(v.data, vec![5.0]);

        let v = eval_vec_expr("10 / 2 - 1", &ctx).unwrap();
        assert_eq!(v.data, vec![4.0]);

        let v = eval_vec_expr("2 ^ 3", &ctx).unwrap();
        assert_eq!(v.data, vec![8.0]);
    }

    #[test]
    fn test_comparison() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("3 > 2", &ctx).unwrap();
        assert_eq!(v.data, vec![1.0]);

        let v = eval_vec_expr("1 > 2", &ctx).unwrap();
        assert_eq!(v.data, vec![0.0]);
    }

    #[test]
    fn test_functions() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("abs(-5)", &ctx).unwrap();
        assert_eq!(v.data, vec![5.0]);

        let v = eval_vec_expr("sqrt(9)", &ctx).unwrap();
        assert_eq!(v.data, vec![3.0]);
    }

    #[test]
    fn test_unary_minus() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("-3 + 5", &ctx).unwrap();
        assert_eq!(v.data, vec![2.0]);
    }

    #[test]
    fn test_spice_numbers() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("1k + 500", &ctx).unwrap();
        assert_eq!(v.data, vec![1500.0]);
    }

    #[test]
    fn test_replace_word() {
        assert_eq!(
            replace_word("abs(a-b)>err*abs(b)", "a", "42"),
            "abs(42-b)>err*abs(b)"
        );
        assert_eq!(
            replace_word("abs(a-b)>err*abs(b)", "b", "7"),
            "abs(a-7)>err*abs(7)"
        );
        assert_eq!(
            replace_word("abs(a-b)>err*abs(b)", "err", "0.1"),
            "abs(a-b)>0.1*abs(b)"
        );
    }

    #[test]
    fn test_user_defined_function() {
        let mut ctx = empty_ctx();
        // Define mismatch(a,b,err) abs(a-b)>err
        ctx.functions.insert(
            "mismatch".to_string(),
            (
                vec!["a".to_string(), "b".to_string(), "err".to_string()],
                "abs(a-b)>err".to_string(),
            ),
        );
        let v = eval_vec_expr("mismatch(10, 11, 0.5)", &ctx).unwrap();
        assert_eq!(v.data, vec![1.0]); // |10-11| = 1 > 0.5
        let v = eval_vec_expr("mismatch(10, 10.1, 0.5)", &ctx).unwrap();
        assert_eq!(v.data, vec![0.0]); // |10-10.1| = 0.1 < 0.5
    }

    // -----------------------------------------------------------------------
    // Comparison operators
    // -----------------------------------------------------------------------

    #[test]
    fn test_less_than() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("1 < 2", &ctx).unwrap();
        assert_eq!(v.data, vec![1.0]);
        let v = eval_vec_expr("2 < 1", &ctx).unwrap();
        assert_eq!(v.data, vec![0.0]);
    }

    #[test]
    fn test_greater_eq() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("3 >= 3", &ctx).unwrap();
        assert_eq!(v.data, vec![1.0]);
        let v = eval_vec_expr("3 >= 4", &ctx).unwrap();
        assert_eq!(v.data, vec![0.0]);
    }

    #[test]
    fn test_less_eq() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("3 <= 3", &ctx).unwrap();
        assert_eq!(v.data, vec![1.0]);
        let v = eval_vec_expr("4 <= 3", &ctx).unwrap();
        assert_eq!(v.data, vec![0.0]);
    }

    #[test]
    fn test_equality() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("5 = 5", &ctx).unwrap();
        assert_eq!(v.data, vec![1.0]);
        let v = eval_vec_expr("5 = 6", &ctx).unwrap();
        assert_eq!(v.data, vec![0.0]);
    }

    #[test]
    fn test_not_equal() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("5 <> 6", &ctx).unwrap();
        assert_eq!(v.data, vec![1.0]);
        let v = eval_vec_expr("5 <> 5", &ctx).unwrap();
        assert_eq!(v.data, vec![0.0]);
    }

    // -----------------------------------------------------------------------
    // Logical operators
    // -----------------------------------------------------------------------

    #[test]
    fn test_logical_or() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("0 or 1", &ctx).unwrap();
        assert_eq!(v.data, vec![1.0]);
        let v = eval_vec_expr("0 or 0", &ctx).unwrap();
        assert_eq!(v.data, vec![0.0]);
        let v = eval_vec_expr("1 or 0", &ctx).unwrap();
        assert_eq!(v.data, vec![1.0]);
    }

    #[test]
    fn test_logical_and() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("1 and 1", &ctx).unwrap();
        assert_eq!(v.data, vec![1.0]);
        let v = eval_vec_expr("1 and 0", &ctx).unwrap();
        assert_eq!(v.data, vec![0.0]);
        let v = eval_vec_expr("0 and 1", &ctx).unwrap();
        assert_eq!(v.data, vec![0.0]);
    }

    // -----------------------------------------------------------------------
    // Built-in math functions
    // -----------------------------------------------------------------------

    #[test]
    fn test_exp() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("exp(0)", &ctx).unwrap();
        assert!((v.data[0] - 1.0).abs() < 1e-10);
        let v = eval_vec_expr("exp(1)", &ctx).unwrap();
        assert!((v.data[0] - std::f64::consts::E).abs() < 1e-10);
    }

    #[test]
    fn test_log_ln() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("log(1)", &ctx).unwrap();
        assert!((v.data[0]).abs() < 1e-10);
        let v = eval_vec_expr("ln(e)", &ctx).unwrap();
        assert!((v.data[0] - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_log10() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("log10(100)", &ctx).unwrap();
        assert!((v.data[0] - 2.0).abs() < 1e-10);
    }

    #[test]
    fn test_sin() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("sin(0)", &ctx).unwrap();
        assert!(v.data[0].abs() < 1e-10);
    }

    #[test]
    fn test_cos() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("cos(0)", &ctx).unwrap();
        assert!((v.data[0] - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_tan() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("tan(0)", &ctx).unwrap();
        assert!(v.data[0].abs() < 1e-10);
    }

    #[test]
    fn test_atan() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("atan(1)", &ctx).unwrap();
        assert!((v.data[0] - std::f64::consts::FRAC_PI_4).abs() < 1e-10);
    }

    #[test]
    fn test_vecmax() {
        let mut ctx = empty_ctx();
        ctx.user_vectors.push(thevenin_types::SimVector::real(
            "vals",
            vec![1.0, 5.0, 3.0, 2.0],
        ));
        let v = eval_vec_expr("vecmax(vals)", &ctx).unwrap();
        assert_eq!(v.data, vec![5.0]);
    }

    #[test]
    fn test_vecmin() {
        let mut ctx = empty_ctx();
        ctx.user_vectors.push(thevenin_types::SimVector::real(
            "vals",
            vec![1.0, 5.0, 3.0, -2.0],
        ));
        let v = eval_vec_expr("vecmin(vals)", &ctx).unwrap();
        assert_eq!(v.data, vec![-2.0]);
    }

    #[test]
    fn test_length() {
        let mut ctx = empty_ctx();
        ctx.user_vectors
            .push(thevenin_types::SimVector::real("vals", vec![1.0, 2.0, 3.0]));
        let v = eval_vec_expr("length(vals)", &ctx).unwrap();
        assert_eq!(v.data, vec![3.0]);
    }

    #[test]
    fn test_max_min_scalar() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("max(3, 7)", &ctx).unwrap();
        assert_eq!(v.data, vec![7.0]);
        let v = eval_vec_expr("min(3, 7)", &ctx).unwrap();
        assert_eq!(v.data, vec![3.0]);
    }

    #[test]
    fn test_vector_fn() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("vector(5)", &ctx).unwrap();
        assert_eq!(v.data, vec![0.0, 1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_unitvec_fn() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("unitvec(4)", &ctx).unwrap();
        assert_eq!(v.data, vec![1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn test_mean_avg() {
        let mut ctx = empty_ctx();
        ctx.user_vectors
            .push(thevenin_types::SimVector::real("vals", vec![2.0, 4.0, 6.0]));
        let v = eval_vec_expr("mean(vals)", &ctx).unwrap();
        assert!((v.data[0] - 4.0).abs() < 1e-10);
        let v = eval_vec_expr("avg(vals)", &ctx).unwrap();
        assert!((v.data[0] - 4.0).abs() < 1e-10);
    }

    #[test]
    fn test_ceil() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("ceil(2.3)", &ctx).unwrap();
        assert_eq!(v.data, vec![3.0]);
    }

    #[test]
    fn test_floor_int() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("floor(2.7)", &ctx).unwrap();
        assert_eq!(v.data, vec![2.0]);
        let v = eval_vec_expr("int(2.7)", &ctx).unwrap();
        assert_eq!(v.data, vec![2.0]);
    }

    #[test]
    fn test_nint_round() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("nint(2.5)", &ctx).unwrap();
        assert_eq!(v.data, vec![3.0]);
        let v = eval_vec_expr("round(2.4)", &ctx).unwrap();
        assert_eq!(v.data, vec![2.0]);
    }

    // -----------------------------------------------------------------------
    // eval_condition
    // -----------------------------------------------------------------------

    #[test]
    fn test_eval_condition_truthy() {
        let ctx = empty_ctx();
        assert!(eval_condition("1", &ctx).unwrap());
        assert!(eval_condition("3 > 2", &ctx).unwrap());
    }

    #[test]
    fn test_eval_condition_falsy() {
        let ctx = empty_ctx();
        assert!(!eval_condition("0", &ctx).unwrap());
        assert!(!eval_condition("1 > 2", &ctx).unwrap());
    }

    // -----------------------------------------------------------------------
    // Unary not
    // -----------------------------------------------------------------------

    #[test]
    fn test_unary_not() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("not 0", &ctx).unwrap();
        assert_eq!(v.data, vec![1.0]);
        let v = eval_vec_expr("not 1", &ctx).unwrap();
        assert_eq!(v.data, vec![0.0]);
        let v = eval_vec_expr("not 42", &ctx).unwrap();
        assert_eq!(v.data, vec![0.0]);
    }

    // -----------------------------------------------------------------------
    // Multiplication and division with vectors (broadcasting)
    // -----------------------------------------------------------------------

    #[test]
    fn test_mul_vectors_broadcast() {
        let mut ctx = empty_ctx();
        ctx.user_vectors
            .push(thevenin_types::SimVector::real("a", vec![1.0, 2.0, 3.0]));
        // scalar * vector
        let v = eval_vec_expr("2 * a", &ctx).unwrap();
        assert_eq!(v.data, vec![2.0, 4.0, 6.0]);
    }

    #[test]
    fn test_div_vectors_broadcast() {
        let mut ctx = empty_ctx();
        ctx.user_vectors
            .push(thevenin_types::SimVector::real("a", vec![10.0, 20.0, 30.0]));
        let v = eval_vec_expr("a / 10", &ctx).unwrap();
        assert_eq!(v.data, vec![1.0, 2.0, 3.0]);
    }

    // -----------------------------------------------------------------------
    // Nested expressions
    // -----------------------------------------------------------------------

    #[test]
    fn test_nested_sqrt_abs() {
        let ctx = empty_ctx();
        let v = eval_vec_expr("sqrt(abs(-16))", &ctx).unwrap();
        assert!((v.data[0] - 4.0).abs() < 1e-10);
    }

    // -----------------------------------------------------------------------
    // Error cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_unknown_function_error() {
        let ctx = empty_ctx();
        let result = eval_vec_expr("bogus(1)", &ctx);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown function"));
    }

    #[test]
    fn test_wrong_arg_count_error() {
        let ctx = empty_ctx();
        let result = eval_vec_expr("abs(1, 2)", &ctx);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected 1 args, got 2"));
    }

    // -----------------------------------------------------------------------
    // VecVal methods
    // -----------------------------------------------------------------------

    #[test]
    fn test_vecval_scalar() {
        let v = VecVal::scalar(42.0);
        assert_eq!(v.data, vec![42.0]);
        assert!(v.imag.is_empty());
        assert!(v.is_scalar());
        assert!(!v.is_complex());
    }

    #[test]
    fn test_vecval_complex_scalar() {
        let v = VecVal::complex_scalar(3.0, 4.0);
        assert_eq!(v.data, vec![3.0]);
        assert_eq!(v.imag, vec![4.0]);
        assert!(v.is_scalar());
        assert!(v.is_complex());
    }

    #[test]
    fn test_vecval_as_scalar() {
        let v = VecVal::real(vec![10.0, 20.0, 30.0]);
        // as_scalar returns last element
        assert_eq!(v.as_scalar(), 30.0);
    }

    #[test]
    fn test_vecval_as_scalar_empty() {
        let v = VecVal::real(vec![]);
        assert_eq!(v.as_scalar(), 0.0);
    }

    #[test]
    fn test_vecval_is_truthy() {
        assert!(VecVal::scalar(1.0).is_truthy());
        assert!(VecVal::scalar(-1.0).is_truthy());
        assert!(!VecVal::scalar(0.0).is_truthy());
    }
}
