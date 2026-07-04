//! Generate cirq's typed AST + resolved-CST lowering from the EXISTING
//! tree-sitter grammar.js, using snark-dsl's codegen (the same call vix's own
//! build.rs makes). grammar.js is evaluated by boa — no node in the loop.

use std::env;
use std::path::PathBuf;

use snark_dsl::typed_ast::{TypedAstConfig, generate_typed_ast};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    // experiments/spike-cirq-ast -> repo root is two levels up.
    let repo = manifest.parent().unwrap().parent().unwrap().to_path_buf();
    let real_grammar_js = repo.join("cirq-grammar/grammar.js");
    let ann_js = manifest.join("cirq_ast.snark.js");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=cirq_ast.snark.js");
    println!("cargo:rerun-if-changed={}", real_grammar_js.display());

    // WORKAROUND (snark-dsl bug, for feedback): typed_ast::rust_field_name only
    // dodges the keyword `type` (-> `ty`); every other Rust keyword field name is
    // emitted raw, so cirq's `field("else", …)` on `ternary_expression` produces
    // `pub else: …`, which won't parse. Fix belongs in rust_field_name (raw-escape
    // reserved idents). Here we rename just that field in a build-time COPY of the
    // grammar — the real grammar and the cargo cache stay untouched, and
    // conditional.cirq has no ternary so runtime parsing is unaffected.
    let src = std::fs::read_to_string(&real_grammar_js).expect("read cirq grammar.js");
    let patched = src
        .replace("field(\"else\"", "field(\"else_branch\"")
        // FINDING: snark derives the typed AST purely from field()-labeled children.
        // cirq's grammar leaves circuit bodies as bare `repeat($._circuit_item)`, so
        // the codegen captures only `name` and DROPS the body. A real migration means
        // fielding every child the AST needs. Demonstrate the fix for circuit bodies:
        // `CircuitDecl` then gains `items: Vec<CircuitItem>`.
        .replace(
            "repeat($._circuit_item)",
            "repeat(field(\"item\", $._circuit_item))",
        );
    let grammar_js = out.join("cirq_grammar.patched.js");
    std::fs::write(&grammar_js, patched).expect("write patched grammar");

    generate_typed_ast(&TypedAstConfig {
        grammar_js: &grammar_js,
        annotations_js: &ann_js,
        out_dir: &out,
        grammar_output: "cirq_grammar.json",
        ast_output: "cirq_ast.rs",
        annotation_source_name: "cirq_ast.snark.js",
        generated_by: "spike-cirq-ast/build.rs",
        language_name: "cirq",
    })
    .expect("generate cirq typed AST");
}
