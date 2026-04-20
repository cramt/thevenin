//! Tree-sitter grammar for the Cirq circuit description language.

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
