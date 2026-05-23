# Run 2 — Write the Full Cirq Language Specification (v0.1)

## Objective

Produce the authoritative written specification for **Cirq**.

This run is still purely about the language design artifact.
It should not implement parser/runtime code.

---

## Primary output

Create:

- `docs/spec/cirq-language-v0.1.md`

Suggested title:

- `Cirq Language Specification v0.1`

---

## What the agent is implementing here

The agent is implementing the **specification document**.

That means defining:

- the syntax of Cirq,
- the semantics of Cirq constructs,
- how names resolve,
- how source sugar canonicalizes,
- what validation rules exist,
- and which things are errors.

This is **not** runtime or solver work.

---

## Scope

The v0.1 spec must cover:

### Top-level constructs

- `netlist`
- `use`
- `global`
- `primitive`
- `subckt`
- `bench`
- `sim`

### Internal constructs

- ports
- params
- local derived bindings
- nets
- instances
- native code-model bindings
- analyses
- save/probe targets
- measurements
- options
- minimal sweep/corner declarations

---

## Required language decisions to lock in

The spec must explicitly settle these choices:

1. **Cirq is named explicitly**.
2. **Named connections are canonical**.
3. **Positional connections may be source sugar only**.
4. **Blocks use `{}` and statements end with `;`**.
5. **No SPICE device-prefix magic**.
6. **Units are first-class and use sane SI-style meaning**.
7. **`primitive` + `impl native` is the v0.1 code-model mechanism**.
8. **`bench` and `sim` are separate from structural design semantics**.
9. **Syntax should remain Tree-sitter-friendly**.

---

## Required sections in the spec

1. introduction and goals
2. non-goals
3. lexical structure
4. file structure
5. literals and units
6. expressions
7. declarations
8. structural semantics
9. native primitive semantics
10. bench and simulation semantics
11. name resolution and scoping
12. canonicalization rules
13. validation rules
14. error model
15. examples
16. versioning notes

---

## Required appendices

### A. EBNF-ish grammar appendix

It does not need to match Tree-sitter syntax exactly, but it must be specific enough that the grammar can be implemented directly.

### B. Canonicalization appendix

Must document at least:

- positional → named connections
- implicit nets → explicit semantic net symbols
- unit/literal normalization
- default/override parameter handling
- how far constant folding goes

---

## Specific required content

### Lexical structure

Define:

- identifiers
- qualified identifiers
- comments
- whitespace handling
- strings
- numeric literals
- unit-bearing literals
- vector/slice references

### Literals and units

Be explicit about how things like these behave:

- `150n`
- `1u`
- `10k`
- `1.8V`
- `20mA`

### Expressions

Keep expressions deliberately small and pure:

- literals
- identifiers
- unary `+ -`
- binary `+ - * /`
- parentheses
- tiny built-in function set only if necessary

### `primitive`

Define typed ports, params, and native bindings.

### `subckt`

Define hierarchy, params, instances, nets, implicit nets, and vectors.

### `bench`

Define structural fixture semantics.

### `sim`

Define declarative simulation intent.

---

## Example coverage

Include examples for:

1. simple inverter
2. hierarchical design
3. opaque native primitive
4. bench + sim pair
5. vector/bus usage
6. positional connection canonicalization

---

## Non-goals

Do **not** include in v0.1:

- inline behavioral equation language
- general scripting
- macros/preprocessor
- arbitrary embedded host-language code
- solver/runtime ABI details beyond binding metadata

---

## Acceptance criteria

This run is complete only if:

1. the full spec file exists,
2. the language is explicitly named Cirq,
3. the grammar and semantics are specific enough for later implementation,
4. canonicalization and validation rules are concrete,
5. the spec does not leave major semantic gaps.
