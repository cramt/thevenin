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

| Function | Description |
|----------|------------|
| `abs(x)` | Absolute value |
| `sqrt(x)` | Square root |
| `exp(x)` | e^x |
| `ln(x)` | Natural logarithm |
| `log10(x)` | Base-10 logarithm |
| `log2(x)` | Base-2 logarithm |
| `pow(x, y)` | x^y (equivalent to `x ** y`) |
| `sin(x)` | Sine (radians) |
| `cos(x)` | Cosine (radians) |
| `tan(x)` | Tangent (radians) |
| `asin(x)` | Arcsine |
| `acos(x)` | Arccosine |
| `atan(x)` | Arctangent |
| `atan2(y, x)` | Two-argument arctangent |
| `min(a, b)` | Minimum |
| `max(a, b)` | Maximum |
| `floor(x)` | Floor |
| `ceil(x)` | Ceiling |
| `round(x)` | Round to nearest integer |

### Thermal Voltage

```cirq
let vt = boltzmann * (temperature + kelvin) / charge
```

This is a common pattern in semiconductor device expressions. A convenience function may be provided:

```cirq
let vt = vt(temperature)   // built-in thermal voltage at given temp in Celsius
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
