# The Cirq embedded-language registry

Cirq's only control-flow / scripting mechanism is the embedded code block:

```cirq
code "control" {
    run
    let gain = v(out) / v(in)
    print gain
}
```

The `code "lang" { … }` form (grammar rule `code_decl`) carries a free-form
language *tag* and a verbatim body. The point is extensibility: the same
mechanism can host the `.control` interpreter today and an embedded JS/Python
engine or a domain-specific DSL later, without adding new Cirq syntax for each.

The **language registry** is what decides which tags are legal and how each is
executed. Because of the workspace's dependency direction it is necessarily
*two* types in two crates — there is no single crate that owns both the
compile-time grammar surface and the runtime simulation context.

```
cirq-ast → cirq-ir → cirq-frontend → thevenin-control
                       (validate)       (execute)
```

`cirq-frontend` (compile) cannot depend on `thevenin-control` (execute) — that
would cycle — so the registry is split along the same seam.

## Half 1 — validation (`cirq_frontend::LanguageRegistry`)

The accepted set of tags. The default accepts only `"control"`.

```rust
// Default: only "control" is accepted.
let circuit = cirq_frontend::compile(source)?;

// Widen the accepted set for a host that embeds extra languages.
let registry = cirq_frontend::LanguageRegistry::default().register("js");
let circuit = cirq_frontend::compile_with_languages(source, &registry)?;
```

During lowering (`cirq-frontend/src/ir_lower.rs`, the `CircuitItem::Code` arm),
a block whose tag is **not** in the registry produces a spanned
`Diagnostic::error` and is dropped from the IR. Before B4 unknown tags were
silently passed through and then ignored at simulate time; now they fail
loudly at compile time, so typos and unsupported languages surface immediately.

The IR (`cirq_ir::CodeBlock::from_lines`) additionally *pre-parses* the body of
`"control"` blocks into the typed `cirq_ir::control::Statement` AST (it links
`cirq-control-grammar`). Other languages keep `parsed: None` and are handed to
their handler as raw lines. This pre-parse is a convenience/perf detail, not
part of the registry contract.

## Half 2 — execution (`thevenin_control::LanguageRegistry`)

A map from tag to a `LanguageHandler`. The default registers only the built-in
`ControlHandler` for `"control"`, so out-of-the-box behaviour is unchanged.

```rust
let mut registry = thevenin_control::language::LanguageRegistry::default();
registry.register("js", Box::new(MyJsHandler));

let result = thevenin_control::execute_code_blocks_ir(&circuit, &registry)?;
```

`execute_code_blocks_ir` builds one `SimContext` for the circuit and runs every
code block whose tag the registry handles, in declaration order, against that
shared context — stopping early when a handler sets an exit code.
`execute_control_block_ir` / `has_control_block_ir` remain as back-compat
wrappers over the default registry.

### The `LanguageHandler` contract

```rust
pub trait LanguageHandler {
    fn execute(
        &self,
        lines: &[String],
        parsed: Option<&[cirq_ir::control::Statement]>,
        ctx: &mut SimContext,
    ) -> Result<(), String>;
}
```

- **Input — `lines`**: the verbatim block body, one entry per source line.
- **Input — `parsed`**: the IR's pre-parsed typed AST when the IR understood
  the language at construction time (today only `"control"`); otherwise `None`,
  and the handler is responsible for parsing `lines` itself.
- **Input/output — `ctx`**: the live `SimContext`, shared across every block in
  the circuit. A handler runs analyses through it (e.g. via
  `thevenin::circuit`) and communicates results purely through side effects:
  - append to `ctx.plots` to surface result vectors,
  - write to `ctx.output` for printed text,
  - set `ctx.exit_code` to request that execution stop.
- **Return**: `Ok(())` on success (results are in `ctx`), or `Err(message)` on
  a hard failure.

## Keeping the two halves in sync

A host that adds a language must register it in *both* halves. The execution
registry exposes its tags so the validation registry can be derived from it:

```rust
let exec = build_execution_registry();          // thevenin_control::LanguageRegistry
let validate = cirq_frontend::LanguageRegistry::with_languages(exec.tags());
let circuit = cirq_frontend::compile_with_languages(source, &validate)?;
let result  = thevenin_control::execute_code_blocks_ir(&circuit, &exec)?;
```

If a tag is accepted at compile time but has no handler at execution time,
`execute_code_blocks_ir` returns an error — deriving one registry from the
other avoids that mismatch.
