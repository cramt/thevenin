# API stability statement

This document defines what the thevenin / Cirq workspace promises to keep
stable across the 1.x release line, and how breaking changes are handled.

## TL;DR

| Crate | 1.x stability bar | Notes |
|---|---|---|
| `thevenin` | **Stable** | The `circuit::simulate*` entry points, `cirq_ir::Circuit` argument shape, `SimResult` output shape, and the public error type `MnaError` will not break in 1.x. |
| `cirq-ir` | **Stable** | The `Circuit`, `Element`, `Analysis`, `DeviceType`, `Value`, and `MeasureExpr` types and their public fields. Adding variants to public enums is a breaking change (see below). |
| `cirq-frontend` | **Stable** | The Cirq source → IR pipeline (`compile`, `ir_lower`). |
| `cirq-spice-import` | **Stable** | The `import_spice` entry point and `IncludeOptions`. |
| `thevenin-control` | **Stable** | The `.control` block interpreter contract. |
| `thevenin-xspice` | **Stable** | The `CodeModelDef` / `CodeModelBuilder` / `CodeModelRegistry` host-API. |
| `cirq-ast` | **Stable** | Source-faithful AST surface (parser output). |
| `cirq-grammar` | **Stable** | Tree-sitter grammar artifacts. |
| `thevenin-types` | **Unstable / internal** | Marked `pub(crate)` everywhere it's reachable; still published at 0.x for `release-plz` lockstep but treated as an internal adapter. Direct callers should migrate to `cirq_ir::Circuit`. |
| `thevenin-test-macro` | **Not published** | Workspace-internal proc-macro for the regression harness. |

## What counts as a breaking change

The workspace follows [SemVer](https://semver.org/spec/v2.0.0.html) with
the standard Rust API-evolution caveats:

### Hard-breaking (requires a 2.0)

- Removing or renaming a public type, function, method, field, or enum
  variant from a stable crate.
- Changing the signature of a public function or method (parameter types,
  return type, generic bounds, `async`-ness).
- Changing the layout of a `#[repr(C)]` type.
- Changing the FFI signature of an exported symbol.
- Tightening trait bounds on existing public methods.

### Soft-breaking (allowed in 1.x with a `#[deprecated]` cycle)

- Renaming a public item, leaving the old name as a `#[deprecated]`
  re-export.
- Marking a method as deprecated in favour of a new one with a different
  signature.
- Tagging an existing variant as deprecated.

### Non-breaking additions (any 1.x release)

- Adding new public items (types, functions, methods, modules, traits).
- Adding new variants to **non-exhaustive** enums (`#[non_exhaustive]`).
- Adding new fields to **non-exhaustive** structs (`#[non_exhaustive]`).
- Adding new methods to a sealed trait.
- Loosening trait bounds on existing public methods.

## `#[non_exhaustive]` policy

The following enums are `#[non_exhaustive]` and may grow new variants in
any 1.x release:

- `cirq_ir::Analysis` — new analysis kinds (e.g. `.disto`) will land here
  without a major bump.
- `cirq_ir::Element` and `cirq_ir::ElementKind` — new device variants
  (e.g. URC, HICUM L2) will land here without a major bump.
- `cirq_ir::DeviceType` — new model kinds.
- `cirq_ir::Value` — new typed-value variants.
- `cirq_ir::Waveform` — new waveform shapes.
- `cirq_ir::MeasureExpr` — new `.meas` clauses.
- `thevenin::MnaError` — new error categories.
- `cirq_spice_import::ImportError` — new failure modes.

Match the rest of an `#[non_exhaustive]` enum with `_ => …` so your code
keeps compiling when we add variants.

## Devices, analyses, options: forward-compatibility

- Adding a new device model (new MOSFET LEVEL, new BJT model, new
  transmission-line variant) is **non-breaking** as long as we extend
  `DeviceType` / `ElementKind` (both `#[non_exhaustive]`).
- Adding a new `.options` key is **non-breaking** — unknown options are
  warn-and-skip on import.
- Adding a new analysis (e.g. `.disto`) is **non-breaking** if
  `Analysis::Disto` is added to the `#[non_exhaustive]` enum.

## Numerical results

We do **not** promise bit-for-bit reproducibility across 1.x releases.
Numerical results may shift within reasonable tolerance when:

- The NR solver changes its damping, voltage-limiting, or convergence
  heuristics.
- A device model gets bug fixes that move the operating point.
- Sparse-LU pivoting heuristics change.

What we *do* promise: results stay within the `RELTOL`/`ABSTOL`/`VNTOL`
budgets set in `.options`, the ngspice regression corpus continues to
pass at the documented tolerances, and any model-level behaviour change
appears in the CHANGELOG with a note pointing at the affected fixtures.

## Deprecation timeline

- Deprecated APIs ship a `#[deprecated(since = "1.x.y", note = "...")]`
  attribute and live for **at least one minor release** before the next
  major bump can remove them.
- The deprecation note names the replacement API.
- We try to land deprecations together with their replacements so users
  always have a clean migration path.

## MSRV

The minimum supported Rust version is the latest stable release at the
time of each thevenin release. We do not pin to an older MSRV because the
project tracks `rustc` features actively (e.g. let-chains, pattern
ergonomics). The current MSRV is declared in each crate's `Cargo.toml`
via `rust-version`.

## What's explicitly excluded from stability

- `thevenin-types` — the legacy SPICE Netlist representation. Treated as
  an internal adapter; the recommended public path is
  `cirq_spice_import::import_spice` → `cirq_ir::Circuit` →
  `thevenin::circuit::simulate`.
- `thevenin-test-macro` — workspace-internal, never published.
- Any module marked `pub(crate)` or living under a `pub(crate) mod`.
- Internal helper functions inside otherwise-public modules — anything
  not re-exported from the crate root is fair game to refactor.

## Security model: `.control` blocks are not sandboxed

Running a netlist is **not** a side-effect-free operation. Both the CLI
(automatically, when a deck contains a `.control` block) and library callers
of `thevenin_control::execute_control_block_ir` will execute the deck's
control commands, which include filesystem sinks and sources:

- `write` / `wrdata` / `print > file` create or truncate arbitrary paths.
- `source` reads arbitrary paths.

Paths honor `.control` variable interpolation and are **not** confined to any
base directory — an absolute path or `..` traversal is resolved as written.
This mirrors stock ngspice behavior (though thevenin exposes no `shell`/`system`
command, so there is no arbitrary code execution). There is no path jail today.

**Treat netlists from untrusted sources as you would a shell script.** Do not
call `execute_control_block_ir` on attacker-supplied decks in a context where
ambient filesystem access is unacceptable (multi-tenant services, wasm hosts
that assume "simulate" is pure) without your own sandboxing. An opt-in path
jail is tracked as a post-1.0 hardening item.

## Reporting compatibility breakage

If a 1.x release breaks your code in a way that violates this document,
file an issue at <https://github.com/cramt/thevenin/issues> with the
minimal failing example. We'll either fix it as a regression or label
the change as an intentional break and bump the version accordingly.
