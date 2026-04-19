# Cirq Language Specification — Types and Values

## Type System

Cirq has a minimal type system focused on circuit description. Types exist primarily to catch errors at the IR level, not to enable general-purpose programming.

### Primitive Types

| Type | Description | Example |
|------|------------|---------|
| `real` | 64-bit floating-point | `3.14`, `1k`, `2.2M` |
| `int` | 64-bit signed integer | `42`, `0xFF` |
| `bool` | Boolean | `true`, `false` |
| `string` | UTF-8 string | `"nmos"` |
| `net` | Electrical net (node) | `vdd`, `gnd`, port names |

### Net Type

The `net` type is special — it represents an electrical connection point. Nets are not numeric values; they are topological identifiers.

```cirq
// These are nets:
port in: net
port out: net

// gnd is a built-in net
```

### Implicit Coercion

- `int` → `real`: always safe
- No other implicit coercions exist

### No General Type Annotations Required

Parameters and port values are inferred from context in most cases:

```cirq
param r_val = 10k           // inferred: real
param count = 4             // inferred: int
param name = "nmos_3v3"     // inferred: string
```

Explicit type annotations are optional and primarily for documentation:

```cirq
param r_val: real = 10k
```

## Values

### Ground

`gnd` is a built-in net representing the global reference node (SPICE node 0).

```cirq
V1: vsource(vdd -> gnd, dc: 5V)
```

### Boolean Values

```cirq
true
false
```

Used in conditional parameter expressions (future) and model selection.

## Expressions

See `08-expressions.md` for the full expression grammar. In brief:

```cirq
// Arithmetic
param total = r1 + r2
param half = vdd / 2

// Function calls (built-in math)
param rms = sqrt(v1**2 + v2**2)

// Ternary (future consideration)
// param val = if condition then a else b
```
