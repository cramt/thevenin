# Cirq Language Specification — Lexical Structure

## Comments

```cirq
// Line comment — extends to end of line

/* Block comment
   can span multiple lines */
```

Block comments nest:
```cirq
/* outer /* inner */ still in outer */
```

## Identifiers

```
identifier = [a-zA-Z_][a-zA-Z0-9_]*
```

Identifiers are case-sensitive. `Vdd` and `vdd` are different names.

Reserved keywords cannot be used as identifiers (see below).

## Keywords

```
circuit    module     port       in         out        inout
let        param      model      analysis   import     as
true       false      gnd        global     options    temp
save       ic         sweep      step
```

## Numeric Literals

### Integer Literals

```cirq
42
0xFF        // hexadecimal (for digital masks, flags)
0b1010      // binary
```

### Floating-Point Literals

```cirq
3.14
1.0e-3
2.5E6
.001        // leading dot allowed
```

### SI Suffix Literals

Numeric literals may carry an SI suffix. The suffix is part of the literal, not a separate operator.

```cirq
1k          // 1_000
2.2M        // 2_200_000
100n        // 100e-9
4.7u        // 4.7e-6
10p         // 10e-12
1f          // 1e-15
330m        // 0.33  (milli)
1G          // 1e9
1T          // 1e12
1Meg        // 1e6  (SPICE compat alias for M)
```

| Suffix | Multiplier | Name |
|--------|-----------|------|
| `T` | 1e12 | tera |
| `G` | 1e9 | giga |
| `M` | 1e6 | mega |
| `Meg` | 1e6 | mega (SPICE compat) |
| `k` | 1e3 | kilo |
| `m` | 1e-3 | milli |
| `u` | 1e-6 | micro |
| `n` | 1e-9 | nano |
| `p` | 1e-12 | pico |
| `f` | 1e-15 | femto |

Note: `M` = mega (not milli). This **differs** from SPICE where `M` = milli. Cirq uses `m` for milli. The SPICE compatibility suffix `Meg` is provided for mega.

Underscores are allowed as visual separators in numeric literals: `1_000_000`, `4.7_000u`.

### Unit Annotations

Values may optionally carry a unit annotation after the number/suffix:

```cirq
10k ohm
5V
100n F
1m A
```

Unit annotations are informational metadata for tooling and documentation. They do not affect the numeric value. The simulator treats `10k ohm` and `10k` identically. See `09-attributes.md` for how units propagate through expressions.

## String Literals

```cirq
"hello world"
"path/to/model.lib"
```

Strings are used for file paths in imports and model library references. They are not general-purpose values.

Escape sequences: `\\`, `\"`, `\n`, `\t`.

## Punctuation and Operators

```
{  }        // blocks
(  )        // grouping, function calls
[  ]        // reserved (future: arrays)
,           // separator
;           // statement terminator (optional — newline also terminates)
:           // type annotation, port direction
=           // assignment, parameter default
->          // connection operator
.           // member access
..          // range (in sweep specs)
+  -  *  /  // arithmetic
**          // exponentiation
==  !=      // equality
<  >  <=  >= // comparison
&&  ||  !   // logical
&  |  ^  ~  // bitwise (reserved, future use)
@           // attribute prefix
```

## Whitespace and Line Handling

Whitespace (spaces, tabs) is insignificant except as token separator.

Newlines are significant as statement terminators, but a statement can span multiple lines if:
- the line ends with an operator (`+`, `-`, `*`, `/`, `->`, `,`, `=`)
- the line ends with an opening delimiter (`{`, `(`, `[`)
- the next line is indented more than the current statement's start

```cirq
// These are equivalent:
R1: resistor(a -> b, 10k ohm)

R1: resistor(
    a -> b,
    10k ohm
)
```

## Semicolons

Semicolons are optional statement terminators. Newlines serve the same purpose. Semicolons are useful for multiple statements on one line:

```cirq
param vdd = 3.3; param gnd_r = 100
```
