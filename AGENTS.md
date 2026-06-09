# Thevenin

A Rust rewrite of [ngspice](https://ngspice.sourceforge.io/), the open-source
mixed-level/mixed-signal electronic circuit simulator. It also introduces
**Cirq**, a native source language that compiles to the same simulator IR.

## Project Structure

A Cargo workspace of focused crates plus a thin CLI:

- `thevenin/` - simulation engine (MNA assembly, NR solver, device stamps, analyses)
- `thevenin-control/`, `thevenin-xspice/`, `thevenin-types/` - `.control` interpreter, XSPICE framework, legacy SPICE netlist types
- `cirq-ir/`, `cirq-ast/`, `cirq-frontend/` - canonical Cirq IR, source AST, and the source → AST → IR pipeline
- `cirq-grammar/`, `cirq-control-grammar/` - tree-sitter grammars (vendored `parser.c`/`scanner.c`, built via `cc` — no node needed)
- `cirq-spice-import/` - SPICE netlist → Cirq IR importer
- `src/` - the `thevenin-cli` binary
- `docs/` - language spec, device matrix, architecture, release docs (see below)
- `examples/` - runnable Rust and `.cirq` examples
- `ngspice-upstream/` - cloned ngspice C source (reference implementation, gitignored, do not modify)
- `scripts/` - test-harness triage + benchmark helpers

See `README.md` for the full crate-by-crate map.

## Development

**Always run commands through `nix develop --command ...`** so that flake.nix stays honest
and no dependency works by accident from the host environment.

```bash
nix develop --command cargo build
nix develop --command cargo nextest run
nix develop --command cargo clippy --workspace -- -D warnings
nix develop --command cargo fmt --check
nix develop --command cargo test --workspace --target wasm32-unknown-unknown   # wasm build
```

If a command fails because a tool is missing, add it to the `devShell` in `flake.nix`
rather than installing it on the host.

## Test harness

`scripts/triage-ignored-tests.ts` triages the currently-ignored ngspice harness
fixtures: it runs the harness, parses the `TRIAGE_JSON:` lines emitted by
`thevenin/tests/harness.rs`, and writes a categorized `triage-report.json`.

```bash
pnpm install        # one-time, installs tsx + zod
pnpm triage         # regenerate triage-report.json
```

### Test fixtures
`ngspice-upstream/` is gitignored, so any test fixture files sourced from it (e.g. `.cir`,
`.spice`, model files) **must be copied** into a tracked fixtures directory (e.g.
`tests/fixtures/` or `<crate>/tests/fixtures/`) before they are referenced by tests. This
ensures tests work on a clean clone without needing the upstream repo checked out.

## Project Goals

### Test-driven development
Write tests first, implement second. Port test cases from ngspice wherever possible — the
`ngspice-upstream/tests/` directory and example circuits in `ngspice-upstream/examples/` are
gold. If ngspice has a test for a behavior, steal it. The ngspice source is the authoritative
reference for correctness.

### Correctness over performance
Get it right first. Optimize later, guided by profiling, not intuition. A slow correct
simulator is infinitely more useful than a fast wrong one. Do not reach for `unsafe`,
hand-rolled SIMD, or clever bit tricks until profiling proves they're needed.

### Make impossible states irrepresentable
Encode invariants in the type system. Use enums over stringly-typed fields, newtypes over
bare primitives, and builder patterns or typestate where construction has constraints. If a
function can't fail for a given input type, the type should make that obvious. Prefer
compile-time guarantees over runtime checks.

### Prefer facet + unsynn over serde + syn
Use `facet` for reflection/serialization and `unsynn` for any proc-macro or parsing work.
Do not pull in `serde` or `syn` unless there is a hard dependency that requires them.

### Keep crates focused
The workspace is already split along natural boundaries (parser, IR, device models, solver,
control, output). Keep it that way: when a crate grows a second responsibility, extract it.
Small focused crates compile faster, test in isolation, and enforce API boundaries.

## Conventions

- Follow the original ngspice architecture where it makes sense, but use idiomatic Rust
- Use `thiserror` for error types
- Use `faer` for linear algebra (sparse + dense LU); the MNA solve is the hot path
- Prefer safe Rust; use `unsafe` only when absolutely necessary for performance-critical paths
- The ngspice source in `ngspice-upstream/` is the authoritative reference for behavior
- Public API changes must respect `docs/api-stability.md` (the workspace is approaching a stable 1.0)
- Never include Co-Authored-By lines in git commits

## Reference docs

- `docs/getting-started.md` - build + first simulation, CLI and library
- `docs/cirq-spec/` - the Cirq language reference
- `docs/devices.md` - device-coverage matrix and known gaps
- `docs/api-stability.md` - the stable 1.x surface and breaking-change policy
- `docs/architecture/` - crate map, Cirq → Thevenin boundary, raw-file format, language registry
- `docs/1.0-checklist.md` - release readiness against the three 1.0 goals
