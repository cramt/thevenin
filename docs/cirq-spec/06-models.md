# Cirq Language Specification — Device Models

## Model Declaration

A `model` block defines device model parameters. This is the Cirq equivalent of SPICE's `.model` card.

```cirq
model d1n4148: diode {
    is = 2.52n
    rs = 0.568
    n = 1.752
    bv = 100
    ibv = 100u
    cjo = 4p
    vj = 0.7
    m = 0.4
    tt = 5.7n
}
```

### Syntax

```
model <name>: <device_type> {
    <param> = <value>
    ...
}
```

A model can optionally extend a base model:

```
model <name>: <base_model> {
    <param> = <value>    // overrides
}
```

The device type must be one of the built-in device kinds:
- `diode`
- `npn`, `pnp` (BJT)
- `nmos`, `pmos` (MOSFET — level selected by parameters)
- `njfet`, `pjfet` (JFET)
- `nmesfet`, `pmesfet` (MESFET)

### Model Levels

For MOSFETs, the `level` parameter selects the model equations:

```cirq
model nch_3v3: nmos {
    level = 1           // Shichman-Hodges
    vto = 0.7
    kp = 110u
    gamma = 0.4
    phi = 0.65
    lambda = 0.04
}

model nch_bsim3: nmos {
    level = 49          // BSIM3v3
    tnom = 27
    version = 3.3
    // ... many parameters
}

model nch_bsim4: nmos {
    level = 54          // BSIM4
    // ...
}
```

## Model Inheritance

A model can extend another model, overriding specific parameters:

```cirq
model nch_base: nmos {
    level = 1
    vto = 0.7
    kp = 110u
    lambda = 0.04
}

model nch_fast: nch_base {
    kp = 150u          // faster switching
    lambda = 0.02
}
```

The derived model inherits all parameters from the base and overrides only what is specified.

## Model Libraries

Models can be organized in separate files and imported:

```cirq
// models/cmos_180nm.cirq
model nch: nmos {
    level = 49
    // ...
}

model pch: pmos {
    level = 49
    // ...
}

// top.cirq
import "models/cmos_180nm.cirq" as cmos

circuit inverter_test {
    M1: pmos(vdd -> out, gate: in, bulk: vdd, model: cmos.pch, w: 2u, l: 180n)
    M2: nmos(out -> gnd, gate: in, bulk: gnd, model: cmos.nch, w: 1u, l: 180n)
}
```

## Using Models

Models are referenced by name in element instantiations:

```cirq
model my_diode: diode {
    is = 1e-14
    n = 1.05
}

D1: diode(a -> b, model: my_diode)
```

The `model` parameter is always named (never positional) to avoid ambiguity.

## SPICE Model Level Mapping

| Cirq Device | SPICE Element | Supported Levels |
|-------------|--------------|-----------------|
| `nmos`/`pmos` | M | 1 (Shichman-Hodges), 2 (MOS2), 3 (MOS3), 6 (MOS6), 49 (BSIM3v3), 54 (BSIM4) |
| `npn`/`pnp` | Q | 1 (Gummel-Poon), 4 (VBIC) |
| `diode` | D | 1 (standard) |
| `njfet`/`pjfet` | J | 1 (Shichman-Hodges) |
| `nmesfet`/`pmesfet` | Z | 1 (Statz) |
