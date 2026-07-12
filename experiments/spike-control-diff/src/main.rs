//! spike-control-diff — the gate-3 differential for `cirq-control-grammar`.
//!
//! The `.control` grammar is the *easy* migration: unlike cirq's main grammar it
//! declares **no external scanner** (`externals: []`, no `scanner.c`), so there is
//! nothing to port to a NESTED/UNTIL primitive — it's a pure structural grammar
//! (if/while/repeat/foreach blocks ending on `end`, echo/set/print/…). This harness
//! just proves snark parses it the same as REAL tree-sitter, node-for-node, over the
//! grammar's own committed test corpus.
//!
//! snark's parse of `cirq-control-grammar/src/grammar.json` is compared against the
//! `tree-sitter` CLI (run in `cirq-control-grammar/`) for every input in the tree-
//! sitter corpus (`test/corpus/*.txt`). Any structural divergence fails the run.
//! Reproduce: `nix develop --command bash -lc "cd experiments && just control-diff"`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use snark::grammar::RawGrammarJson;
use snark::lexical::LexicalFacts;
use snark::lower::weavy::{
    WeavyParsePlan, parse_prepared_weavy_recovering_with_report_and_scanner,
};
use snark::parser::{ParseTable, ParserGrammar};
use snark::validated::ValidatedGrammar;

const CONTROL_GRAMMAR_JSON: &str = include_str!("../../../cirq-control-grammar/src/grammar.json");

/// A prepared grammar: everything the parse entry point needs, built once.
struct Prepared {
    parser: ParserGrammar,
    table: ParseTable,
    plan: WeavyParsePlan,
}

fn prepare(grammar_json: &str) -> Result<Prepared, String> {
    let raw = RawGrammarJson::from_tree_sitter_json_str(grammar_json)
        .map_err(|e| format!("import: {e:?}"))?;
    let validated = ValidatedGrammar::from_raw(&raw).map_err(|e| format!("validate: {e:?}"))?;
    let lexical = LexicalFacts::from_grammar(&validated);
    let parser = ParserGrammar::normalize_from_validated(&validated, &lexical)
        .map_err(|e| format!("normalize: {e:?}"))?
        .prepare_productions_for_items()
        .map_err(|e| format!("prepare: {e:?}"))?;
    let table = ParseTable::from_grammar(&parser).map_err(|e| format!("table: {e:?}"))?;
    let plan =
        WeavyParsePlan::new(&validated, &parser, &table).map_err(|e| format!("plan: {e:?}"))?;
    Ok(Prepared {
        parser,
        table,
        plan,
    })
}

/// snark's named-node s-expression via the RECOVERING parse path — the like-for-like
/// oracle for tree-sitter, which always recovers rather than bailing. No scanner: the
/// control grammar has no externals.
fn snark_sexp(p: &Prepared, input: &str) -> String {
    match parse_prepared_weavy_recovering_with_report_and_scanner(
        &p.plan, &p.parser, &p.table, input, None,
    ) {
        Ok(report) => report.tree().to_sexp(),
        Err(e) => format!("PARSE-ERR: {e:?}"),
    }
}

/// tree-sitter's s-expression via the CLI, run in `cirq-control-grammar/` so it uses
/// the committed grammar.
fn tree_sitter_sexp(grammar_dir: &Path, tmp: &Path, input: &str) -> String {
    fs::write(tmp, input).expect("write ts input");
    let out = Command::new("tree-sitter")
        .arg("parse")
        .arg(tmp)
        .current_dir(grammar_dir)
        .output()
        .expect("run tree-sitter parse");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Canonicalize to the named-node tree: keep only `(` + the node kind that follows,
/// and `)`. Drops field labels (tree-sitter prints them, snark's `to_sexp()` omits
/// them), position ranges, and anonymous quoted terminals — formatting the two tools
/// legitimately differ on.
fn normalize(sexp: &str) -> String {
    let bytes = sexp.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] as char {
            '(' => {
                out.push('(');
                i += 1;
                while i < bytes.len() {
                    let d = bytes[i] as char;
                    if d.is_alphanumeric() || d == '_' {
                        out.push(d);
                        i += 1;
                    } else {
                        break;
                    }
                }
            }
            ')' => {
                out.push(')');
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

/// Comment nodes attach to different parents in the two tools (`extras`); strip them
/// so the comparison is about grammar structure, not extras placement.
fn strip_comments(normalized: &str) -> String {
    normalized
        .replace("(line_comment)", "")
        .replace("(inline_comment)", "")
        .replace("(comment)", "")
}

/// Parse a tree-sitter corpus file into `(name, input)` pairs. Format is
/// `===\nname\n===\ninput\n---\ntree`, repeated. The expected tree is ignored — the
/// `tree-sitter` CLI regenerates it as the oracle.
fn parse_corpus(text: &str) -> Vec<(String, String)> {
    let is_eq = |l: &str| l.starts_with("====");
    let is_dash = |l: &str| l.starts_with("----");
    let lines: Vec<&str> = text.lines().collect();
    let mut cases = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !is_eq(lines[i]) {
            i += 1;
            continue;
        }
        i += 1; // past opening `===`
        let mut name = Vec::new();
        while i < lines.len() && !is_eq(lines[i]) {
            name.push(lines[i]);
            i += 1;
        }
        i += 1; // past closing `===`
        let mut input = Vec::new();
        while i < lines.len() && !is_dash(lines[i]) {
            input.push(lines[i]);
            i += 1;
        }
        // skip the tree section up to the next `===`
        while i < lines.len() && !is_eq(lines[i]) {
            i += 1;
        }
        cases.push((name.join(" ").trim().to_string(), input.join("\n")));
    }
    cases
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    // experiments/spike-control-diff -> repo root is two levels up.
    let repo = manifest.parent().unwrap().parent().unwrap();
    let grammar_dir = repo.join("cirq-control-grammar");
    let corpus_dir = grammar_dir.join("test/corpus");
    let tmp = env::temp_dir().join("spike-control-diff-in.control");

    if Command::new("tree-sitter")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("[spike-control-diff] `tree-sitter` CLI not found — run under `nix develop`");
        std::process::exit(1);
    }

    let prepared = match prepare(CONTROL_GRAMMAR_JSON) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[spike-control-diff] grammar preparation failed: {e}");
            std::process::exit(1);
        }
    };

    let mut corpus_files: Vec<PathBuf> = fs::read_dir(&corpus_dir)
        .expect("read control corpus")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "txt"))
        .collect();
    corpus_files.sort();

    let mut cases: Vec<(String, String)> = Vec::new();
    for path in &corpus_files {
        let file = path.file_stem().unwrap().to_string_lossy();
        let text = fs::read_to_string(path).expect("read corpus file");
        for (name, input) in parse_corpus(&text) {
            cases.push((format!("{file}/{name}"), input));
        }
    }

    println!("=== spike-control-diff: snark vs tree-sitter over the control corpus ===\n");
    let mut fail = 0usize;
    for (name, input) in &cases {
        let sn = normalize(&snark_sexp(&prepared, input));
        let ts = normalize(&tree_sitter_sexp(&grammar_dir, &tmp, input));
        let exact = sn == ts && !sn.is_empty();
        let modulo_comments =
            !exact && !sn.is_empty() && strip_comments(&sn) == strip_comments(&ts);
        let agree = exact || modulo_comments;

        let verdict = if exact {
            "agree  "
        } else if modulo_comments {
            "agree* "
        } else {
            "DIVERGE"
        };
        println!("[{verdict}] {name}");
        if !agree {
            let raw_sn = snark_sexp(&prepared, input);
            println!("         snark:       {}", elide(&raw_sn, 240));
            println!("         snark(norm):  {}", elide(&sn, 240));
            println!("         tree-sitter:  {}", elide(&ts, 240));
            fail += 1;
        }
    }

    let _ = fs::remove_file(&tmp);

    println!("\n(agree* = identical once comment nodes are stripped; tree-sitter and");
    println!(" snark attach `extras` to different parents — benign for AST lowering.)");
    println!("\n=== summary ===");
    println!(
        "control corpus: {} case(s), {fail} divergence(s) \
         (pure structural — the control grammar has no external scanner)",
        cases.len(),
    );

    if fail > 0 {
        eprintln!("\n[spike-control-diff] FAILED: {fail} case(s) diverged from tree-sitter");
        std::process::exit(1);
    }
    println!("\n[spike-control-diff] done");
}

fn elide(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() > max {
        let truncated: String = s.chars().take(max).collect();
        format!("{truncated}…")
    } else {
        s
    }
}
