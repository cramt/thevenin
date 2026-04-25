# Cirq + Thevenin Agent Plan Pack (v3)

This plan pack is for introducing **Cirq** as the new source language and canonical semantic model, while refactoring the existing **Thevenin** project to consume a cleaner IR instead of staying tied to SPICE-shaped baggage.

This version intentionally uses **smaller, more explicit runs** than the earlier plan.
Each numbered file is meant to be feasible as **one agent run**.

---

## Primary architectural direction

The intended long-term pipeline is:

```text
Cirq source
  -> Tree-sitter CST
  -> Cirq AST
  -> Canonical Cirq IR
  -> Thevenin-facing execution / elaboration layer
  -> existing simulation runtime
```

And for legacy compatibility:

```text
SPICE source
  -> SPICE import model
  -> Canonical Cirq IR
  -> Thevenin-facing execution / elaboration layer
  -> existing simulation runtime
```

The important point is that **Cirq IR becomes the semantic center**.
The solver/runtime already exists and is **not** the focus of this plan pack.

---

## Important constraints for every run

### 1. Do not assume Thevenin internals beyond what is directly observable

The coding agent should inspect the existing Rust workspace and adapt to it.
Do **not** assume crate names, module boundaries, or current internal architecture beyond the fact that Thevenin already exists as a Rust workspace.

### 2. Do not re-plan the solver/runtime

The solver/runtime/model execution side is already present.
The work here is about:

- language/frontend design,
- canonical IR design,
- frontend-to-IR lowering,
- integration boundaries,
- SPICE compatibility,
- migration strategy.

### 3. Use standard Tree-sitter as the default grammar path

Use a conventional Tree-sitter grammar project with:

- `grammar.js`
- corpus tests
- query files
- generated parser artifacts
- Rust consuming the generated parser

Do **not** use `rust-sitter` as the default path unless a future human explicitly changes that decision.

### 4. Keep the IR split explicit

Use at least these conceptual layers:

- **Cirq AST** — source-oriented
- **Canonical Cirq IR** — language/tooling-facing semantic representation
- **Thevenin-facing execution/elaboration layer** — backend-facing normalized representation

### 5. Prefer incremental migration

Do not require a big-bang rewrite.
Each run should leave the workspace in a state where progress can be validated independently.

---

## Suggested run order

1. `01-workspace-inventory-and-target-architecture.md` ✅
2. `02-cirq-language-spec.md` ✅
3. `03-tree-sitter-cirq-grammar.md` ✅
4. `04-cirq-ast-and-parser-integration.md` ✅
5. `05-canonical-cirq-ir.md` ✅
6. `06-cirq-to-thevenin-boundary.md` ✅
7. `07-spice-import-to-cirq-ir.md` ✅
8. `08-tests-migration-and-adoption.md` ✅
9. `09-feature-parity-gaps.md` ✅

---

## What success looks like

By the end of runs 01–08, the project has:

- a full Cirq language specification,
- a standard Tree-sitter grammar for Cirq,
- Rust parsing/lowering into Cirq AST,
- a canonical Cirq IR,
- a defined boundary between Cirq IR and the Thevenin backend-facing layer,
- a SPICE import path into Cirq IR,
- and a migration/testing strategy that lets Thevenin gradually stop depending on SPICE-shaped structures.

Run 09 extends this with full SPICE feature parity — every construct that
`thevenin-types::Netlist` can represent must also be representable in Cirq IR,
so the Cirq path never silently drops information the simulator needs.

---

## Explicit non-goals

Do **not** drift into these unless later requested:

- redesigning the numerical solver,
- redesigning matrix assembly/convergence logic,
- implementing a new simulator runtime from scratch,
- inventing a full behavioral modeling language in v0.1,
- building a full IDE/LSP stack,
- macro/preprocessor systems,
- arbitrary embedded scripting.
