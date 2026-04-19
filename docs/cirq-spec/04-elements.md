# Cirq Language Specification — Elements

## Element Syntax

Every element instantiation follows the same pattern:

```cirq
name: element_type(connections, parameters...)
```

The name is a user-chosen identifier. The element type is a keyword or model reference. Connections use the `->` operator to indicate current flow direction.

## Connection Operator

The `->` operator connects two nets through an element, indicating conventional current flow direction (positive terminal → negative terminal):

```cirq
R1: resistor(a -> b, 10k)      // current flows from a to b when v(a) > v(b)
V1: vsource(vdd -> gnd, dc: 5) // positive terminal is vdd
```

For elements with more than two terminals, named connections are used:

```cirq
Q1: npn(collector: c, base: b, emitter: e, model: bc547)
M1: nmos(drain -> source, gate: g, bulk: gnd, model: nch, w: 1u, l: 180n)
```

## Passive Elements

### Resistor

```cirq
R1: resistor(a -> b, 10k)
R2: resistor(a -> b, resistance: 10k)       // named parameter
R3: resistor(a -> b, resistance: 10k, tc1: 0.001, tc2: 0.0001)  // temp coeffs
```

Parameters:
- `resistance` (positional 1): resistance value — **required**
- `tc1`: first-order temperature coefficient — optional, default 0
- `tc2`: second-order temperature coefficient — optional, default 0

### Capacitor

```cirq
C1: capacitor(a -> b, 100n)
C2: capacitor(a -> b, capacitance: 100p, ic: 0)  // initial condition
```

Parameters:
- `capacitance` (positional 1): capacitance value — **required**
- `ic`: initial voltage across capacitor — optional

### Inductor

```cirq
L1: inductor(a -> b, 10u)
L2: inductor(a -> b, inductance: 1m, ic: 0)
```

Parameters:
- `inductance` (positional 1): inductance value — **required**
- `ic`: initial current through inductor — optional

### Mutual Inductance (Coupled Inductors)

```cirq
K1: coupling(L1, L2, coefficient: 0.99)
```

Parameters:
- Positional: two or more inductor references
- `coefficient`: coupling coefficient (0 to 1) — **required**

## Sources

### Voltage Source

```cirq
V1: vsource(vdd -> gnd, dc: 5)
V2: vsource(a -> b, dc: 0, ac: 1)
V3: vsource(a -> b, dc: 3.3, ac: 1, phase: 90)
```

Parameters:
- `dc`: DC value — optional, default 0
- `ac`: AC magnitude for small-signal analysis — optional
- `phase`: AC phase in degrees — optional, default 0
- Waveform specification (see below)

### Current Source

```cirq
I1: isource(a -> b, dc: 1m)
I2: isource(a -> b, dc: 0, ac: 0.5)
```

Same parameter structure as voltage source.

### Waveform Specifications

Sources can carry transient waveforms:

```cirq
// Pulse
V_clk: vsource(clk -> gnd,
    pulse: { v1: 0, v2: 3.3, delay: 0, rise: 1n, fall: 1n, width: 5n, period: 10n }
)

// Sinusoidal
V_sig: vsource(sig -> gnd,
    sin: { offset: 0, amplitude: 1, freq: 1k }
)

// Piecewise Linear
V_ramp: vsource(ramp -> gnd,
    pwl: [(0, 0), (1u, 0), (2u, 5), (10u, 5)]
)

// Exponential
V_exp: vsource(a -> gnd,
    exp: { v1: 0, v2: 5, td1: 1n, tau1: 10n, td2: 50n, tau2: 20n }
)
```

## Semiconductor Devices

### Diode

```cirq
D1: diode(anode -> cathode, model: d1n4148)
D2: diode(a -> b, model: zener_5v1, area: 2)
```

Parameters:
- `model`: device model reference — **required**
- `area`: area multiplier — optional, default 1
- `ic`: initial condition — optional

### BJT (NPN/PNP)

```cirq
Q1: npn(collector: c, base: b, emitter: e, model: bc547)
Q2: pnp(emitter: e, base: b, collector: c, model: bc557, area: 2)
Q3: npn(collector: c, base: b, emitter: e, substrate: sub, model: q2n2222)
```

Parameters:
- `model`: device model reference — **required**
- `area`: area multiplier — optional, default 1
- Connections: `collector`, `base`, `emitter`, optional `substrate`

### MOSFET

```cirq
M1: nmos(drain -> source, gate: g, bulk: gnd,
    model: nch, w: 10u, l: 180n)

M2: pmos(source -> drain, gate: g, bulk: vdd,
    model: pch, w: 20u, l: 180n,
    ad: 5p, as: 5p, pd: 10u, ps: 10u)
```

Parameters:
- `model`: device model reference — **required**
- `w`: channel width — **required**
- `l`: channel length — **required**
- `ad`, `as`: drain/source area — optional
- `pd`, `ps`: drain/source perimeter — optional
- `nrd`, `nrs`: drain/source resistance squares — optional
- Connections: `drain`, `source` (via `->` or named), `gate`, `bulk`

### JFET

```cirq
J1: njfet(drain -> source, gate: g, model: j201)
J2: pjfet(drain -> source, gate: g, model: pjf1)
```

## Controlled Sources

### Voltage-Controlled Voltage Source (VCVS)

```cirq
E1: vcvs(out_p -> out_n, control: ctrl_p -> ctrl_n, gain: 10)
```

### Voltage-Controlled Current Source (VCCS)

```cirq
G1: vccs(out_p -> out_n, control: ctrl_p -> ctrl_n, transconductance: 1m)
```

### Current-Controlled Voltage Source (CCVS)

```cirq
H1: ccvs(out_p -> out_n, sense: V_sense, transresistance: 100)
```

### Current-Controlled Current Source (CCCS)

```cirq
F1: cccs(out_p -> out_n, sense: V_sense, gain: 50)
```

## Transmission Lines

```cirq
T1: tline(in_p -> in_n, out_p -> out_n, z0: 50, td: 1n)
```
