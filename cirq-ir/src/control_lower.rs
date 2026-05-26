//! Tree-sitter CST → `Vec<Statement>` lowerer.
//!
//! Mirrors the cirq-frontend pipeline: `cirq-control-grammar` produces a
//! Tree-sitter CST; this module walks the CST and lowers it to the typed
//! [`Statement`] AST that [`crate::control::parse_control_block`] returns.
//!
//! Per-statement body parsing (numbers with SI suffixes, `parse_alter`'s
//! `[ ... ]` vector form, set's `key=value` pairs, etc.) is delegated to
//! the existing `super::control` helpers so the two parse paths stay in
//! lock-step and the existing test suite continues to validate behaviour
//! end-to-end.

use crate::control::{self, Statement};
use tree_sitter::{Node, Tree};

/// Lower an already-parsed Tree-sitter tree to a statement list.
pub fn lower_tree(tree: &Tree, source: &str) -> Result<Vec<Statement>, String> {
    let root = tree.root_node();
    if root.kind() != "source_file" {
        return Err(format!(
            "expected source_file at root, got {}",
            root.kind()
        ));
    }
    lower_block(root, source)
}

fn lower_block(node: Node<'_>, source: &str) -> Result<Vec<Statement>, String> {
    let mut stmts = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(stmt) = lower_statement(child, source)? {
            stmts.push(stmt);
        }
    }
    Ok(stmts)
}

fn lower_statement(node: Node<'_>, source: &str) -> Result<Option<Statement>, String> {
    match node.kind() {
        // Both comment shapes map to the same Comment marker the line-
        // based parser emitted for `*` lines and `$ ...` inline comments.
        "line_comment" | "inline_comment" => Ok(Some(Statement::Comment)),

        "let_stmt" => control::parse_let(rest_after_first_word(node_text(node, source))).map(Some),
        "echo_stmt" => Ok(Some(Statement::Echo(control::parse_echo(rest_after_first_word(
            node_text(node, source),
        ))))),
        "if_stmt" => lower_if(node, source).map(Some),
        "foreach_stmt" => lower_foreach(node, source).map(Some),
        "while_stmt" => lower_while(node, source).map(Some),
        "repeat_stmt" => lower_repeat(node, source).map(Some),
        "save_stmt" => Ok(Some(control::parse_save(rest_after_first_word(node_text(node, source))))),
        "quit_stmt" => control::parse_quit(rest_after_first_word(node_text(node, source))).map(Some),
        "set_stmt" => control::parse_set(rest_after_first_word(node_text(node, source))).map(Some),
        "setplot_stmt" => Ok(Some(Statement::Setplot(
            rest_after_first_word(node_text(node, source)).to_string(),
        ))),
        "define_stmt" => control::parse_define(rest_after_first_word(node_text(node, source))).map(Some),
        "compose_stmt" => control::parse_compose(rest_after_first_word(node_text(node, source))).map(Some),
        "alter_stmt" => control::parse_alter(rest_after_first_word(node_text(node, source))).map(Some),
        "strcmp_stmt" => control::parse_strcmp(rest_after_first_word(node_text(node, source))).map(Some),
        "print_stmt" => control::parse_print(rest_after_first_word(node_text(node, source))).map(Some),
        "write_stmt" => Ok(Some(control::parse_write(rest_after_first_word(node_text(node, source))))),
        "run_analysis" => Ok(Some(Statement::RunAnalysis(trim_terminators(node_text(node, source)).to_string()))),
        "eprint_stmt" => Ok(Some(Statement::Eprint(
            rest_after_first_word(node_text(node, source))
                .split_whitespace()
                .map(|s| s.to_string())
                .collect(),
        ))),
        "stop_when_stmt" => control::parse_stop(rest_after_first_word(node_text(node, source))).map(Some),
        "resume_stmt" => Ok(Some(Statement::Resume)),
        "source_stmt" => control::parse_source(rest_after_first_word(node_text(node, source))).map(Some),
        "measure_stmt" => control::parse_measure(rest_after_first_word(node_text(node, source))).map(Some),

        // Catch-all node emitted by the grammar for non-keyword lines.
        // The line-based parser treats unknown commands as no-op
        // comments — match that.
        "unknown_stmt" => Ok(Some(Statement::Comment)),

        // Stray `end` (top-level, not closing a block) is silently
        // skipped by the line-based parser. Likewise here.
        "end" => Ok(None),

        _ => Ok(None),
    }
}

fn node_text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or("")
}

fn trim_terminators(text: &str) -> &str {
    text.trim_end_matches(['\n', '\r', ';']).trim()
}

/// Strip the leading keyword (and any leading whitespace) from a statement's
/// source text so the body can be handed to the existing per-statement
/// helpers, which all take "everything after the keyword".
fn rest_after_first_word(text: &str) -> &str {
    let trimmed = trim_terminators(text);
    match trimmed.split_once(char::is_whitespace) {
        Some((_, rest)) => rest.trim(),
        None => "",
    }
}

// ─── Block statements ──────────────────────────────────────────────

fn lower_if(node: Node<'_>, source: &str) -> Result<Statement, String> {
    let cond = field_text(node, "condition", source).to_string();
    let body = match node.child_by_field_name("body") {
        Some(b) => lower_block(b, source)?,
        None => Vec::new(),
    };
    let else_body = match node.child_by_field_name("else_body") {
        Some(b) => lower_block(b, source)?,
        None => Vec::new(),
    };
    Ok(Statement::If {
        cond,
        body,
        else_body,
    })
}

fn lower_foreach(node: Node<'_>, source: &str) -> Result<Statement, String> {
    let var = field_text(node, "var", source).to_string();
    if var.is_empty() {
        return Err("foreach without variable name".to_string());
    }
    let mut values = Vec::new();
    let mut cursor = node.walk();
    for child in node.children_by_field_name("values", &mut cursor) {
        values.push(node_text(child, source).to_string());
    }
    let body = match node.child_by_field_name("body") {
        Some(b) => lower_block(b, source)?,
        None => Vec::new(),
    };
    Ok(Statement::Foreach { var, values, body })
}

fn lower_while(node: Node<'_>, source: &str) -> Result<Statement, String> {
    let cond = field_text(node, "condition", source).to_string();
    if cond.is_empty() {
        return Err("while without condition".to_string());
    }
    let body = match node.child_by_field_name("body") {
        Some(b) => lower_block(b, source)?,
        None => Vec::new(),
    };
    Ok(Statement::While { cond, body })
}

fn lower_repeat(node: Node<'_>, source: &str) -> Result<Statement, String> {
    let count = field_text(node, "count", source).to_string();
    if count.is_empty() {
        return Err("repeat without count".to_string());
    }
    let body = match node.child_by_field_name("body") {
        Some(b) => lower_block(b, source)?,
        None => Vec::new(),
    };
    Ok(Statement::Repeat { count, body })
}

fn field_text<'a>(node: Node<'_>, name: &str, source: &'a str) -> &'a str {
    node.child_by_field_name(name)
        .map(|n| node_text(n, source))
        .unwrap_or("")
}
