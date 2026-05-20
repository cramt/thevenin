# thevenin

A SPICE circuit simulator written in Rust, targeting both native and WebAssembly.

Thevenin is a from-scratch rewrite of [ngspice](https://ngspice.sourceforge.io/) in idiomatic Rust. It parses standard SPICE netlists and runs simulations with the same algorithms — Modified Nodal Analysis, Newton-Raphson iteration, sparse direct solvers — but with Rust's type safety and `wasm32` support.

It also introduces **Cirq**, a new source language that targets the same simulator. SPICE input is first imported into Cirq's intermediate representation; the IR is now the canonical input to the simulator's analyses.

## Workspace layout

| Crate                  | Description                                                                                  |
|------------------------|----------------------------------------------------------------------------------------------|
| `thevenin`             | Simulation engine — MNA assembly, NR solver, analysis drivers, device stamps.                |
| `thevenin-types`       | Legacy SPICE Netlist representation. Internal adapter between the parser and the simulator. |
| `thevenin-cirq`        | Cirq-source convenience entry points (parse + simulate from raw SPICE source).               |
| `thevenin-control`     | `.control` block interpreter (run/let/print/alter/resume/quit).                              |
| `thevenin-xspice`      | XSPICE code-model framework.                                                                 |
| `cirq-ast`             | Source-faithful AST for the Cirq language with span information.                             |
| `cirq-grammar`         | Tree-sitter grammar for Cirq with highlight / fold / locals queries.                         |
| `cirq-ir`              | Canonical Cirq IR (name-resolved, parameter-evaluated, model-linked).                        |
| `cirq-frontend`        | Cirq source → AST → IR pipeline, plus the IR → Netlist adapter.                              |
| `cirq-spice-import`    | SPICE source / `thevenin-types::Netlist` → Cirq IR.                                          |

## Supported analyses

- **DC operating point** (`.op`)
- **DC sweep** (`.dc`)
- **AC small-signal** (`.ac`)
- **Transient** (`.tran`)
- **Noise** (`.noise`)
- **Sensitivity** (`.sens`)
- **Transfer function** (`.tf`)
- **Pole-zero** (`.pz`)

Every analysis above takes a `cirq_ir::Circuit` directly; `thevenin-types::Netlist` is an internal compatibility layer for the SPICE parser.

## Device models

- Resistors, capacitors, inductors, mutual coupling
- Independent voltage/current sources (DC, AC, pulse, sin, PWL, exp, AM, SFFM)
- Dependent sources (VCVS, VCCS, CCVS, CCCS)
- Behavioural sources (B element, `V=` / `I=` expressions)
- Transmission lines (LTRA / TXL / CPL)
- Diodes
- BJTs (Gummel-Poon level 1, VBIC level 4)
- MOSFETs (levels 1-3)
- JFETs / MESFETs / HFETs
- BSIM3v3, BSIM4
- BSIM3SOI (FD, PD, DD)
- XSPICE code models (A element)

## Quick start

```rust
use cirq_spice_import::import_spice;
use thevenin::circuit::simulate;
use thevenin_types::VectorData;

let source = "Voltage divider
V1 in 0 1.0
R1 in mid 1k
R2 mid 0 2k
.op
.end
";
let circuits = import_spice(source).unwrap();
let result = simulate(&circuits[0]).unwrap();

for plot in &result.plots {
    for vec in &plot.vecs {
        if let VectorData::Real(values) = &vec.data
            && let Some(&v) = values.first()
        {
            println!("{}: {:.6}", vec.name, v);
        }
    }
}
```

For Cirq source input, use `cirq_frontend::compile` instead of `import_spice` to get a `cirq_ir::Circuit` directly.

## CLI

```bash
thevenin run circuit.cir     # SPICE netlist
thevenin run circuit.cirq    # Cirq source
```

## Building

All commands run inside the Nix dev shell to keep `flake.nix` honest:

```bash
nix develop --command cargo build
nix develop --command cargo nextest run
nix develop --command cargo nextest run -p thevenin --test harness  # ngspice regression corpus
nix develop --command cargo clippy --workspace -- -D warnings
nix develop --command cargo fmt --check

# WebAssembly
nix develop --command cargo test --workspace --target wasm32-unknown-unknown
```

## Test coverage

The regression harness runs every test fixture from `ngspice-upstream/tests/` through SPICE → Cirq IR → simulate and diffs against the ngspice reference output. Current state: 100 passing, 7 skipped. See `thevenin/tests/ignore.toml` for the skip reasons and `docs/future-work.md` for the diagnosis of each.

## Status

The project is pre-1.0. The simulator core and the Cirq IR pipeline are stable enough for regression-corpus coverage, but the public API is subject to change. See `docs/migration/` for the ongoing Stage 4 retirement work that's pruning the `thevenin-types::Netlist`-shaped API surface in favour of `cirq_ir::Circuit`.

## License

BSD-3-Clause
