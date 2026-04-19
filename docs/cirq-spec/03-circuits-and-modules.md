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
- parameter declarations
- element instantiations
- module instantiations
- analysis commands
- nested module definitions (inline)

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

## Import

Modules can be imported from other files:

```cirq
import "standard_cells.cirq"
import "models/nmos_3v3.cirq" as nmos_lib

circuit top {
    inv1: nmos_lib.inverter(...)
}
```

Import resolves at the file level. Circular imports are an error.
