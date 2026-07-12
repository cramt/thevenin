# snark + weavy spike — findings

Trying Amos' unmerged `snark-playground-rebased` branch (facet-rs/facet PR #2431)
against thevenin/cirq, as an exercise + feedback. Three spikes, validated with the commands below:

| crate | what it proves |
|-------|----------------|
| `spike-weavy` | thevenin's `.control` interpreter modeled on weavy's lowered-program substrate — `let`/`print` + a named-block call, with call-frame state surviving block return. |
| `spike-snark` | cirq's **existing** tree-sitter `grammar.json` parsed by snark into a 52-node, field-labeled CST (params, if/elseif/else, element_inst, nested analysis). |
| `spike-cirq-ast` | snark-dsl codegen turns cirq's `grammar.js` (+ a 7-line annotations file) into a **typed, spanned AST** (47 types, 1223 lines) and lowers `conditional.cirq` into it. |
| `spike-ts-diff` | **differential oracle (gate 3):** snark's parse of cirq's committed grammar vs REAL tree-sitter (CLI) over the corpus. 15/15 valid inputs (7 examples + 8 parser probes) match structurally; scanner-backed `code` blocks are where it splits — see below. |

Everything runs in an isolated nested workspace (`experiments/`, own lockfile),
excluded from the root so thevenin's crates stay on facet 0.46 while this sandbox
pulls facet 0.50-rc. Toolchain: rustc 1.94.1 (nix devShell).

## Reproducing

From the repo root:

```bash
nix develop --command bash -lc "cd experiments && just check"
```

## What worked well

1. **Consuming monorepo crates by git-dep "just works."** `snark`/`weavy`/`snark-dsl`
   as `{ git, branch }` resolved their whole facet-0.50-rc subgraph with no
   vendoring or submodules. Cold builds ~11s (weavy) to a few min (snark-dsl+boa).
2. **No facet-version civil war.** thevenin is on facet **0.46** (crates.io); snark
   needs **0.50-rc**. Because snark's parse API takes a grammar-json *string* and
   returns its *own* CST, facet types never cross the boundary — the two majors
   coexist with zero conflict. Worth advertising: *adopt snark without upgrading
   your facet.*
3. **grammar.js via boa = no node.** `snark-dsl` (feature `typed-ast`) evaluates
   grammar.js with boa_engine, so codegen needs no Node toolchain. Matches cirq's
   own "no node needed" stance perfectly.
4. **tree-sitter compat is real for this slice**: snark accepted a grammar that
   declares an **external scanner** (`externals: [$.code_body]`) and parsed a
   sample that doesn't hit those external tokens. Scanner-backed `code` blocks
   remain untested.
5. **weavy is a lovely minimal substrate.** BYO `Op` + one `Step` trait, and
   `Control::CallBlock` gives call frames for free. A `.control` core in ~40 lines.
6. **Derived cardinality + auto-boxing are great.** `ParamDecl { ty: Option<…>,
   value: Option<Expr> }`, `items: Vec<CircuitItem>`, and recursive `Expr`
   variants boxed automatically (`Box<TernaryExpression>`). Nothing hand-derived.

## Bug (with fix)

**`typed_ast::rust_field_name` only raw-escapes the keyword `type`, not others.**
`snark-dsl/src/typed_ast.rs:1132`:

```rust
fn rust_field_name(name: &str, mult: Mult) -> String {
    match (mult, name) {
        (Mult::Many, "leaf") => "leaves".to_string(),
        (Mult::Many, s) if s.ends_with('y') => format!("{}ies", &s[..s.len()-1]),
        (Mult::Many, s) if !s.ends_with('s') => format!("{s}s"),
        (_, "type") => "ty".to_string(),   // only `type` is handled
        (_, s) => s.to_string(),           // every other keyword emitted raw
    }
}
```

cirq has `field("else", …)` on `ternary_expression`, so codegen emitted
`pub else: Expr` / `self.else` → won't parse (`else` is a keyword). Suggested fix:
raw-escape reserved idents (`r#{s}`) for the keywords that permit it, and keep the
rename trick (`type`→`ty`) only for the few that can't be raw (`crate/self/super/Self`).
Worked around here by renaming just that field in a build-time grammar copy.

## Main finding for a real migration

**snark derives the typed AST purely from `field()`-labeled children.** cirq's
tree-sitter grammar was written for CST *walking*, so bodies are bare
`repeat($._circuit_item)` with no field. Result: `CircuitDecl` captured only
`name` and **dropped the entire body**. Fielding it —
`repeat(field("item", $._circuit_item))` — immediately gave
`items: Vec<CircuitItem>` with the full param/conditional/element tree.

So the migration cost isn't Rust, it's **grammar enrichment**: add `field()` to
every child the AST needs. For a grammar authored the snark/vix way from the start
this is free; for an existing tree-sitter grammar it's the real work. Might be
worth a codegen lint: "rule X has unfielded named children not present in its AST."

## Ergonomics nits

- The parse entry point is a 6-step incantation (`RawGrammarJson → ValidatedGrammar
  → LexicalFacts → ParserGrammar → prepare_productions_for_items → ParseTable →
  WeavyParsePlan → parse`). Had to crib it from `vix/src/lib.rs`. A
  `snark::Parser::from_tree_sitter_json(&str)` one-liner would lower the bar a lot.
- Each pipeline stage returns a distinct error type → per-stage `map_err`. A unified
  `snark::ParseError` would clean up `?`-composition.
- The generated code depends on a `crate::support` module with an implicit surface
  (Span/Spanned/span/node_text/decode_*/field_one/field_opt/fields). It's copied
  from vix here; shipping it as `snark::support` (or generating it) would remove a
  copy-paste dependency between every consumer and vix's internals.

## Gate 3 — differential vs real tree-sitter (`spike-ts-diff`)

The 07-07 verdict held snark adoption on three gates; gate 3 was "snark passes
differentially against the cirq corpus incl. scanner-backed `code` blocks."
`spike-ts-diff` runs cirq's *committed* `grammar.json` through snark and compares
its named-node tree against the `tree-sitter` CLI (which compiles `scanner.c`).
Reproduce: `nix develop --command bash -lc "cd experiments && just ts-diff"`.

**Structural grammar: snark matches tree-sitter on every valid input tested.**
All 7 example `.cirq` files, plus 8 parser probes for constructs the examples miss
(ternary + nested ternary — the rule that carried the keyword-`else` field —,
right-assoc `**`, precedence, unary, and simple/alias/named imports). 15/15
node-for-node identical once (a) field labels are dropped — snark's `to_sexp()`
omits them, tree-sitter prints them — and (b) comment nodes are stripped: the two
attach `extras` (comments) to different parents, which is benign for AST lowering.
So for the whole non-scanner language, snark's parser is a faithful drop-in — **no
parser bug surfaced.** (The one real snark bug, keyword field escaping, is in
snark-*dsl* codegen, not the parser — fixed by facet#2465.)

**Error recovery differs (info-only, not a bug).** On malformed input snark's
recovery tree is coarser than tree-sitter's: a missing `}` collapses snark to a
bare `(ERROR)` root (tree-sitter keeps the partial tree + inserts `MISSING "}"`);
a missing `)` makes snark drop the whole malformed `element_inst` (tree-sitter
reconstructs it + `MISSING ")"`). Recovery is heuristic and matching tree-sitter's
exact recovery tree is out of differential scope — the harness reports these but
does not fail on them.

**Scanner-backed `code "lang" { … }` blocks: snark passes only without braces.**
snark runs with *no hosted external scanner*, so `code_body` falls back to cirq's
placeholder rule `token(prec(-1, /[^}]+/))`. Result, by case:

| input | snark | tree-sitter | verdict |
|-------|-------|-------------|---------|
| empty `code "rust" {}` | `(code_decl (string_literal))` | same | agree |
| brace-free body | `(code_decl … (code_body))` | same | agree |
| nested braces `{ a: { c } }` | **`PARSE-ERR: NoToken`** | full `(code_body)` | **DIVERGE** |
| `}` inside a string literal | **`PARSE-ERR: NoToken`** | full `(code_body)` | **DIVERGE** |

The fallback regex stops at the first `}`; the parser then can't close `code_decl`
and hard-errors. `scanner.c` instead counts brace depth and skips string literals,
so it swallows the whole body. **This confirms the memory's risk precisely: snark
cannot parse a realistic `code` block until cirq's `scanner.c` is hosted in snark's
scanner host — the internal placeholder is a strict subset of the real language.**

Gate 3 status: **partial pass.** Structural corpus ✅; scanner-backed blocks blocked
on hosting the C scanner (next slice below).

## Not covered (next slices)

- **Host cirq's `scanner.c` in snark** and re-run `spike-ts-diff` — the one thing
  standing between snark and a full gate-3 pass. **Blocked upstream (verified
  2026-07-12 at cramt/facet main `ce6a9e03`):** snark's *parser* does not yet
  execute external scanners mid-parse. The parse tables model external tokens
  (`parser.rs`: `ExternalId`, `ScannerSnapshotId`, valid-symbol sets) and
  `lexical.rs` derives the valid-external masks, but `runtime_input.rs` never
  invokes a scanner — no `extern "C"`, no `.scan()` in the parse loop. The only
  compiled-scanner execution lives in `snark-scanner-host`, a `publish = false`
  **test crate hardcoded to the reduced-CSS fixture** (CSS symbols + path +
  `CSS_EXTERNAL_SYMBOL_COUNT = 3` baked in) that proves the tree-sitter ABI in
  isolation. snark's own `docs/architecture/lexer-scanner-abi.md` names the gap:
  *"the missing pieces are … hardened lexical automata, parser-state masks,
  **scanner execution state**, and oracle checks … the right boundary for the
  next runtime work."* So this slice is not implementable in the thevenin spike;
  it waits on snark growing a scanner-execution runtime (mask → invoke hosted
  scanner → consume result token). Until then `code` blocks with braces are
  unparseable and gate 3 stays partial.
- weavy's **async suspend/resume lane** (`weavy::r#async`) for `.control`'s `resume`.
- Comparing the generated AST against hand-written `cirq-ast` field-by-field.
