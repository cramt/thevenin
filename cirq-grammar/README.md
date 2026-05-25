# tree-sitter-cirq

Tree-sitter grammar for the **Cirq** circuit description language, part of the [Thevenin](https://github.com/cramt/thevenin) project.

Cirq is a typed, structured alternative to SPICE netlists. This grammar parses Cirq source files into a concrete syntax tree (CST) suitable for editor tooling and AST lowering.

## Building

Requires `tree-sitter-cli` ≥ 0.25.

```bash
# Generate the parser from grammar.js
tree-sitter generate

# Run the corpus tests
tree-sitter test
```

In the Thevenin project, all commands run through `nix develop`:

```bash
nix develop --command tree-sitter generate
nix develop --command tree-sitter test
```

## Node Types

### Top-level declarations

| Node | Description |
|------|-------------|
| `source_file` | Root node |
| `circuit_decl` | Circuit declaration |
| `module_decl` | Reusable module (subcircuit) |
| `model_decl` | Device model definition |
| `import_decl` | File import |

### Body declarations

| Node | Description |
|------|-------------|
| `port_decl` | Port with direction (`in`, `out`, `inout`) |
| `param_decl` | Parameter with optional default |
| `let_decl` | Computed binding |
| `global_decl` | Global net declaration |
| `element_inst` | Element instantiation (resistor, source, etc.) |
| `module_inst` | Module instantiation (qualified name) |
| `analysis_decl` | Analysis command (op, dc, ac, tran, etc.) |

### Expressions

| Node | Description |
|------|-------------|
| `binary_expression` | Binary operation with fields `left`, `operator`, `right` |
| `unary_expression` | Unary operation with fields `operator`, `operand` |
| `call_expression` | Function call with field `function` |
| `paren_expression` | Parenthesized expression(s) / tuple |
| `list_literal` | `[...]` list |
| `block_literal` | `{ key: val, ... }` block |

### Literals and names

| Node | Description |
|------|-------------|
| `number_literal` | Numeric literal with optional SI suffix (k, M, Meg, u, n, p, f, etc.) |
| `string_literal` | Double-quoted string |
| `boolean_literal` | `true` or `false` |
| `gnd` | Built-in ground net |
| `identifier` | Simple name |
| `qualified_name` | Dotted name (e.g. `lib.module`) |

### Arguments and connections

| Node | Description |
|------|-------------|
| `argument_list` | Comma-separated arguments |
| `argument` | Single argument (expression, named arg, or connection) |
| `named_argument` | `name: value` |
| `connection` | `from -> to` |
| `named_connection` | `name: from -> to` |

### Other

| Node | Description |
|------|-------------|
| `attribute` | `@name` or `@name(args)` |
| `analysis_setting` | `name: value` inside analysis block |
| `sweep_spec` | `sweep source: start..stop step incr` |
| `model_param` | `name = value` inside model block |
| `block_entry` | `key: value` inside block literal |
| `port_direction` | `in`, `out`, or `inout` |
| `line_comment` | `// ...` |
| `block_comment` | `/* ... */` |

## Expression Precedence

From lowest to highest:

1. `||` (left)
2. `&&` (left)
3. `==` `!=` (left)
4. `<` `>` `<=` `>=` (left)
5. `+` `-` (left)
6. `*` `/` `%` (left)
7. `**` (right)
8. Unary `-` `!`
9. Function call

## Query Files

- **`queries/highlights.scm`** — Syntax highlighting for keywords, literals, declaration names, operators, comments, and attributes.
- **`queries/locals.scm`** — Local scope analysis: circuits/modules as scopes; params, lets, ports, instances as definitions.
- **`queries/tags.scm`** — Symbol tagging for navigation: circuits, modules, models, params, lets, ports, instances.
- **`queries/injections.scm`** — Language injection for `code "lang" { ... }` blocks. Maps short names (`js`, `ts`, `py`, `rs`, `sh`, `md`) to canonical grammars and falls through to the literal string otherwise (so `code "rust" {}`, `code "python" {}`, etc. work as well).

### Embedded code block limitations

The body of a `code "lang" { ... }` block is consumed by a hand-written external scanner (`src/scanner.c`) that counts nested braces and skips over simple `"..."` / `'...'` string literals. The following lexical features are **not** understood, and a `}` appearing inside one of them will close the block early:

- Line comments (`// }`, `# }`, `-- }`, `; }`)
- Block comments (`/* } */`, `(* } *)`, `<!-- } -->`)
- Multiline string forms: Python triple-quoted strings, JS template literals (including `${...}` interpolation), Rust/C++ raw strings, shell here-docs, Lua long brackets
- JS regex literals containing braces in character classes (`/[}]/`)

In practice, object literals, function bodies, and ordinary `"..."` strings work correctly. The limitations are real for users of the affected languages and should be addressed before claiming full embedded-language support. Line-comment handling is the highest-payoff next step.

## Tests

Corpus tests are organized by feature under `test/corpus/`:

| File | Coverage |
|------|----------|
| `basics.txt` | Minimal circuits, modules, models, imports, sweep |
| `imports_and_globals.txt` | Import variants, global declarations |
| `params_and_lets.txt` | Params (typed, attributed, no-default), let bindings |
| `models.txt` | Models, inheritance, attributes |
| `modules.txt` | Ports, params, instances, inheritance, nesting, qualified names |
| `connections.txt` | Positional, named, arrow, mixed arguments |
| `analysis.txt` | op, dc, ac, tran, double sweep, multiple analyses |
| `expressions.txt` | Precedence, associativity, unary, function calls, literals |
| `waveforms_and_literals.txt` | Block literals (pulse), list literals (PWL), booleans |
| `full_circuits.txt` | Complete circuits, semicolons, comments |
| `error_recovery.txt` | Missing braces, missing parens, recovery after errors |

## License

BSD-3-Clause
