# CirQ Format Specification
**Version 0.3 — Draft**

A human- and machine-friendly schema for describing electronic circuits.
Serialization-agnostic: use JSON, YAML, TOML, or anything that maps to the same data model.

---

# Format Version

r[format.version]
Every CirQ document MUST include a top-level `cirq` field containing the format version as a string (e.g. `"0.3"`). Documents without this field are invalid.

r[format.version.semver]
The version string MUST follow semantic versioning. Parsers SHOULD reject documents whose major version is higher than the parser's supported major version.

---

# Top-Level Structure

r[doc.name]
Every CirQ document MUST include a top-level `name` field containing a non-empty string identifying the circuit.

r[doc.description]
A top-level `description` field MAY be present. If present it MUST be a human-readable string. It has no semantic effect.

r[doc.components]
A top-level `components` field MUST be present and MUST be a list. It MAY be empty. It contains all component and port instances in the circuit.

r[doc.subcircuits]
A top-level `subcircuits` field MAY be present. If present it MUST be a list of subcircuit definitions. If absent it is treated as an empty list.

---

# Nets

r[net.implicit]
Nets are implicit. A net is created automatically when its name is first referenced by any pin or port `net` field. No explicit net declaration is required or supported at the top level.

r[net.name]
A net name MUST be a non-empty string. Net names are case-sensitive and unique within their containing circuit or subcircuit scope.

r[net.reserved]
The names `gnd`, `0`, `vdd`, `vcc`, and `vss` are reserved by convention as common supply and ground rails. They carry no special electrical behavior — they are ordinary nets whose meaning is established by the designer.

---

# Domain Inference

r[domain.values]
The domain of a net MUST be one of: `analog`, `digital`, `mixed`, or `unspecified`.

r[domain.inferred]
Net domain is always inferred from connectivity. It MUST NOT be declared directly on a net.

r[domain.inference.analog]
A net whose connected pins all belong to analog primitives MUST be inferred as `analog`.

r[domain.inference.digital]
A net whose connected pins all belong to digital primitives MUST be inferred as `digital`.

r[domain.inference.mixed]
A net connected to both analog and digital primitive pins MUST be inferred as `mixed`.

r[domain.inference.cell]
A net connected only to `cell` pins inherits the domain declared on those pins in the model library. If no model library entry exists for the cell, the net domain is `unspecified`.

r[domain.inference.unspecified]
If domain cannot be determined by any of the above rules, the net domain is `unspecified`.

r[domain.override]
A `port` component MAY include an explicit `domain` field to override the inferred domain of its net. This is the only location where domain may be manually specified.

## Primitive Domain Classification

r[domain.primitives.analog]
The following primitive types are classified as **analog**: `resistor`, `capacitor`, `inductor`, `transformer`, `crystal`, `diode`, `zener`, `led`, `schottky`, `npn`, `pnp`, `nmos`, `pmos`, `njfet`, `pjfet`, `vsource`, `isource`, `vref`.

r[domain.primitives.digital]
The following primitive types are classified as **digital**: `and`, `or`, `not`, `nand`, `nor`, `xor`, `xnor`, `buf`, `dff`, `dff_sr`, `mux2`, `latch`.

r[domain.primitives.cell]
Components with `type: cell` are not classified as analog or digital. Their pin domains are resolved via a model library.

---

# Components

r[component.id]
Every component MUST have an `id` field. The `id` MUST be a non-empty string, unique within the containing circuit or subcircuit scope.

r[component.type]
Every component MUST have a `type` field referencing a primitive type, the name of a subcircuit defined in `subcircuits`, or one of the special types `port` and `cell`.

r[component.description]
A component MAY include a `description` string field. It has no semantic effect.

r[component.model]
A component MAY include a `model` string field referencing an external manufacturer part number or model name. It is opaque to the format and resolved externally.

r[component.tags]
A component MAY include a `tags` field containing a list of strings. Tags have no semantic effect and are reserved for tooling use.

## Pins

r[component.pins]
All non-port components MUST include a `pins` field mapping pin names to net names. Both keys and values MUST be non-empty strings.

r[component.pins.names]
Pin names for primitive types are defined in the primitive type tables in this specification. For subcircuit instances, pin names MUST match the `id` fields of the subcircuit's `port` components.

r[component.pins.net-creation]
Each net name referenced in a `pins` field implicitly creates that net in the current scope if it does not already exist.

## Value

r[component.value]
Passive components (`resistor`, `capacitor`, `inductor`, `crystal`, `vref`) SHOULD include a `value` field. `vsource` and `isource` MAY include a `value` field. Other primitive types MUST NOT include a `value` field.

r[component.value.format]
The `value` field MUST be either a number or a string. When a number, it is used directly. When a string, it MUST be parseable as a decimal number optionally followed by an SI prefix suffix, or as scientific notation (e.g. `"1e-9"`). The SI prefix suffixes are: `a` (10⁻¹⁸), `f` (10⁻¹⁵), `p` (10⁻¹²), `n` (10⁻⁹), `u` (10⁻⁶), `m` (10⁻³), `k` (10³), `meg` (10⁶), `G` (10⁹), `t` (10¹²). Prefix matching is case-insensitive except that `m` always means milli (10⁻³); use `meg` for mega (10⁶). The SPICE extension `mil` (25.4x10⁻⁶) is also accepted. Unit symbols MUST NOT be included. Parsers MUST accept both forms and treat them equivalently (e.g. `10000`, `"10k"`, and `"1e4"` all represent the same value).

## Params

r[component.params]
A component MAY include a `params` field containing a key-value map of additional type-specific parameters. Both keys and values MUST be strings.

---

# Port Components

r[port.type]
A component with `type: port` defines an external connection point on the circuit or subcircuit. It is the only mechanism for declaring a circuit's interface.

r[port.net]
A port component MUST include a `net` field containing the name of the internal net it exposes. This implicitly creates that net if it does not already exist.

r[port.direction]
A port component MUST include a `direction` field with one of the values: `input`, `output`, `inout`, or `passive`.

r[port.domain.override]
A port component MAY include a `domain` field to override the inferred domain of its net. When present, the value MUST be one of: `analog`, `digital`, `mixed`.

r[port.no-pins]
Port components MUST NOT include a `pins` field.

r[port.id-as-name]
The `id` of a port component serves as the external port name. When the circuit is instantiated as a subcircuit, the parent circuit uses the port `id` as the pin name.

---

# Primitive Pin Definitions

## Passive Primitives

r[prim.resistor]
`resistor` has pins: `p` (positive), `n` (negative).

r[prim.capacitor]
`capacitor` has pins: `p` (positive), `n` (negative).

r[prim.inductor]
`inductor` has pins: `p`, `n`.

r[prim.transformer]
`transformer` has pins: `p1`, `n1` (primary), `p2`, `n2` (secondary). The turns ratio MUST be specified in `params.ratio`.

r[prim.crystal]
`crystal` has pins: `p`, `n`. The `value` field specifies resonant frequency in Hz.

## Diodes

r[prim.diode]
`diode` has pins: `a` (anode), `k` (cathode).

r[prim.zener]
`zener` has pins: `a`, `k`.

r[prim.led]
`led` has pins: `a`, `k`.

r[prim.schottky]
`schottky` has pins: `a`, `k`.

## Transistors

r[prim.npn]
`npn` has pins: `b` (base), `c` (collector), `e` (emitter).

r[prim.pnp]
`pnp` has pins: `b`, `c`, `e`.

r[prim.nmos]
`nmos` has pins: `g` (gate), `d` (drain), `s` (source). Pin `b` (bulk) is optional; if omitted, the bulk is implicitly connected to the same net as the source pin (`s`).

r[prim.pmos]
`pmos` has pins: `g` (gate), `d` (drain), `s` (source). Pin `b` (bulk) is optional; if omitted, the bulk is implicitly connected to the same net as the source pin (`s`).

r[prim.njfet]
`njfet` has pins: `g`, `d`, `s`.

r[prim.pjfet]
`pjfet` has pins: `g`, `d`, `s`.

## Sources

r[prim.vsource]
`vsource` has pins: `p`, `n`. It is a structural placeholder representing an ideal voltage source. Waveform and stimulus definitions are out of scope for this format.

r[prim.isource]
`isource` has pins: `p`, `n`. Structural placeholder for an ideal current source.

r[prim.vref]
`vref` has pins: `p`, `n`. Semantic alias for a fixed voltage reference. The `value` field specifies voltage.

## Digital Primitives

r[prim.and]
`and` has pins: `a`, `b`, `y`. Multi-input variants use additional inputs `c`, `d`, etc.

r[prim.or]
`or` has pins: `a`, `b`, `y`.

r[prim.not]
`not` has pins: `a`, `y`.

r[prim.nand]
`nand` has pins: `a`, `b`, `y`.

r[prim.nor]
`nor` has pins: `a`, `b`, `y`.

r[prim.xor]
`xor` has pins: `a`, `b`, `y`.

r[prim.xnor]
`xnor` has pins: `a`, `b`, `y`.

r[prim.buf]
`buf` has pins: `a`, `y`.

r[prim.dff]
`dff` has pins: `d`, `clk`, `q`. Pin `qn` (inverted output) is optional.

r[prim.dff-sr]
`dff_sr` has pins: `d`, `clk`, `s`, `r`, `q`. Pin `qn` is optional.

r[prim.mux2]
`mux2` has pins: `a`, `b`, `s` (select), `y`.

r[prim.latch]
`latch` has pins: `d`, `en`, `q`.

## Cell (Black Box)

r[prim.cell]
A component with `type: cell` represents a black-box component whose internal structure is not described in the circuit file. It MUST include a `model` field. Its `pins` field may use any pin names; pin domains are resolved via an external model library.

---

# Subcircuits

r[subckt.name]
Every subcircuit MUST have a `name` field containing a non-empty string. Subcircuit names MUST be unique within the `subcircuits` list of a document.

r[subckt.components]
Every subcircuit MUST have a `components` field structured identically to the top-level `components` field.

r[subckt.interface]
A subcircuit's external interface is defined exclusively by its `port` components. A subcircuit with no `port` components has no external interface.

r[subckt.instantiation]
A subcircuit is instantiated by creating a component whose `type` matches the subcircuit's `name`. The `pins` field of the instance MUST map the subcircuit's port `id` values to nets in the parent scope.

r[subckt.instantiation.pins-complete]
All ports of a subcircuit MUST be connected when instantiated. Omitting a port from the `pins` map of an instance is an error.

r[subckt.scope]
Net names inside a subcircuit are scoped to that subcircuit. They do not conflict with net names in the parent circuit or other subcircuits.

r[subckt.description]
A subcircuit MAY include a `description` field. It has no semantic effect.

---

# File Conventions

r[file.ext.yaml]
CirQ documents serialized as YAML SHOULD use the extension `.circuit.yaml`.

r[file.ext.json]
CirQ documents serialized as JSON SHOULD use the extension `.circuit.json`.

r[file.ext.lib]
Subcircuit library files SHOULD use the extension `.lib.yaml` or `.lib.json`.

r[file.ext.layout]
Layout sidecar files MUST use the extension `.layout.json` and MUST be named after their corresponding circuit file (e.g. `foo.circuit.yaml` → `foo.layout.json`).

---

# Layout Sidecar

r[layout.optional]
The layout sidecar is entirely optional. A circuit file is fully valid without a corresponding layout file.

r[layout.not-handwritten]
The layout sidecar is intended for GUI tool consumption and generation only. It is not designed for hand-editing.

r[layout.version]
The layout sidecar MUST include a top-level `cirq_layout` field containing the layout format version as a string.

r[layout.circuit]
The layout sidecar MUST include a `circuit` field containing the `name` of the circuit it corresponds to.

## Component Placement

r[layout.component.keys]
The `components` field of the layout sidecar is an object whose keys are component `id` values from the circuit file.

r[layout.component.position]
Each component entry MUST include integer `x` and `y` fields defining the position of the component's origin in canvas units.

r[layout.component.rotation]
Each component entry MUST include a `rotation` field. The value MUST be one of `0`, `90`, `180`, or `270`, representing clockwise rotation in degrees.

r[layout.component.mirror]
Each component entry MUST include a `mirror` boolean field. When `true`, the component is horizontally flipped before rotation is applied.

## Wire Routing

r[layout.wire.per-net]
Wires are stored in a `nets` object whose keys are net names.

r[layout.wire.segments]
Each net entry MUST include a `segments` list. Each segment MUST be a four-element array `[x1, y1, x2, y2]` of integers.

r[layout.wire.manhattan]
All wire segments MUST be axis-aligned (Manhattan routing). A segment is axis-aligned if `x1 == x2` or `y1 == y2`. Diagonal segments are invalid.

r[layout.wire.junctions]
Junction dots at T-intersections MUST be inferred by the renderer from segment geometry. They MUST NOT be stored explicitly in the sidecar.

r[layout.wire.ordering]
Segments within a net MAY appear in any order. Renderers MUST NOT assume contiguity or ordering.

## Viewport

r[layout.viewport]
The layout sidecar MAY include a `viewport` object. If present it MUST contain `x`, `y` (numbers), and `zoom` (positive number) fields representing the last saved camera state.

---

# Examples

## RC Low-Pass Filter

```yaml
cirq: "0.3"
name: rc_lowpass
description: "First-order RC low-pass filter, fc ≈ 1.6kHz"

components:
  - id: IN
    type: port
    direction: input
    net: vin

  - id: OUT
    type: port
    direction: output
    net: vout

  - id: GND
    type: port
    direction: passive
    net: gnd

  - id: R1
    type: resistor
    value: "10k"
    pins: { p: vin, n: vout }

  - id: C1
    type: capacitor
    value: "10n"
    pins: { p: vout, n: gnd }
```

## AC to DC Rectifier

```yaml
cirq: "0.3"
name: ac_dc_rectifier
description: "Full-wave bridge rectifier with capacitor filter."

components:
  - id: AC_P
    type: port
    direction: input
    net: ac_p

  - id: AC_N
    type: port
    direction: input
    net: ac_n

  - id: DC_OUT
    type: port
    direction: output
    net: dc_out

  - id: GND
    type: port
    direction: passive
    net: gnd

  - id: D1
    type: diode
    description: "Positive half cycle, AC_P path"
    pins: { a: ac_p, k: dc_out }

  - id: D2
    type: diode
    description: "Positive half cycle, AC_N path"
    pins: { a: ac_n, k: dc_out }

  - id: D3
    type: diode
    description: "Return path, AC_P side"
    pins: { a: gnd, k: ac_p }

  - id: D4
    type: diode
    description: "Return path, AC_N side"
    pins: { a: gnd, k: ac_n }

  - id: C1
    type: capacitor
    value: "1000u"
    description: "Bulk filter capacitor"
    pins: { p: dc_out, n: gnd }

  - id: R1
    type: resistor
    value: "100k"
    description: "Capacitor bleed resistor"
    pins: { p: dc_out, n: gnd }
```

## Mixed-Signal Comparator

```yaml
cirq: "0.3"
name: analog_comparator
description: "Analog threshold comparator with digital output"

components:
  - id: IN
    type: port
    direction: input
    net: vin

  - id: OUT
    type: port
    direction: output
    net: dout
    domain: digital     # override: cell output is ambiguous without model library

  - id: VDD
    type: port
    direction: passive
    net: vdd

  - id: GND
    type: port
    direction: passive
    net: gnd

  - id: U1
    type: cell
    model: "LM393"
    description: "Open-collector comparator"
    pins:
      IN_P: vin
      IN_N: vref
      OUT:  cmp_raw
      VCC:  vdd
      GND:  gnd

  - id: R1
    type: resistor
    value: "10k"
    description: "Pull-up"
    pins: { p: vdd, n: cmp_raw }

  - id: R2
    type: resistor
    value: "10k"
    description: "Voltage divider top"
    pins: { p: vdd, n: vref }

  - id: R3
    type: resistor
    value: "10k"
    description: "Voltage divider bottom"
    pins: { p: vref, n: gnd }

  - id: BUF1
    type: buf
    description: "Re-drive to clean digital output"
    pins: { a: cmp_raw, y: dout }
```

---

## What This Format Intentionally Omits

- **Simulation commands** — use a separate sim config format
- **Waveform / stimulus definitions** — same
- **Component layout geometry** — in the sidecar `.layout.json`
- **Timing constraints** — out of scope
- **Technology / PDK parameters** — reference via `model:` string, resolved externally
- **Bus / vector nets** — not in v0.3; treat multi-bit nets as individual named nets
- **Model libraries** — pin domain declarations for `cell` types; planned for a future version

---

*CirQ is an open schema. No tooling required to read or write it — any YAML/JSON library works.*
