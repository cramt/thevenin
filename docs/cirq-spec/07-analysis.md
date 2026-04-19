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
    input: in -> gnd           // input port
    output: out -> gnd         // output port
    transfer: voltage          // voltage | current
    analysis: both             // poles | zeros | both
}
```

## Sensitivity Analysis

```cirq
analysis sens {
    output: v(out)             // output variable
}
```

## Transfer Function

```cirq
analysis tf {
    output: v(out)             // output variable
    source: V1                 // input source
}
```

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
