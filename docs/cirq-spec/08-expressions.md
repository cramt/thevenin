# Cirq Language Specification — Expressions

## Expression Contexts

Expressions appear in:
- parameter values: `param r = 10k * 2`
- element parameters: `R1: resistor(a -> b, r1 + r2)`
- analysis specifications: `sweep V1: 0..vdd step vdd/100`

## Operator Precedence (highest to lowest)

| Precedence | Operators | Associativity | Description |
|-----------|-----------|---------------|-------------|
| 1 | `()` | — | Grouping |
| 2 | `func()` | — | Function call |
| 3 | `-` `!` | right | Unary negation, logical not |
| 4 | `**` | right | Exponentiation |
| 5 | `*` `/` `%` | left | Multiplication, division, modulo |
| 6 | `+` `-` | left | Addition, subtraction |
| 7 | `<` `>` `<=` `>=` | left | Comparison |
| 8 | `==` `!=` | left | Equality |
| 9 | `&&` | left | Logical AND |
| 10 | `||` | left | Logical OR |

## Arithmetic Expressions

```cirq
param a = 10k
param b = a * 2          // 20k
param c = a + b          // 30k
param d = (a * b) / c    // 6666.67
param e = 2 ** 10        // 1024
```

Division by zero is a compile-time error if detectable, runtime error otherwise.

## Built-in Functions

### Math Functions

| Function | Description | Status |
|----------|------------|--------|
| `abs(x)` | Absolute value | ✓ implemented |
| `sqrt(x)` | Square root | ✓ implemented |
| `exp(x)` | e^x | ✓ implemented |
| `ln(x)` | Natural logarithm | ✓ implemented |
| `log(x)` | Natural logarithm (alias for `ln`) | ✓ implemented |
| `log10(x)` | Base-10 logarithm | ✓ implemented |
| `pow(x, y)` | x^y (equivalent to `x ** y`) | ✓ implemented |
| `sin(x)` | Sine (radians) | ✓ implemented |
| `cos(x)` | Cosine (radians) | ✓ implemented |
| `tan(x)` | Tangent (radians) | ✓ implemented |
| `min(a, b)` | Minimum | ✓ implemented |
| `max(a, b)` | Maximum | ✓ implemented |

### User-Defined Functions

Users can define named functions using Haskell-style syntax:

```cirq
limit(x, lo, hi) = min(max(x, lo), hi)
clamp01(v) = limit(v, 0, 1)
```

Function declarations are allowed at the top level of a file or inside circuit/module bodies. The body is a single expression. See `05-parameters.md` for how functions interact with parameter scoping.

### Thermal Voltage

```cirq
let vt = pi * 2   // use built-in constants in expressions
```

## String Expressions

Strings do not support arithmetic. They can only be compared for equality:

```cirq
param model_name = "nmos_3v3"
// model_name == "nmos_3v3"  → true
```

## Net Expressions

Nets are not values. They cannot participate in arithmetic expressions. They can only be:
- used in connections (`a -> b`)
- compared for identity (`net1 == net2`)
- passed as port connections
