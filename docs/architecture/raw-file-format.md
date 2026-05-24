# Raw file output

Thevenin emits ngspice-compatible "raw files" via
[`thevenin::raw_output`](../../thevenin/src/raw_output.rs). Three formats are
supported:

| Function            | Format            | Use                                                                                              |
| ------------------- | ----------------- | ------------------------------------------------------------------------------------------------ |
| `write_binary_raw`  | ngspice raw, binary | Canonical interchange — KiCad, ngspice's own `source` command, scripts using `ngspice -r`. |
| `write_ascii_raw`   | ngspice raw, ASCII  | Same format, ASCII data section. Useful for diffing / hand inspection.                       |
| `write_csv`         | CSV                 | Convenience for plotting tools (matplotlib, gnuplot, pandas) that don't speak raw files.     |

The raw-file grammar is documented in the [ngspice manual,
chapter 13.7](https://ngspice.sourceforge.io/docs/ngspice-manual.pdf). What
follows is the brief summary plus the thevenin-specific choices.

## Layout

Each plot is one self-contained block: text header, then a data section.
A file may concatenate multiple plots back-to-back; ngspice and tools
like `gwave` read them all.

```
Title: <free text>
Date: <human-readable timestamp>
Plotname: <human-readable analysis name>
Flags: real | complex
No. Variables: <N>
No. Points: <P>
Variables:
        0       <name0>         <type0>
        1       <name1>         <type1>
        ...
        N-1     <nameN-1>       <typeN-1>
Values:                 # ASCII variant
 0      <p0 v0>
        <p0 v1>
        ...
 1      <p1 v0>
        ...
P-1     <pP-1 v(N-1)>
```

Or for binary:

```
... (same header lines) ...
Binary:
<N × P f64 values, point-major, little-endian>
```

Complex plots double each value into `(re, im)` pairs:

- ASCII: `re,im` per cell.
- Binary: two consecutive `f64`s per cell.
- Real-valued vectors inside a complex plot are padded with a zero
  imaginary part. This matches ngspice's `%.*e,0.0` layout and keeps the
  row stride uniform across columns.

## Plotname mapping

Thevenin uses these strings (matching ngspice's defaults):

| Plot tag (from `SimPlot.name`) | `Plotname:` header               |
| ------------------------------ | -------------------------------- |
| `op*`                          | `Operating Point`                |
| `dc*`                          | `DC transfer characteristic`     |
| `tran*`                        | `Transient Analysis`             |
| `ac*`                          | `AC Analysis`                    |
| `noise*`                       | `Noise Spectral Density`         |
| `pz*`                          | `Pole-Zero Analysis`             |
| `tf*`                          | `Transfer Function`              |
| `sens*`                        | `Sensitivity Analysis`           |

Unknown plot tags fall back to `Operating Point`.

## Variable types

The `<type>` column is inferred from the vector name:

| Name pattern         | Type        |
| -------------------- | ----------- |
| `time`               | `time`      |
| `frequency`          | `frequency` |
| `v(...)`             | `voltage`   |
| `i(...)` or `*#branch` | `current`   |
| anything else        | `notype`    |

Vectors named `<src>#branch` (thevenin's internal branch-current naming)
are rewritten to `i(<src>)` in the `Variables:` block so consumers see
the same names ngspice writes.

## thevenin-specific choices

- **Binary endianness is fixed to IEEE 754 little-endian.** ngspice writes
  in host byte order, which means a binary raw file produced on a big-endian
  system is unreadable on a little-endian box. Thevenin standardises so
  files round-trip across machines.
- **Date format is `YYYY-MM-DD HH:MM:SS UTC`.** ngspice writes
  `asctime(localtime(now))`, which encodes the system locale; thevenin's
  output is locale-independent for reproducibility. Consumers ignore this
  field — it's metadata only.
- **No `Command:` header.** ngspice emits a `Command: ngspice-X, Build Y`
  line; thevenin omits it. The field is metadata and isn't parsed by any
  consumer.
- **No `Option:` / `Dimensions:` / `min=` / `max=` annotations.** These
  belong to ngspice's interactive plot machinery; thevenin's batch-style
  output doesn't track them.
- **CSV emits the first plot only.** CSV has no notion of concatenated
  plots, so a multi-plot `SimResult` collapses to its first entry. Use
  raw if you need multi-plot output.

## Wire-up

Inside `.control` blocks the `write` command routes to these functions:

```
write [filename] [vec1 vec2 ...]
```

- No filename ⇒ `thevenin.raw`.
- No vector list ⇒ all vectors from the current plot.
- Filename ending in `.csv` ⇒ CSV.
- Otherwise raw; ASCII if the `filetype` variable is set to `ascii`
  (`set filetype = ascii`), binary by default.

## File I/O

All three writers take `&mut impl std::io::Write`, so callers can target
a `File`, `Vec<u8>`, stdout, a network socket, or anything else that
implements `Write`. There is no implicit allocation; the writer streams
each row directly into the underlying sink.
