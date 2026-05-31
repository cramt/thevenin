//! Tree-sitter grammar for the ngspice `.control` scripting language.
//!
//! Exposes the compiled Tree-sitter [`language`] for `.control` / `.endc`
//! scripts plus a [`parse`] convenience. The resulting CST is lowered to the
//! typed statement AST in [`cirq-ir`](https://docs.rs/cirq-ir)'s `control`
//! module and executed by
//! [`thevenin-control`](https://docs.rs/thevenin-control).
//!
//! ```
//! let tree = cirq_control_grammar::parse("let x = 1\nprint x\n")
//!     .expect("parser configured");
//! assert_eq!(tree.root_node().kind(), "source_file");
//! ```

use tree_sitter::Language;

unsafe extern "C" {
    fn tree_sitter_control() -> *const tree_sitter::ffi::TSLanguage;
}

/// Returns the Tree-sitter [`Language`] for the `.control` language.
pub fn language() -> Language {
    unsafe { Language::from_raw(tree_sitter_control()) }
}

/// Parse `.control` source text into a Tree-sitter CST.
///
/// Returns `None` only if the parser itself fails (should not happen for
/// valid Tree-sitter setups).
pub fn parse(source: &str) -> Option<tree_sitter::Tree> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&language()).ok()?;
    parser.parse(source, None)
}
