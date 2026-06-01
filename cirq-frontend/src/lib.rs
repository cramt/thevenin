//! Cirq frontend — compile Cirq source into canonical [`cirq_ir::Circuit`] IR.
//!
//! This crate owns the Cirq compilation pipeline:
//!
//! ```text
//! Cirq source ──▶ Tree-sitter CST ──▶ cirq-ast ──▶ cirq-ir Circuit
//!                (cirq-grammar)        (lower)      (ir_lower)
//! ```
//!
//! The result is a name-resolved, parameter-evaluated [`cirq_ir::Circuit`] —
//! the canonical input to the simulator ([`thevenin::circuit`](https://docs.rs/thevenin)).
//!
//! # Entry points
//!
//! - [`compile`] — Cirq source → [`cirq_ir::Circuit`].
//! - [`compile_file`] — same, resolving `include` directives relative to a base
//!   directory.
//! - [`compile_to_netlist`] / [`compile_file_to_netlist`] — compile and then
//!   lower to one [`thevenin_types::Netlist`] per analysis (the SPICE-shaped
//!   adapter), via [`to_netlist`].
//!
//! Errors are returned as a `Vec` of [`Diagnostic`]s carrying source spans for
//! rich reporting (see the [`diagnostics`] module).
//!
//! [`Diagnostic`]: diagnostics::Diagnostic
//!
//! # Example
//!
//! ```
//! let circuit = cirq_frontend::compile(
//!     "circuit divider {
//!          V1: vsource(in -> gnd, dc: 1.0)
//!          R1: resistor(in -> mid, 1k)
//!          R2: resistor(mid -> gnd, 2k)
//!          analysis op {}
//!      }",
//! )
//! .expect("compiles");
//!
//! assert_eq!(circuit.name, "divider");
//! assert_eq!(circuit.elements.len(), 3);
//! ```
//!
//! # Modules
//!
//! - [`parser`] — Tree-sitter parse into a CST.
//! - [`lower`] — CST → [`cirq_ast`].
//! - [`ir_lower`] — AST → [`cirq_ir::Circuit`] (name resolution, parameter
//!   evaluation, subcircuit flattening, validation).
//! - [`to_netlist`] — [`cirq_ir::Circuit`] → [`thevenin_types::Netlist`].
//! - [`control_analysis`] — parse a `.control` analysis command straight to
//!   [`cirq_ir::Analysis`].

pub mod control_analysis;
pub mod diagnostics;
pub mod ir_lower;
pub mod lower;
pub mod parser;
pub mod resolve;
pub mod to_netlist;

use std::collections::BTreeSet;
use std::path::Path;

use diagnostics::{Diagnostic, Severity};

/// The set of `code "lang" { … }` language tags the compiler will accept.
///
/// Cirq's embedded code blocks ([`cirq_ir::CodeBlock`]) carry a free-form
/// language tag. This registry is the **compile-time** half of the language
/// registry: [`compile_with_languages`] rejects any block whose tag is not
/// registered here, with a spanned diagnostic, so typos and unsupported
/// languages fail loudly instead of being silently dropped.
///
/// The **execution-time** half lives in `thevenin_control::LanguageRegistry`
/// (a tag → handler map). A host that registers extra handlers there should
/// mirror the tags into this registry — `thevenin_control::LanguageRegistry::tags()`
/// returns exactly the set to pass to [`LanguageRegistry::with_languages`] —
/// so validation and execution stay in sync.
///
/// The default accepts only `"control"`, matching the built-in `.control`
/// interpreter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageRegistry {
    tags: BTreeSet<String>,
}

impl Default for LanguageRegistry {
    /// Accepts only `"control"`.
    fn default() -> Self {
        let mut tags = BTreeSet::new();
        tags.insert("control".to_string());
        Self { tags }
    }
}

impl LanguageRegistry {
    /// An empty registry that accepts no code-block languages.
    pub fn empty() -> Self {
        Self {
            tags: BTreeSet::new(),
        }
    }

    /// Build a registry from an explicit set of accepted tags.
    pub fn with_languages<I, S>(tags: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            tags: tags.into_iter().map(Into::into).collect(),
        }
    }

    /// Register an additional accepted language tag (builder-style).
    pub fn register(mut self, tag: impl Into<String>) -> Self {
        self.tags.insert(tag.into());
        self
    }

    /// Whether `tag` is accepted by this registry.
    pub fn accepts(&self, tag: &str) -> bool {
        self.tags.contains(tag)
    }

    /// The accepted tags, sorted, for diagnostics and host synchronisation.
    pub fn tags(&self) -> impl Iterator<Item = &str> {
        self.tags.iter().map(String::as_str)
    }
}

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
    compile_with_languages(source, &LanguageRegistry::default())
}

/// Like [`compile`], but validates `code "lang" { … }` blocks against an
/// explicit [`LanguageRegistry`]. Any block whose language tag is not accepted
/// produces a spanned error diagnostic.
pub fn compile_with_languages(
    source: &str,
    languages: &LanguageRegistry,
) -> Result<cirq_ir::Circuit, Vec<Diagnostic>> {
    let ast = parse(source)?;
    ir_lower::lower_to_ir_with_languages(&ast, languages)
}

/// Full compilation pipeline with import resolution.
///
/// Like [`compile`], but resolves `import` declarations by reading files
/// relative to `base_dir`. Use this when compiling a file from disk.
pub fn compile_file(source: &str, base_dir: &Path) -> Result<cirq_ir::Circuit, Vec<Diagnostic>> {
    compile_file_with_languages(source, base_dir, &LanguageRegistry::default())
}

/// Like [`compile_file`], but validates `code "lang" { … }` blocks against an
/// explicit [`LanguageRegistry`].
pub fn compile_file_with_languages(
    source: &str,
    base_dir: &Path,
    languages: &LanguageRegistry,
) -> Result<cirq_ir::Circuit, Vec<Diagnostic>> {
    let ast = parse(source)?;
    let (resolved, resolve_diags) = resolve::resolve_imports(ast, base_dir, &[]);

    let has_errors = resolve_diags.iter().any(|d| d.severity == Severity::Error);
    if has_errors {
        return Err(resolve_diags);
    }

    ir_lower::lower_to_ir_with_languages(&resolved, languages).map_err(|mut ir_diags| {
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
