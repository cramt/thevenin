# Run 1 — Inspect the Existing Workspace and Define the Target Architecture

## Objective

Before adding new language/frontend pieces, inspect the current Rust workspace and write down the **target architecture and migration map** for introducing Cirq.

This run is deliberately small and explicit.
It should not make large structural code changes yet.

---

## Purpose

The goal of this run is to prevent the rest of the work from making bad assumptions.

The agent should:

1. inspect the current workspace layout,
2. identify which crates are already relevant to parsing, netlist representation, model execution, or simulation configuration,
3. propose where the new Cirq-related pieces should live,
4. define the high-level boundaries between:
   - source parsing,
   - canonical Cirq IR,
   - backend-facing elaboration,
   - existing runtime execution.

---

## Hard rule

Do **not** assume internal details that are not visible in the repository.

The output of this run must be based on what is actually present in the workspace.

---

## Deliverables

Create a document such as:

- `docs/architecture/cirq-integration-plan.md`

and optionally a shorter summary such as:

- `docs/architecture/cirq-crate-map.md`

---

## What the document must contain

### 1. Workspace inventory

List:

- current workspace crates,
- their apparent responsibilities,
- which ones should remain untouched for now,
- which ones seem likely integration points for Cirq.

Do not guess at semantics beyond what code/docstrings/module names make reasonably clear.

### 2. Proposed Cirq-related additions

Propose where the following concepts should live:

- Cirq spec/docs
- Tree-sitter grammar project
- Cirq AST types
- canonical Cirq IR
- frontend/lowering code
- SPICE import layer
- Thevenin-facing execution/elaboration boundary
- CLI/test entry points

The proposal may use new crates or extend existing crates.
The key requirement is that the plan is explicit and compatible with the existing workspace.

### 3. Layer diagram

Write a concrete layer diagram like:

```text
Cirq source -> CST -> AST -> canonical IR -> execution/elaboration layer -> runtime
SPICE source -> import model -> canonical IR -> execution/elaboration layer -> runtime
```

### 4. Migration philosophy

Define the migration approach explicitly:

- incremental, not big-bang,
- adapter layers acceptable,
- old SPICE-shaped paths may coexist temporarily,
- new work should converge on Cirq IR as the center.

### 5. Boundary definitions

Write short definitions for:

- **Cirq AST**
- **Canonical Cirq IR**
- **Thevenin-facing execution/elaboration layer**
- **Runtime input boundary**

These do not need full type definitions yet, but they must be clear enough to guide later runs.

---

## Suggested implementation steps

1. inspect the workspace root and current crates
2. inspect docs/README files if present
3. identify likely extension points
4. write the architecture document
5. if useful, add a simple crate-placement map

---

## Non-goals

Do **not** in this run:

- write the Cirq grammar
- define full AST/IR structs
- implement parser code
- implement SPICE import
- refactor runtime internals

This run is about **ground truth and placement**.

---

## Acceptance criteria

This run is complete only if:

1. there is a written architecture document,
2. it is based on the actual workspace contents,
3. it identifies where Cirq-related work should land,
4. it clearly separates canonical Cirq IR from backend-facing execution structures,
5. it explicitly recommends incremental migration instead of a big-bang rewrite.
