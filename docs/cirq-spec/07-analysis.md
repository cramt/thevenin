# Cirq Language Specification — Analysis Commands

## Analysis Declaration

Analysis commands tell the simulator what to compute. They are declared inside a `circuit` block.

```cirq
circuit my_circuit {
    // ... elements ...

    analysis op {}

    analysis dc {
        sweep V1: 0..5 step 0.1
    }

    analysis ac {
        start: 1
        stop: 1G
        points: 100
        scale: decade
    }

    analysis tran {
        step: 1n
        stop: 100n
    }
}
```

## DC Operating Point

```cirq
analysis op {}
```

No parameters needed. Computes the DC bias point.

## DC Sweep

```cirq
analysis dc {
    sweep V1: 0..5 step 0.1
}

// Double sweep:
analysis dc {
    sweep V1: 0..5 step 0.1
    sweep V2: 0..3.3 step 0.5
}
```

Parameters:
- `sweep <source>: <start>..<stop> step <increment>`

## AC Small-Signal Analysis

```cirq
analysis ac {
    start: 1           // start frequency (Hz)
    stop: 1G           // stop frequency (Hz)
    points: 100        // number of points
    scale: decade       // decade | octave | linear
}
```

Parameters:
- `start`: start frequency — **required**
- `stop`: stop frequency — **required**
- `points`: number of points per decade/octave, or total for linear — **required**
- `scale`: frequency scale — **required**, one of `decade`, `octave`, `linear`

## Transient Analysis

```cirq
analysis tran {
    step: 1n            // suggested time step
    stop: 100n          // simulation end time
}

// With initial conditions:
analysis tran {
    step: 10n
    stop: 1u
    uic: true           // use initial conditions from element IC specs
}
```

Parameters:
- `step`: maximum time step — **required**
- `stop`: end time — **required**
- `start`: start time — optional, default 0
- `tmax`: maximum internal timestep — optional (solver picks automatically if omitted)
- `uic`: use initial conditions — optional, default false

## Noise Analysis

```cirq
analysis noise {
    output: out         // output net
    reference: gnd      // reference net (optional, default gnd)
    source: V1          // input source for input-referred noise
    start: 1
    stop: 1G
    points: 100
    scale: decade
}
```

## Pole-Zero Analysis

```cirq
analysis pz {
    input_pos: in              // input positive node
    input_neg: gnd             // input negative node
    output_pos: out            // output positive node
    output_neg: gnd            // output negative node
    transfer: voltage          // voltage | current
    analysis: both             // poles | zeros | both
}
```

The node names also accept the short aliases `in_pos`, `in_neg`, `out_pos`, `out_neg`.

## Sensitivity Analysis

```cirq
// DC sensitivity (default)
analysis sens {
    output: v(out)             // output variable
}

// AC sensitivity sweep — adds an AC frequency scan
analysis sens {
    output: v(out)
    ac: true
    scale: decade              // decade | octave | linear
    points: 10
    fstart: 1
    fstop: 1G
}
```

Parameters:
- `output`: signal whose sensitivity is computed — **required**
- `ac`: enable AC variant — optional, default `false`
- `scale`, `points`, `fstart`, `fstop`: required when `ac: true`

## Transfer Function

```cirq
analysis tf {
    output: v(out)             // output variable
    source: V1                 // input source
}
```

## Measurements

A `measure` declaration records a post-simulation measurement on the results
of a preceding analysis. It is the native Cirq counterpart of SPICE's `.meas`
directive. The header has three pieces:

1. The literal keyword `measure`.
2. An analysis-kind identifier (`tran`, `ac`, `dc`, ...). The measurement is
   evaluated against the results of that analysis when the simulator runs.
3. A name. The measured value appears as a vector of this name in the
   `measurements` result plot.

### Expression form (preferred)

A measurement is an expression: `measure <kind> <name> = <expr>`. The
expression layer is the same one used by `let` and `param`, so derived and
conditional measurements need no special syntax.

```cirq
analysis tran {
    step: 1n
    stop: 100n
}

// Waveform probes — functions that reduce a result vector to a scalar.
measure tran vout_max = max(v(out), from: 10n, to: 50n)
measure tran settle   = when(v(out) == 4.95, rise: 1)
measure tran td       = delay(from: cross(v(in),  0.5, rise: 1),
                              to:   cross(v(out), 0.5, fall: 1))
measure tran q_inj     = integ(i(vd), from: 0, to: 20n)

// Derived — plain arithmetic over earlier measurements.
measure tran swing = vout_max - vout_min

// Conditional / pass-fail — comparison + ternary.
measure tran bw_ok = (swing > 100m) ? 1 : 0
measure tran spec_pass = (td < 80p && swing > 100m) ? 1 : 0
```

**Probe functions.** Each reduces a result vector to a scalar and maps 1:1
onto a SPICE `.meas` keyword:

| Function | Meaning | SPICE keyword |
|----------|---------|---------------|
| `max(v, from:, to:)` (also `min`, `avg`, `rms`, `pp`) | aggregate over an optional window | `MAX`/`MIN`/`AVG`/`RMS`/`PP` |
| `integ(v, from:, to:)` | trapezoidal integral | `INTEG` |
| `find(v, at: t)` / `find(v, when: cross(...))` | value at a point or crossing | `FIND` |
| `when(v == level, rise: n)` | sweep value at a crossing | `WHEN` |
| `deriv(v, at: t)` | numerical derivative | `DERIV` |
| `delay(from: cross(...), to: cross(...))` | time between two events | `TRIG`/`TARG` |

Signals are referenced with `v(node)` and `i(element)`. A crossing event is
`cross(signal, threshold, rise:|fall:|cross: n)`, where the occurrence `n` is
a 1-based index or the keyword `last`. The optional `from:`/`to:` arguments
bound the search window.

**Derived & conditional.** Any expression that is not a probe call is a
scalar expression over earlier measurements (referenced by name). It supports
arithmetic (`+ - * /`), comparisons (`< > <= >= == !=`), logical combinators
(`&& || !`), and the ternary `cond ? then : else`. Booleans are numeric — true
is `1.0`, false is `0.0`, and any non-zero value is treated as true — so the
classic SPICE pass/fail idiom `param='(vout_diff < 100k) ? 1 : 0'` is written
directly as `(vout_diff < 100k) ? 1 : 0`.

### Block form (legacy)

The original block form remains as an escape hatch. The name is a string
literal and the body wraps a verbatim SPICE `.meas` clause string in a `spec:`
field:

```cirq
measure tran "vout_max" { spec: "MAX v(out)" }
measure tran "rise" {
    spec: "TRIG v(out) VAL=0.5 RISE=1 TARG v(out) VAL=4.5 RISE=1"
}
```

Both forms lower to the same typed IR (`MeasureExpr`), and the expression form
synthesizes a canonical clause string, so native and SPICE-imported
measurements stay identical in the IR and round-trip losslessly. A measurement
that cannot be lowered surfaces an error diagnostic at its source span.

Status of advanced `.meas` features (`ERROR` mode, file-referenced `PARAM`):
planned.

## Multiple Analyses

A circuit can contain multiple analysis commands. They run in declaration order:

```cirq
circuit amplifier_test {
    // ... elements ...

    analysis op {}                    // first: find bias point
    analysis ac {                     // then: frequency response
        start: 1
        stop: 10G
        points: 200
        scale: decade
    }
    analysis tran {                   // then: step response
        step: 1n
        stop: 1u
    }
}
```
