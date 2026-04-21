# Cirq Language Specification — Parameters

## Parameter Declaration

Parameters are named constants that can be used throughout a circuit or module.

```cirq
param vdd = 3.3
param r_load = 10k
param temperature = 27
```

### Scope

Parameters are scoped to their enclosing block (circuit or module).

```cirq
circuit top {
    param vdd = 5              // visible in top

    module child {
        param vdd = 3.3        // shadows top.vdd inside child
        param local = 100      // only visible in child
    }
}
```

### Required Parameters (No Default)

Module parameters without defaults must be provided at instantiation:

```cirq
module amplifier {
    port in: in
    port out: out
    port vdd: inout
    port vss: inout

    param gain                 // REQUIRED — no default
    param bandwidth = 1M      // optional — has default

    // ...
}

// Must provide gain:
amp1: amplifier(in: sig, out: buf, vdd: vdd, vss: gnd, gain: 20)
```

### Parameter Expressions

Parameter values can reference other parameters:

```cirq
param r1 = 10k
param r2 = r1 * 2             // 20k
param r_parallel = (r1 * r2) / (r1 + r2)
```

Forward references are not allowed — a parameter must be declared before it is used. This keeps evaluation order simple and predictable.

## Let Bindings

`let` introduces a local computed value. Unlike `param`, `let` bindings cannot be overridden at instantiation:

```cirq
module divider {
    port in: in
    port out: out
    param r_top = 10k
    param r_bot = 20k

    let ratio = r_bot / (r_top + r_bot)  // computed, not overridable

    R1: resistor(in -> out, r_top)
    R2: resistor(out -> gnd, r_bot)
}
```

## Built-in Constants

| Name | Value | Description | Status |
|------|-------|-------------|--------|
| `pi` | 3.14159... | Pi | ✓ implemented |
| `e` | 2.71828... | Euler's number | ✓ implemented |

## Parameter Validation

Parameters can carry validation constraints via attributes:

```cirq
@range(0, 1)
param coupling_k = 0.5

@positive
param width = 1u

@choices("nmos", "pmos")
param device_type = "nmos"
```

See `09-attributes.md` for attribute syntax. Validation is checked at IR lowering time.
