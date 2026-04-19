# Run 8 — Add Migration Tests, Adoption Strategy, and Cleanup Plan

## Objective

Turn the earlier runs into a practical migration path by adding:

- test coverage across the new Cirq-centered pipeline,
- a staged adoption plan,
- and a cleanup strategy for retiring old SPICE-shaped dependencies over time.

This run is intentionally about integration confidence rather than a giant code dump.

---

## Deliverables

Create or update:

- migration/adoption documentation
- end-to-end tests across the new pipeline
- snapshots / fixtures / semantic comparison helpers
- a cleanup checklist for old interfaces that can be retired gradually

Suggested docs:

- `docs/migration/cirq-adoption-plan.md`
- `docs/migration/old-path-retirement-checklist.md`

---

## Required adoption plan contents

The document must define staged adoption such as:

### Stage 1

Cirq parsing + canonical IR exist, but old paths still coexist.

### Stage 2

Canonical Cirq IR lowers into the backend-facing execution layer for selected flows.

### Stage 3

SPICE import also routes through canonical Cirq IR.

### Stage 4

Old SPICE-shaped interfaces begin to retire as coverage and confidence increase.

This does not need to prescribe exact release versions, but it must prescribe the order of adoption.

---

## Required test coverage

Add integration tests for:

1. Cirq source -> AST -> canonical IR
2. Cirq source -> canonical IR -> execution/elaboration layer
3. SPICE source -> canonical Cirq IR
4. optional SPICE -> Cirq emit -> Cirq parse -> Cirq IR
5. semantic equivalence checks at the canonical IR level where appropriate

If the workspace already has a testing convention, follow it.

---

## Required comparison strategy

Define one or both of:

- deterministic structural comparison on canonical Cirq IR
- snapshot-based approval tests for representative fixtures

The key requirement is that meaning is compared at the **IR level**, not only by emitted text.

---

## Required cleanup checklist

Create a checklist that can later guide retirement of old paths, with items like:

- identify old parser-shaped interfaces still in use
- identify SPICE-specific assumptions that now have Cirq IR replacements
- identify temporary adapters that should later collapse away
- identify duplicated concepts that should converge on Cirq IR semantics

Do not remove everything in this run.
Just make the retirement plan explicit and test-backed.

---

## Non-goals

Do **not** in this run:

- force all legacy paths to be deleted immediately
- rewrite the entire workspace structure

This run is about confidence, sequencing, and cleanup planning.

---

## Acceptance criteria

This run is complete only if:

1. there is a written staged adoption plan
2. there are integration tests spanning the new Cirq-centered flow
3. semantic comparison happens at the IR level
4. there is an explicit cleanup checklist for retiring old SPICE-shaped interfaces over time
5. the migration story is incremental and realistic
