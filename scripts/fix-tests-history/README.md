# Fix-Tests History

Per-topic history files for the overnight test fixer agent. The agent reads
relevant files when investigating a specific test, and appends findings after
each iteration.

## Files

| File | Contents |
|---|---|
| `applied-fixes.md` | Chronological table of all fixes (numbered) |
| `failed-investigations.md` | Approaches that were tried and ruled out |
| `vbic.md` | VBIC model test status, root cause analysis, self-heating details |
| `bsim3soi.md` | BSIM3SOI (DD/FD/PD) test status, fixes, remaining discrepancies |
| `transmission-line.md` | CPL/LTRA/TXL test status |
| `general-circuits.md` | rtlinv, schmitt, mosamp, HFET inverter |
| `missing-features.md` | .control interpreter, BSIM1/2, and other unimplemented features |

## Convention

When adding findings, append to the relevant file (or create a new one for a
new device model / subsystem). Use a `## Session N findings (date)` heading.
Keep entries concise — focus on what was tried, what was found, and whether
it's worth retrying.
