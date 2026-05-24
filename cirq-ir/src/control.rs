//! Typed AST and parser for `.control` blocks.
//!
//! These types are the IR-level representation of a `.control` block's
//! statement structure. They live here so the IR can carry the parsed form
//! alongside the verbatim source — interpreters consume the typed AST
//! directly instead of re-parsing strings at execution time.
//!
//! Expressions inside statements (the right-hand side of `let`, the
//! condition on `if`, etc.) remain stringly typed; their evaluation against
//! the running vector environment is the interpreter's job, not the IR's.

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

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
    /// `while <cond> ... end`
    ///
    /// Executes `body` repeatedly while `cond` evaluates to a non-zero value.
    /// `cond` is re-evaluated against the live vector environment on every
    /// iteration. The executor caps iteration count at
    /// [`MAX_LOOP_ITERS`] to keep a runaway condition from hanging the
    /// interpreter.
    While { cond: String, body: Vec<Statement> },
    /// `repeat <n> ... end`
    ///
    /// Executes `body` exactly `n` times. `count` is an expression that is
    /// evaluated once at entry (matching ngspice's semantics); mutating the
    /// referenced variables inside the body does not change the loop count.
    /// `n <= 0` ⇒ zero iterations. Hard-capped at [`MAX_LOOP_ITERS`].
    Repeat { count: String, body: Vec<Statement> },
    /// `save <vec_spec> [<vec_spec> ...]`
    ///
    /// Inside `.control`, `save v(out) i(v1)` appends to the same recording
    /// set that the netlist-level `.save` directive populates (i.e. it
    /// extends `Circuit::save`) so the next `run` / `op` / `tran` / etc.
    /// honours the additions. The strings are not re-parsed here — the
    /// existing output-vector parser handles them downstream.
    Save { specs: Vec<String> },
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
    /// `write [filename] [vector_list]` — write simulation results to a
    /// file. With no filename defaults to `thevenin.raw`; with no vector
    /// list saves all vectors from the current plot. Format is determined
    /// by extension: `.csv` → CSV, anything else → ngspice raw (binary
    /// unless the `filetype` variable is set to `ascii`).
    Write {
        file: Option<String>,
        vectors: Vec<String>,
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

/// Upper bound on the number of iterations any `.control` loop (`while` /
/// `repeat`) is allowed to execute before the interpreter aborts the run.
///
/// Matches ngspice's hardcoded loop cap. Exposed so that both the IR and
/// the executor agree on the limit.
pub const MAX_LOOP_ITERS: usize = 10_000;

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Parse raw `.control` lines into a list of statements.
pub fn parse_control_block(lines: &[String]) -> Result<Vec<Statement>, String> {
    let mut stmts = Vec::new();
    let mut iter = lines.iter().enumerate().peekable();
    parse_block(&mut iter, &mut stmts, None)?;
    Ok(stmts)
}

type LineIter<'a> = std::iter::Peekable<std::iter::Enumerate<std::slice::Iter<'a, String>>>;

fn parse_block(
    iter: &mut LineIter<'_>,
    stmts: &mut Vec<Statement>,
    terminator: Option<&str>,
) -> Result<(), String> {
    while let Some(&(line_no, line)) = iter.peek() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('*') {
            iter.next();
            stmts.push(Statement::Comment);
            continue;
        }

        let lower = trimmed.to_lowercase();
        let keyword = lower.split_whitespace().next().unwrap_or("");

        // Check for terminators
        match keyword {
            "end" | ".endc" => {
                if terminator.is_some() {
                    iter.next();
                    return Ok(());
                }
                iter.next();
                continue;
            }
            "else" => {
                if terminator == Some("if") {
                    // Don't consume — caller handles else
                    return Ok(());
                }
                iter.next();
                continue;
            }
            _ => {}
        }

        iter.next();
        let stmt = parse_statement(trimmed, line_no, iter)?;
        stmts.push(stmt);
    }

    if let Some(term) = terminator {
        return Err(format!("unterminated {term} block"));
    }
    Ok(())
}

fn parse_statement(
    line: &str,
    _line_no: usize,
    iter: &mut LineIter<'_>,
) -> Result<Statement, String> {
    // Strip inline comments ($ not preceded by &)
    let line = strip_inline_comment(line);
    let trimmed = line.trim();

    let lower = trimmed.to_lowercase();
    let keyword = lower.split_whitespace().next().unwrap_or("");
    let rest = trimmed[keyword.len()..].trim();

    match keyword {
        "let" => parse_let(rest),
        "echo" => Ok(Statement::Echo(parse_echo(rest))),
        "if" => parse_if(rest, iter),
        "foreach" => parse_foreach(rest, iter),
        "while" => parse_while(rest, iter),
        "repeat" => parse_repeat(rest, iter),
        "save" => Ok(parse_save(rest)),
        "quit" => parse_quit(rest),
        "set" => parse_set(rest),
        "setplot" => Ok(Statement::Setplot(rest.to_string())),
        "define" => parse_define(rest),
        "compose" => parse_compose(rest),
        "alter" => parse_alter(rest),
        "strcmp" => parse_strcmp(rest),
        "print" => parse_print(rest),
        "write" => Ok(parse_write(rest)),
        "eprint" | "eprvcd" => Ok(Statement::Eprint(
            rest.split_whitespace().map(|s| s.to_string()).collect(),
        )),
        "stop" => parse_stop(rest),
        "resume" => Ok(Statement::Resume),
        "op" | "dc" | "ac" | "tran" | "sens" | "noise" | "pz" | "tf" | "run" => {
            Ok(Statement::RunAnalysis(trimmed.to_string()))
        }
        _ => {
            // Could be a comment starting with $ or unknown command
            if trimmed.starts_with('$') {
                Ok(Statement::Comment)
            } else {
                // Treat unknown commands as comments/no-ops for robustness
                Ok(Statement::Comment)
            }
        }
    }
}

fn strip_inline_comment(line: &str) -> String {
    // Strip `$ comment` but not `$var` or `$&var`
    let mut result = String::new();
    let mut chars = line.chars().peekable();
    let mut in_quotes = false;
    while let Some(c) = chars.next() {
        if c == '"' {
            in_quotes = !in_quotes;
            result.push(c);
        } else if c == '$' && !in_quotes {
            // Check if it's a variable reference ($var or $&var) or a comment
            if let Some(&next) = chars.peek() {
                if next == ' ' || next == '\t' {
                    // Inline comment — stop here
                    break;
                }
                // Variable reference — keep going
                result.push(c);
            } else {
                break;
            }
        } else {
            result.push(c);
        }
    }
    result
}

fn parse_let(rest: &str) -> Result<Statement, String> {
    // `name = expr`
    if let Some(eq_pos) = rest.find('=') {
        let name = rest[..eq_pos].trim().to_string();
        let expr = rest[eq_pos + 1..].trim().to_string();
        Ok(Statement::Let { name, expr })
    } else {
        Err(format!("let without '=': {rest}"))
    }
}

fn parse_echo(rest: &str) -> Vec<EchoFragment> {
    let mut fragments = Vec::new();
    let mut current = String::new();
    let mut chars = rest.chars().peekable();
    let mut in_quotes = false;

    while let Some(c) = chars.next() {
        if c == '"' {
            in_quotes = !in_quotes;
            continue;
        }
        if c == '$' {
            // Flush literal
            if !current.is_empty() {
                fragments.push(EchoFragment::Literal(current.clone()));
                current.clear();
            }
            // Check for $& (vector scalar)
            if chars.peek() == Some(&'&') {
                chars.next();
                let name: String = chars
                    .by_ref()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                fragments.push(EchoFragment::VecScalar(name));
            } else {
                let name: String = chars
                    .by_ref()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                fragments.push(EchoFragment::VarRef(name));
            }
            continue;
        }
        current.push(c);
    }
    if !current.is_empty() {
        fragments.push(EchoFragment::Literal(current));
    }
    fragments
}

fn parse_if(cond: &str, iter: &mut LineIter<'_>) -> Result<Statement, String> {
    let mut body = Vec::new();
    parse_block(iter, &mut body, Some("if"))?;

    let mut else_body = Vec::new();
    // Check if we stopped at an `else`
    if let Some(&(_, line)) = iter.peek() {
        let kw = line.trim().to_lowercase();
        if kw.starts_with("else") {
            iter.next();
            parse_block(iter, &mut else_body, Some("if"))?;
        }
    }

    Ok(Statement::If {
        cond: cond.to_string(),
        body,
        else_body,
    })
}

fn parse_foreach(rest: &str, iter: &mut LineIter<'_>) -> Result<Statement, String> {
    let parts: Vec<&str> = rest.split_whitespace().collect();
    if parts.is_empty() {
        return Err("foreach without variable name".to_string());
    }
    let var = parts[0].to_string();
    let values: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();

    let mut body = Vec::new();
    parse_block(iter, &mut body, Some("foreach"))?;

    Ok(Statement::Foreach { var, values, body })
}

fn parse_while(rest: &str, iter: &mut LineIter<'_>) -> Result<Statement, String> {
    let cond = rest.trim();
    if cond.is_empty() {
        return Err("while without condition".to_string());
    }
    let mut body = Vec::new();
    parse_block(iter, &mut body, Some("while"))?;
    Ok(Statement::While {
        cond: cond.to_string(),
        body,
    })
}

fn parse_repeat(rest: &str, iter: &mut LineIter<'_>) -> Result<Statement, String> {
    let count = rest.trim();
    if count.is_empty() {
        return Err("repeat without count".to_string());
    }
    let mut body = Vec::new();
    parse_block(iter, &mut body, Some("repeat"))?;
    Ok(Statement::Repeat {
        count: count.to_string(),
        body,
    })
}

/// Parse a `.control`-level `save` command: `save v(out) i(v1) ...`.
///
/// Whitespace splits the spec list; empty `save` is a no-op (matches
/// ngspice). Specs are not validated here — downstream consumers parse them
/// via the existing output-vector parser.
fn parse_save(rest: &str) -> Statement {
    let specs: Vec<String> = rest.split_whitespace().map(|s| s.to_string()).collect();
    Statement::Save { specs }
}

fn parse_quit(rest: &str) -> Result<Statement, String> {
    if rest.is_empty() {
        Ok(Statement::Quit(Some(0)))
    } else {
        let code = rest
            .trim()
            .parse::<i32>()
            .map_err(|_| format!("invalid quit code: {rest}"))?;
        Ok(Statement::Quit(Some(code)))
    }
}

fn parse_set(rest: &str) -> Result<Statement, String> {
    let mut pairs = Vec::new();
    // Simple parsing: split on whitespace, look for key=value or just key
    let mut remaining = rest;
    while !remaining.is_empty() {
        remaining = remaining.trim();
        if remaining.is_empty() {
            break;
        }
        if let Some(eq_pos) = remaining.find('=') {
            let before_eq = remaining[..eq_pos].trim();
            // Key is the last whitespace-separated token before =
            let key = before_eq
                .split_whitespace()
                .next_back()
                .unwrap_or(before_eq)
                .to_string();
            let after_eq = remaining[eq_pos + 1..].trim();
            // Value is until next whitespace (or quoted)
            let (val, rest_after) = if let Some(stripped) = after_eq.strip_prefix('"') {
                if let Some(end) = stripped.find('"') {
                    (stripped[..end].to_string(), stripped[end + 1..].trim())
                } else {
                    (stripped.to_string(), "")
                }
            } else {
                let end = after_eq.find(' ').unwrap_or(after_eq.len());
                (after_eq[..end].to_string(), after_eq[end..].trim())
            };
            pairs.push((key, Some(val)));
            remaining = rest_after;
        } else {
            let end = remaining.find(' ').unwrap_or(remaining.len());
            let key = remaining[..end].to_string();
            pairs.push((key, None));
            remaining = remaining[end..].trim();
        }
    }
    Ok(Statement::Set(pairs))
}

fn parse_define(rest: &str) -> Result<Statement, String> {
    // `name(arg1,arg2,...) body`
    if let Some(paren_start) = rest.find('(') {
        if let Some(paren_end) = rest.find(')') {
            let name = rest[..paren_start].trim().to_string();
            let args_str = &rest[paren_start + 1..paren_end];
            let args: Vec<String> = args_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            let body = rest[paren_end + 1..].trim().to_string();
            Ok(Statement::Define { name, args, body })
        } else {
            Err(format!("define: missing ')': {rest}"))
        }
    } else {
        Err(format!("define: missing '(': {rest}"))
    }
}

fn parse_compose(rest: &str) -> Result<Statement, String> {
    // `name values expr1 expr2 ...`
    // Expressions may contain parentheses (e.g. v(n1), ln(2.7), i(vm2)/i(vm1))
    let parts: Vec<&str> = rest.split_whitespace().collect();
    if parts.len() < 2 {
        return Err(format!("compose: need name and values: {rest}"));
    }
    let name = parts[0].to_string();
    let start = if parts[1].eq_ignore_ascii_case("values") {
        2
    } else {
        1
    };
    // Re-join tokens respecting parenthesis balance
    let mut value_exprs = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    for part in &parts[start..] {
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(part);
        depth += part.chars().filter(|&c| c == '(').count() as i32;
        depth -= part.chars().filter(|&c| c == ')').count() as i32;
        if depth <= 0 {
            value_exprs.push(current.clone());
            current.clear();
            depth = 0;
        }
    }
    if !current.is_empty() {
        value_exprs.push(current);
    }
    Ok(Statement::Compose { name, value_exprs })
}

fn parse_alter(rest: &str) -> Result<Statement, String> {
    // `@device[param] = value` or `@device[param] = [ v1 v2 ... ]`
    if let Some(eq_pos) = rest.find('=') {
        let spec = rest[..eq_pos].trim().to_string();
        let val_str = rest[eq_pos + 1..].trim();
        let value = if val_str.starts_with('[') {
            let inner = val_str.trim_start_matches('[').trim_end_matches(']').trim();
            let vals: Result<Vec<f64>, _> =
                inner.split_whitespace().map(parse_spice_number).collect();
            AlterValue::Vector(vals.map_err(|e| format!("alter: {e}"))?)
        } else {
            AlterValue::Scalar(parse_spice_number(val_str).map_err(|e| format!("alter: {e}"))?)
        };
        Ok(Statement::Alter { spec, value })
    } else {
        Err(format!("alter without '=': {rest}"))
    }
}

fn parse_strcmp(rest: &str) -> Result<Statement, String> {
    let parts: Vec<&str> = rest.split_whitespace().collect();
    if parts.len() < 3 {
        return Err(format!("strcmp: need result, a, b: {rest}"));
    }
    Ok(Statement::Strcmp {
        result: parts[0].to_string(),
        a: parts[1].to_string(),
        b: parts[2].to_string(),
    })
}

fn parse_print(rest: &str) -> Result<Statement, String> {
    // Check for `> file` redirect
    let (exprs_str, file) = if let Some(redir_pos) = rest.find('>') {
        let file = rest[redir_pos + 1..].trim().to_string();
        let exprs_str = rest[..redir_pos].trim();
        (exprs_str, Some(file))
    } else {
        (rest, None)
    };
    let exprs: Vec<String> = exprs_str
        .split_whitespace()
        .filter(|s| !s.eq_ignore_ascii_case("col") && !s.eq_ignore_ascii_case("line"))
        .map(|s| s.to_string())
        .collect();
    Ok(Statement::Print { exprs, file })
}

/// Parse `write [filename] [vec1 vec2 ...]`. The first token (if any) is
/// treated as the filename when it looks like one (contains `.` or `/`,
/// or has no parens / no `$`); otherwise it's part of the vector list and
/// the default filename `thevenin.raw` is used.
fn parse_write(rest: &str) -> Statement {
    let parts: Vec<&str> = rest.split_whitespace().collect();
    if parts.is_empty() {
        return Statement::Write {
            file: None,
            vectors: Vec::new(),
        };
    }
    let first = parts[0];
    let looks_like_filename = first.contains('.')
        || first.contains('/')
        || first.contains('\\')
        || (!first.contains('(') && !first.starts_with('$') && !first.starts_with('@'));
    let (file, vectors) = if looks_like_filename {
        (
            Some(first.to_string()),
            parts[1..].iter().map(|s| s.to_string()).collect(),
        )
    } else {
        (None, parts.iter().map(|s| s.to_string()).collect())
    };
    Statement::Write { file, vectors }
}

/// Parse `stop when <condition>`. Only `stop when time = <value>` is supported
/// today (the form `regression/misc/resume-1.cir` uses); other conditions
/// return an error so the failure is loud rather than silently ignored.
fn parse_stop(rest: &str) -> Result<Statement, String> {
    let rest = rest.trim();
    let lower = rest.to_lowercase();
    let after_when = lower
        .strip_prefix("when")
        .ok_or_else(|| format!("stop: expected 'when', got: {rest}"))?
        .trim_start();
    // Re-slice the original rest to preserve case for the value, using the
    // length difference to locate where the condition body begins.
    let body = rest[rest.len() - after_when.len()..].trim();

    // Time form: `time = <value>` or `time=<value>`.
    if let Some(after_time) = body
        .strip_prefix("time")
        .or_else(|| body.strip_prefix("TIME"))
    {
        let after_eq = after_time
            .trim_start()
            .strip_prefix('=')
            .ok_or_else(|| format!("stop when time: expected '=', got: {body}"))?
            .trim();
        // Time values commonly carry an `s` (seconds) suffix on top of an SI
        // prefix — e.g. `1ms` is one millisecond. parse_spice_number doesn't
        // strip 's' (it conflicts with `f`/`s` SI handling elsewhere), so
        // strip it here before delegating.
        let value_str = after_eq
            .strip_suffix('s')
            .or_else(|| after_eq.strip_suffix('S'))
            .unwrap_or(after_eq);
        let val = parse_spice_number(value_str)
            .map_err(|e| format!("stop when time: cannot parse value '{after_eq}': {e}"))?;
        return Ok(Statement::StopWhen(StopCondition::TimeEq(val)));
    }

    Err(format!(
        "stop when: only `time = <value>` is supported, got: {body}"
    ))
}

/// Parse a SPICE number with optional SI suffix.
///
/// Handles SPICE conventions: `42mA` = 42e-3, `1kHz` = 1e3, `100uF` = 100e-6.
/// Trailing unit characters (V, A, Hz, Ohm, s, F, H, etc.) are stripped.
pub fn parse_spice_number(s: &str) -> Result<f64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty number".to_string());
    }

    // Try direct parse first
    if let Ok(v) = s.parse::<f64>() {
        return Ok(v);
    }

    // Strip trailing unit designators (V, A, Hz, Ohm, s, F, H, W, etc.)
    let s = strip_unit_suffix(s);

    // Try again after stripping units
    if let Ok(v) = s.parse::<f64>() {
        return Ok(v);
    }

    // Check for SI suffix
    let lower = s.to_lowercase();
    let (num_part, multiplier) = if let Some(stripped) = lower.strip_suffix("meg") {
        (&s[..stripped.len()], 1e6)
    } else if s.is_empty() {
        return Err("empty number".to_string());
    } else {
        let last = s.chars().last().unwrap();
        let mult = match last.to_ascii_lowercase() {
            't' => 1e12,
            'g' => 1e9,
            'k' => 1e3,
            'm' => 1e-3,
            'u' => 1e-6,
            'n' => 1e-9,
            'p' => 1e-12,
            'f' => 1e-15,
            'a' => 1e-18,
            _ => return Err(format!("cannot parse number: {s}")),
        };
        (&s[..s.len() - 1], mult)
    };

    num_part
        .parse::<f64>()
        .map(|v| v * multiplier)
        .map_err(|_| format!("cannot parse number: {s}"))
}

/// Strip common SPICE unit suffixes (V, A, Hz, Ohm, s, F, H, W).
///
/// Handles: `5V`, `42mA`, `1kHz`, `100uF`, `0.5mA`, etc.
fn strip_unit_suffix(s: &str) -> &str {
    let lower = s.to_lowercase();
    // Multi-char units first
    for suffix in &["hz", "ohm", "ohms"] {
        if lower.ends_with(suffix) {
            return &s[..s.len() - suffix.len()];
        }
    }
    // Single-char units: V, A, W (not s/f/h which conflict with SI prefixes)
    for &unit in &['v', 'a', 'w'] {
        if lower.ends_with(unit) {
            return &s[..s.len() - 1];
        }
    }
    s
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_spice_number() {
        assert!((parse_spice_number("1k").unwrap() - 1e3).abs() < 1e-10);
        assert!((parse_spice_number("42m").unwrap() - 42e-3).abs() < 1e-10);
        assert!((parse_spice_number("3.5n").unwrap() - 3.5e-9).abs() < 1e-20);
        assert!((parse_spice_number("1e-3").unwrap() - 1e-3).abs() < 1e-15);
        assert!((parse_spice_number("100").unwrap() - 100.0).abs() < 1e-10);
    }

    #[test]
    fn test_parse_echo() {
        let frags = parse_echo(r#""Note: err =" $&err"#);
        assert!(frags.len() >= 2);
    }

    #[test]
    fn test_parse_simple_block() {
        let lines: Vec<String> = vec!["echo hello".to_string(), "quit 0".to_string()];
        let stmts = parse_control_block(&lines).unwrap();
        assert!(stmts.len() == 2);
    }

    #[test]
    fn test_parse_strcmp() {
        let lines = vec!["strcmp __flag $curplot $gold".to_string()];
        let stmts = parse_control_block(&lines).unwrap();
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Statement::Strcmp { result, a, b } => {
                assert_eq!(result, "__flag");
                assert_eq!(a, "$curplot");
                assert_eq!(b, "$gold");
            }
            other => panic!("expected Strcmp, got {:?}", other),
        }
    }

    #[test]
    fn test_strip_inline_comment() {
        assert_eq!(strip_inline_comment("hello $ comment"), "hello ");
        assert_eq!(strip_inline_comment("hello $var world"), "hello $var world");
        assert_eq!(
            strip_inline_comment("strcmp __flag $curplot $gold"),
            "strcmp __flag $curplot $gold"
        );
    }

    #[test]
    fn test_parse_if_else() {
        let lines: Vec<String> = vec![
            "if 1 > 0".to_string(),
            "  echo yes".to_string(),
            "else".to_string(),
            "  echo no".to_string(),
            "end".to_string(),
        ];
        let stmts = parse_control_block(&lines).unwrap();
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Statement::If {
                body, else_body, ..
            } => {
                assert_eq!(body.len(), 1);
                assert_eq!(else_body.len(), 1);
            }
            other => panic!("expected If, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_let_success() {
        let lines = vec!["let x = 42 + 1".to_string()];
        let stmts = parse_control_block(&lines).unwrap();
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Statement::Let { name, expr } => {
                assert_eq!(name, "x");
                assert_eq!(expr, "42 + 1");
            }
            other => panic!("expected Let, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_let_missing_eq() {
        let result = parse_let("x 42");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("let without '='"));
    }

    #[test]
    fn test_parse_foreach_with_values() {
        let lines = vec![
            "foreach val 1 2 3".to_string(),
            "  echo $val".to_string(),
            "end".to_string(),
        ];
        let stmts = parse_control_block(&lines).unwrap();
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Statement::Foreach { var, values, body } => {
                assert_eq!(var, "val");
                assert_eq!(values, &["1", "2", "3"]);
                assert_eq!(body.len(), 1);
            }
            other => panic!("expected Foreach, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_foreach_empty_var_error() {
        let lines = vec!["foreach".to_string(), "end".to_string()];
        let result = parse_control_block(&lines);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("foreach without variable"));
    }

    #[test]
    fn test_parse_quit_no_code() {
        let result = parse_quit("").unwrap();
        assert!(matches!(result, Statement::Quit(Some(0))));
    }

    #[test]
    fn test_parse_quit_explicit_code() {
        let result = parse_quit("42").unwrap();
        assert!(matches!(result, Statement::Quit(Some(42))));
    }

    #[test]
    fn test_parse_quit_invalid_code() {
        let result = parse_quit("notanumber");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid quit code"));
    }

    #[test]
    fn test_parse_set_key_only() {
        let result = parse_set("nobreak").unwrap();
        match result {
            Statement::Set(pairs) => {
                assert_eq!(pairs.len(), 1);
                assert_eq!(pairs[0].0, "nobreak");
                assert!(pairs[0].1.is_none());
            }
            other => panic!("expected Set, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_set_key_eq_value() {
        let result = parse_set("filetype=ascii").unwrap();
        match result {
            Statement::Set(pairs) => {
                assert_eq!(pairs.len(), 1);
                assert_eq!(pairs[0].0, "filetype");
                assert_eq!(pairs[0].1.as_deref(), Some("ascii"));
            }
            other => panic!("expected Set, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_set_key_eq_quoted() {
        let result = parse_set(r#"title="my circuit""#).unwrap();
        match result {
            Statement::Set(pairs) => {
                assert_eq!(pairs.len(), 1);
                assert_eq!(pairs[0].0, "title");
                assert_eq!(pairs[0].1.as_deref(), Some("my circuit"));
            }
            other => panic!("expected Set, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_set_multiple_key_only() {
        let result = parse_set("nobreak wr_vecnames").unwrap();
        match result {
            Statement::Set(pairs) => {
                assert_eq!(pairs.len(), 2);
                assert_eq!(pairs[0].0, "nobreak");
                assert!(pairs[0].1.is_none());
                assert_eq!(pairs[1].0, "wr_vecnames");
                assert!(pairs[1].1.is_none());
            }
            other => panic!("expected Set, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_set_multiple_key_value() {
        let result = parse_set("filetype=ascii color=0").unwrap();
        match result {
            Statement::Set(pairs) => {
                assert_eq!(pairs.len(), 2);
                assert_eq!(pairs[0].0, "filetype");
                assert_eq!(pairs[0].1.as_deref(), Some("ascii"));
                assert_eq!(pairs[1].0, "color");
                assert_eq!(pairs[1].1.as_deref(), Some("0"));
            }
            other => panic!("expected Set, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_define_success() {
        let result = parse_define("myfunc(a, b) a + b").unwrap();
        match result {
            Statement::Define { name, args, body } => {
                assert_eq!(name, "myfunc");
                assert_eq!(args, &["a", "b"]);
                assert_eq!(body, "a + b");
            }
            other => panic!("expected Define, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_define_missing_lparen() {
        let result = parse_define("myfunc a,b) a+b");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing '('"));
    }

    #[test]
    fn test_parse_define_missing_rparen() {
        let result = parse_define("myfunc(a,b a+b");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing ')'"));
    }

    #[test]
    fn test_parse_compose_with_values_keyword() {
        let result = parse_compose("myvec values 1 2 3").unwrap();
        match result {
            Statement::Compose { name, value_exprs } => {
                assert_eq!(name, "myvec");
                assert_eq!(value_exprs, &["1", "2", "3"]);
            }
            other => panic!("expected Compose, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_compose_without_values_keyword() {
        let result = parse_compose("myvec 1 2 3").unwrap();
        match result {
            Statement::Compose { name, value_exprs } => {
                assert_eq!(name, "myvec");
                assert_eq!(value_exprs, &["1", "2", "3"]);
            }
            other => panic!("expected Compose, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_compose_paren_balanced() {
        let result = parse_compose("myvec values v(n1) ln(2.7)").unwrap();
        match result {
            Statement::Compose { name, value_exprs } => {
                assert_eq!(name, "myvec");
                assert_eq!(value_exprs, &["v(n1)", "ln(2.7)"]);
            }
            other => panic!("expected Compose, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_alter_scalar() {
        let result = parse_alter("@r1[resistance] = 1000").unwrap();
        match result {
            Statement::Alter { spec, value } => {
                assert_eq!(spec, "@r1[resistance]");
                match value {
                    AlterValue::Scalar(v) => assert!((v - 1000.0).abs() < 1e-10),
                    _ => panic!("expected Scalar"),
                }
            }
            other => panic!("expected Alter, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_alter_vector() {
        let result = parse_alter("@v1[pulse] = [ 0 5 0 1n 1n 10n 20n ]").unwrap();
        match result {
            Statement::Alter { spec, value } => {
                assert_eq!(spec, "@v1[pulse]");
                match value {
                    AlterValue::Vector(v) => {
                        assert_eq!(v.len(), 7);
                        assert!((v[0]).abs() < 1e-15);
                        assert!((v[1] - 5.0).abs() < 1e-10);
                    }
                    _ => panic!("expected Vector"),
                }
            }
            other => panic!("expected Alter, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_alter_missing_eq() {
        let result = parse_alter("@r1[resistance] 1000");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("alter without '='"));
    }

    #[test]
    fn test_parse_print_basic() {
        let result = parse_print("v(out) i(vin)").unwrap();
        match result {
            Statement::Print { exprs, file } => {
                assert_eq!(exprs, &["v(out)", "i(vin)"]);
                assert!(file.is_none());
            }
            other => panic!("expected Print, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_print_redirect() {
        let result = parse_print("v(out) > results.txt").unwrap();
        match result {
            Statement::Print { exprs, file } => {
                assert_eq!(exprs, &["v(out)"]);
                assert_eq!(file.as_deref(), Some("results.txt"));
            }
            other => panic!("expected Print, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_print_col_line_filtered() {
        let result = parse_print("col v(out) line i(vin)").unwrap();
        match result {
            Statement::Print { exprs, file } => {
                assert_eq!(exprs, &["v(out)", "i(vin)"]);
                assert!(file.is_none());
            }
            other => panic!("expected Print, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_write_no_args() {
        match parse_write("") {
            Statement::Write { file, vectors } => {
                assert!(file.is_none());
                assert!(vectors.is_empty());
            }
            other => panic!("expected Write, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_write_with_filename_only() {
        match parse_write("results.raw") {
            Statement::Write { file, vectors } => {
                assert_eq!(file.as_deref(), Some("results.raw"));
                assert!(vectors.is_empty());
            }
            other => panic!("expected Write, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_write_with_filename_and_vectors() {
        match parse_write("out.raw v(out) i(v1)") {
            Statement::Write { file, vectors } => {
                assert_eq!(file.as_deref(), Some("out.raw"));
                assert_eq!(vectors, vec!["v(out)".to_string(), "i(v1)".to_string()]);
            }
            other => panic!("expected Write, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_write_vectors_only_uses_default_filename() {
        // `v(out)` looks like a vector (has parens), so no filename is
        // captured and the executor falls back to thevenin.raw.
        match parse_write("v(out) i(v1)") {
            Statement::Write { file, vectors } => {
                assert!(file.is_none(), "no filename inferred from vector list");
                assert_eq!(vectors, vec!["v(out)".to_string(), "i(v1)".to_string()]);
            }
            other => panic!("expected Write, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_write_csv_filename() {
        match parse_write("out.csv v(a) v(b)") {
            Statement::Write { file, vectors } => {
                assert_eq!(file.as_deref(), Some("out.csv"));
                assert_eq!(vectors.len(), 2);
            }
            other => panic!("expected Write, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_write_routed_through_top_level_parser() {
        // The `parse_statement` dispatcher should route `write` correctly.
        let stmts = parse_control_block(&["write out.raw v(out)".to_string(), ".endc".to_string()])
            .unwrap();
        match &stmts[0] {
            Statement::Write { file, vectors } => {
                assert_eq!(file.as_deref(), Some("out.raw"));
                assert_eq!(vectors, &vec!["v(out)".to_string()]);
            }
            other => panic!("expected Write, got {:?}", other),
        }
    }

    #[test]
    fn test_strip_unit_suffix_hz() {
        assert_eq!(strip_unit_suffix("100Hz"), "100");
    }

    #[test]
    fn test_strip_unit_suffix_ohm() {
        assert_eq!(strip_unit_suffix("47ohm"), "47");
        assert_eq!(strip_unit_suffix("47Ohms"), "47");
    }

    #[test]
    fn test_strip_unit_suffix_v() {
        assert_eq!(strip_unit_suffix("5V"), "5");
    }

    #[test]
    fn test_strip_unit_suffix_a() {
        assert_eq!(strip_unit_suffix("10A"), "10");
    }

    #[test]
    fn test_strip_unit_suffix_w() {
        assert_eq!(strip_unit_suffix("1W"), "1");
    }

    #[test]
    fn test_strip_unit_suffix_no_suffix() {
        assert_eq!(strip_unit_suffix("100"), "100");
    }

    #[test]
    fn test_parse_spice_number_meg() {
        assert!((parse_spice_number("2meg").unwrap() - 2e6).abs() < 1e-3);
        assert!((parse_spice_number("2MEG").unwrap() - 2e6).abs() < 1e-3);
    }

    #[test]
    fn test_parse_spice_number_with_unit_stripping() {
        assert!((parse_spice_number("5V").unwrap() - 5.0).abs() < 1e-10);
        assert!((parse_spice_number("42mA").unwrap() - 42e-3).abs() < 1e-10);
        assert!((parse_spice_number("1kHz").unwrap() - 1e3).abs() < 1e-3);
    }

    #[test]
    fn test_parse_spice_number_empty_error() {
        let result = parse_spice_number("");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty number"));
    }

    #[test]
    fn parse_stop_when_time_eq_milliseconds() {
        let lines = vec!["stop when time = 1ms".to_string()];
        let stmts = parse_control_block(&lines).unwrap();
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Statement::StopWhen(StopCondition::TimeEq(t)) => {
                assert!((t - 1e-3).abs() < 1e-18);
            }
            other => panic!("expected StopWhen, got {other:?}"),
        }
    }

    #[test]
    fn parse_stop_when_time_eq_no_spaces() {
        let lines = vec!["stop when time=500us".to_string()];
        let stmts = parse_control_block(&lines).unwrap();
        match &stmts[0] {
            Statement::StopWhen(StopCondition::TimeEq(t)) => {
                assert!((t - 5e-4).abs() < 1e-18);
            }
            other => panic!("expected StopWhen, got {other:?}"),
        }
    }

    #[test]
    fn parse_stop_unsupported_condition_errors() {
        let result = parse_stop("when v(out) > 1");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("only `time"));
    }

    #[test]
    fn parse_resume() {
        let lines = vec!["resume".to_string()];
        let stmts = parse_control_block(&lines).unwrap();
        assert!(matches!(stmts[0], Statement::Resume));
    }

    #[test]
    fn parse_while_block() {
        let lines = vec![
            "while $i > 0".to_string(),
            "  let i = $i - 1".to_string(),
            "end".to_string(),
        ];
        let stmts = parse_control_block(&lines).unwrap();
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Statement::While { cond, body } => {
                assert_eq!(cond, "$i > 0");
                assert_eq!(body.len(), 1);
            }
            other => panic!("expected While, got {other:?}"),
        }
    }

    #[test]
    fn parse_while_empty_condition_errors() {
        let lines = vec!["while".to_string(), "end".to_string()];
        let result = parse_control_block(&lines);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("while without condition"));
    }

    #[test]
    fn parse_while_unterminated_errors() {
        let lines = vec!["while 1".to_string(), "  echo hi".to_string()];
        let result = parse_control_block(&lines);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unterminated while"));
    }

    #[test]
    fn parse_repeat_block() {
        let lines = vec![
            "repeat 3".to_string(),
            "  echo hi".to_string(),
            "end".to_string(),
        ];
        let stmts = parse_control_block(&lines).unwrap();
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Statement::Repeat { count, body } => {
                assert_eq!(count, "3");
                assert_eq!(body.len(), 1);
            }
            other => panic!("expected Repeat, got {other:?}"),
        }
    }

    #[test]
    fn parse_repeat_empty_count_errors() {
        let lines = vec!["repeat".to_string(), "end".to_string()];
        let result = parse_control_block(&lines);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("repeat without count"));
    }

    #[test]
    fn parse_save_multiple_specs() {
        let lines = vec!["save v(out) i(v1) v(mid)".to_string()];
        let stmts = parse_control_block(&lines).unwrap();
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Statement::Save { specs } => {
                assert_eq!(specs, &["v(out)", "i(v1)", "v(mid)"]);
            }
            other => panic!("expected Save, got {other:?}"),
        }
    }

    #[test]
    fn parse_save_empty_specs_is_ok() {
        let lines = vec!["save".to_string()];
        let stmts = parse_control_block(&lines).unwrap();
        assert_eq!(stmts.len(), 1);
        match &stmts[0] {
            Statement::Save { specs } => assert!(specs.is_empty()),
            other => panic!("expected Save, got {other:?}"),
        }
    }
}
