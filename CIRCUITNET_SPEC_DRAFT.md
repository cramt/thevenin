# CirQ Format Specification
**Version 0.3 — Draft**

A human- and machine-friendly schema for describing electronic circuits with full structural
compatibility with SPICE netlists. Serialization-agnostic: use JSON, YAML, TOML, or anything
that maps to the same data model.

Every structural element of a SPICE netlist has a lossless round-trip representation in CirQ.
Simulation commands (`.OP`, `.TRAN`, `.AC`, etc.) are explicitly out of scope — they belong in
a separate simulation configuration.

---

# Format Version

r[format.version]
Every CirQ document MUST include a top-level `cirq` field containing the format version as
a string (e.g. `"0.3"`). Documents without this field are invalid.

r[format.version.semver]
The version string MUST follow semantic versioning. Parsers SHOULD reject documents whose
major version is higher than the parser's supported major version.

---

# Top-Level Structure

r[doc.name]
Every CirQ document MUST include a top-level `name` field containing a non-empty string
identifying the circuit. This corresponds to the SPICE title line.

r[doc.description]
A top-level `description` field MAY be present. If present it MUST be a human-readable string.
It has no semantic effect.

r[doc.components]
A top-level `components` field MUST be present and MUST be a list. It MAY be empty. It contains
all component and port instances in the circuit.

r[doc.subcircuits]
A top-level `subcircuits` field MAY be present. If present it MUST be a list of subcircuit
definitions. If absent it is treated as an empty list. Corresponds to SPICE `.SUBCKT` blocks.

r[doc.models]
A top-level `models` field MAY be present. If present it MUST be a list of model definitions.
Corresponds to SPICE `.MODEL` statements.

r[doc.params]
A top-level `params` field MAY be present. If present it MUST be a key-value map of global
parameter definitions. Corresponds to SPICE `.PARAM` statements. Values MUST be numbers or
strings (strings may contain expressions).

r[doc.globals]
A top-level `globals` field MAY be present. If present it MUST be a list of net name strings
that are visible across all subcircuit scopes. Corresponds to SPICE `.GLOBAL`.

r[doc.includes]
A top-level `includes` field MAY be present. If present it MUST be a list of include objects.
Corresponds to SPICE `.INCLUDE` and `.LIB` directives.

r[doc.functions]
A top-level `functions` field MAY be present. If present it MUST be a list of function
definition objects. Corresponds to SPICE `.FUNC` statements.

r[doc.options]
A top-level `options` field MAY be present. If present it MUST be a key-value map. Corresponds
to SPICE `.OPTIONS`. Values are strings or numbers.

r[doc.temperature]
A top-level `temperature` field MAY be present. If present it MUST be a number specifying the
circuit temperature in degrees Celsius. Corresponds to SPICE `.TEMP`.

---

# Nets

r[net.implicit]
Nets are implicit. A net is created automatically when its name is first referenced by any
pin or port `net` field. No explicit net declaration is required or supported.

r[net.name]
A net name MUST be a non-empty string. Net names are case-sensitive and unique within their
containing circuit or subcircuit scope.

r[net.ground]
The net name `"0"` is the global reference ground node, matching SPICE convention. It is
implicitly present in every scope.

r[net.reserved]
The names `gnd`, `vdd`, `vcc`, and `vss` are reserved by convention as common supply and
ground rails. They carry no special electrical behavior — they are ordinary nets whose meaning
is established by the designer.

---

# Components

r[component.id]
Every component MUST have an `id` field. The `id` MUST be a non-empty string, unique within
the containing circuit or subcircuit scope. Corresponds to the SPICE instance name (e.g. `R1`,
`M3`, `X1`).

r[component.type]
Every component MUST have a `type` field referencing one of: a primitive type name, the name
of a subcircuit defined in `subcircuits`, or one of the special types `port` and `cell`.

r[component.description]
A component MAY include a `description` string field. It has no semantic effect.

r[component.model]
A component MAY include a `model` string field referencing a model name defined in `models`
or resolved externally. Corresponds to the SPICE model name parameter on elements like
`D1 a k MyDiodeModel`.

r[component.tags]
A component MAY include a `tags` field containing a list of strings. Tags have no semantic
effect and are reserved for tooling use.

## Pins

r[component.pins]
All non-port components MUST include a `pins` field. For primitive types, this is an object
mapping pin names to net names. For subcircuit instances, this is an ordered list of net names
matching the subcircuit's port order (preserving SPICE positional semantics), or an object
mapping port names to net names.

r[component.pins.names]
Pin names for primitive types are defined in the primitive type tables below. For subcircuit
instances using object form, pin names MUST match the `id` fields of the subcircuit's `port`
components.

r[component.pins.net-creation]
Each net name referenced in a `pins` field implicitly creates that net in the current scope
if it does not already exist.

## Value

r[component.value]
Passive components (`resistor`, `capacitor`, `inductor`, `crystal`, `vref`) SHOULD include a
`value` field. `vsource` and `isource` MAY include a `value` field. Other primitive types
MUST NOT include a `value` field.

r[component.value.format]
The `value` field MUST be either a number or a string. When a number, it is used directly.
When a string, it MUST be parseable as a decimal number optionally followed by an SI prefix
character, or as scientific notation (e.g. `"1e-9"`). The SI prefix characters are:
`a` (10⁻¹⁸), `f` (10⁻¹⁵), `p` (10⁻¹²), `n` (10⁻⁹), `u` (10⁻⁶), `m` (10⁻³), `k` (10³),
`meg` (10⁶), `G` (10⁹), `T` (10¹²). Prefix matching is case-insensitive. The SPICE-specific
suffix `mil` (25.4×10⁻⁶) is also accepted. Unit symbols MUST NOT be included.

**Note:** Following SPICE convention, `m` means milli (10⁻³), NOT mega. Use `meg` for 10⁶.
Parsers MUST check for `meg` before matching `m`.

## Params

r[component.params]
A component MAY include a `params` field containing a key-value map of instance parameters.
Values MUST be numbers, strings, or expression strings (enclosed in braces `{}`). Corresponds
to SPICE instance parameters like `W=10u L=0.5u M=2`.

## Initial Conditions

r[component.ic]
A component MAY include an `ic` field specifying initial conditions. For two-terminal
components (capacitors, inductors), this is a single number. For transmission lines, this is
a list of numbers `[v1, i1, v2, i2]`. Corresponds to SPICE `IC=` syntax.

## Off Flag

r[component.off]
A component MAY include an `off` boolean field. When `true`, the device starts in the off
state for DC operating point computation. Corresponds to the SPICE `OFF` keyword on transistors.

---

# Port Components

r[port.type]
A component with `type: port` defines an external connection point on the circuit or
subcircuit. It is the only mechanism for declaring interface pins.

r[port.net]
A port component MUST include a `net` field containing the name of the internal net it exposes.

r[port.direction]
A port component MUST include a `direction` field with one of the values: `input`, `output`,
`inout`, or `passive`.

r[port.order]
A port component MAY include an `order` integer field specifying its positional index in the
subcircuit interface. This is REQUIRED for SPICE round-tripping since SPICE subcircuit ports
are positional. If omitted, order is determined by declaration order in the `components` list.

r[port.no-pins]
Port components MUST NOT include a `pins` field.

r[port.id-as-name]
The `id` of a port component serves as the external port name.

---

# Primitive Pin Definitions

## Passive Primitives

r[prim.resistor]
`resistor` has pins: `p` (positive), `n` (negative).
SPICE: `R<name> <p> <n> [model] <value> [params]`

r[prim.capacitor]
`capacitor` has pins: `p` (positive), `n` (negative).
SPICE: `C<name> <p> <n> [model] <value> [params]`

r[prim.inductor]
`inductor` has pins: `p`, `n`.
SPICE: `L<name> <p> <n> [model] <value> [params]`

r[prim.crystal]
`crystal` has pins: `p`, `n`. The `value` field specifies resonant frequency in Hz.

## Coupled Inductors

r[prim.coupling]
`coupling` specifies mutual inductance between two inductors. It has no pins — instead it
references two inductor component IDs and a coupling coefficient.
Required fields: `inductors` (list of exactly two component `id` strings), `coefficient` (number between 0 and 1).
SPICE: `K<name> <L1> <L2> <coefficient>`

## Diodes

r[prim.diode]
`diode` has pins: `a` (anode), `k` (cathode). MUST include `model`.
SPICE: `D<name> <a> <k> <model> [params]`

r[prim.zener]
`zener` has pins: `a`, `k`. Semantic alias — uses `diode` in SPICE.

r[prim.led]
`led` has pins: `a`, `k`. Semantic alias — uses `diode` in SPICE.

r[prim.schottky]
`schottky` has pins: `a`, `k`. Semantic alias — uses `diode` in SPICE.

## Transistors

r[prim.npn]
`npn` has pins: `c` (collector), `b` (base), `e` (emitter). Pin `s` (substrate) is optional.
MUST include `model`.
SPICE: `Q<name> <c> <b> <e> [<s>] <model> [params]`

r[prim.pnp]
`pnp` has pins: `c`, `b`, `e`. Pin `s` is optional. MUST include `model`.
SPICE: `Q<name> <c> <b> <e> [<s>] <model> [params]`

r[prim.nmos]
`nmos` has pins: `d` (drain), `g` (gate), `s` (source), `b` (bulk). Pin `body` is optional
(SOI 5-terminal). MUST include `model`.
SPICE: `M<name> <d> <g> <s> <b> [<body>] <model> [params]`

r[prim.pmos]
`pmos` has pins: `d`, `g`, `s`, `b`. Pin `body` is optional. MUST include `model`.
SPICE: `M<name> <d> <g> <s> <b> [<body>] <model> [params]`

r[prim.njfet]
`njfet` has pins: `d` (drain), `g` (gate), `s` (source). MUST include `model`.
SPICE: `J<name> <d> <g> <s> <model> [params]`

r[prim.pjfet]
`pjfet` has pins: `d`, `g`, `s`. MUST include `model`.
SPICE: `J<name> <d> <g> <s> <model> [params]`

r[prim.mesfet]
`mesfet` has pins: `d`, `g`, `s`. MUST include `model`.
SPICE: `Z<name> <d> <g> <s> <model> [params]`

## Sources

r[prim.vsource]
`vsource` has pins: `p` (positive), `n` (negative).
The `value` field specifies DC value. AC, waveform, and stimulus specifications are stored
in the `params` field (see Source Parameters below).
SPICE: `V<name> <p> <n> [DC <val>] [AC <mag> [<phase>]] [<waveform>]`

r[prim.isource]
`isource` has pins: `p`, `n`. Same value/params semantics as `vsource`.
SPICE: `I<name> <p> <n> [DC <val>] [AC <mag> [<phase>]] [<waveform>]`

r[prim.vref]
`vref` has pins: `p`, `n`. Semantic alias for a fixed voltage reference.

### Source Parameters

r[source.dc]
The DC value of a source is specified in the `value` field or `params.dc`.

r[source.ac]
AC analysis magnitude and phase are specified as `params.ac_mag` and `params.ac_phase`
(both numbers). Phase defaults to 0 if omitted.

r[source.waveform]
Transient waveform is specified in the `waveform` field, which is an object with a `type`
field and type-specific parameters:

| Type      | Parameters                                     |
|-----------|-------------------------------------------------|
| `pulse`   | `v1`, `v2`, `td`, `tr`, `tf`, `pw`, `per`       |
| `sin`     | `v0`, `va`, `freq`, `td`, `theta`, `phi`         |
| `exp`     | `v1`, `v2`, `td1`, `tau1`, `td2`, `tau2`         |
| `pwl`     | `points` (list of `[time, value]` pairs)         |
| `sffm`    | `v0`, `va`, `fc`, `fs`, `md`                     |
| `am`      | `va`, `vo`, `fc`, `fs`, `td`                     |

All waveform parameters are numbers. Omitted parameters use SPICE defaults (typically 0).

## Controlled Sources

r[prim.vcvs]
`vcvs` (Voltage-Controlled Voltage Source) has pins: `p` (out+), `n` (out-),
`cp` (control+), `cn` (control-). Requires `params.gain` (number).
SPICE: `E<name> <p> <n> <cp> <cn> <gain>`

r[prim.vccs]
`vccs` (Voltage-Controlled Current Source) has pins: `p`, `n`, `cp`, `cn`.
Requires `params.gm` (transconductance, number).
SPICE: `G<name> <p> <n> <cp> <cn> <gm>`

r[prim.cccs]
`cccs` (Current-Controlled Current Source) has pins: `p`, `n`.
Requires `params.vsource` (id of a zero-volt vsource used as ammeter) and `params.gain`.
SPICE: `F<name> <p> <n> <vsource> <gain>`

r[prim.ccvs]
`ccvs` (Current-Controlled Voltage Source) has pins: `p`, `n`.
Requires `params.vsource` and `params.transresistance`.
SPICE: `H<name> <p> <n> <vsource> <transresistance>`

## Behavioral Sources

r[prim.bsource]
`bsource` (Behavioral Source) has pins: `p`, `n`.
MUST include either `params.v` (voltage expression) or `params.i` (current expression).
Expressions are strings using SPICE expression syntax (node voltages as `V(net)`, currents
as `I(vsource)`, math operators, and built-in functions).
SPICE: `B<name> <p> <n> V={expr}` or `B<name> <p> <n> I={expr}`

## Switches

r[prim.vswitch]
`vswitch` (Voltage-Controlled Switch) has pins: `p`, `n`, `cp` (control+), `cn` (control-).
MUST include `model`.
SPICE: `S<name> <p> <n> <cp> <cn> <model> [params]`

r[prim.iswitch]
`iswitch` (Current-Controlled Switch) has pins: `p`, `n`.
MUST include `model` and `params.vsource` (sensing element).
SPICE: `W<name> <p> <n> <vsource> <model> [params]`

## Transmission Lines

r[prim.tline]
`tline` (Ideal Transmission Line) has pins: `p1`, `n1` (port 1), `p2`, `n2` (port 2).
Parameters in `params`: `z0` (impedance), `td` (delay) or `freq`+`nl` (frequency/wavelengths).
SPICE: `T<name> <p1> <n1> <p2> <n2> [params]`

r[prim.ltra]
`ltra` (Lossy Transmission Line) has pins: `p1`, `n1`, `p2`, `n2`. MUST include `model`.
SPICE: `O<name> <p1> <n1> <p2> <n2> <model> [params]`

r[prim.txl]
`txl` (Single-Conductor Transmission Line) has pins: `in`, `out`, `gnd_in`, `gnd_out`.
MUST include `model`.
SPICE: `Y<name> <in> <gnd_in> <out> <gnd_out> <model> [params]`

## XSPICE Code Models

r[prim.xspice]
`xspice` represents an XSPICE code model instance. MUST include `model`.
Pins are specified as an ordered list of connections. Each connection is either a string
(scalar net name) or a list of strings (vector port).
SPICE: `A<name> <connections...> <model>`

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
A component with `type: cell` represents a black-box component whose internal structure is
not described in the circuit file. It MUST include a `model` field. Its `pins` field may use
any pin names.

---

# Models

r[model.name]
Every model MUST have a `name` field containing a unique non-empty string.

r[model.type]
Every model MUST have a `type` field. Valid values: `diode`, `npn`, `pnp`, `nmos`, `pmos`,
`njfet`, `pjfet`, `mesfet`, `ltra`, `txl`, `cpl`, `vswitch`, `iswitch`, or any XSPICE
code model type.

r[model.level]
A model MAY include a `level` integer field to select among model variants (e.g. MOSFET
level 1 vs 49 vs 54). Corresponds to the SPICE `LEVEL=` parameter.

r[model.params]
A model MUST include a `params` field containing a key-value map of model parameters.
Values MUST be numbers, strings, or expression strings. Parameter names are
case-insensitive by convention.

```yaml
# SPICE: .MODEL N1 NPN LEVEL=4 IS=1e-16 BF=100 RB=10
models:
  - name: N1
    type: npn
    level: 4
    params:
      is: 1e-16
      bf: 100
      rb: 10
```

---

# Subcircuits

r[subckt.name]
Every subcircuit MUST have a `name` field containing a non-empty string. Subcircuit names
MUST be unique within the `subcircuits` list.

r[subckt.components]
Every subcircuit MUST have a `components` field structured identically to the top-level
`components` field.

r[subckt.interface]
A subcircuit's external interface is defined by its `port` components. Ports MUST have
`order` fields for deterministic SPICE round-tripping.

r[subckt.params]
A subcircuit MAY include a `params` field with default parameter values. These can be
overridden by the instantiating component's `params`. Corresponds to SPICE
`.SUBCKT name ports PARAMS: key=default`.

r[subckt.scope]
Net names inside a subcircuit are scoped to that subcircuit. They do not conflict with net
names in the parent circuit or other subcircuits. Global nets (declared in `globals`) are
the exception.

r[subckt.instantiation]
A subcircuit is instantiated by creating a component whose `type` matches the subcircuit's
`name`. Corresponds to SPICE `X<name> <ports...> <subckt>`.

r[subckt.instantiation.pins-complete]
All ports of a subcircuit MUST be connected when instantiated.

r[subckt.description]
A subcircuit MAY include a `description` field.

---

# Includes

r[include.file]
An include object MUST have a `file` field containing a file path string.

r[include.section]
An include object MAY have a `section` field specifying a library section name.
When present, only that section is included. Corresponds to SPICE `.LIB "file" section`.
When absent, the entire file is included. Corresponds to SPICE `.INCLUDE "file"`.

```yaml
includes:
  - file: "models/cmos.lib"
    section: "tt"          # .LIB "models/cmos.lib" tt
  - file: "passives.lib"   # .INCLUDE "passives.lib"
```

---

# Functions

r[func.name]
Every function MUST have a `name` field.

r[func.args]
Every function MUST have an `args` field containing a list of argument name strings.

r[func.body]
Every function MUST have a `body` field containing the expression string.

```yaml
functions:
  - name: parallel
    args: [r1, r2]
    body: "{(r1 * r2) / (r1 + r2)}"
```

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
A net connected only to `cell` pins inherits the domain declared on those pins in the model
library. If no model library entry exists, the net domain is `unspecified`.

r[domain.inference.unspecified]
If domain cannot be determined by any of the above rules, the net domain is `unspecified`.

r[domain.override]
A `port` component MAY include an explicit `domain` field to override the inferred domain
of its net.

## Primitive Domain Classification

r[domain.primitives.analog]
**Analog** primitives: `resistor`, `capacitor`, `inductor`, `coupling`, `crystal`, `diode`,
`zener`, `led`, `schottky`, `npn`, `pnp`, `nmos`, `pmos`, `njfet`, `pjfet`, `mesfet`,
`vsource`, `isource`, `vref`, `vcvs`, `vccs`, `cccs`, `ccvs`, `bsource`, `vswitch`,
`iswitch`, `tline`, `ltra`, `txl`.

r[domain.primitives.digital]
**Digital** primitives: `and`, `or`, `not`, `nand`, `nor`, `xor`, `xnor`, `buf`, `dff`,
`dff_sr`, `mux2`, `latch`.

r[domain.primitives.cell]
Components with `type: cell` or `type: xspice` are not classified as analog or digital.

---

# SPICE Compatibility

## Element Type Mapping

| CirQ type    | SPICE prefix | Notes                                    |
|--------------|-------------|------------------------------------------|
| `resistor`   | `R`         |                                          |
| `capacitor`  | `C`         |                                          |
| `inductor`   | `L`         |                                          |
| `coupling`   | `K`         | Not a physical component — links inductors |
| `diode`      | `D`         |                                          |
| `npn`        | `Q`         | Distinguished by model type              |
| `pnp`        | `Q`         | Distinguished by model type              |
| `nmos`       | `M`         | Distinguished by model type              |
| `pmos`       | `M`         | Distinguished by model type              |
| `njfet`      | `J`         | Distinguished by model type              |
| `pjfet`      | `J`         | Distinguished by model type              |
| `mesfet`     | `Z`         |                                          |
| `vsource`    | `V`         |                                          |
| `isource`    | `I`         |                                          |
| `vcvs`       | `E`         |                                          |
| `vccs`       | `G`         |                                          |
| `cccs`       | `F`         |                                          |
| `ccvs`       | `H`         |                                          |
| `bsource`    | `B`         |                                          |
| `vswitch`    | `S`         |                                          |
| `iswitch`    | `W`         |                                          |
| `tline`      | `T`         |                                          |
| `ltra`       | `O`         |                                          |
| `txl`        | `Y`         |                                          |
| `xspice`     | `A`         |                                          |
| (subcircuit) | `X`         | `type` matches subcircuit name           |

## Pin Order Mapping

When converting to SPICE, pins are emitted in the canonical order defined by each primitive's
pin table. When converting from SPICE, positional nodes are mapped to named pins in order.

| CirQ type   | SPICE pin order                          |
|-------------|------------------------------------------|
| `resistor`  | `p n`                                    |
| `capacitor` | `p n`                                    |
| `inductor`  | `p n`                                    |
| `diode`     | `a k`                                    |
| `npn`/`pnp` | `c b e [s]`                             |
| `nmos`/`pmos`| `d g s b [body]`                        |
| `njfet`/`pjfet`| `d g s`                               |
| `mesfet`    | `d g s`                                  |
| `vsource`/`isource` | `p n`                           |
| `vcvs`/`vccs` | `p n cp cn`                           |
| `cccs`/`ccvs` | `p n`                                  |
| `vswitch`   | `p n cp cn`                              |
| `iswitch`   | `p n`                                    |
| `tline`/`ltra` | `p1 n1 p2 n2`                        |

---

# File Conventions

r[file.ext.yaml]
CirQ documents serialized as YAML SHOULD use the extension `.cirq.yaml`.

r[file.ext.json]
CirQ documents serialized as JSON SHOULD use the extension `.cirq.json`.

r[file.ext.toml]
CirQ documents serialized as TOML SHOULD use the extension `.cirq.toml`.

r[file.ext.lib]
Library files SHOULD use the extension `.lib.yaml` or `.lib.json`.

r[file.ext.layout]
Layout sidecar files MUST use the extension `.layout.json` and MUST be named after their
corresponding circuit file (e.g. `foo.cirq.yaml` → `foo.layout.json`).

---

# Layout Sidecar

The layout sidecar contains **all** visual/GUI metadata for a circuit. It is the clean
separation point: the `.cirq.yaml` is the circuit (what an agent or text editor user cares
about), the `.layout.json` is how it looks on screen (what the GUI cares about). Deleting
the layout file and regenerating it from scratch is always a valid operation — nothing
functionally meaningful is lost.

## Core Principles

r[layout.optional]
The layout sidecar is entirely optional. A circuit file is fully valid without one. A GUI
tool SHOULD be capable of auto-generating a layout from a bare circuit file.

r[layout.disposable]
The layout sidecar is disposable. Tools MUST NOT store functionally meaningful information
in the layout. An agent modifying the circuit MAY ignore the layout entirely — the GUI is
responsible for repairing or regenerating placement after structural changes.

r[layout.not-handwritten]
The layout sidecar is intended for GUI tool consumption and generation only. It is not
designed for hand-editing and humans should never need to read it.

r[layout.authoritative-ids]
The layout file references components and nets by the `id` and net names from the circuit
file. If a component or net exists in the layout but not in the circuit, the layout entry
is stale and MUST be ignored. If a component or net exists in the circuit but not in the
layout, the GUI MUST auto-place it.

## Top-Level Structure

r[layout.version]
The layout sidecar MUST include a top-level `cirq_layout` field containing the layout format
version as a string (e.g. `"0.3"`).

r[layout.circuit]
The layout sidecar MUST include a `circuit` field containing the `name` of the circuit it
corresponds to. This is used to validate that the layout matches its circuit file.

r[layout.grid]
The layout sidecar MAY include a `grid` object with:
- `size` (integer, default 10): grid spacing in canvas units
- `subdivisions` (integer, default 1): number of minor grid divisions per major grid cell
- `snap` (boolean, default true): whether components and wires snap to grid

r[layout.viewport]
The layout sidecar MAY include a `viewport` object representing the last saved camera state:
- `x` (number): horizontal center of the viewport in canvas units
- `y` (number): vertical center of the viewport in canvas units
- `zoom` (positive number): zoom level (1.0 = 100%)

r[layout.components]
The layout sidecar MAY include a `components` object whose keys are component `id` values
from the circuit file.

r[layout.nets]
The layout sidecar MAY include a `nets` object whose keys are net names from the circuit file.

r[layout.annotations]
The layout sidecar MAY include an `annotations` list of visual-only elements (text labels,
bounding boxes, divider lines, etc.) that have no electrical meaning.

r[layout.subcircuits]
The layout sidecar MAY include a `subcircuits` object whose keys are subcircuit names. Each
entry contains a nested layout (with its own `components`, `nets`, `annotations`) for the
subcircuit's internal schematic view.

## Component Placement

r[layout.component.position]
Each component entry MUST include `x` and `y` integer fields defining the position of the
component's origin (pin 1 / anchor point) in canvas units.

r[layout.component.rotation]
Each component entry MUST include a `rotation` field. The value MUST be one of `0`, `90`,
`180`, or `270`, representing clockwise rotation in degrees.

r[layout.component.mirror]
Each component entry MUST include a `mirror` boolean field. When `true`, the component is
horizontally flipped before rotation is applied.

r[layout.component.symbol]
A component entry MAY include a `symbol` string field to override the default symbol used
for rendering. This allows the same electrical component type to be drawn with different
visual representations (e.g. US vs EU resistor symbol, different op-amp body styles).

r[layout.component.label]
A component entry MAY include a `label` object controlling how the component's ID text is
displayed:
- `visible` (boolean, default true): whether to show the label
- `x` (integer): horizontal offset from component origin
- `y` (integer): vertical offset from component origin
- `anchor` (string, default `"left"`): text anchor — `"left"`, `"center"`, or `"right"`

r[layout.component.value_label]
A component entry MAY include a `value_label` object with the same structure as `label`,
controlling display of the component's value text (e.g. "10k", "100n").

r[layout.component.color]
A component entry MAY include a `color` string field (CSS hex, e.g. `"#ff0000"`) to override
the default component color. If absent, the GUI uses its default color scheme.

## Wire Routing

r[layout.wire.per-net]
Wires are stored in the `nets` object, keyed by net name.

r[layout.wire.segments]
Each net entry MUST include a `segments` list. Each segment is a four-element integer array
`[x1, y1, x2, y2]` representing a wire from point (x1, y1) to point (x2, y2).

r[layout.wire.manhattan]
All wire segments MUST be axis-aligned (Manhattan routing). A segment is axis-aligned if
`x1 == x2` or `y1 == y2`. Diagonal segments are invalid.

r[layout.wire.junctions]
Junction dots at T-intersections and multi-way junctions MUST be inferred by the renderer
from segment geometry. They MUST NOT be stored explicitly.

r[layout.wire.ordering]
Segments within a net MAY appear in any order. Renderers MUST NOT assume contiguity or
ordering.

r[layout.wire.color]
A net entry MAY include a `color` string field (CSS hex) to override the default wire color
for all segments of that net.

r[layout.wire.style]
A net entry MAY include a `style` string field: `"solid"` (default), `"dashed"`, or
`"dotted"`.

## Net Labels

r[layout.netlabel]
A net entry MAY include a `labels` list. Each label is an object placed on the schematic to
visually identify the net at a specific location, avoiding the need to route a wire across
the entire sheet.

r[layout.netlabel.fields]
Each net label object MUST include:
- `x` (integer): position in canvas units
- `y` (integer): position in canvas units
- `rotation` (integer): `0`, `90`, `180`, or `270`

Each net label object MAY include:
- `style` (string): `"label"` (default, simple text), `"flag"` (pointed flag shape),
  `"power"` (power rail symbol like VDD bar or ground symbol)
- `power_symbol` (string): when `style` is `"power"`, specifies the symbol type —
  `"bar"` (VDD/VCC bar), `"ground"` (standard ground), `"chassis"` (chassis ground),
  `"earth"` (earth ground). If absent, the renderer picks based on the net name.

The label text is always the net name — it is not stored redundantly.

## Power Symbols

r[layout.power]
Power and ground symbols are net labels with `style: "power"`. They are purely visual
representations of a net connection. The net they refer to is determined by the net they
are listed under in the `nets` object. This means a ground symbol under the `"0"` net key
is visually a ground symbol but electrically just a connection to net `"0"`.

## Annotations

r[layout.annotation.types]
Each annotation object MUST include a `type` field. Supported types:

### Text
```json
{
  "type": "text",
  "x": 100, "y": 50,
  "text": "Bias network",
  "size": 14,
  "color": "#666666",
  "rotation": 0,
  "anchor": "left"
}
```

### Rectangle
```json
{
  "type": "rect",
  "x": 0, "y": 0,
  "width": 200, "height": 150,
  "color": "#cccccc",
  "fill": "#f8f8f8",
  "stroke_width": 1,
  "corner_radius": 0,
  "label": "Power stage"
}
```

### Line
```json
{
  "type": "line",
  "x1": 0, "y1": 100,
  "x2": 500, "y2": 100,
  "color": "#cccccc",
  "stroke_width": 1,
  "style": "dashed"
}
```

### Image
```json
{
  "type": "image",
  "x": 300, "y": 50,
  "width": 100, "height": 80,
  "href": "logo.png"
}
```

r[layout.annotation.no-semantics]
Annotations MUST NOT carry any electrical or structural meaning. They are purely visual
aids for the human reader.

r[layout.annotation.id]
Each annotation MAY include an `id` string field for stable referencing across saves.
If absent, the annotation has no stable identity.

## Subcircuit Layouts

r[layout.subcircuit.nested]
Each entry in the `subcircuits` object is a self-contained layout for that subcircuit's
internal schematic. It has the same structure as the top-level layout (minus `cirq_layout`,
`circuit`, and `grid` — it inherits those from the parent).

r[layout.subcircuit.instance-overrides]
When a subcircuit is instantiated multiple times, all instances share the same internal
layout. Per-instance visual overrides are not supported — the subcircuit's schematic looks
the same regardless of where it is instantiated.

---

# Layout Example

Given this circuit file (`inverter.cirq.yaml`):
```yaml
cirq: "0.3"
name: CMOS Inverter

models:
  - name: NMOD
    type: nmos
    level: 1
    params: { vto: 0.7, kp: 110e-6 }
  - name: PMOD
    type: pmos
    level: 1
    params: { vto: -0.7, kp: 50e-6 }

components:
  - id: M1
    type: pmos
    model: PMOD
    pins: { d: out, g: in, s: vdd, b: vdd }
    params: { w: "10u", l: "0.5u" }

  - id: M2
    type: nmos
    model: NMOD
    pins: { d: out, g: in, s: "0", b: "0" }
    params: { w: "5u", l: "0.5u" }

  - id: VDD
    type: vsource
    value: 5
    pins: { p: vdd, n: "0" }
```

The corresponding layout (`inverter.layout.json`):
```json
{
  "cirq_layout": "0.3",
  "circuit": "CMOS Inverter",
  "grid": {
    "size": 10,
    "subdivisions": 2,
    "snap": true
  },
  "viewport": {
    "x": 150,
    "y": 120,
    "zoom": 1.5
  },
  "components": {
    "M1": {
      "x": 200, "y": 60,
      "rotation": 0,
      "mirror": false,
      "label": { "visible": true, "x": 30, "y": -5, "anchor": "left" },
      "value_label": { "visible": true, "x": 30, "y": 10, "anchor": "left" }
    },
    "M2": {
      "x": 200, "y": 160,
      "rotation": 0,
      "mirror": false,
      "label": { "visible": true, "x": 30, "y": -5, "anchor": "left" },
      "value_label": { "visible": true, "x": 30, "y": 10, "anchor": "left" }
    },
    "VDD": {
      "x": 60, "y": 100,
      "rotation": 0,
      "mirror": false
    }
  },
  "nets": {
    "vdd": {
      "segments": [
        [200, 40, 200, 20],
        [60, 80, 60, 20],
        [60, 20, 200, 20]
      ],
      "labels": [
        { "x": 200, "y": 10, "rotation": 0, "style": "power", "power_symbol": "bar" }
      ]
    },
    "0": {
      "segments": [
        [200, 180, 200, 220],
        [60, 120, 60, 220],
        [60, 220, 200, 220]
      ],
      "labels": [
        { "x": 200, "y": 230, "rotation": 0, "style": "power", "power_symbol": "ground" }
      ]
    },
    "out": {
      "segments": [
        [200, 80, 200, 140],
        [200, 110, 280, 110]
      ],
      "labels": [
        { "x": 280, "y": 110, "rotation": 0, "style": "label" }
      ]
    },
    "in": {
      "segments": [
        [180, 60, 120, 60],
        [120, 60, 120, 160],
        [120, 160, 180, 160]
      ],
      "labels": [
        { "x": 110, "y": 110, "rotation": 0, "style": "label" }
      ]
    }
  },
  "annotations": [
    {
      "type": "rect",
      "x": 100, "y": 0,
      "width": 200, "height": 250,
      "color": "#e0e0e0",
      "fill": "#fafafa",
      "stroke_width": 1,
      "corner_radius": 4,
      "label": "Inverter core"
    },
    {
      "type": "text",
      "x": 150, "y": -20,
      "text": "W/L ratio: 2:1 (P:N)",
      "size": 11,
      "color": "#999999",
      "rotation": 0,
      "anchor": "center"
    }
  ]
}
```

Note how the circuit file has zero visual information, and the layout file has zero electrical
information. An agent asked to "change M1 to W=20u" only touches the `.cirq.yaml`. The GUI
re-renders from the unchanged layout — maybe the value label text updates, but that's
derived from the circuit file, not stored in the layout.

---

# Examples

## Example 1: RC Low-Pass Filter

### SPICE
```spice
RC Low-Pass Filter
R1 vin vout 10k
C1 vout 0 10n
.END
```

### CirQ
```yaml
cirq: "0.3"
name: RC Low-Pass Filter

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
    net: "0"

  - id: R1
    type: resistor
    value: "10k"
    pins: { p: vin, n: vout }

  - id: C1
    type: capacitor
    value: "10n"
    pins: { p: vout, n: "0" }
```

## Example 2: CMOS Inverter with Models

### SPICE
```spice
CMOS Inverter
.MODEL NMOD NMOS LEVEL=1 VTO=0.7 KP=110U
.MODEL PMOD PMOS LEVEL=1 VTO=-0.7 KP=50U
M1 out in vdd vdd PMOD W=10u L=0.5u
M2 out in 0 0 NMOD W=5u L=0.5u
VDD vdd 0 DC 5
.END
```

### CirQ
```yaml
cirq: "0.3"
name: CMOS Inverter

models:
  - name: NMOD
    type: nmos
    level: 1
    params: { vto: 0.7, kp: 110e-6 }
  - name: PMOD
    type: pmos
    level: 1
    params: { vto: -0.7, kp: 50e-6 }

components:
  - id: M1
    type: pmos
    model: PMOD
    pins: { d: out, g: in, s: vdd, b: vdd }
    params: { w: "10u", l: "0.5u" }

  - id: M2
    type: nmos
    model: NMOD
    pins: { d: out, g: in, s: "0", b: "0" }
    params: { w: "5u", l: "0.5u" }

  - id: VDD
    type: vsource
    value: 5
    pins: { p: vdd, n: "0" }
```

## Example 3: Pulse Source with Waveform

### SPICE
```spice
Pulse Test
V1 in 0 DC 0 PULSE(0 5 10n 1n 1n 50n 100n)
R1 in out 1k
C1 out 0 100p
.END
```

### CirQ
```yaml
cirq: "0.3"
name: Pulse Test

components:
  - id: V1
    type: vsource
    value: 0
    pins: { p: in, n: "0" }
    waveform:
      type: pulse
      v1: 0
      v2: 5
      td: 10e-9
      tr: 1e-9
      tf: 1e-9
      pw: 50e-9
      per: 100e-9

  - id: R1
    type: resistor
    value: "1k"
    pins: { p: in, n: out }

  - id: C1
    type: capacitor
    value: "100p"
    pins: { p: out, n: "0" }
```

## Example 4: Op-Amp Subcircuit

### SPICE
```spice
Op-Amp Test
.SUBCKT opamp inp inn out vcc vee
R1 inp inn 1MEG
E1 mid 0 inp inn 100k
R2 mid out 100
.ENDS opamp
X1 signal ref output vdd vss opamp
VDD vdd 0 DC 15
VSS vss 0 DC -15
.END
```

### CirQ
```yaml
cirq: "0.3"
name: Op-Amp Test

subcircuits:
  - name: opamp
    components:
      - { id: inp, type: port, direction: input,   net: inp, order: 0 }
      - { id: inn, type: port, direction: input,   net: inn, order: 1 }
      - { id: out, type: port, direction: output,  net: out, order: 2 }
      - { id: vcc, type: port, direction: passive,  net: vcc, order: 3 }
      - { id: vee, type: port, direction: passive,  net: vee, order: 4 }
      - { id: R1, type: resistor, value: "1meg", pins: { p: inp, n: inn } }
      - { id: E1, type: vcvs, pins: { p: mid, n: "0", cp: inp, cn: inn }, params: { gain: 100000 } }
      - { id: R2, type: resistor, value: 100, pins: { p: mid, n: out } }

components:
  - id: X1
    type: opamp
    pins: [signal, ref, output, vdd, vss]

  - id: VDD
    type: vsource
    value: 15
    pins: { p: vdd, n: "0" }

  - id: VSS
    type: vsource
    value: -15
    pins: { p: vss, n: "0" }
```

## Example 5: Controlled Sources & Coupling

### SPICE
```spice
Transformer and VCCS
L1 in 0 10m
L2 out 0 10m
K1 L1 L2 0.95
G1 bias 0 in 0 0.001
R1 bias 0 10k
.END
```

### CirQ
```yaml
cirq: "0.3"
name: Transformer and VCCS

components:
  - id: L1
    type: inductor
    value: "10m"
    pins: { p: in, n: "0" }

  - id: L2
    type: inductor
    value: "10m"
    pins: { p: out, n: "0" }

  - id: K1
    type: coupling
    inductors: [L1, L2]
    coefficient: 0.95

  - id: G1
    type: vccs
    pins: { p: bias, n: "0", cp: in, cn: "0" }
    params: { gm: 0.001 }

  - id: R1
    type: resistor
    value: "10k"
    pins: { p: bias, n: "0" }
```

## Example 6: Behavioral Source

### SPICE
```spice
Behavioral Voltage Limiter
V1 in 0 DC 0 SIN(0 10 1k)
B1 out 0 V={min(max(V(in), -5), 5)}
R1 out 0 1k
.END
```

### CirQ
```yaml
cirq: "0.3"
name: Behavioral Voltage Limiter

components:
  - id: V1
    type: vsource
    value: 0
    pins: { p: in, n: "0" }
    waveform:
      type: sin
      v0: 0
      va: 10
      freq: 1000

  - id: B1
    type: bsource
    pins: { p: out, n: "0" }
    params: { v: "min(max(V(in), -5), 5)" }

  - id: R1
    type: resistor
    value: "1k"
    pins: { p: out, n: "0" }
```

## Example 7: Parameterized Subcircuit with Globals

### SPICE
```spice
Parameterized NAND
.GLOBAL vdd
.PARAM Wn=5u Wp=10u Lmin=0.5u
.SUBCKT nand2 a b out PARAMS: W_N=Wn W_P=Wp
MP1 out a vdd vdd PMOD W={W_P} L=Lmin
MP2 out b vdd vdd PMOD W={W_P} L=Lmin
MN1 mid a 0 0 NMOD W={W_N} L=Lmin
MN2 out b mid 0 NMOD W={W_N} L=Lmin
.ENDS nand2
X1 in1 in2 out1 nand2
X2 in3 in4 out2 nand2 PARAMS: W_N=10u W_P=20u
.END
```

### CirQ
```yaml
cirq: "0.3"
name: Parameterized NAND

globals: [vdd]

params:
  Wn: "5u"
  Wp: "10u"
  Lmin: "0.5u"

subcircuits:
  - name: nand2
    params: { W_N: "Wn", W_P: "Wp" }
    components:
      - { id: a,   type: port, direction: input,  net: a,   order: 0 }
      - { id: b,   type: port, direction: input,  net: b,   order: 1 }
      - { id: out, type: port, direction: output, net: out, order: 2 }
      - id: MP1
        type: pmos
        model: PMOD
        pins: { d: out, g: a, s: vdd, b: vdd }
        params: { w: "{W_P}", l: "{Lmin}" }
      - id: MP2
        type: pmos
        model: PMOD
        pins: { d: out, g: b, s: vdd, b: vdd }
        params: { w: "{W_P}", l: "{Lmin}" }
      - id: MN1
        type: nmos
        model: NMOD
        pins: { d: mid, g: a, s: "0", b: "0" }
        params: { w: "{W_N}", l: "{Lmin}" }
      - id: MN2
        type: nmos
        model: NMOD
        pins: { d: out, g: b, s: mid, b: "0" }
        params: { w: "{W_N}", l: "{Lmin}" }

components:
  - id: X1
    type: nand2
    pins: [in1, in2, out1]

  - id: X2
    type: nand2
    pins: [in3, in4, out2]
    params: { W_N: "10u", W_P: "20u" }
```

---

# What This Format Intentionally Omits

- **Simulation commands** (`.OP`, `.TRAN`, `.AC`, `.DC`, `.NOISE`, etc.) — use a separate
  simulation configuration format
- **Output commands** (`.SAVE`, `.PROBE`, `.PRINT`) — same
- **Control blocks** (`.CONTROL`/`.ENDC`) — these are ngspice scripting, not circuit data
- **Conditional blocks** (`.IF`/`.ENDIF`) — preprocessing; resolve before conversion
- **Component layout geometry** — in the sidecar `.layout.json`
- **Timing constraints** — out of scope
- **Bus / vector nets** — treat multi-bit nets as individual named nets

---

# Name Candidates

The working name for this format is **CirQ** (Circuit Query / Circuit Schema). Other
candidates considered:

- **CirQ** — short, distinctive, suggests "circuit" + structured query
- **CircuitNet** — descriptive but conflicts with neural network terminology
- **Ohm** — simple, evocative, but too generic for searchability
- **Nettle** — net + list, memorable

---

*CirQ is an open schema. No tooling required to read or write it — any YAML/JSON library works.*
