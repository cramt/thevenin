# Cirq Language Specification — Parameters

## Parameter Declaration

r[param.decl]

Parameters are named constants that can be used throughout a circuit or module.

```cirq
param vdd = 3.3
param r_load = 10k
param temperature = 27
```

### Scope

r[param.scope]

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

r[param.required]

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

r[param.expr]

Parameter values can reference other parameters:

```cirq
param r1 = 10k
param r2 = r1 * 2             // 20k
param r_parallel = (r1 * r2) / (r1 + r2)
```

r[param.no-forward-ref]

Forward references are not allowed — a parameter must be declared before it is used. This keeps evaluation order simple and predictable.

## Let Bindings

r[param.let]

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

r[param.constants]

| Name | Value | Description | Status |
|------|-------|-------------|--------|
| `pi` | 3.14159... | Pi | ✓ implemented |
| `e` | 2.71828... | Euler's number | ✓ implemented |

## Parameter Validation

r[param.validation]

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

## Compile-Time Conditionals

r[param.conditional]

`if / elseif / else` blocks select which declarations exist, resolved at
lowering time — not during simulation. Each condition is a constant expression
over params and literals; only the first true branch (or the `else` body)
reaches the IR.

```cirq
circuit amp {
    param corner = "tt"        // ff | tt | ss
    param vdd = 1.8
    param fast = true

    // Conditional parameter selection.
    if corner == "ff" {
        param vto = 0.35
    } elseif corner == "ss" {
        param vto = 0.55
    } else {
        param vto = 0.45
    }

    // Conditional element inclusion — the resistor only exists when true.
    if vdd > 1.5 {
        Rprot: resistor(in -> gate, 10k)
    }

    // Bodies hold any circuit item, including whole analyses and nested ifs.
    if fast {
        analysis tran { step: 10p; stop: 5n }
    } else {
        analysis tran { step: 100p; stop: 50n }
    }
}
```

Rules:

- **Compile-time, not runtime.** The condition is folded during lowering; the
  non-taken branch never reaches the IR. This is distinct from the runtime
  ternary (`cond ? a : b`, an expression) and from `.control` script `if`.
- **Conditions must be constant-foldable** — params and literals only, with
  arithmetic, comparisons (`< > <= >= == !=`, including string `==`/`!=`),
  logical operators (`&& || !`), and the ternary. A condition that cannot be
  reduced to a scalar at lowering time is a compile error.
- **Declaration order.** A condition sees the params declared before the `if`;
  params declared inside a taken branch are in scope for items that follow it.
- **Any circuit item** may appear in a branch body: `param`, `let`, elements,
  `model`, `analysis`, module instances, or a nested `if`.
- **Works at circuit and module scope**, since module bodies accept the same
  items.

This is the native counterpart of SPICE's `.if/.elseif/.else/.endif`, which the
importer maps onto the same construct.
