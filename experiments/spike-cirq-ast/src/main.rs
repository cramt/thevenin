//! spike-cirq-ast — the real headline: generate a TYPED cirq AST from the
//! existing grammar via snark-dsl codegen, then parse a real `.cirq` file into
//! it. This is the "-1,157 lines" fable moment, tried on cirq.
//!
//! `ast` is produced by build.rs into OUT_DIR and `include!`d here, exactly as
//! vix does it. `mod support` provides the runtime the generated lowering calls.

mod support;

pub mod ast {
    include!(concat!(env!("OUT_DIR"), "/cirq_ast.rs"));
}

use snark::grammar::RawGrammarJson;
use snark::lexical::LexicalFacts;
use snark::lower::weavy::{WeavyParsePlan, parse_prepared_weavy_with_report};
use snark::parser::{ParseTable, ParserGrammar};
use snark::validated::ValidatedGrammar;

// The generated grammar.json (boa-evaluated from grammar.js at build time).
const CIRQ_GRAMMAR_JSON: &str = include_str!(concat!(env!("OUT_DIR"), "/cirq_grammar.json"));
const CIRQ_SAMPLE: &str = include_str!("../../../examples/cirq/conditional.cirq");

fn stage<T, E: std::fmt::Debug>(label: &str, r: Result<T, E>) -> Result<T, String> {
    r.map_err(|e| format!("[{label}] {e:?}"))
}

fn main() -> Result<(), String> {
    let raw = stage(
        "from_tree_sitter_json_str",
        RawGrammarJson::from_tree_sitter_json_str(CIRQ_GRAMMAR_JSON),
    )?;
    let validated = stage(
        "ValidatedGrammar::from_raw",
        ValidatedGrammar::from_raw(&raw),
    )?;
    let lexical = LexicalFacts::from_grammar(&validated);
    let parser = stage(
        "normalize_from_validated",
        ParserGrammar::normalize_from_validated(&validated, &lexical),
    )?;
    let parser = stage(
        "prepare_productions",
        parser.prepare_productions_for_items(),
    )?;
    let table = stage(
        "ParseTable::from_grammar",
        ParseTable::from_grammar(&parser),
    )?;
    let plan = stage(
        "WeavyParsePlan::new",
        WeavyParsePlan::new(&validated, &parser, &table),
    )?;

    let report = stage(
        "parse",
        parse_prepared_weavy_with_report(&plan, &parser, &table, CIRQ_SAMPLE),
    )?;
    let resolved = report
        .accepted_resolved_tree(&parser, CIRQ_SAMPLE)
        .ok_or("no accepted parse")?;

    // The payoff: CST -> generated typed AST. `source_file` is `repeat(_top_level)`,
    // so there's no wrapper struct — the top level is a Vec<TopLevel>, one per item.
    let items: Vec<ast::TopLevel> = resolved
        .children()
        .iter()
        .filter(|c| c.named() && !c.extra())
        .map(ast::lower_top_level)
        .collect();

    println!(
        "[spike-cirq-ast] lowered cirq -> {} top-level item(s) of the generated typed AST\n",
        items.len()
    );
    println!("{items:#?}");
    Ok(())
}
