# Bounded resource-usage observations

Workcell can observe the material resource condition of one process already
bound to a live `workcell.harness-instance/v1` record. The native command is
on-demand and bounded; it does not start a background metrics service:

```text
workcell --json instances usage <instance_ref> \
  [--pid <pid>] [--interval-ms <0..60000>] \
  [--correlation-ref <opaque-ref>]...
```

The result is `workcell.resource-usage/v1`. Its versioned schema and neutral
conformance specimen live under `crates/workcell-runtime/{schemas,fixtures}`.

## Evidence and identity

The local provider reads only `pid`, process start marker, cumulative CPU
time, RSS and executable identity from `ps`; Linux additionally resolves
`/proc/<pid>/exe`. It never requests or returns process arguments or
environment values. CPU time and RSS are observed. CPU utilisation is derived
from the cumulative CPU-time delta over the measured wall interval. The first
portable provider does not establish peak RSS, GPU, VRAM, per-process network
or portable storage-I/O bytes, so those fields say `unsupported` and carry no
numeric value.

The collector requires the PID to belong to the named live HarnessInstance.
It checks the registry binding before and after the interval and checks the OS
process start marker and executable across both samples. PID reuse, executable
replacement, instance revival/rebinding or movement to another Workcell is a
named unavailable result, never a continuation of the old interval.

An instance with several PIDs requires an explicit `--pid`. A projected
instance expectation has no live target PID and therefore cannot be observed;
the target must first complete its normal re-detection. Once re-detected, a
reading names the target Workcell and target-side material process while
retaining the stable HarnessInstance identity.

`--correlation-ref` values are opaque external references. Factory can place
an Execution or workflow reference there without Workcell interpreting or
owning those semantics.
