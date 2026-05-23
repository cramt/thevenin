# Run 4 — Integrate the Parser into Rust and Define Cirq AST

## Objective

Wire the generated Cirq Tree-sitter parser into the existing Rust workspace and build the **Cirq AST layer plus CST → AST lowering**.

This run should stop at AST.
Do not define the full canonical IR yet.

---

## Deliverables

Create or update Rust code so the workspace can:

1. parse Cirq source into a Tree-sitter CST
2. lower CST into a typed Cirq AST
3. report syntax/lowering diagnostics
4. expose basic parse/dump entry points

---

## Workspace integration rule

The agent must integrate with the **existing Rust workspace** instead of assuming a blank repository.

If new crates are added, place them according to the architecture document from Run 1.
If existing crates are a better fit, use them.

---

## Recommended responsibilities

At minimum, introduce clear homes for:

- source file/span handling
- diagnostics
- Cirq AST node definitions
- Tree-sitter parser wrapper
- CST → AST lowering logic
- optional CLI entry points

Do not spread raw Tree-sitter node handling across the entire codebase.

---

## AST requirements

The Cirq AST must be source-oriented but cleaner than the CST.

Required families include:

- source file root
- top-level declarations
- ports
- params
- lets
- nets
- instances
- native impls
- analyses/options/save/measure constructs
- expressions
- qualified names
- literals

Every meaningful AST node should carry source spans.

---

## Lowering requirements

Implement a dedicated CST → AST lowering layer.

Rules:

- validate the presence/shape of required CST children
- emit diagnostics rather than panicking
- preserve spans
- continue on sibling nodes where practical when recovery is possible

---

## CLI/debug requirements

Add minimal parse/debug functionality such as:

- parse a Cirq file
- dump CST
- dump AST

If the workspace already has CLI patterns, follow them.

---

## Tests

Write tests for:

- identifier lowering
- expression precedence AST shape
- instance lowering
- native impl lowering
- bench/sim lowering
- malformed input diagnostics

Use snapshots for AST and diagnostics if that fits the workspace’s testing style.

---

## Non-goals

Do **not** in this run:

- define canonical Cirq IR
- implement semantic resolution
- implement SPICE import
- refactor backend execution code

---

## Acceptance criteria

This run is complete only if:

1. Cirq source can be parsed in Rust
2. CST can be lowered to AST
3. diagnostics are emitted with spans
4. there is a clear boundary around raw Tree-sitter usage
5. tests cover both valid and invalid inputs
