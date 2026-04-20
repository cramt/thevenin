# Cirq IR to Thevenin Netlist Boundary

## Overview

The boundary adapter (`cirq-frontend/src/to_netlist.rs`) converts `cirq_ir::Circuit`
into one or more `thevenin_types::Netlist` values. This is the handoff point between
the Cirq language frontend and the Thevenin simulation engine.

```
Cirq source  -->  Tree-sitter CST  -->  Cirq AST  -->  Cirq IR  -->  Thevenin Netlist  -->  Simulator
                                                         ^                ^
                                                    cirq-ir crate    to_netlist.rs
```

## What belongs in Cirq IR

- Resolved names (nets, elements, models, parameters)
- Evaluated constant expressions (all `Value` nodes are concrete)
- Flattened subcircuits / modules
- Validated connections (correct terminal count per element kind)
- Semantic analysis diagnostics

The IR is a **language-level** representation. It uses domain types like `Id`,
`ElementKind`, `Connection`, and `DeviceType` that correspond directly to Cirq
syntax concepts.

## What belongs only in the execution layer (Netlist)

- SPICE naming conventions (prefix letters: R, C, L, V, I, D, Q, M, J, K, E, F, G, H, O)
- Ground node represented as the string `"0"` (Cirq IR uses `Id(0)` for `gnd`)
- `Source` structs with DC/AC/waveform decomposition
- Model kind strings (`"NPN"`, `"NMOS"`, `"D"`, etc.)
- Analysis parameters in SPICE format (`.dc src start stop step`, `.ac DEC n fstart fstop`)
- `.global` directives
- `.param` directives

## Conversion rules

### Net names

| Cirq IR | Netlist |
|---------|---------|
| `Id(0)` / `gnd` | `"0"` |
| `Net { name, .. }` | The net name string directly |

### Element naming

SPICE elements must start with the correct type letter. The adapter prepends the
letter if the Cirq name does not already start with it:

- `r1` (resistor) -> `Rr1` (since `r` does not match `R` case-insensitively... actually it does, so stays `r1`)
- `myres` (resistor) -> `Rmyres`
- `V1` (voltage source) -> `V1` (already correct)

### Element kind mapping

| Cirq IR `ElementKind` | SPICE prefix | `thevenin_types::ElementKind` |
|------------------------|--------------|-------------------------------|
| `Resistor` | R | `Resistor` |
| `Capacitor` | C | `Capacitor` |
| `Inductor` | L | `Inductor` |
| `VoltageSource` | V | `VoltageSource` |
| `CurrentSource` | I | `CurrentSource` |
| `Diode` | D | `Diode` |
| `Npn` / `Pnp` | Q | `Bjt` |
| `Nmos` / `Pmos` | M | `Mosfet` |
| `NJfet` / `PJfet` | J | `Jfet` |
| `Vcvs` | E | `Vcvs` |
| `Vccs` | G | `Vccs` |
| `Ccvs` | H | `Ccvs` |
| `Cccs` | F | `Cccs` |
| `TransmissionLine` | O | `Ltra` |
| `Coupling` | K | `MutualCoupling` |

### Model kind mapping

| `DeviceType` | SPICE model kind string |
|--------------|------------------------|
| `Diode` | `"D"` |
| `Npn` | `"NPN"` |
| `Pnp` | `"PNP"` |
| `Nmos` | `"NMOS"` |
| `Pmos` | `"PMOS"` |
| `NJfet` | `"NJF"` |
| `PJfet` | `"PJF"` |
| `NMesfet` | `"NMF"` |
| `PMesfet` | `"PMF"` |

### Analysis mapping

| Cirq IR | Netlist |
|---------|---------|
| `Op` | `Op` |
| `Dc { sweeps }` | `Dc { src, start, stop, step, src2 }` |
| `Ac { start, stop, points, scale }` | `Ac { variation, n, fstart, fstop }` |
| `Tran { step, stop, start, uic }` | `Tran { tstep, tstop, tstart, tmax }` |
| `Noise { ... }` | `Noise { output, ref_node, src, variation, n, fstart, fstop }` |
| `Pz { ... }` | `Pz { node_i, node_g, node_j, node_k, input_type, analysis_type }` |
| `Sens { output }` | `Sens { output }` |
| `Tf { output, source }` | `Tf { output, input }` |

Multiple analyses produce multiple netlists with shared circuit items.
No analyses defaults to a single `.op` netlist.

## What the adapter does NOT handle

- Subcircuit expansion (must be done during IR lowering)
- Waveform construction (PULSE, SIN, etc.) -- not yet represented in Cirq IR
- `.include` / `.lib` directives
- SPICE `.options`
- Behavioral sources (B elements)
- XSPICE code model instances
- Initial condition specifications beyond UIC flag
