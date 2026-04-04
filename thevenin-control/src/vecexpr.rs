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
    let mut left = parse_unary(tokens, pos, ctx)?;
    while *pos < tokens.len() {
        match &tokens[*pos] {
            Token::Star => {
                *pos += 1;
                let right = parse_unary(tokens, pos, ctx)?;
                left = vec_complex_binop(&left, &right, |ar, ai, br, bi| {
                    (ar * br - ai * bi, ar * bi + ai * br)
                });
            }
            Token::Slash => {
                *pos += 1;
                let right = parse_unary(tokens, pos, ctx)?;
                left = vec_complex_binop(&left, &right, |ar, ai, br, bi| {
                    let denom = br * br + bi * bi;
                    if denom != 0.0 {
                        ((ar * br + ai * bi) / denom, (ai * br - ar * bi) / denom)
                    } else {
                        (f64::INFINITY, 0.0)
                    }
                });
            }
            Token::Caret => {
                *pos += 1;
                let right = parse_unary(tokens, pos, ctx)?;
                left = vec_binop(&left, &right, |a, b| a.powf(b));
            }
            _ => break,
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

    // Fall back to original model definitions
    for item in &ctx.netlist.items {
        if let thevenin_types::Item::Model(model) = item
            && model.name.eq_ignore_ascii_case(device)
        {
            for p in &model.params {
                if p.name.to_uppercase() == param_upper
                    && let thevenin_types::Expr::Num(v) = &p.value
                {
                    return Some(*v);
                }
            }
        }
    }

    // Search element instance parameters (e.g., @v1[dc], @r1[resistance])
    for item in &ctx.netlist.items {
        if let thevenin_types::Item::Element(el) = item
            && el.name.eq_ignore_ascii_case(device)
        {
            return resolve_element_param(&el.kind, param);
        }
    }

    None
}

/// Resolve a vector-valued `@device[param]` query (e.g., `@v1[pulse]`).
fn resolve_device_param_vec(spec: &str, ctx: &SimContext) -> Option<VecVal> {
    let spec = spec.strip_prefix('@')?;
    let bracket = spec.find('[')?;
    let end = spec.find(']')?;
    let device = &spec[..bracket];
    let param = &spec[bracket + 1..end];

    for item in &ctx.netlist.items {
        if let thevenin_types::Item::Element(el) = item
            && el.name.eq_ignore_ascii_case(device)
        {
            return resolve_element_param_vec(&el.kind, param);
        }
    }
    None
}

/// Resolve a vector-valued parameter from an element's kind (e.g., pulse waveform).
fn resolve_element_param_vec(kind: &thevenin_types::ElementKind, param: &str) -> Option<VecVal> {
    use thevenin_types::{ElementKind, Expr, Waveform};
    let param_lower = param.to_lowercase();
    match kind {
        ElementKind::VoltageSource { source, .. } | ElementKind::CurrentSource { source, .. } => {
            if param_lower == "pulse"
                && let Some(Waveform::Pulse {
                    v1,
                    v2,
                    td,
                    tr,
                    tf,
                    pw,
                    per,
                }) = &source.waveform
            {
                let expr_val = |e: &Expr| -> f64 { if let Expr::Num(v) = e { *v } else { 0.0 } };
                let vals = vec![
                    expr_val(v1),
                    expr_val(v2),
                    td.as_ref().map_or(0.0, expr_val),
                    tr.as_ref().map_or(0.0, expr_val),
                    tf.as_ref().map_or(0.0, expr_val),
                    pw.as_ref().map_or(0.0, expr_val),
                    per.as_ref().map_or(0.0, expr_val),
                ];
                return Some(VecVal::real(vals));
            }
            None
        }
        _ => None,
    }
}

/// Resolve a parameter from an element's kind.
fn resolve_element_param(kind: &thevenin_types::ElementKind, param: &str) -> Option<f64> {
    use thevenin_types::ElementKind;
    let param_lower = param.to_lowercase();
    match kind {
        ElementKind::VoltageSource { source, .. } => match param_lower.as_str() {
            "dc" => source.dc.as_ref().and_then(|e| {
                if let thevenin_types::Expr::Num(v) = e {
                    Some(*v)
                } else {
                    None
                }
            }),
            _ => None,
        },
        ElementKind::Resistor { value, .. } => match param_lower.as_str() {
            "resistance" | "r" => {
                if let thevenin_types::Expr::Num(v) = value {
                    Some(*v)
                } else {
                    None
                }
            }
            _ => None,
        },
        _ => None,
    }
}

/// Strip SPICE unit suffixes from a number string (V, A, W, Hz, Ohm).
fn strip_unit_suffix_str(s: &str) -> &str {
    let lower = s.to_lowercase();
    for suffix in &["hz", "ohm", "ohms"] {
        if lower.ends_with(suffix) {
            return &s[..s.len() - suffix.len()];
        }
    }
    for &unit in &['v', 'a', 'w'] {
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
}
