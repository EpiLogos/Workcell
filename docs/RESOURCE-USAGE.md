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
process start marker and executable across both samples. When the recording
scan stored a start marker for that PID (`executions` in the instance
record), the live sample must match it: a mismatch is a stale binding — the
host recycled the PID and the recorded process generation was replaced —
refused by name, never attributed to the old interval. Records without stored
start evidence (manual registration, older records) disclose the gap with an
empty `executions` array and skip the check. PID reuse, executable
replacement, instance revival/rebinding or movement to another Workcell is a
named unavailable result, never a continuation of the old interval.

The live scanner binds each PID to the executable path the host actually
reports and fingerprints that binary when readable. It also records each
observed execution's process start marker, so simultaneous executions of one
executable stay individually correlated and a PID observed under a different
marker than the record holds is named in the scan report as a
`generation_replacements` entry instead of being silently refreshed. Start
markers are host-local scheduling facts, not global identities: they prove
process generations within one machine's observations and nothing more.
Installation detection is
only matching vocabulary; it cannot substitute its candidate executable for a
different running binary. Processes from the same harness family but distinct
executables therefore remain distinct HarnessInstances instead of being
collapsed or made unobservable by a later identity check.

An instance with several PIDs requires an explicit `--pid`. A projected
instance expectation has no live target PID and therefore cannot be observed;
the target must first complete its normal re-detection. Once re-detected, a
reading names the target Workcell and target-side material process while
retaining the stable HarnessInstance identity.

`--correlation-ref` values are opaque external references. Factory can place
an Execution or workflow reference there without Workcell interpreting or
owning those semantics.
