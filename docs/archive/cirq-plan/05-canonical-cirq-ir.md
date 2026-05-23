# Run 5 — Define the Canonical Cirq IR and AST → IR Lowering

## Objective

Define the **canonical Cirq IR** and implement lowering from Cirq AST into that IR.

This run is where Cirq becomes the main semantic representation.

---

## Deliverables

Create:

- canonical Cirq IR types
- AST → IR lowering code
- semantic name resolution for language-level constructs
- canonicalization rules enforced in code
- deterministic IR dump/debug support

---

## Design goal

The canonical Cirq IR must represent what a Cirq design **means** in a language/tooling sense.

It should be:

- deterministic
- serializable
- source-format independent
- backend-neutral
- explicit where source syntax is implicit

---

## Required IR concerns

The canonical Cirq IR should model at least:

- imports
- globals
- primitives
- subckts
- benches
- sims
- ports
- params
- nets
- instances
- native implementation bindings
- analysis/save/measure/options data

---

## Required canonicalization

Implement or define code for:

### 1. Positional → named connections

Canonical IR stores named connections only.

### 2. Implicit nets → explicit semantic net symbols

All net references in IR must resolve to explicit net identities.

### 3. Defaults / overrides / lets

Define how parameter defaults and derived values appear in the IR.
Handle cycles as errors.

### 4. Literal/unit normalization

Normalize numeric values into a canonical internal form.

### 5. Qualified/native binding normalization

Represent `impl native` bindings structurally and consistently.

---

## Required semantic checks in this run

At minimum:

- duplicate declarations
- unknown targets
- unknown named ports
- wrong positional arity
- duplicate port binding
- unknown parameter overrides
- invalid native impl declarations
- unavailable backend impls when checked in this phase
- cyclic param/let dependencies

---

## Required outputs for debugging/tests

Add deterministic dumps of the canonical IR.
JSON is a good option if it fits the workspace.

The goal is to make semantic comparisons easy in later runs.

---

## Tests

Write tests for:

- declaration indexing
- name resolution
- positional connection lowering
- implicit net creation/resolution
- duplicate detection
- param/let cycle detection
- stable IR dumping

---

## Non-goals

Do **not** in this run:

- define the Thevenin-facing execution/elaboration boundary in full detail
- import SPICE yet
- refactor runtime code yet

This run is about the **canonical language/tooling IR**.

---

## Acceptance criteria

This run is complete only if:

1. canonical Cirq IR types exist
2. AST → IR lowering is implemented
3. canonicalization rules are enforced
4. semantic checks work for major language errors
5. IR can be dumped deterministically for tests and comparison
