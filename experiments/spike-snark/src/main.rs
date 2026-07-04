//! spike-snark — feed cirq's EXISTING tree-sitter grammar.json into snark and
//! parse a real `.cirq` file with it.
//!
//! The whole pitch: cirq already ships a tree-sitter `grammar.json` (produced by
//! `tree-sitter generate`). snark advertises tree-sitter compatibility via
//! `RawGrammarJson::from_tree_sitter_json_str`, so in principle the migration is
//! "point snark at the grammar we already have." This spike tests exactly that,
//! reusing the pipeline the `vix` crate uses internally.
//!
//! Known risk under test: the main cirq grammar declares an EXTERNAL scanner
//! (scanner.c). External tokens are the classic tree-sitter drop-in blocker, so
//! if this stage fails, that failure is itself the useful feedback.

use snark::grammar::RawGrammarJson;
use snark::lexical::LexicalFacts;
use snark::lower::weavy::{WeavyParsePlan, parse_prepared_weavy_with_report};
use snark::parser::{ParseTable, ParserGrammar};
use snark::validated::ValidatedGrammar;

// cirq's committed tree-sitter grammar (with external scanner) + a real sample.
const CIRQ_GRAMMAR_JSON: &str = include_str!("../../../cirq-grammar/src/grammar.json");
const CIRQ_SAMPLE: &str = include_str!("../../../examples/cirq/conditional.cirq");

fn stage<T, E: std::fmt::Debug>(label: &str, r: Result<T, E>) -> Result<T, String> {
    r.map_err(|e| format!("[{label}] {e:?}"))
}

fn build_and_parse() -> Result<(), String> {
    let raw = stage(
        "from_tree_sitter_json_str",
        RawGrammarJson::from_tree_sitter_json_str(CIRQ_GRAMMAR_JSON),
    )?;
    let validated = stage("ValidatedGrammar::from_raw", ValidatedGrammar::from_raw(&raw))?;
    let lexical = LexicalFacts::from_grammar(&validated);
    let parser = stage(
        "ParserGrammar::normalize_from_validated",
        ParserGrammar::normalize_from_validated(&validated, &lexical),
    )?;
    let parser = stage("prepare_productions_for_items", parser.prepare_productions_for_items())?;
    let table = stage("ParseTable::from_grammar", ParseTable::from_grammar(&parser))?;
    let plan = stage("WeavyParsePlan::new", WeavyParsePlan::new(&validated, &parser, &table))?;

    println!("[spike-snark] grammar built: parsing {} bytes of cirq", CIRQ_SAMPLE.len());
    let report = stage(
        "parse_prepared_weavy_with_report",
        parse_prepared_weavy_with_report(&plan, &parser, &table, CIRQ_SAMPLE),
    )?;

    match report.accepted_resolved_tree(&parser, CIRQ_SAMPLE) {
        Some(root) => {
            println!("[spike-snark] OK — accepted parse, root kind = {:?}", root.kind());
            let mut named = 0usize;
            dump(&root, 0, &mut named);
            println!("[spike-snark] {named} named nodes in tree — non-degenerate parse");
            Ok(())
        }
        None => Err("parse produced no accepted tree".to_string()),
    }
}

/// Shallow structural dump: named (non-extra, non-token) nodes, 4 levels deep,
/// enough to prove params / if-blocks / component decls all parsed.
fn dump(n: &snark::parser::ResolvedCstNode, depth: usize, named: &mut usize) {
    if depth > 3 {
        return;
    }
    for child in n.children() {
        if child.extra() || child.text().is_some() {
            continue; // skip whitespace/comments and anonymous tokens
        }
        *named += 1;
        let field = child.field().map(|f| format!("{f}: ")).unwrap_or_default();
        println!("{:indent$}{field}{}", "", child.kind(), indent = depth * 2 + 2);
        dump(child, depth + 1, named);
    }
}

fn main() {
    match build_and_parse() {
        Ok(()) => println!("[spike-snark] done"),
        Err(e) => {
            eprintln!("[spike-snark] FAILED at stage {e}");
            std::process::exit(1);
        }
    }
}
