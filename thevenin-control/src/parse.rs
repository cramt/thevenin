//! Parser for `.control` block lines into [`Statement`] AST.

use crate::ast::{AlterValue, EchoFragment, Statement};

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
        "quit" => parse_quit(rest),
        "set" => parse_set(rest),
        "setplot" => Ok(Statement::Setplot(rest.to_string())),
        "define" => parse_define(rest),
        "compose" => parse_compose(rest),
        "alter" => parse_alter(rest),
        "strcmp" => parse_strcmp(rest),
        "print" => parse_print(rest),
        "eprint" | "eprvcd" => Ok(Statement::Eprint(
            rest.split_whitespace().map(|s| s.to_string()).collect(),
        )),
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

fn parse_if(
    cond: &str,
    iter: &mut LineIter<'_>,
) -> Result<Statement, String> {
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

fn parse_foreach(
    rest: &str,
    iter: &mut LineIter<'_>,
) -> Result<Statement, String> {
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
                    (
                        stripped[..end].to_string(),
                        stripped[end + 1..].trim(),
                    )
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
            let inner = val_str
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim();
            let vals: Result<Vec<f64>, _> = inner
                .split_whitespace()
                .map(parse_spice_number)
                .collect();
            AlterValue::Vector(vals.map_err(|e| format!("alter: {e}"))?)
        } else {
            AlterValue::Scalar(
                parse_spice_number(val_str).map_err(|e| format!("alter: {e}"))?,
            )
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

/// Parse a SPICE number with optional SI suffix (public for exec module).
pub(crate) fn parse_spice_number_pub(s: &str) -> Result<f64, String> {
    parse_spice_number(s)
}

/// Parse a SPICE number with optional SI suffix.
///
/// Handles SPICE conventions: `42mA` = 42e-3, `1kHz` = 1e3, `100uF` = 100e-6.
/// Trailing unit characters (V, A, Hz, Ohm, s, F, H, etc.) are stripped.
fn parse_spice_number(s: &str) -> Result<f64, String> {
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
        let lines: Vec<String> = vec![
            "echo hello".to_string(),
            "quit 0".to_string(),
        ];
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
        assert_eq!(strip_inline_comment("strcmp __flag $curplot $gold"),
            "strcmp __flag $curplot $gold");
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
            Statement::If { body, else_body, .. } => {
                assert_eq!(body.len(), 1);
                assert_eq!(else_body.len(), 1);
            }
            other => panic!("expected If, got {:?}", other),
        }
    }
}
