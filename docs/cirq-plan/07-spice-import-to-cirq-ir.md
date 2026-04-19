# Run 7 — Implement SPICE Import into Canonical Cirq IR

## Objective

Implement a practical **SPICE import path** that maps a defined subset of SPICE into **canonical Cirq IR**.

This is the legacy compatibility bridge.

---

## Architectural rule

The import path must be:

```text
SPICE source -> SPICE import model -> canonical Cirq IR
```

Do **not** implement SPICE compatibility as direct text rewriting into Cirq source only.
If a Cirq source emitter exists, it should be downstream of canonical IR, not the primary semantic path.

---

## Deliverables

Create:

- a documented supported SPICE subset
- a SPICE import model / parser representation
- lowering into canonical Cirq IR
- optional Cirq DSL emission for debugging/review

---

## Supported subset for v0.1

Support these first:

### Directives

- `.subckt`
- `.ends`
- `.param`
- `.include`
- `.lib` only where safe, otherwise warn

### Elements

- resistor `R`
- capacitor `C`
- inductor `L`
- voltage source `V` (basic forms)
- current source `I` (basic forms)
- MOSFET `M`
- subckt instance `X`

### Comments / numerics

- common leading comment forms
- common SPICE numeric formats and suffixes

Unsupported constructs should emit diagnostics/warnings.

---

## Required mapping behavior

### `.subckt`

Must map to Cirq `subckt` semantics.

### `X...` instances

Must lower to named connections using the target subckt port list.

### Primitive/passive devices

Must map into Cirq’s canonical instance model with named pins/params.

### Ground mapping

Define one consistent mapping for node `0`, preferably to semantic `gnd`.

### `.param`

Map to Cirq parameter semantics where safe.

---

## Tests

Create fixtures for at least:

1. passive network
2. MOS inverter subckt
3. hierarchical subckt instance
4. param usage
5. unsupported construct with diagnostic

The central invariant should be:

```text
SPICE input -> Cirq IR
(optional Cirq emit -> Cirq parse -> Cirq IR)
semantic equivalence at the canonical IR level
```

---

## Non-goals

Do **not** in this run:

- chase every dialect edge case
- implement full `.control` compatibility
- attempt total SPICE parity

Prefer a correct subset over fake breadth.

---

## Acceptance criteria

This run is complete only if:

1. there is a documented subset
2. SPICE input can lower into canonical Cirq IR
3. representative fixtures work
4. unsupported constructs fail/warn deliberately
5. canonical IR is used as the semantic comparison point
