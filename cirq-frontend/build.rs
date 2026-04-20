use std::path::Path;
use std::process::Command;

fn main() {
    let grammar_dir = Path::new("../cirq-grammar");
    let src_dir = grammar_dir.join("src");
    let parser_c = src_dir.join("parser.c");

    // Regenerate when the grammar definition changes.
    println!(
        "cargo:rerun-if-changed={}",
        grammar_dir.join("grammar.js").display()
    );
    println!("cargo:rerun-if-changed={}", parser_c.display());

    // If the generated parser doesn't exist (clean clone) or grammar.js is
    // newer, run `tree-sitter generate` to produce it.
    if !parser_c.exists() || grammar_js_newer(&grammar_dir.join("grammar.js"), &parser_c) {
        let status = Command::new("tree-sitter")
            .arg("generate")
            .current_dir(grammar_dir)
            .status()
            .expect(
                "failed to run `tree-sitter generate`. \
                 Make sure tree-sitter-cli is available (e.g. via `nix develop`).",
            );
        assert!(
            status.success(),
            "tree-sitter generate failed with {status}"
        );
    }

    cc::Build::new()
        .include(&src_dir)
        .file(&parser_c)
        .warnings(false)
        .compile("tree_sitter_cirq");
}

/// Returns true if `grammar_js` has a newer mtime than `parser_c`.
/// Falls back to true (regenerate) on any metadata error.
fn grammar_js_newer(grammar_js: &Path, parser_c: &Path) -> bool {
    let Ok(gm) = std::fs::metadata(grammar_js) else {
        return true;
    };
    let Ok(pm) = std::fs::metadata(parser_c) else {
        return true;
    };
    let (Ok(gt), Ok(pt)) = (gm.modified(), pm.modified()) else {
        return true;
    };
    gt > pt
}
