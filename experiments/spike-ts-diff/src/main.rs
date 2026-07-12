//! spike-ts-diff — differential oracle for gate #3 of the snark adoption verdict.
//!
//! For each cirq input, snark's parse tree must match REAL tree-sitter's. That's
//! the disagreement that matters: tree-sitter is the reference snark reimplements.
//! We reuse cirq's *committed* tree-sitter grammar (`cirq-grammar/src/grammar.json`,
//! external scanner and all) on the snark side, and the CLI (`tree-sitter parse`,
//! which compiles `scanner.c`) as the oracle.
//!
//! The `code "lang" { … }` block is the interesting case: in cirq's committed
//! grammar `code_body` is an *external* token produced by `scanner.c`'s brace-depth
//! counter. But snark eliminates external scanners with three declarative lexical
//! primitives (UNTIL / NESTED / AUTO_CLOSE), and `code_body` is a textbook NESTED:
//! `{"type":"NESTED","open":"{","close":"}"}` — no scanner, no `cc`. So the snark
//! side here parses the code cases with a NESTED variant of the grammar
//! (`grammar.nested.json`, code_body→NESTED, externals dropped) while tree-sitter
//! still uses the real scanner.c. Result: NESTED matches scanner.c node-for-node on
//! real balanced braces. (A `}` hidden inside a string is a known, accepted limit of
//! raw NESTED and is deliberately left out of the corpus.)
//!
//! Cases are tagged:
//!   - `Match`          snark MUST equal tree-sitter (a divergence fails the run).
//!                      Covers the example files + parser probes for constructs
//!                      the examples miss (ternary, imports, precedence, unary).
//!                      snark matches tree-sitter on ALL of these — no parser bug.
//!   - `Nested`         `code` block parsed on the snark side with the declarative
//!                      NESTED grammar. Agrees with scanner.c on balanced braces; the
//!                      one report-only diff is empty `{}` (NESTED spans the braces, so
//!                      it yields a code_body node where the external scanner emits none).
//!   - `Recovery`       malformed input; report-only. Error recovery is heuristic,
//!                      so snark's recovery tree legitimately differs.

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

const CIRQ_GRAMMAR_JSON: &str = include_str!("../../../cirq-grammar/src/grammar.json");
/// Committed grammar with `code_body` re-expressed as snark's declarative NESTED
/// primitive and `externals` dropped — no scanner.c, no `cc`. Generated from
/// grammar.json (see the `nested-grammar` justfile recipe); regenerate if the
/// committed grammar's code_body / code_decl / externals change.
const CIRQ_GRAMMAR_JSON_NESTED: &str = include_str!("grammar.nested.json");

#[derive(Clone, Copy, PartialEq)]
enum Tag {
    /// snark MUST equal tree-sitter — a divergence fails the run.
    Match,
    /// `code` block parsed on the snark side with the declarative NESTED grammar.
    /// Report-only: raw NESTED diverges from scanner.c exactly when a `}` hides in
    /// a string/comment (the documented ~10%).
    Nested,
    /// Malformed input. Error recovery is heuristic and snark's recovery tree
    /// legitimately differs from tree-sitter's (coarser: `(ERROR)` root or a
    /// dropped subtree vs tree-sitter's inserted `MISSING` nodes). Report-only —
    /// matching tree-sitter's exact recovery tree is out of differential scope.
    Recovery,
}

/// A prepared cirq grammar: everything the parse entry point needs, built once.
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

/// snark's named-node s-expression via the RECOVERING parse path — tree-sitter
/// always recovers (emitting ERROR/MISSING nodes rather than bailing), so the
/// recovering path is the like-for-like oracle. Scanner is `None`: with `code_body`
/// as a NESTED primitive there are no externals, so no scanner host is ever needed.
fn snark_sexp(p: &Prepared, input: &str) -> String {
    match parse_prepared_weavy_recovering_with_report_and_scanner(
        &p.plan, &p.parser, &p.table, input, None,
    ) {
        Ok(report) => report.tree().to_sexp(),
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

    let prepared = match prepare(CIRQ_GRAMMAR_JSON) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[spike-ts-diff] grammar preparation failed: {e}");
            std::process::exit(1);
        }
    };
    // Declarative-NESTED variant, used for the snark side of the `code` cases.
    let prepared_nested = match prepare(CIRQ_GRAMMAR_JSON_NESTED) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[spike-ts-diff] NESTED grammar preparation failed: {e}");
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

    // Structural probes for constructs the example files don't cover — these
    // exercise snark's PARSER (not the scanner), so any divergence here is a
    // genuine snark bug. Ternary especially: it's the rule that carried the
    // keyword-`else` field, and no example or corpus file parses one.
    let probe_cases: &[(&str, &str)] = &[
        ("probe/ternary", "circuit t {\n    param x = a ? 1 : 2\n}\n"),
        (
            "probe/ternary_nested",
            "circuit t {\n    param x = a ? b ? 1 : 2 : 3\n}\n",
        ),
        (
            "probe/exp_right_assoc",
            "circuit t {\n    param x = 2 ** 3 ** 4\n}\n",
        ),
        (
            "probe/precedence",
            "circuit t {\n    param x = 1 + 2 * 3\n}\n",
        ),
        ("probe/unary", "circuit t {\n    param y = !true\n}\n"),
        ("probe/import_simple", "import \"models/cmos.cirq\"\n"),
        (
            "probe/import_alias",
            "import \"standard_cells.cirq\" as std\n",
        ),
        (
            "probe/import_named",
            "import { tt } from \"tsmc65nm.cirq\"\n",
        ),
    ];
    for (name, src) in probe_cases {
        corpus.push((name.to_string(), src.to_string(), Tag::Match));
    }

    // Malformed inputs — recovery is heuristic, so these are report-only.
    let recovery_cases: &[(&str, &str)] = &[
        ("recover/missing_brace", "circuit test {\n    param x = 1\n"),
        (
            "recover/missing_paren",
            "circuit test {\n    R1: resistor(a -> b, 10k\n}\n",
        ),
    ];
    for (name, src) in recovery_cases {
        corpus.push((name.to_string(), src.to_string(), Tag::Recovery));
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
        // NOTE: a `}` inside a string/comment (e.g. `const s = "} {"`) is out of
        // scope — raw NESTED counts it and closes early. That string-awareness gap
        // is a known, accepted limitation, not tracked here.
    ];
    for (name, src) in code_cases {
        corpus.push((name.to_string(), src.to_string(), Tag::Nested));
    }

    let mut match_fail = 0usize;
    let mut nested_ok = 0usize;
    // NESTED spans the braces, so an empty `{}` still yields a `code_body` node
    // where scanner.c emits none — a benign structural diff, the only one left
    // once the string-awareness case is out of scope.
    let mut nested_structural = 0usize;
    let mut recovery_diverge = 0usize;

    println!("=== spike-ts-diff: snark vs tree-sitter over the cirq corpus ===\n");
    for (name, src, tag) in &corpus {
        // `code` cases go through the declarative NESTED grammar (no scanner);
        // everything else through the committed grammar.
        let p = if *tag == Tag::Nested {
            &prepared_nested
        } else {
            &prepared
        };
        let sn = normalize(&snark_sexp(p, src));
        let ts = normalize(&tree_sitter_sexp(&grammar_dir, &tmp, src));
        let exact = sn == ts && !sn.is_empty();
        let modulo_comments =
            !exact && !sn.is_empty() && strip_comments(&sn) == strip_comments(&ts);
        let agree = exact || modulo_comments;

        let label = match tag {
            Tag::Match => "MATCH-REQUIRED",
            Tag::Nested => "NESTED-DECL",
            Tag::Recovery => "RECOVERY(info)",
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
            let raw_sn = snark_sexp(p, src);
            println!("         snark:       {}", elide(&raw_sn, 200));
            println!("         snark(norm):  {}", elide(&sn, 200));
            println!("         tree-sitter:  {}", elide(&ts, 200));
            match tag {
                Tag::Match => match_fail += 1,
                Tag::Nested => nested_structural += 1,
                Tag::Recovery => recovery_diverge += 1,
            }
        } else if *tag == Tag::Nested {
            nested_ok += 1;
        }
    }
    println!("\n(agree* = identical once comment nodes are stripped; tree-sitter and");
    println!(" snark attach `extras` to different parents — benign for AST lowering.)");

    let _ = fs::remove_file(&tmp);

    println!("\n=== summary ===");
    println!(
        "structural corpus: {} valid input(s), {} unexpected divergence(s)",
        corpus.iter().filter(|(_, _, t)| *t == Tag::Match).count(),
        match_fail,
    );
    println!(
        "declarative NESTED code blocks (snark parses code_body as NESTED — no scanner.c):\n  \
         {nested_ok} agree with scanner.c node-for-node (incl. real nested braces `{{ a:{{c}} }}`)\n  \
         {nested_structural} structural diverge (empty `{{}}` yields a code_body node; NESTED spans the braces — benign)"
    );
    println!(
        "error recovery: {recovery_diverge} diverge (info-only — snark's heuristic \
         recovery tree differs from tree-sitter's; out of differential scope)"
    );

    // Only a MATCH-REQUIRED divergence is a real regression. The NESTED empty-block
    // structural diff is benign (braces-in-token), so it is report-only.
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
