# Capacity admission

Workcell admission accounts agent work against the **actual machine** — its
observed memory envelope, cgroup limits and schedulable parallelism — so that
useful work is possible while the resident field (gateways, services, desktop,
other lanes' builds) is protected. Nothing here invents machine specifications:
every number in a readback is either an OS reading or a declared operator
value, and the policy that connects them is named in the same readback.

## The three distinct quantities

1. **Observed envelope** (`HostObservation`) — `/proc/meminfo` MemTotal and
   MemAvailable, the observing process's cgroup v2 `memory.max`/`memory.current`
   (`None` = unlimited), and schedulable parallelism. Re-observed at every
   composition; facts, never defaults.
2. **Task budget** (`CapacityBudget`) — the enforceable ceiling this Workcell
   may commit to new task allocations. Derived by a visible policy
   (`fraction-of-ceiling:40%,cpu-reserve:1` by default): 40% of the enforceable
   memory ceiling (cgroup limit when finite, else MemTotal), CPUs minus one
   reserved for the resident field. The policy is adjustable
   (`CollapsedLocalConfig::budget_policy`, or a fixed budget via
   `capacity_budget` for deterministic tests and operator-declared budgets);
   the *numbers* it operates on are always this host's readings.
3. **Committed allocations** (`AdmissionLedger`) — the sum of *minimum*
   resource floors of live material worlds, accounted **simultaneously**: a
   second allocation that independently fits but does not fit beside the first
   is refused with the actual reason. The ledger is durable at
   `<state-root>/admission.json` (schema `workcell.admission/v1`).

## Minimum is not a ceiling

A demand's `ResourceRequirement.minimum` is a **floor the host must be able to
satisfy**. Mapping it to a provider hard limit would cap the workload at
exactly its floor and kill it the moment it needed what it asked for. Only
`ResourceRequirement.maximum` — an explicit caller-authorised ceiling — becomes
an enforced limit:

- Docker maps `maximum` to `--memory`/`--cpus`; a minimum-only demand gets no
  hard flags and is admitted against the budget instead.
- The CLI demand grammar accepts `key=min..max[:unit]` (`memory=256..512:MiB`).
- `--demand-json` carries `maximum` per resource; the control codec preserves
  it end to end. Absent `maximum` decodes as absent — never silently zeroed.

OpenSandbox renders `minimum` into its lifecycle request as a scheduler
request (a floor the backend guarantees), which is that protocol's native
semantics; Workcell admission has already accounted the floor before any
provider prepares.

## Lifecycle integration

- **Prepare** admits before any provider prepares. A required floor the host
  cannot commit beside the rest raises `WorkcellError::Capacity` — the
  wire/error kind is `waiting-for-capacity`, with the actual reason naming the
  resource, the committed sum and the ceiling. It is a retryable, expected
  outcome (CLI exit code 11), never provider unavailability and never a silent
  downgrade to weaker placement: required placement remains required.
- **Failure** releases the reservation immediately (a failed prepare must not
  hold budget). The provider's own partial effects remain its documented
  cleanup responsibility; admission retires only the accounting.
- **Release** frees the committed budget through the same material lifecycle.
  A failed release leaves the reservation in place — the material may still
  exist.
- **Re-entry** (`register_world`) rebuilds the reservation from durable world
  provenance recorded at first prepare, so a world that returns after a host
  restart keeps occupying the budget it was admitted under.
- **Reconcile** retires reservations whose world is verifiably absent from the
  control plane's registered worlds. In-flight reservations (prepare started,
  no world yet) are kept — only presence facts retire them.

## Readback

`workcell status --json` carries an `admission` object: the budget (with its
policy name and observation timestamp), committed totals and per-reservation
records, what remains available, and the most recent `waiting` entries with
their actual reasons. This is the consumption surface for lanes that present
capacity to a human or another agent: read it, do not re-derive it.

The ledger keeps the last 32 declined admissions as bounded evidence — why a
demand waited, when, and against which budget — without accumulating an
unbounded audit trail on a small machine.

## Boundaries

- Admission is host-level and pre-provider: it proves the *budget* shape, not
  provider-specific capacity (a remote pool's occupancy is that provider's
  material state and stays there).
- `maximum` ceilings are enforced by the provider that supports them (Docker).
  A provider without ceiling support that receives a maximum either maps it or
  refuses by name; it never claims an unenforced ceiling.
- The budget protects against *simultaneous overcommitment by Workcell-placed
  material*. It is not a general OS memory guarantee: processes outside
  Workcell admission can still pressure the host; that is what the resident
  reserve and the observed envelope are for.
