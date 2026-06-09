# Cirq Language Specification — Circuits and Modules

## Circuit Declaration

r[circuit.decl]

A Cirq file contains one or more `circuit` declarations. A circuit is the top-level unit — it defines a complete netlist with elements, parameters, and analysis commands.

```cirq
circuit voltage_divider {
    param vdd = 5V

    V1: vsource(in -> gnd, dc: vdd)
    R1: resistor(in -> mid, 1k)
    R2: resistor(mid -> gnd, 2k)

    analysis op {}
}
```

### Circuit Name

The circuit name is an identifier. It serves as the title (equivalent to SPICE's title line).

### Circuit Body

r[circuit.body]

The circuit body is a block `{ ... }` containing:
- parameter declarations (`param`, `let`)
- element instantiations
- module instantiations
- analysis commands
- nested module definitions (inline)
- model definitions
- user-defined functions (see `08-expressions.md`)
- `options { ... }` blocks
- `temp <value>` declarations
- `save { ... }` blocks
- `ic { ... }` initial condition blocks
- `global <net>` declarations
- `measure { ... }` blocks (see `07-analysis.md`)

## Module Declaration

r[module.decl]

Modules are reusable subcircuit definitions. They are the Cirq equivalent of SPICE `.subckt`.

```cirq
module inverter {
    port in: in
    port out: out
    port vdd: inout
    port vss: inout

    param wp = 2u
    param wn = 1u
    param l = 180n

    M1: pmos(vdd -> out, gate: in, bulk: vdd, w: wp, l: l)
    M2: nmos(out -> vss, gate: in, bulk: vss, w: wn, l: l)
}
```

### Ports

r[module.port]

Ports define the external interface of a module. Every port has:
- a name
- a direction: `in`, `out`, or `inout`

```cirq
port input_signal: in       // signal flows in
port output_signal: out     // signal flows out
port power_rail: inout      // bidirectional (power, feedback)
```

Port direction is metadata for tooling and validation. Electrically, all ports are equivalent nets. The simulator doesn't enforce direction — but Cirq IR validation may warn on obvious violations (e.g., driving an `in` port from inside the module).

### Port Ordering

r[module.port.order]

Ports are ordered by declaration order. This matters for positional instantiation (see below).

### Bus ports

r[module.port.bus]

A port may declare a **width** to become a bus — a group of N nets:

```cirq
module tap {
    port d[8]: in            // literal width
    port q[width]: out       // parameter-driven width (const-generics style)
    param width = 8
    // ...
}
```

A bus is sugar for the scalar nets `base.0 … base.N-1`. A single line is
referenced with a subscript, `d[2]` (which is the net `d.2`); the whole bus is
referenced by its bare name, `d`.

```cirq
R0: resistor(d[0] -> q[0], 1k)   // one line
T1: tap(d: in8, q: out8)         // whole-bus binding (in8/out8 are buses)
```

A whole-bus binding connects index-wise: inside `tap`, `d[i]` resolves to the
caller's `in8.i`. Widths are checked where both are statically known; binding a
bus to a scalar (or mismatched width) is an error.

> **No generate loops (yet).** Cirq has no compile-time loop, so a `port
> d[width]` module cannot *generate* width-many elements — buses are for
> bundling, pass-through, and explicit per-line indexing (`d[0]`, `d[1]`, …).
> Range slices (`d[0:4]`) and a `generate`-style loop are post-1.0; the `[ ]`
> index syntax itself is in (see [`01-lexical.md`](01-lexical.md)).

## Module Instantiation

r[module.instantiate]

Modules are instantiated like elements, with explicit port connections:

```cirq
// Named port connections (preferred):
inv1: inverter(in: signal_a, out: signal_b, vdd: vdd, vss: gnd)

// Positional port connections (for simple modules):
inv2: inverter(signal_a, signal_b, vdd, gnd)

// Mixed: positional first, then named
inv3: inverter(signal_a, signal_b, vdd: vdd, vss: gnd)
```

### Parameter Override

r[module.param-override]

Module parameters can be overridden at instantiation:

```cirq
inv_fast: inverter(
    in: clk, out: clk_buf, vdd: vdd, vss: gnd,
    wp: 4u, l: 90n
)
```

Only parameters with defaults can be overridden. Parameters without defaults are required.

## Nesting

r[module.nesting]

Modules can be defined inline within circuits or other modules:

```cirq
circuit top {
    module buffer {
        port a: in
        port z: out
        port vdd: inout
        port vss: inout

        inv1: inverter(in: a, out: mid, vdd: vdd, vss: vss)
        inv2: inverter(in: mid, out: z, vdd: vdd, vss: vss)
    }

    buf1: buffer(a: input, z: output, vdd: vdd, vss: gnd)
}
```

## Net Declaration

r[net.implicit]

Nets are created implicitly by use — if a name appears in a connection position that isn't a port or `gnd`, it creates a local net.

Explicit net declaration is optional but useful for documentation:

```cirq
// Implicit (net 'mid' created by first use):
R1: resistor(in -> mid, 1k)
R2: resistor(mid -> gnd, 2k)
```

## Global Nets

r[net.global]

`gnd` is the only built-in global net. Other global nets can be declared:

```cirq
circuit top {
    // All module instances share this net
    global vdd

    // ...
}
```

## Options

r[circuit.options]

Simulation options are set using an `options` block:

```cirq
circuit top {
    options {
        gmin: 1e-12
        abstol: 1e-12
        reltol: 1e-3
    }
}
```

Each setting is a key-value pair. Options correspond to SPICE `.options` settings.

## Temperature

r[circuit.temp]

The simulation temperature (in °C) is set with `temp`:

```cirq
circuit top {
    temp 85
}
```

If omitted, the default temperature is 27°C.

## Save Targets

r[circuit.save]

The `save` block specifies which signals to record during simulation:

```cirq
circuit top {
    save {
        v(out)
        v(mid, gnd)
        i(R1)
    }
}
```

Save targets can be:
- `v(node)` — node voltage
- `v(node1, node2)` — differential voltage
- `i(element)` — current through an element
- bare identifier — raw signal name

## Initial Conditions

r[circuit.ic]

The `ic` block sets initial node voltages for transient analysis:

```cirq
circuit top {
    ic {
        v(out) = 1.5
        v(mid) = 0.8
    }
}
```

Initial conditions are used with `uic: true` in transient analysis, or as hints for the DC operating point solver.

## Embedded Code Blocks

r[circuit.code-block]

A `code` block embeds a verbatim block of source text from another
language. The simulator currently recognises `"control"`, which routes
to the `.control` interpreter:

```cirq
circuit demo {
    V1: vsource(in -> gnd, dc: 5)
    R1: resistor(in -> out, 1k)
    R2: resistor(out -> gnd, 2k)

    analysis op {}

    code "control" {
        run
        let gain = v(out) / v(in)
        print gain
        quit 0
    }
}
```

Contents of the block are preserved as raw text and dispatched at
simulate time.

### Language registry

r[circuit.language-registry]

The set of accepted language tags is governed by a **language registry**, in
two halves:

- **Compile time** — `cirq_frontend::LanguageRegistry` is the set of accepted
  tags. `cirq_frontend::compile` accepts only `"control"`; a block with any
  other tag is **rejected with a spanned diagnostic** rather than silently
  dropped. A host that supports additional languages compiles with
  `cirq_frontend::compile_with_languages(source, &registry)`, registering the
  extra tags first.
- **Execution time** — `thevenin_control::LanguageRegistry` maps each tag to a
  `LanguageHandler`. The default handles only `"control"` (the `.control`
  interpreter). Hosts register additional handlers
  (`registry.register("js", Box::new(JsHandler))`) and run blocks with
  `thevenin_control::execute_code_blocks_ir(circuit, &registry)`.

The two halves are kept in sync by the host: `LanguageRegistry::tags()` on the
execution registry returns exactly the tags to feed into
`cirq_frontend::LanguageRegistry::with_languages`.

A `LanguageHandler` receives the verbatim block body, the IR's pre-parsed AST
when available (today only for `"control"`), and the live simulation context;
it reports results purely through side effects on that context (appended
plots, printed output, an exit code). The full contract is documented in
[`docs/architecture/language-registry.md`](../architecture/language-registry.md).

This is the extensibility mechanism for embedding other languages (an embedded
JS engine, a Python block, a domain-specific DSL) without adding new Cirq
syntax for each.


## Import

r[import.decl]

Modules can be imported from other files:

```cirq
import "standard_cells.cirq"
import "models/nmos_3v3.cirq" as nmos_lib

circuit top {
    inv1: nmos_lib.inverter(...)
}
```

Import resolves at the file level. The imported file is parsed and its top-level declarations (modules, models, functions) are merged into the importing file's AST.

r[import.resolution]

Import resolution:
- Paths are resolved relative to the importing file's directory
- Circular imports are detected and reported as errors
- Diamond dependencies (A imports B and C, both import D) are deduplicated automatically
- Recursive imports are supported
