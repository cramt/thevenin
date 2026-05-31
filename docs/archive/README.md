# Archived Docs

Historical planning and migration docs preserved here for reference. They
describe work that has landed; see `docs/1.0-checklist.md` for active 1.0
scope.

## Contents

- `cirq-plan/` — the original Cirq IR design documents covering the language
  spec, grammar, AST/parser, canonical IR, the Cirq↔Thevenin boundary, SPICE
  import, tests-migration plan, and feature-parity gaps.
- `migration/` — the Stage-by-Stage migration to a Circuit-input simulator:
  the Cirq adoption plan, MNA-on-IR pivot plan, harness status, and the
  old-path retirement checklist.
- `prd-simulation-engine.md` — the original founding PRD (pre-Cirq), kept for
  the original design intent. Superseded by `docs/1.0-checklist.md`.

Internal cross-links inside these directories use the old (pre-archive) paths;
they have not been rewritten. Treat the contents as a snapshot of the work
as it landed.
