# Getting started with Thevenin

Thevenin is a SPICE circuit simulator written in Rust. It runs the same
analyses ngspice does — DC operating point, DC/AC sweeps, transient, noise,
sensitivity, transfer function, pole-zero, and Fourier/FFT post-processing —
over the same numerical core (Modified Nodal Analysis, Newton–Raphson, sparse
direct solve). You can feed it a standard **SPICE netlist** or write circuits in
**Cirq**, Thevenin's own circuit-description language.

This guide takes you from a clean checkout to your first simulation, in both the
CLI and the library, and points you at the reference docs for everything else.

---

## 1. Prerequisites

The repository ships a Nix flake that pins the full toolchain (Rust, the
tree-sitter grammars, `wasm` target, test runner). You only need
[Nix with flakes enabled](https://nixos.org/download). Every command below runs
inside the dev shell so the build never depends on whatever happens to be on
your host:

```bash
nix develop --command cargo build
```

The first build compiles the whole workspace (12 crates) and may take a few
minutes; afterwards it is incremental.

> Throughout this guide, `nix develop --command <cmd>` runs `<cmd>` in the
> pinned shell. If you'd rather drop into the shell once, run `nix develop` and
> then use plain `cargo …` inside it.

---

## 2. Run your first simulation (CLI)

The CLI builds as the `thevenin-cli` binary. In-repo, invoke it through Cargo:

```bash
nix develop --command cargo run -- run examples/cirq/voltage_divider.cirq
```

You should see the operating point printed in ngspice's batch style:

```
op1:
  v(mid) = [3.333333]
  v(in) = [5.000000]
  v1#branch = [-0.001667]
```

The same command accepts a SPICE netlist — the input format is chosen by
extension (`.cirq` → Cirq source, anything else → SPICE):

```bash
nix develop --command cargo run -- run my_circuit.cir
```

When you install the binary (`cargo install --path .`), the command is simply:

```bash
thevenin-cli run circuit.cirq
thevenin-cli run circuit.cir
```

---

## 3. Write your first Cirq circuit

Cirq is a small, typed language designed to express anything you'd write in
SPICE, but with named ports, real parameters, expressions, and modules. The
"hello world" is a resistive divider — see
[`examples/cirq/voltage_divider.cirq`](../examples/cirq/voltage_divider.cirq):

```cirq
// Simple voltage divider — the "hello world" of circuit simulation.

circuit voltage_divider {
    param vdd = 5

    V1: vsource(in -> gnd, dc: vdd)
    R1: resistor(in -> mid, 1k)
    R2: resistor(mid -> gnd, 2k)

    analysis op {}
}
```

A few things to notice:

- **`circuit <name> { … }`** is the top-level container.
- **`param vdd = 5`** declares a parameter; it can be referenced in any value
  and overridden when the circuit is instantiated as a module.
- **`V1: vsource(in -> gnd, dc: vdd)`** — every element is `name: kind(ports,
  args)`. The `in -> gnd` arrow names the element's nets positionally; `dc: vdd`
  is a named argument. `1k`, `100n`, `2u` are SI-suffixed literals.
- **`analysis op {}`** requests a DC operating point. Other analyses
  (`tran`, `ac`, `dc`, `noise`, …) take a brace block of settings — see the
  AC example below.

An AC sweep with expressions and a `let` binding
([`examples/cirq/rc_filter.cirq`](../examples/cirq/rc_filter.cirq)):

```cirq
circuit rc_lowpass {
    param r = 10k
    param c = 100n

    let f_cutoff = 1 / (2 * pi * r * c)   // ≈ 159 Hz

    V1: vsource(in -> gnd, dc: 0, ac: 1)
    R1: resistor(in -> out, r)
    C1: capacitor(out -> gnd, c)

    analysis ac {
        start: 1
        stop: 1M
        points: 100
        scale: decade
    }
}
```

More worked examples live in [`examples/cirq/`](../examples/cirq/): a
[CMOS inverter](../examples/cirq/cmos_inverter.cirq) with a `.model` and a
transient pulse, [hierarchical modules](../examples/cirq/hierarchical.cirq),
[measurements](../examples/cirq/measurements.cirq),
[Fourier analysis](../examples/cirq/fourier.cirq), and
[compile-time conditionals](../examples/cirq/conditional.cirq).

The full language reference is in [`cirq-spec/`](cirq-spec/00-overview.md).

---

## 4. Run an existing SPICE netlist

If you already have SPICE decks, you don't need to translate them. Thevenin's
importer accepts standard ngspice-dialect netlists:

```spice
Voltage Divider
V1 in 0 1.0
R1 in mid 1k
R2 mid 0 2k
.op
.end
```

```bash
nix develop --command cargo run -- run divider.cir
```

`.include` and `.lib` directives are resolved relative to the input file (and
any extra search paths). What is and isn't supported — devices, analyses,
directives — is documented in [`devices.md`](devices.md) and
[`cirq-spec/10-spice-compat.md`](cirq-spec/10-spice-compat.md). HSPICE/PSPICE
dialect extensions are out of scope; run those through their own tool's
preprocessor first.

---

## 5. Use Thevenin as a library

The simulator is a set of crates you can depend on directly. The canonical
input to every analysis is a [`cirq_ir::Circuit`]; you obtain one either by
compiling Cirq source or by importing SPICE.

### From a SPICE string

```rust
use cirq_spice_import::import_spice;
use thevenin::circuit::simulate;

let circuit = import_spice(
    "Voltage Divider
V1 in 0 1.0
R1 in mid 1k
R2 mid 0 2k
.op
.end
",
)
.expect("parse")
.pop()
.expect("at least one circuit");

let result = simulate(&circuit).expect("simulate");

let plot = result.plot().expect("a plot");
for vec in plot.voltages() {
    println!("{:>16} = {:.6}", vec.name, vec.data.as_real()[0]);
}
```

### From Cirq source

Swap the importer for the frontend; everything downstream is identical:

```rust
use thevenin::circuit::simulate;

let circuit = cirq_frontend::compile(
    "circuit divider {
         V1: vsource(in -> gnd, dc: 1.0)
         R1: resistor(in -> mid, 1k)
         R2: resistor(mid -> gnd, 2k)
         analysis op {}
     }",
)
.expect("compile");

let result = simulate(&circuit).expect("simulate");
let vmid = result["v(mid)"].data.as_real()[0];
assert!((vmid - 0.6667).abs() < 1e-3);
```

Runnable versions of these — plus DC sweep, AC, and transient — are in the
repository's [`examples/`](../examples/) directory:

```bash
nix develop --command cargo run --example dc_operating_point
nix develop --command cargo run --example ac_analysis
nix develop --command cargo run --example transient
```

### Which entry point?

| Call | Use when |
|---|---|
| [`thevenin::circuit::simulate`] | Run **every** analysis the circuit declares, in order. The usual choice. |
| `simulate_op` / `simulate_dc` / `simulate_tran` / `simulate_ac` / … | Run a single analysis of one kind. |
| `simulate_four` / `simulate_fft` | Fourier / FFT post-processing of a preceding transient. |

The stability guarantees for these APIs across the 1.x line are spelled out in
[`api-stability.md`](api-stability.md).

---

## 6. Getting results out

A `SimResult` holds named result plots in memory, but you can also write
standard interchange files — directly or from a `.control` script:

- **ngspice raw file** (binary, little-endian, or ASCII) — the canonical format
  read by KiCad, matplotlib, gnuplot, and regression frameworks.
- **CSV** — header row plus one row per timestep/frequency point.

From a `.control` block inside a netlist:

```spice
.control
run
write output.raw v(out) v(in)
write output.csv v(out)
.endc
```

The binary layout is documented in
[`architecture/raw-file-format.md`](architecture/raw-file-format.md). The full
list of `.control` commands the interpreter supports is in
[`cirq-spec/03-circuits-and-modules.md`](cirq-spec/03-circuits-and-modules.md)
and the [`thevenin-control`](https://docs.rs/thevenin-control) API docs.

---

## 7. Develop against the codebase

```bash
nix develop --command cargo nextest run                          # full workspace suite
nix develop --command cargo nextest run -p thevenin --test harness   # ngspice regression corpus
nix develop --command cargo clippy --workspace -- -D warnings    # lints (warnings are errors)
nix develop --command cargo fmt --check                          # formatting
nix develop --command cargo doc --no-deps --workspace --open     # API docs
nix develop --command cargo test --workspace --target wasm32-unknown-unknown   # wasm build
```

The regression harness replays every fixture from `ngspice-upstream/tests/`
through SPICE → Cirq IR → simulate and diffs against ngspice's reference output.

---

## Where to go next

- **[`cirq-spec/`](cirq-spec/00-overview.md)** — the complete Cirq language
  reference (lexical structure, types, elements, models, expressions,
  analyses, SPICE-compat notes).
- **[`devices.md`](devices.md)** — the device-coverage matrix with per-row
  source pointers and known gaps.
- **[`api-stability.md`](api-stability.md)** — what the 1.x API promises and how
  breaking changes are handled.
- **[`architecture/`](architecture/cirq-crate-map.md)** — how the crates fit
  together (the Cirq crate map, the Cirq → Thevenin boundary, the raw-file
  format, the embedded-language registry).
- **[`1.0-checklist.md`](1.0-checklist.md)** — current release readiness against
  the three 1.0 goals.
- **[`future-work.md`](future-work.md)** — diagnosis of the skipped regression
  fixtures and post-1.0 work.

[`cirq_ir::Circuit`]: https://docs.rs/cirq-ir
[`thevenin::circuit::simulate`]: https://docs.rs/thevenin
