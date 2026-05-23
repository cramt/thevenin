# Run 6 — Define and Implement the Cirq IR → Thevenin Boundary

## Objective

Define the **backend-facing boundary** between canonical Cirq IR and the existing Thevenin execution side.

This run should not assume hidden details about current backend internals.
Instead, it should work by introducing a clearly named and well-documented **execution/elaboration layer** that Thevenin can consume.

---

## Core idea

This run is about the boundary:

```text
Canonical Cirq IR -> execution/elaboration layer -> existing runtime
```

The main goal is to prevent solver/backend concerns from polluting canonical Cirq IR.

---

## Deliverables

Create:

1. a written boundary document
2. execution/elaboration data structures or traits
3. lowering code from canonical Cirq IR into that boundary representation
4. at least one minimal adapter path into the existing backend-facing code

---

## Required document

Create something like:

- `docs/architecture/cirq-to-thevenin-boundary.md`

This document must explain:

- what belongs in canonical Cirq IR
- what belongs only in the backend-facing execution layer
- what information must be resolved during lowering
- what should *not* leak back into Cirq IR

---

## What the execution/elaboration layer should contain

Without assuming current internals, define a layer oriented around concepts like:

- resolved instance targets
- explicit node identities / connection tables
- resolved/defaulted parameter values
- selected native implementation bindings
- normalized analysis requests
- other data needed to drive existing execution code

The exact type layout should fit the workspace, but the boundary should be explicit.

---

## Required design rules

1. canonical Cirq IR stays backend-neutral
2. backend-facing execution data may be more explicit and elaborated
3. source-format details do not cross this boundary
4. backend-specific assumptions should be isolated to this layer or below
5. the lowering should be testable independently of runtime execution

---

## Required implementation steps

1. define the boundary document
2. define execution/elaboration structs or traits
3. implement canonical Cirq IR → execution-layer lowering
4. add a minimal integration point into the existing backend path
5. add tests for the lowering boundary

---

## Non-goals

Do **not** in this run:

- redesign solver algorithms
- rewrite all backend internals
- remove every old path immediately

This run is about establishing the new **clean seam**.

---

## Acceptance criteria

This run is complete only if:

1. the Cirq IR → Thevenin boundary is documented explicitly
2. the execution/elaboration layer exists in code
3. canonical Cirq IR can lower into it
4. at least one backend-facing integration path exists
5. tests validate the boundary independently of full simulation behavior
