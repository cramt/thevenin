# Run 3 — Implement the Standard Tree-sitter Grammar for Cirq

## Objective

Implement the **standard Tree-sitter grammar** for Cirq using the language spec from the previous run.

This run is syntax-only.

---

## Required implementation style

Use a conventional Tree-sitter grammar project.

Expected shape:

```text
grammar/tree-sitter-cirq/
  grammar.js
  package.json
  tree-sitter.json
  src/
  queries/
  test/
```

Use **pnpm** if JS package management is needed.

Do **not** use `rust-sitter` for this run.

---

## Deliverables

Create or update at least:

- `grammar/tree-sitter-cirq/grammar.js`
- `grammar/tree-sitter-cirq/queries/highlights.scm`
- `grammar/tree-sitter-cirq/queries/locals.scm`
- `grammar/tree-sitter-cirq/queries/tags.scm`
- corpus test files under `grammar/tree-sitter-cirq/test/corpus/`
- a grammar README

---

## Grammar goals

1. parse the full Cirq v0.1 surface syntax into a stable CST
2. provide good node names and fields for AST lowering
3. recover reasonably from broken/incomplete input
4. support editor-facing query files

---

## Required node coverage

Recommended node kinds include:

- `source_file`
- `netlist_decl`
- `use_decl`
- `global_decl`
- `primitive_decl`
- `subckt_decl`
- `bench_decl`
- `sim_decl`
- `port_decl`
- `param_decl`
- `let_decl`
- `net_decl`
- `instance_decl`
- `native_impl_decl`
- `analysis_decl`
- `option_decl`
- `save_decl`
- `measure_decl`
- `identifier`
- `qualified_identifier`
- `string_literal`
- `number_literal`
- `unit_literal`
- `binary_expr`
- `unary_expr`
- `connection_arg`
- `param_arg`
- `vector_ref`
- `slice_ref`

Use fields like:

- `name`
- `target`
- `body`
- `ports`
- `params`
- `backend`
- `abi`
- `package`
- `library`
- `symbol`

---

## Required grammar behavior

### Extras

Treat whitespace and comments as extras.

### Expression precedence

Implement explicit precedence for:

1. grouping
2. unary `+ -`
3. `* /`
4. `+ -`

### Recovery

Grammar should recover around:

- missing semicolons
- incomplete blocks
- incomplete argument lists
- malformed connection maps

---

## Query file goals

### `highlights.scm`

Highlight:

- keywords
- strings
- numbers/unit literals
- comments
- declaration heads
- built-in names if needed

### `locals.scm`

Capture structural locals for:

- params
- lets
- ports
- local nets
- instance names

### `tags.scm`

Tag:

- `primitive`
- `subckt`
- `bench`
- `sim`

---

## Required tests

Write corpus coverage for:

1. minimal file
2. imports and globals
3. primitive with native impl
4. primitive with backend-specific impls
5. subckt with params and instances
6. positional connections
7. named connections
8. vector/slice references
9. bench + sim file
10. malformed file recovery

Also include malformed cases for:

- missing semicolon
- unterminated block
- malformed instance
- malformed `impl native`

---

## Acceptance criteria

This run is complete only if:

1. `tree-sitter generate` succeeds
2. `tree-sitter test` succeeds
3. the grammar covers all major spec constructs
4. queries exist and are useful
5. CST names/fields are stable enough for the Rust frontend
