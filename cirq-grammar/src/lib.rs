//! Tree-sitter grammar for the Cirq circuit-description language.
//!
//! Exposes the compiled Tree-sitter [`language`] for Cirq plus a [`parse`]
//! convenience that produces a concrete syntax tree. This is the lexing/parsing
//! front of the Cirq pipeline; [`cirq-frontend`](https://docs.rs/cirq-frontend)
//! lowers the resulting CST into the [`cirq-ast`](https://docs.rs/cirq-ast) and
//! then [`cirq-ir`](https://docs.rs/cirq-ir).
//!
//! The crate also ships Tree-sitter `highlights` / `locals` / `folds` queries
//! for editor integration.
//!
//! ```
//! let tree = cirq_grammar::parse("circuit c { R1: resistor(a -> b, 1k) }")
//!     .expect("parser configured");
//! assert_eq!(tree.root_node().kind(), "source_file");
//! ```

use tree_sitter::Language;

unsafe extern "C" {
    fn tree_sitter_cirq() -> *const tree_sitter::ffi::TSLanguage;
}

/// Returns the Tree-sitter [`Language`] for Cirq.
pub fn language() -> Language {
    unsafe { Language::from_raw(tree_sitter_cirq()) }
}

/// Parse Cirq source text into a Tree-sitter CST.
///
/// Returns `None` only if the parser itself fails (should not happen for
/// valid Tree-sitter setups).
pub fn parse(source: &str) -> Option<tree_sitter::Tree> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&language()).ok()?;
    parser.parse(source, None)
}
