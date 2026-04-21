# Cirq Language Specification — Circuits and Modules

## Circuit Declaration

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

## Module Declaration

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

Ports are ordered by declaration order. This matters for positional instantiation (see below).

## Module Instantiation

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

Module parameters can be overridden at instantiation:

```cirq
inv_fast: inverter(
    in: clk, out: clk_buf, vdd: vdd, vss: gnd,
    wp: 4u, l: 90n
)
```

Only parameters with defaults can be overridden. Parameters without defaults are required.

## Nesting

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

Nets are created implicitly by use — if a name appears in a connection position that isn't a port or `gnd`, it creates a local net.

Explicit net declaration is optional but useful for documentation:

```cirq
// Implicit (net 'mid' created by first use):
R1: resistor(in -> mid, 1k)
R2: resistor(mid -> gnd, 2k)
```

## Global Nets

`gnd` is the only built-in global net. Other global nets can be declared:

```cirq
circuit top {
    // All module instances share this net
    global vdd

    // ...
}
```

## Options

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

The simulation temperature (in °C) is set with `temp`:

```cirq
circuit top {
    temp 85
}
```

If omitted, the default temperature is 27°C.

## Save Targets

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

## Import

Modules can be imported from other files:

```cirq
import "standard_cells.cirq"
import "models/nmos_3v3.cirq" as nmos_lib

circuit top {
    inv1: nmos_lib.inverter(...)
}
```

Import resolves at the file level. The imported file is parsed and its top-level declarations (modules, models, functions) are merged into the importing file's AST.

Import resolution:
- Paths are resolved relative to the importing file's directory
- Circular imports are detected and reported as errors
- Diamond dependencies (A imports B and C, both import D) are deduplicated automatically
- Recursive imports are supported
