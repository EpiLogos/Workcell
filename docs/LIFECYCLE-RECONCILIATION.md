# Lifecycle, recovery, and reconciliation

**Ticket:** Workcell #11 / W10 / F.11  
**Scope:** provider-neutral desired/observed reconciliation for already materialised worlds

## Invariants

A material binding is not its semantic subject. A provider handle may disappear, move, be suspended, be snapshotted, or be released without changing the `Project`, `Run`, `Candidate`, `Agent`, or other external semantic reference carried by the world.

`BindingPresence` records the lifecycle standing of the material side:

- `Present`
- `Missing`
- `Released`
- `Suspended`
- `Snapshotted`
- `Stale`

`MaterialisedExecutionWorld` retains its persistence and retention semantics alongside the binding graph. This is required for restart/recovery decisions; persistence is not inferred from provider brands or paths.

## Recovery after process restart

A durable world record may be registered into a fresh `PreparedWorldControlPlane` after process restart. Reconciliation then asks the currently registered provider to advertise the persisted offer and observe the persisted material allocation.

The recovery rule is deliberately conservative:

- successful observation recovers `Missing`/`Stale` provenance to `Present`;
- `NotFound` becomes `Missing`;
- a missing provider or withdrawn offer becomes `Stale`;
- no reconciliation path silently allocates a replacement beneath the old material identity.

Provider adapters that support restart recovery must therefore be able to reconstruct their operational handle from persisted allocation properties/provenance. Process-local maps may cache that information but cannot be its only source.

## Persistence-aware loss

When desired state is `present` but material is absent:

- ephemeral material is reported as `lost`;
- non-ephemeral material is reported as requiring `recover`;
- previously released material is reported as requiring `rematerialise`;
- suspended material is reported as requiring `resume`;
- snapshotted material is reported as requiring `restore`.

These are explicit reconciliation deltas. The current prepared-world control plane does not pretend that recovery/rematerialisation has happened when generic preparation is owned by a later orchestration layer.

## Lifecycle convergence

The currently accepted desired material states are:

- `present`
- `released`
- `suspended`
- `snapshotted`

Lifecycle actions are applied through the provider port that owns the existing binding. The resulting `ReleaseDisposition` is persisted back into `BindingPresence` after every successful provider operation.

This update is incremental by design. If cleanup succeeds for binding A and fails for binding B, A remains recorded as released/suspended/snapshotted rather than being rolled back to a fictitious pre-cleanup world state.

Repeated convergence is idempotent at the control-plane boundary. Once a binding already has the requested lifecycle presence, the provider operation is not repeated.

## Observation and health

`observe()` asks providers for live state. Cached `binding.health` is provenance, not sufficient observation by itself. Provider/resource disappearance is returned as an unavailable `MaterialObservation` carrying an `observation_error` instead of being hidden.

World health is derived from active binding health and lifecycle presence. A world with no present bindings is unavailable unless material is deliberately suspended/snapshotted, in which case it is degraded rather than falsely healthy.

## Deliberate boundary

`PreparedWorldControlPlane::prepare()` remains unsupported. F.11 does not collapse material preparation, recovery policy, or provider selection into the reconciliation loop.

Reconciliation identifies the delta and performs lifecycle operations that are valid on already bound material. Later orchestration may use `recover`, `rematerialise`, `resume`, or `restore` deltas to form a new provider-neutral plan while preserving semantic identity.
