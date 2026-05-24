# Device Coverage

This document lists every device kind the thevenin simulator can stamp today,
the ngspice model levels each one implements, and the gaps that are still on
the 1.0 roadmap. The authoritative source of truth is the `ElementKind` match
in [thevenin/src/mna_ir.rs](../thevenin/src/mna_ir.rs); model-level dispatch
lives in the same file and is cross-checked against
[thevenin/src/mna.rs](../thevenin/src/mna.rs). For the roadmap context, see
[1.0-checklist.md](1.0-checklist.md).

## Passive elements

| Element | SPICE letter | thevenin status | Notes |
|---|---|---|---|
| Resistor | R | implemented | Linear `value`; `.model R` with `RSH` + `L`/`W` + `NARROW`; `m`/`scale` multipliers; flicker-noise params (KF/AF/EF). Plain-element `tc=tc1,tc2` is **not** parsed for R — temperature coefficients are only honoured on behavioural `B` sources. |
| Capacitor | C | implemented | Linear `value`; `IC=` initial condition; flicker-noise params (KF/AF). |
| Inductor | L | implemented | Linear `value`; `IC=` initial condition. |
| Mutual coupling | K | implemented | Pair-wise coupling between two named inductors. |

## Sources

| Element | SPICE letter | thevenin status | Notes |
|---|---|---|---|
| Independent voltage | V | implemented | DC + AC (mag/phase) + transient waveform. |
| Independent current | I | implemented | DC + AC + transient waveform. |
| VCVS | E | implemented | Linear voltage-controlled voltage source. |
| VCCS | G | implemented | Linear voltage-controlled current source. |
| CCVS | H | implemented | Linear current-controlled voltage source. |
| CCCS | F | implemented | Linear current-controlled current source. |
| Behavioural | B | implemented | `V=expr` and `I=expr` modes ([thevenin/src/mna.rs](../thevenin/src/mna.rs) expression engine); supports tail `tc1=`/`tc2=`/`reciproctc=` temperature scaling. |

Transient waveforms supported on V and I (all six from
[cirq-ir/src/lib.rs](../cirq-ir/src/lib.rs) `Waveform`): `PULSE`, `SIN`,
`EXP`, `PWL`, `SFFM`, `AM`.

## Diodes

| Model | ngspice level | thevenin status | Source file | Notes |
|---|---|---|---|---|
| SPICE Shockley diode | (n/a — single model) | implemented | [diode.rs](../thevenin/src/diode.rs) | IS, N, RS, BV/IBV, CJO, VJ, M, TT, KF, AF; pnjlim voltage limiting. |

## BJTs

| Model | ngspice level | thevenin status | Source file | Notes |
|---|---|---|---|---|
| Gummel-Poon (default) | 1 | implemented | [bjt.rs](../thevenin/src/bjt.rs) | Ebers-Moll/Gummel-Poon with Early effect (VAF/VAR) and high-injection rolloff (IKF/IKR). |
| VBIC95 | 4 | implemented | [vbic.rs](../thevenin/src/vbic.rs) | DC + NR stamps. Self-heating (Vrth) and excess phase (NQS) **not** implemented. |
| HICUM L0 / L2 | 7 / 8 | not yet — assumed in 1.0 | — | "Weird BJT" enumeration pass still pending per checklist A1. |

## MOSFETs

| Model | ngspice level | thevenin status | Source file | Notes |
|---|---|---|---|---|
| Shichman-Hodges | 1 | implemented | [mosfet.rs](../thevenin/src/mosfet.rs) | Default when no LEVEL is supplied. Body effect (GAMMA), channel-length modulation (LAMBDA), bulk diodes. |
| Grove-Frohman | 2 | implemented | [mos2.rs](../thevenin/src/mos2.rs) | Velocity saturation, short/narrow channel effects, subthreshold conduction. |
| MOS3 (semi-empirical) | 3 | implemented | [mos3.rs](../thevenin/src/mos3.rs) | Liu/Kwok short-channel: DIBL via ETA, mobility degradation via THETA, velocity-saturation cap via VMAX, channel-length modulation via KAPPA, junction-depth effect via XJ, plus subthreshold conduction via NFS. Bulk diodes and overlap caps shared with the Level-1 path. |
| Sakurai-Newton n-th power | 6 | implemented | [mos6.rs](../thevenin/src/mos6.rs) | Exponential I-V instead of quadratic. |
| BSIM3v3 | 8 / 49 | implemented | [bsim3.rs](../thevenin/src/bsim3.rs) | BSIM3v3.2.4. Both LEVEL=8 and LEVEL=49 dispatch here. Size-dependent params + W/L binning. |
| BSIM4 | 14 / 54 | implemented | [bsim4.rs](../thevenin/src/bsim4.rs) | Gate tunneling current, GIDL/GISL, advanced capacitance, RDSMOD external R nodes. |
| BSIM3SOI-FD | 55 | implemented | [bsim3soi_fd.rs](../thevenin/src/bsim3soi_fd.rs) | Fully-depleted SOI. Back-gate (E node), optional body contact, no parasitic BJT/GIDL. |
| BSIM3SOI-DD | 56 | implemented | [bsim3soi_dd.rs](../thevenin/src/bsim3soi_dd.rs) | Double-diffused SOI; PD-style 4-component junction diodes + impact ionization. |
| BSIM3SOI-PD | 57 | implemented | [bsim3soi_pd.rs](../thevenin/src/bsim3soi_pd.rs) | Partially-depleted SOI; floating body, self-heating disabled (SHMOD=0). |
| BSIM1 | — (ngspice level varies) | not yet — in 1.0 scope | — | ~3500 LOC port from ngspice. |
| BSIM2 | — (ngspice level varies) | not yet — in 1.0 scope | — | ~2500 LOC port from ngspice. |
| VDMOS | — | not yet — in 1.0 scope | — | Power-MOSFET model. |
| HiSIM (bulk) | — | not yet — in 1.0 scope | — | Modern compact model. |
| HiSIMHV | — | not yet — in 1.0 scope | — | High-voltage HiSIM variant. |

## JFETs / MESFETs / HFETs

| Model | ngspice level | thevenin status | Source file | Notes |
|---|---|---|---|---|
| JFET (N-channel / P-channel) | 1 | implemented | [jfet.rs](../thevenin/src/jfet.rs) | SPICE Level-1 JFET with PN-diode gate junctions. Parsed via `J` element. |
| MESFET (Statz/Curtice) | 1 | implemented | [mesfet.rs](../thevenin/src/mesfet.rs) | `.model NMF/PMF level=1`. Stamped from `Z` element when the model resolves to NMF/PMF + level 1. |
| MESA (Ytterdal/Lee/Shur/Fjeldly) | 2 / 3 / 4 | implemented | [mesa.rs](../thevenin/src/mesa.rs) | GaAs MESFET family: mesa1 (basic mu modulation), mesa2 (dual-doped hetero), mesa3 (charge-based + NMAX). Parsed via `Z` element. |
| HFET1 | 5 | implemented | [hfet.rs](../thevenin/src/hfet.rs) | Heterojunction FET, ngspice `hfet1`. `.model NHFET/PHFET level=5`. |
| HFET2 | 6 | implemented | [hfet.rs](../thevenin/src/hfet.rs) | Same crate, ngspice `hfet2`. `.model NHFET/PHFET level=6`. |

## Transmission lines

| Element | SPICE letter | thevenin status | Source file | Notes |
|---|---|---|---|---|
| LTRA (lossy 2-port) | O | implemented | [ltra.rs](../thevenin/src/ltra.rs) | Convolution-based; `.model LTRA`. |
| TXL (single lossy line) | Y | implemented | [txl.rs](../thevenin/src/txl.rs) | Padé approximation of Y(s) and propagation. `.model TXL`. |
| CPL (coupled multiconductor) | P | implemented | [cpl.rs](../thevenin/src/cpl.rs) | Jacobi eigendecomposition + Padé per modal line. `.model CPL`. |
| Ideal lossless line | T | implemented | [tline.rs](../thevenin/src/tline.rs) | `T<name> n1+ n1- n2+ n2- Z0=val [TD=delay \| F=freq [NL=count]] [IC=v1,i1,v2,i2]`. DC = wire (V1=V2, I1=-I2), transient = method of characteristics with `VecDeque` history + linear interpolation, AC = closed-form lossless ABCD matrix. |
| Uniform RC line | U | **not implemented** — in 1.0 scope | — | URC. Confirmed in scope; checklist A1. |

## Switches

| Type | SPICE letter | thevenin status | Source file | Notes |
|---|---|---|---|---|
| Voltage-controlled switch | S | implemented | [switch.rs](../thevenin/src/switch.rs) | `.model NAME SW (Vt=… Vh=… Ron=… Roff=…)`. Hysteretic conductance; latched state carries across NR iterations + timesteps. `VSWITCH` accepted as a model-kind alias. |
| Current-controlled switch | W | implemented | [switch.rs](../thevenin/src/switch.rs) | `.model NAME CSW (It=… Ih=… Ron=… Roff=…)`. Senses branch current through a named voltage source. `ISWITCH` accepted as a model-kind alias. |

## XSPICE code models

The `A` element is implemented end-to-end: the SPICE parser recognises it
([thevenin-types/src/parse.rs](../thevenin-types/src/parse.rs) `'A'` branch),
the IR carries it as `ElementKind::Xspice` with structured scalar/array
connections ([cirq-ir/src/lib.rs](../cirq-ir/src/lib.rs)), and stamping is
dispatched through the registry in
[thevenin-xspice/src/registry.rs](../thevenin-xspice/src/registry.rs).

The registry itself ships **empty** — there are no built-in code models. Hosts
register their own `CodeModelDef` values (see
[thevenin-xspice/src/model.rs](../thevenin-xspice/src/model.rs)
`CodeModelBuilder`) before running simulation. Dynamic library (`cm` shared
object) loading is explicitly out of scope for 1.0 per the checklist.

## Subcircuits and includes

| Feature | thevenin status | Notes |
|---|---|---|
| `.subckt` / `.ends` (nested) | implemented | Full hierarchy with parameter overrides. |
| `X` element instantiation | implemented | Positional + `PARAMS:` keyword form. |
| `.include` / `.lib` | importer recognises but does not resolve | File I/O is a real gap — see checklist C5. |

## Known gaps for 1.0

Items the checklist tags as in-scope for the 1.0 cut but where the source
tree currently has no implementation:

- **URC** transmission line (`U` element).
- **BSIM1**, **BSIM2** MOSFET levels.
- **VDMOS** power MOSFET.
- **HiSIM** and **HiSIMHV** compact models.
- Other "weird MOSFET / niche device" variants (HICUM2, NUMOS, …) flagged in
  the checklist for an enumeration pass against
  `ngspice-upstream/src/spicelib/devices/`.

## Out-of-scope models

All of BSIM1, BSIM2, HiSIM, HiSIMHV, VDMOS, and URC are explicitly **in**
1.0 scope per the latest decisions log (see
[1.0-checklist.md](1.0-checklist.md) "Decisions log"). The only firm
exclusion is dynamic XSPICE plug-in loading (compiled-in code models via
the registry stay in scope).
