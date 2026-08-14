# Deployment profiles and reference specimens

**Ticket:** Workcell #12 / W11 / F.12  
**Status:** provider-neutral deployment composition over the public Workcell contract

## Principle

A deployment profile describes one physical arrangement capable of presenting the Workcell contract. It does not create a new semantic execution model.

The invariant surface remains:

```text
semantic ExecutionDemand
        ↓
Workcell discover / plan / observe / expose / collect / release / reconcile
        ↓
provider inventory + placement + bindings
```

Profiles may differ in Workcell identity, aggregate health, aggregate capacity, provider offers and opaque placement references. The semantic demand does not acquire hostnames, IP addresses, container names, cluster objects or deployment-specific provider brands.

`DeploymentProfile` is therefore data. It contains:

- an ordinary `WorkcellRef`;
- aggregate `HealthState`;
- provider-neutral capacity entries;
- opaque role → `PlacementRef` relations;
- descriptive metadata.

It produces the same `PreparedWorldControlPlane` used by every other current Workcell composition.

## Proof specimens

The conformance fixture uses three deliberately different arrangements.

| Specimen | Shape | Example placement |
|---|---|---|
| `collapsed-local` | all relevant roles may be supplied by one local operational domain | `execution → same-host` |
| `reference-ubuntu-worker` | the intended Ubuntu worker-laptop specimen, with its provider inventory configured independently | `execution/state → worker-host` |
| `distributed-fake-provider` | provider roles intentionally placed in more than one opaque domain | `execution → compute-domain-a`, `state → state-domain-b` |

These strings are fixture/configuration values, not variants in the Rust type system. There is no `UbuntuWorkcell`, `LocalWorkcell`, `DistributedWorkcell`, semantic cluster type, or Kubernetes prerequisite.

The Ubuntu specimen is particularly important: it is the first rich deployment target described by the Workcell architecture, but it must continue to prove the abstraction rather than define it.

## Discovery

`Discovery` reports the operational domain directly:

```text
Workcell identity
health
aggregate capacity
provider offers
```

Provider-local capacity remains on individual `OperationalOffer` values. Aggregate capacity answers what the Workcell currently reports about its overall operational domain without erasing provider detail.

Optional provider disappearance changes the offer set. It does not mutate the caller's `ExecutionDemand` or semantic subject refs.

## Parity fixture

`deployment_profiles.rs` sends the same provider-neutral demand to all three profiles:

```text
required affordance: shell
optional affordance: gpu
```

The proof expects:

- the same demand identity and requirement vocabulary across profiles;
- a satisfiable plan in each profile when `shell` is offered;
- the same optional omission when `gpu` is absent;
- different Workcell identities/capacity/health where configured;
- different provider refs and physical placement metadata;
- optional-provider removal to alter discovery offers only;
- no reference-deployment terminology in the semantic demand source;
- no Kubernetes/cluster ontology in the deployment-profile implementation.

`deployment_parity_report()` exposes the physical differences for inspection without converting them into semantic demand fields.

## Reference Ubuntu profile

A real Ubuntu worker profile can compose the already established providers according to actual host availability, for example:

```text
workspace         Git/worktree provider
execution         Docker and, when available, MicroVM/Arrakis provider
project runtime   Docker Compose provider
artifact storage  directory/object provider
services          configured local or remote services
```

The exact provider inventory is deployment configuration. If Docker, Arrakis, a GPU or another optional provider is absent, discovery changes accordingly. Nothing in `ExecutionDemand` changes from `requires isolated execution`, `requires project:self`, `requires browser/application exposure`, or the other provider-neutral requirement language.

No fixed worker hostname or IP belongs in the semantic contract. Addressing is a material binding concern.

## Distributed composition

A distributed Workcell remains one coherent operational resolution domain even when providers live in several places. The proof fixture uses opaque placement labels only. A later production implementation may use ordinary SSH, remote provider APIs, VMs, cloud resources or other transport/fabric mechanisms without requiring a Kubernetes-shaped ontology.

The defining test is unchanged: the semantic client expresses the world it requires; the Workcell resolves where and how that world can be supplied.
