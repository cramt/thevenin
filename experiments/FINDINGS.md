# snark + weavy spike — findings

Trying Amos' unmerged `snark-playground-rebased` branch (facet-rs/facet PR #2431)
against thevenin/cirq, as an exercise + feedback. Three spikes, all green:

| crate | what it proves |
|-------|----------------|
| `spike-weavy` | thevenin's `.control` interpreter modeled on weavy's lowered-program substrate — `let`/`print` + a named-block call, with call-frame state surviving block return. |
| `spike-snark` | cirq's **existing** tree-sitter `grammar.json` parsed by snark into a 52-node, field-labeled CST (params, if/elseif/else, element_inst, nested analysis). |
| `spike-cirq-ast` | snark-dsl codegen turns cirq's `grammar.js` (+ a 7-line annotations file) into a **typed, spanned AST** (47 types, 1223 lines) and lowers `conditional.cirq` into it. |

Everything runs in an isolated nested workspace (`experiments/`, own lockfile),
excluded from the root so thevenin's crates stay on facet 0.46 while this sandbox
pulls facet 0.50-rc. Toolchain: rustc 1.94.1 (nix devShell).

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
4. **tree-sitter compat is real**, including an **external-scanner** grammar
   (cirq declares `externals: [$.code_body]`) — parsed without special handling
   for a sample that doesn't hit the external tokens.
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

## Not covered (next slices)

- weavy's **async suspend/resume lane** (`weavy::r#async`) for `.control`'s `resume`.
- snark's typed-AST codegen exercised only on `conditional.cirq`; broader corpus +
  the external `code "lang" { … }` blocks (which need scanner.c) untested.
- Comparing the generated AST against hand-written `cirq-ast` field-by-field.
