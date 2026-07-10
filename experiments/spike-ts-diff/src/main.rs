//! spike-ts-diff — differential oracle for gate #3 of the snark adoption verdict.
//!
//! For each cirq input, snark's parse tree must match REAL tree-sitter's. That's
//! the disagreement that matters: tree-sitter is the reference snark reimplements.
//! We reuse cirq's *committed* tree-sitter grammar (`cirq-grammar/src/grammar.json`,
//! external scanner and all) on the snark side, and the CLI (`tree-sitter parse`,
//! which compiles `scanner.c`) as the oracle.
//!
//! The open question this closes: FINDINGS.md only proved snark parses a sample
//! that never hits the external scanner. The `code "lang" { … }` block DOES —
//! `code_body` is an external token produced by `scanner.c`'s brace-depth counter.
//! Snark here runs WITHOUT a hosted scanner (the differential entry points pass
//! no scanner), so it can only fall back to the grammar's internal placeholder
//! rule `code_body: token(prec(-1, /[^}]+/))`. This harness quantifies exactly
//! where that fallback agrees with, and diverges from, the real scanner.
//!
//! Cases are tagged:
//!   - `Match`          snark MUST equal tree-sitter (a divergence fails the run).
//!   - `ScannerBacked`  hits the external scanner; report-only, since snark has
//!                      no hosted scanner wired yet. Divergence here IS the gate-3
//!                      finding, not a regression.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use snark::grammar::RawGrammarJson;
use snark::lexical::LexicalFacts;
use snark::lower::weavy::{WeavyParsePlan, parse_prepared_weavy_tree};
use snark::parser::{ParseTable, ParserGrammar};
use snark::validated::ValidatedGrammar;

const CIRQ_GRAMMAR_JSON: &str = include_str!("../../../cirq-grammar/src/grammar.json");

#[derive(Clone, Copy, PartialEq)]
enum Tag {
    Match,
    ScannerBacked,
}

/// A prepared cirq grammar: everything the parse entry point needs, built once.
struct Prepared {
    parser: ParserGrammar,
    table: ParseTable,
    plan: WeavyParsePlan,
}

fn prepare() -> Result<Prepared, String> {
    let raw = RawGrammarJson::from_tree_sitter_json_str(CIRQ_GRAMMAR_JSON)
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

fn snark_sexp(p: &Prepared, input: &str) -> String {
    match parse_prepared_weavy_tree(&p.plan, &p.parser, &p.table, input) {
        Ok(tree) => tree.to_sexp(),
        Err(e) => format!("PARSE-ERR: {e:?}"),
    }
}

/// tree-sitter's s-expression via the CLI, run in cirq-grammar/ so it uses the
/// committed grammar + compiled scanner.c. The input is written to an absolute
/// temp path; cwd selects the grammar, not the file location.
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

/// Canonicalize to the named-node tree: keep only `(` + the node kind that
/// immediately follows it, and `)`. Everything else is s-expression *formatting*
/// the two tools legitimately differ on — tree-sitter prints field labels
/// (`name: (identifier …)`), position ranges (`[r,c] - [r,c]`) and anonymous
/// quoted terminals; snark's `to_sexp()` omits field labels. A field label is a
/// bare word NOT preceded by `(`, so anchoring on `(` drops it while keeping the
/// structure both sides must agree on.
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

/// Structural comparison with comment nodes removed — tree-sitter attaches
/// `extras` (comments) to a different parent than snark does, a well-known and
/// usually-benign divergence. Stripping them isolates whether the *grammar*
/// structure agrees.
fn strip_comments(normalized: &str) -> String {
    normalized
        .replace("(line_comment)", "")
        .replace("(block_comment)", "")
        .replace("(comment)", "")
}

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    // experiments/spike-ts-diff -> repo root is two levels up.
    let repo = manifest.parent().unwrap().parent().unwrap();
    let grammar_dir = repo.join("cirq-grammar");
    let examples = repo.join("examples/cirq");
    let tmp = env::temp_dir().join("spike-ts-diff-in.cirq");

    if Command::new("tree-sitter")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("[spike-ts-diff] `tree-sitter` CLI not found on PATH — run under `nix develop`");
        std::process::exit(1);
    }

    let prepared = match prepare() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[spike-ts-diff] grammar preparation failed: {e}");
            std::process::exit(1);
        }
    };

    // Corpus. Example .cirq files exercise the structural grammar (no externals);
    // the inline code-block cases target the external scanner specifically.
    let mut corpus: Vec<(String, String, Tag)> = Vec::new();

    let mut example_files: Vec<PathBuf> = fs::read_dir(&examples)
        .expect("read examples/cirq")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "cirq"))
        .collect();
    example_files.sort();
    for path in example_files {
        let name = format!("example/{}", path.file_name().unwrap().to_string_lossy());
        let src = fs::read_to_string(&path).expect("read example");
        corpus.push((name, src, Tag::Match));
    }

    // Scanner-backed code blocks, escalating brace complexity.
    let code_cases: &[(&str, &str)] = &[
        ("code/empty", "circuit demo {\n    code \"rust\" {}\n}\n"),
        (
            "code/brace_free",
            "circuit demo {\n    code \"js\" {\n        const x = 1;\n    }\n    R1: resistor(a -> b, 1k)\n}\n",
        ),
        (
            "code/nested_braces",
            "circuit demo {\n    code \"js\" {\n        const obj = { a: 1, b: { c: 2 } };\n    }\n    R1: resistor(a -> b, 1k)\n}\n",
        ),
        (
            "code/string_with_braces",
            "circuit demo {\n    code \"js\" {\n        const s = \"} fake brace {\";\n    }\n    R1: resistor(a -> b, 1k)\n}\n",
        ),
    ];
    for (name, src) in code_cases {
        corpus.push((name.to_string(), src.to_string(), Tag::ScannerBacked));
    }

    let mut match_fail = 0usize;
    let mut scanner_diverge = 0usize;
    let mut scanner_ok = 0usize;

    println!("=== spike-ts-diff: snark vs tree-sitter over the cirq corpus ===\n");
    for (name, src, tag) in &corpus {
        let sn = normalize(&snark_sexp(&prepared, src));
        let ts = normalize(&tree_sitter_sexp(&grammar_dir, &tmp, src));
        let exact = sn == ts && !sn.is_empty();
        let modulo_comments =
            !exact && !sn.is_empty() && strip_comments(&sn) == strip_comments(&ts);
        let agree = exact || modulo_comments;

        let label = match tag {
            Tag::Match => "MATCH-REQUIRED",
            Tag::ScannerBacked => "SCANNER-BACKED",
        };
        let verdict = if exact {
            "agree  "
        } else if modulo_comments {
            "agree* "
        } else {
            "DIVERGE"
        };
        println!("[{verdict}] ({label}) {name}");
        if !agree {
            // Raw (un-normalized) snark output makes parse errors legible.
            let raw_sn = snark_sexp(&prepared, src);
            println!("         snark:       {}", elide(&raw_sn, 200));
            println!("         snark(norm):  {}", elide(&sn, 200));
            println!("         tree-sitter:  {}", elide(&ts, 200));
            match tag {
                Tag::Match => match_fail += 1,
                Tag::ScannerBacked => scanner_diverge += 1,
            }
        } else if *tag == Tag::ScannerBacked {
            scanner_ok += 1;
        }
    }
    println!("\n(agree* = identical once comment nodes are stripped; tree-sitter and");
    println!(" snark attach `extras` to different parents — benign for AST lowering.)");

    let _ = fs::remove_file(&tmp);

    println!("\n=== summary ===");
    println!(
        "structural corpus: {} example(s), {} unexpected divergence(s)",
        corpus.iter().filter(|(_, _, t)| *t == Tag::Match).count(),
        match_fail,
    );
    println!(
        "scanner-backed code blocks: {scanner_ok} agree, {scanner_diverge} diverge \
         (snark has no hosted external scanner; divergence is the gate-3 finding)"
    );

    // Only a MATCH-REQUIRED divergence is a real regression. Scanner divergence is
    // expected until cirq's scanner.c is hosted in snark, so it does not fail CI.
    if match_fail > 0 {
        eprintln!(
            "\n[spike-ts-diff] FAILED: {match_fail} structural case(s) diverged from tree-sitter"
        );
        std::process::exit(1);
    }
    println!("\n[spike-ts-diff] done");
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
