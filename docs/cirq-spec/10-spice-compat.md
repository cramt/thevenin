# Cirq Language Specification — SPICE Compatibility

## Goal

Every valid SPICE netlist that Thevenin supports can be mechanically translated to Cirq. This section documents the mapping rules.

## Element Name Mapping

| SPICE Prefix | Cirq Element Type |
|-------------|-------------------|
| R | `resistor` |
| C | `capacitor` |
| L | `inductor` |
| K | `coupling` |
| V | `vsource` |
| I | `isource` |
| B | `behavioral` |
| D | `diode` |
| Q | `npn` / `pnp` |
| M | `nmos` / `pmos` |
| J | `njfet` / `pjfet` |
| Z | `nmesfet` / `pmesfet` |
| E | `vcvs` |
| G | `vccs` |
| H | `ccvs` |
| F | `cccs` |
| T | `tline` |
| X | module instantiation |

## Node Mapping

| SPICE | Cirq |
|-------|------|
| Node `0` | `gnd` |
| Named nodes (e.g., `in`, `mid`) | Same identifier |
| Numeric nodes (e.g., `1`, `2`, `3`) | Mapped to `n1`, `n2`, `n3` etc. |

## Directive Mapping

| SPICE Directive | Cirq Equivalent |
|----------------|-----------------|
| Title line | `circuit <name>` (derived from title) |
| `.subckt` | `module` |
| `.model` | `model` |
| `.param` | `param` |
| `.op` | `analysis op {}` |
| `.dc` | `analysis dc { sweep ... }` |
| `.ac` | `analysis ac { ... }` |
| `.tran` | `analysis tran { ... }` |
| `.noise` | `analysis noise { ... }` |
| `.pz` | `analysis pz { ... }` |
| `.sens` | `analysis sens { ... }` |
| `.tf` | `analysis tf { ... }` |
| `.include` | `import "..."` |
| `.lib` | `import "..." as ...` |
| `.end` | `}` (closing circuit block) |
| `.ends` | `}` (closing module block) |
| `.ic` | `ic { v(node) = value }` block |
| `.nodeset` | (future: `hint` block) |
| `.options` | `options { key: value }` block |
| `.global` | `global <net>` |
| `.save` | `save { v(node) i(elem) }` block |
| `.temp` | `temp <value>` |
| `.func` | `name(args) = expr` function declaration |
| `.control` / `.endc` | `code "control" { ... }` block |

## SI Suffix Differences

| Suffix | SPICE Meaning | Cirq Meaning |
|--------|--------------|--------------|
| `M` | milli (1e-3) | mega (1e6) |
| `m` | milli (1e-3) | milli (1e-3) |
| `Meg` | mega (1e6) | mega (1e6) |

The SPICE importer handles this mapping automatically. When converting SPICE `M` to Cirq, it becomes `m` (milli).

## Example Translation

### SPICE

```spice
Voltage Divider
V1 in 0 DC 5
R1 in mid 1k
R2 mid 0 2k
.op
.end
```

### Cirq

```cirq
circuit voltage_divider {
    V1: vsource(in -> gnd, dc: 5)
    R1: resistor(in -> mid, 1k)
    R2: resistor(mid -> gnd, 2k)

    analysis op {}
}
```

### SPICE (CMOS Inverter)

```spice
CMOS Inverter
.model nch nmos level=1 vto=0.7 kp=110u
.model pch pmos level=1 vto=-0.7 kp=55u
Vdd vdd 0 1.8
Vin in 0 PULSE(0 1.8 0 1n 1n 5n 10n)
M1 out in vdd vdd pch W=2u L=180n
M2 out in 0 0 nch W=1u L=180n
.tran 0.1n 20n
.end
```

### Cirq

```cirq
circuit cmos_inverter {
    model nch: nmos {
        level = 1
        vto = 0.7
        kp = 110u
    }

    model pch: pmos {
        level = 1
        vto = -0.7
        kp = 55u
    }

    Vdd: vsource(vdd -> gnd, dc: 1.8)
    Vin: vsource(in -> gnd,
        pulse: { v1: 0, v2: 1.8, td: 0, tr: 1n, tf: 1n, pw: 5n, per: 10n }
    )

    M1: pmos(vdd -> out, gate: in, bulk: vdd, model: pch, w: 2u, l: 180n)
    M2: nmos(out -> gnd, gate: in, bulk: gnd, model: nch, w: 1u, l: 180n)

    analysis tran {
        step: 100p
        stop: 20n
    }
}
```

## What Cannot Be Automatically Translated

The SPICE importer covers everything the harness corpus uses, but the
Cirq source language has gaps relative to what the importer accepts:

- **`.meas` / `.measure` from Cirq source.** The IR carries a
  `MeasureSpec` and the SPICE importer populates it. Lowering a `meas`
  declaration from Cirq source isn't wired yet.
- **`.four` (Fourier analysis).** Not represented in IR.
- **User-defined XSPICE code models in Cirq source.** XSPICE *instances*
  (A elements) round-trip fine via `xspice(...)`; defining a new code
  model from Cirq syntax (the `.cmodel`-equivalent) is not designed.

`.control` / `.endc` blocks round-trip through `code "control" { ... }`.
CPL multiconductor lines round-trip through `coupled_line P1 { ... }`
(see `04-elements.md`).
