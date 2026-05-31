//! Cirq frontend pipeline -- orchestrates parsing, lowering, and IR generation.
//!
//! This crate provides the high-level entry points for processing Cirq source
//! files into either Cirq IR (for tooling) or `thevenin_types::Netlist` (for
//! simulation).

pub mod control_analysis;
pub mod diagnostics;
pub mod ir_lower;
pub mod lower;
pub mod parser;
pub mod resolve;
pub mod to_netlist;

use std::path::Path;

use diagnostics::{Diagnostic, Severity};

/// Parse Cirq source text into a [`cirq_ast::SourceFile`].
///
/// Returns `Ok(source_file)` when parsing and lowering succeed without errors.
/// Returns `Err(diagnostics)` if there are any errors (the vec is guaranteed
/// non-empty).
///
/// Even on `Err`, the diagnostics contain span information so callers can
/// present precise error messages.
pub fn parse(source: &str) -> Result<cirq_ast::SourceFile, Vec<Diagnostic>> {
    let tree = match parser::parse(source) {
        Some(t) => t,
        None => {
            return Err(vec![Diagnostic::error(
                "tree-sitter failed to produce a parse tree",
            )]);
        }
    };

    let (sf, diags) = lower::lower(&tree, source);

    let has_errors = diags.iter().any(|d| d.severity == Severity::Error);
    if has_errors { Err(diags) } else { Ok(sf) }
}

/// Full compilation pipeline: source text -> AST -> IR.
///
/// This is a convenience function that chains [`parse`] and
/// [`ir_lower::lower_to_ir`].
pub fn compile(source: &str) -> Result<cirq_ir::Circuit, Vec<Diagnostic>> {
    let ast = parse(source)?;
    ir_lower::lower_to_ir(&ast)
}

/// Full compilation pipeline with import resolution.
///
/// Like [`compile`], but resolves `import` declarations by reading files
/// relative to `base_dir`. Use this when compiling a file from disk.
pub fn compile_file(source: &str, base_dir: &Path) -> Result<cirq_ir::Circuit, Vec<Diagnostic>> {
    let ast = parse(source)?;
    let (resolved, resolve_diags) = resolve::resolve_imports(ast, base_dir, &[]);

    let has_errors = resolve_diags.iter().any(|d| d.severity == Severity::Error);
    if has_errors {
        return Err(resolve_diags);
    }

    ir_lower::lower_to_ir(&resolved).map_err(|mut ir_diags| {
        let mut all = resolve_diags;
        all.append(&mut ir_diags);
        all
    })
}

/// Full pipeline: Cirq source text -> [`thevenin_types::Netlist`] values.
///
/// Parses, lowers to AST, lowers to IR, then converts to one or more
/// `Netlist` values (one per analysis). If the source has no analysis
/// commands, a single `.op` netlist is produced.
pub fn compile_to_netlist(source: &str) -> Result<Vec<thevenin_types::Netlist>, Vec<Diagnostic>> {
    let circuit = compile(source)?;
    to_netlist::circuit_to_netlists(&circuit).map_err(|e| vec![Diagnostic::error(e.to_string())])
}

/// Full pipeline with import resolution: Cirq source file -> Netlists.
///
/// Like [`compile_to_netlist`], but resolves imports from `base_dir`.
pub fn compile_file_to_netlist(
    source: &str,
    base_dir: &Path,
) -> Result<Vec<thevenin_types::Netlist>, Vec<Diagnostic>> {
    let circuit = compile_file(source, base_dir)?;
    to_netlist::circuit_to_netlists(&circuit).map_err(|e| vec![Diagnostic::error(e.to_string())])
}
