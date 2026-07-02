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
- **Sensitivity** (`.sens`, DC and AC)
- **Transfer function** (`.tf`)
- **Pole-zero** (`.pz`)
- **Fourier post-processing** (`.four` — DFT + THD)
- **FFT post-processing** (`.fft` — windowed Cooley-Tukey)
- **Multi-temperature sweep** (`.temp 25 50 100`)

Every analysis above takes a `cirq_ir::Circuit` directly; `thevenin-types::Netlist` is an internal compatibility layer for the SPICE parser.

## Device models

| Family | Coverage |
|---|---|
| Passive | R, L, C, K (mutual coupling) |
| Independent sources | V, I — DC, AC mag/phase, plus PULSE / SIN / EXP / PWL / SFFM / AM transient |
| Linear dependent sources | E (VCVS), G (VCCS), H (CCVS), F (CCCS) |
| Behavioural | B `V=expr` / `I=expr` with full expression engine |
| Diodes | SPICE Shockley (IS, N, RS, BV, CJO, TT, KF/AF, pnjlim) |
| BJTs | Gummel-Poon (LEVEL=1), VBIC95 (LEVEL=4) |
| MOSFETs | Shichman-Hodges (1), Grove-Frohman (2), MOS3 short-channel (3), Sakurai-Newton (6), BSIM1 (4), BSIM2 (5), BSIM3v3 (8/49), BSIM4 (14/54), BSIM3SOI FD/DD/PD (55/56/57), HiSIM2 (68 — faithful DC I-V, golden-verified; charges/AC deferred), HiSIMHV (73 — partial, no HV extensions yet), VDMOS power MOSFET |
| Other FETs | JFET, MESFET (Statz/Curtice), MESA (Ytterdal/Lee/Shur/Fjeldly), HFET1, HFET2 |
| Transmission lines | LTRA (O), TXL (Y), CPL (P), ideal lossless line (T) |
| Switches | S (voltage-controlled), W (current-controlled) — hysteretic |
| XSPICE | A element with compiled-in code-model registry |
| Subcircuits | `.subckt` / `X` (nested, parameter overrides) |

For per-row source-file pointers and known gaps, see [`docs/devices.md`](docs/devices.md).

## What's not supported

- **HSPICE / PSPICE dialect netlists** — anything beyond what stock ngspice itself parses is post-1.0. Translate or pre-process upstream.
- **Dynamic XSPICE plug-ins** (`cm` shared objects). Compiled-in code models via the registry stay in scope.
- **`.disto`** small-signal distortion (HD2/HD3/IM2/IM3) — deferred post-1.0. Use `.tran` + `.four`/`.fft` for distortion analysis in the meantime.
- **`.pss`** periodic steady-state — not in mainline ngspice.
- **HiSIMHV high-voltage extensions** — RDRIFT region, body resistance, breakdown not yet modelled. LEVEL=73 dispatches into the simplified HiSIM core; use VDMOS for power circuits until the HV core lands.
- **Interactive shell features** — command history, `cd`/`pwd`, `shell`, gnuplot integration. Use `.control` scripts instead.
- **Co-simulation** — Verilog-A, TCL, external solvers.

## Quick start

New here? The [**Getting Started guide**](docs/getting-started.md) walks from a
clean checkout through your first CLI run, writing a Cirq circuit, importing a
SPICE netlist, and using the library API.

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

## Output formats

The simulator emits results in several formats for tool interop:

- **ngspice raw file** (binary, little-endian IEEE 754) — the canonical interchange format for KiCad, matplotlib, gnuplot, etc.
- **ngspice raw file** (ASCII) — same shape, ASCII variant.
- **CSV** — header row + comma-separated values per timestep/frequency point.
- **Text "batch"** output mirroring ngspice's stdout (`.print` / `.plot`).

Wire from `.control` scripts via `write filename.raw` / `write filename.csv`.
See [`docs/architecture/raw-file-format.md`](docs/architecture/raw-file-format.md) for the binary layout.

## Test coverage

The regression harness runs every test fixture from `ngspice-upstream/tests/` through SPICE → Cirq IR → simulate and diffs against the ngspice reference output — **106 passing, 1 skipped** (`general/rtlinv.cir`, a BJT saturation-recovery timing gap under active investigation). Across the full workspace it's **1474 passing, 1 skipped**. See `thevenin/tests/ignore.toml` for the skip reason and `docs/future-work.md` for the diagnosis.

## Status

The project is approaching 1.0. The simulator core, the Cirq IR pipeline, the
Cirq source language, and the SPICE importer are all feature-complete relative
to the [`docs/1.0-checklist.md`](docs/1.0-checklist.md) targets. Remaining work
before the 1.0 cut is release process (version bumps, publish-dry-run CI) plus a
small set of explicit post-1.0 deferrals: `.disto`, full HiSIM/HiSIMHV physics
(DC works today), and a handful of niche device models (HICUM2, MOS9, SOI3,
BSIMSOI 4.x, JFET2) that each have a covering alternative already shipping. The
[`docs/api-stability.md`](docs/api-stability.md) statement defines the stable
1.x surface. See `docs/archive/migration/` for the historical Stage 4 retirement
work that pruned the `thevenin-types::Netlist`-shaped API surface in favour of
`cirq_ir::Circuit`.

## Documentation

- [**Getting Started**](docs/getting-started.md) — clean checkout to first simulation, CLI and library.
- [Cirq language spec](docs/cirq-spec/00-overview.md) — the full source-language reference.
- [Device coverage matrix](docs/devices.md) — supported models and known gaps.
- [API stability](docs/api-stability.md) — the stable 1.x surface and breaking-change policy.
- [Architecture](docs/architecture/cirq-crate-map.md) — crate map, Cirq → Thevenin boundary, raw-file format, language registry.
- [1.0 checklist](docs/1.0-checklist.md) / [future work](docs/future-work.md) — release readiness and post-1.0 backlog.

Per-crate API docs render on [docs.rs](https://docs.rs); build them locally with
`nix develop --command cargo doc --no-deps --workspace --open`.

## License

BSD-3-Clause
