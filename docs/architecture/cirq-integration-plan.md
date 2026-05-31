# Cirq Integration Plan

> **Historical reference.** This doc captures the design intent from
> when Cirq was being introduced. The integration has landed; Cirq IR
> *is* the semantic center now. The current code matches the layout
> below.

## Workspace Inventory

### Existing Crates

| Crate | Responsibility | Cirq Relevance |
|-------|---------------|----------------|
| `thevenin-types` | SPICE netlist types (`Netlist`, `Item`, `Expr`, `Source`, etc.) and parser | **Internal compatibility layer.** Cirq IR is the canonical simulator input; the Netlist is the SPICE-shaped adapter the parser produces. |
| `thevenin` | Core simulation engine: MNA assembly, Newton solver, device models, analysis drivers (DC, AC, transient, noise, PZ, SENS, TF) | Consumes `cirq_ir::Circuit` directly via `thevenin::circuit::*`. The `&Netlist`-shaped wrappers remain for internal tests. |
| `thevenin-control` | `.control` block interpreter | Routes through `execute_control_block_ir(&Circuit)`. Fully IR-native on the input side: TEMPER eval, `@device[param]` alters, and analysis-command parsing all operate on `cirq_ir::Circuit` (only simulator *result* types come from `thevenin-types`). |
| `thevenin-xspice` | XSPICE code model framework | Orthogonal; Cirq's `xspice(...)` element binds to models in this registry. |
| `thevenin-test-macro` | Proc-macro for test harness | Unchanged. |
| `thevenin-cli` (root) | CLI binary | `thevenin run <file>` handles both `.cir` (SPICE) and `.cirq` (Cirq) input, routing through the IR pipeline in both cases. |

### Key Observation

The existing `thevenin-types::Netlist` is SPICE-shaped: it carries SPICE element names (R1, V1), SPICE-style `.model` cards, positional parameters, etc. Cirq's job is to provide a cleaner semantic model that doesn't inherit these historical quirks, while still being able to represent everything SPICE can.

---

## Proposed Cirq Crate Layout

```text
thevenin/                          # workspace root
├── cirq-grammar/                  # Tree-sitter grammar project (NOT a Rust crate)
│   ├── grammar.js
│   ├── corpus/                    # Tree-sitter test corpus
│   ├── queries/                   # Highlight/indent/fold queries
│   └── src/                       # Generated parser (C files)
│
├── cirq-ast/                      # Rust crate: Cirq AST types + Tree-sitter → AST lowering
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                 # AST type definitions
│       ├── lower.rs               # CST → AST lowering
│       └── span.rs                # Source span tracking
│
├── cirq-ir/                       # Rust crate: canonical Cirq IR
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                 # IR type definitions
│       ├── lower.rs               # AST → IR lowering (name resolution, type checking)
│       └── validate.rs            # IR validation passes
│
├── cirq-frontend/                 # Rust crate: pipeline orchestration
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                 # parse() → AST → IR pipeline
│       └── diagnostics.rs         # Error reporting
│
├── cirq-spice-import/             # Rust crate: SPICE → Cirq IR bridge
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       └── convert.rs             # thevenin_types::Netlist → cirq_ir::Circuit
│
├── thevenin-types/                # existing — stays as-is
├── thevenin/                      # existing — stays as-is
└── ...
```

---

## Pipeline Architecture

### Primary Path (Cirq Source)

```text
Cirq source (.cirq file)
  │
  ├─ Tree-sitter parser (cirq-grammar)
  │    produces: concrete syntax tree (CST)
  │
  ├─ CST → AST lowering (cirq-ast)
  │    produces: Cirq AST (source-faithful, with spans)
  │
  ├─ AST → IR lowering (cirq-ir)
  │    produces: Canonical Cirq IR (resolved, validated)
  │    - name resolution
  │    - subcircuit flattening
  │    - parameter evaluation
  │    - type checking
  │
  ├─ IR → Netlist adapter (in cirq-frontend or thevenin boundary)
  │    produces: thevenin_types::Netlist
  │
  └─ Simulation (thevenin)
       produces: SimResult
```

### Legacy Path (SPICE Import)

```text
SPICE source (.spice / .cir file)
  │
  ├─ SPICE parser (thevenin-types, existing)
  │    produces: thevenin_types::Netlist
  │
  ├─ Netlist → Cirq IR converter (cirq-spice-import)
  │    produces: Canonical Cirq IR
  │    - maps SPICE elements to Cirq IR nodes
  │    - preserves semantics, drops SPICE syntax quirks
  │
  └─ (same IR → Netlist → simulation path as above)
```

### Transitional Path (Direct SPICE, unchanged)

```text
SPICE source → thevenin-types parser → Netlist → thevenin::simulate()
```

This path continues to work exactly as today. No breakage.

---

## Boundary Definitions

### Cirq AST

The **source-oriented** representation. Preserves:
- exact source spans for every node
- original identifier names as written
- syntactic structure (blocks, nesting, ordering)
- comments and whitespace boundaries (via Tree-sitter CST)

Does NOT:
- resolve references
- evaluate parameters
- flatten subcircuits
- validate connectivity

### Canonical Cirq IR

The **semantic center** of the system. This is what all tools should converge on.

Properties:
- all names resolved to unique IDs
- parameters evaluated to concrete values (or symbolic with resolved bindings)
- subcircuits instantiated/flattened
- device models attached to their instances
- connectivity expressed as a node graph
- analysis commands normalized
- validated: no dangling references, no type mismatches

### Thevenin-Facing Execution Layer

The **adapter** that converts Cirq IR into `thevenin_types::Netlist` for simulation.

This layer exists because:
- the solver expects `Netlist` today
- we don't want to rewrite the solver's input interface in one shot
- it lets us validate Cirq IR correctness by round-tripping through the existing simulator

Long-term, the solver may consume Cirq IR directly, making this layer disappear.

### Runtime Input Boundary

Today: `thevenin::simulate(&Netlist) -> SimResult`

This function signature is the runtime's contract. The Cirq integration doesn't change it — it just provides a new way to produce the `Netlist` input.

---

## Migration Philosophy

1. **Incremental, not big-bang.** Every change should leave the workspace buildable and testable.
2. **Adapter layers are acceptable.** The IR → Netlist bridge is explicitly a transitional adapter.
3. **Old SPICE paths coexist.** The existing `thevenin-types` parser and `Netlist` type are not deprecated — they remain the primary path until Cirq is proven.
4. **New work converges on Cirq IR.** Any new frontend tooling (linting, formatting, IDE support) should target Cirq IR, not SPICE-shaped types.
5. **Test parity.** Every circuit that works through the SPICE path must also work through the Cirq path (once the Cirq frontend is complete). Test this via round-trip: SPICE → Cirq IR → Netlist → simulate, vs SPICE → Netlist → simulate, results must match.
