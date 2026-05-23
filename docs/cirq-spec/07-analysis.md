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

A `measure` block records a post-simulation measurement on the results of a
preceding analysis. It is the native Cirq counterpart of SPICE's `.meas`
directive:

```cirq
analysis tran {
    step: 1n
    stop: 100n
}

measure tran "rise" {
    spec: "TRIG v(out) VAL=0.5 RISE=1 TARG v(out) VAL=4.5 RISE=1"
}

measure tran "vout_max" {
    spec: "MAX v(out)"
}

measure tran "settle" {
    spec: "WHEN v(out)=4.95 RISE=1"
}

measure tran "vout_swing" {
    spec: "PARAM=vout_max - vout_min"
}
```

The header has three pieces:

1. The literal keyword `measure`.
2. An analysis-kind identifier (`tran`, `ac`, `dc`, ...). The measurement is
   evaluated against the results of that analysis when the simulator runs.
3. A string literal naming the measurement. The value appears as a vector of
   this name in the `measurements` result plot.

The body holds a single required field:

- `spec`: a string literal carrying the measurement clauses. The contents
  use the same syntax as the right-hand side of a SPICE `.meas` directive
  (everything after `.meas <type> <name>`). All keywords supported by the
  importer (`MAX`/`MIN`/`AVG`/`RMS`/`PP`, `INTEG`, `FIND`, `WHEN`,
  `TRIG`/`TARG`, `DERIV`, `PARAM=`) work here unchanged.

Reusing the SPICE clause syntax keeps native Cirq `measure` blocks and
SPICE-imported `.meas` directives identical in the IR, and makes
round-tripping between the two source forms lossless. A measure block whose
`spec` cannot be parsed surfaces an error diagnostic pointing at the spec
string. Status of advanced `.meas` features (`ERROR` mode, conditional
`IF`, file-referenced `PARAM`): planned.

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
