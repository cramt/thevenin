# Cirq Language Specification — Attributes

## Attribute Syntax

r[attr.syntax]

Attributes are metadata annotations that do not affect simulation semantics. They provide information for tooling, documentation, and validation.

```cirq
@attribute_name
@attribute_name(arg1, arg2)
@attribute_name(key: value)
```

Attributes attach to the next declaration.

## Built-in Attributes

### @range

r[attr.range]

Constrains a parameter to a numeric range. Checked at IR validation.

```cirq
@range(0, 1)
param coupling = 0.5

@range(min: 0)
param width = 1u
```

### @positive

r[attr.positive]

Shorthand for `@range(min: 0, exclusive: true)`.

```cirq
@positive
param resistance = 10k
```

### @choices

r[attr.choices]

Restricts a string parameter to a set of allowed values.

```cirq
@choices("decade", "octave", "linear")
param scale = "decade"
```

### @deprecated

r[attr.deprecated]

Marks a parameter or module as deprecated. Tooling should warn on use.

```cirq
@deprecated("use inverter_v2 instead")
module inverter_v1 {
    // ...
}
```

### @description

r[attr.description]

Attaches a documentation string to a declaration.

```cirq
@description("Supply voltage for the entire chip")
param vdd = 1.8
```

### @unit

r[attr.unit]

Declares the physical unit of a parameter (informational).

```cirq
@unit("ohm")
param r_load = 10k

@unit("Hz")
param f_clk = 100M
```

## Custom Attributes

r[attr.custom]

Users may define arbitrary attributes. Unrecognized attributes are preserved in the AST/IR for external tooling but ignored by the simulator.

```cirq
@layout(x: 100, y: 200)
M1: nmos(drain -> source, gate: g, bulk: gnd, model: nch, w: 1u, l: 180n)
```

## Attribute Targets

r[attr.targets]

Attributes can be applied to:
- `param` declarations
- `let` bindings
- element instantiations
- module declarations
- circuit declarations
- port declarations

```cirq
@description("Test circuit for CMOS inverter")
circuit inverter_test {

    @description("Input signal port")
    port in: in

    @positive
    param vdd_voltage = 1.8

    @description("Pull-up transistor")
    M1: pmos(...)
}
```
