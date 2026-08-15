# Deployment profiles and reference specimens

**Ticket:** Workcell #12 / W11 / F.12; next tranche #20/#24  
**Status:** provider-neutral deployment composition over the public Workcell contract

## Principle

A deployment profile describes one physical arrangement capable of presenting the Workcell contract. It does not create a new semantic execution model.

The invariant surface remains:

```text
semantic ExecutionDemand
        ↓
Workcell discover / plan / prepare / observe / expose / collect / release / reconcile
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

It produces the same material-world/control-plane semantics used by every other Workcell composition.

## Control shape is also deployment data

A profile may be operated locally through the embedded/native command path or remotely through the optional Workcell Control Service.

```text
collapsed local
workcell CLI -> core -> local providers

reference server
workcell client/SDK -> authenticated Workcell Control Service -> core -> server providers
```

This distinction does not create `LocalWorkcell` and `ServerWorkcell` types. The Control Service is normal for a remotely operated Workcell but optional for ordinary local use.

## Proof specimens

The conformance fixture uses deliberately different arrangements.

| Specimen | Shape | Example placement |
|---|---|---|
| `collapsed-local` | zero-setup local operational domain using ordinary host/filesystem facilities first | `execution → same-host` |
| `reference-ubuntu-worker` | intended Ubuntu server/worker specimen with independently configured provider inventory and normally a Control Service | `execution/state → worker-host` |
| `distributed-fake-provider` | provider roles intentionally placed in more than one opaque domain | `execution → compute-domain-a`, `state → state-domain-b` |

These strings are fixture/configuration values, not variants in the Rust type system. There is no `UbuntuWorkcell`, `LocalWorkcell`, `DistributedWorkcell`, semantic cluster type, `AgentGatewayWorkcell`, or Kubernetes prerequisite.

The Ubuntu specimen is important because it is the first rich remote deployment target, but it must continue to prove the abstraction rather than define it.

## Collapsed-local is the portability floor

An ordinary supported computer should be a valid Workcell before Docker, Arrakis, a dedicated server or a Workcell daemon is installed.

The baseline target for #20 is therefore approximately:

```text
workspace         local directory / Git-worktree facilities
execution         host-process provider
artifact storage  local filesystem provider
services          host-process/local logical bindings
control           native CLI / embedded application path
```

Discovery may report richer optional providers when they exist. Installing Docker, Arrakis, a GPU provider or a remote Workcell changes the offer set; it does not change the meaning or identity of an existing provider-neutral demand.

A missing preferred isolation/snapshot affordance must degrade explicitly. A missing required affordance must fail. Zero setup is not permission to pretend unavailable properties exist.

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

## Parity fixtures

The basic parity fixture sends the same provider-neutral demand to all deployment profiles:

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

The next parity tranche also uses a realistic persistent-service/agent-hosting demand:

```text
required:
  long-lived execution
  writable durable state
  supervised lifecycle
  authenticated interactive binding
  health/readiness observation
preferred/optional:
  streaming/event ingress
  stronger isolation
  snapshot/recovery
```

The same demand must be testable against collapsed-local and reference-server profiles. Differences belong in offers, bindings, degradation and provenance, not in semantic identity.

`deployment_parity_report()` or its successor exposes the physical differences for inspection without converting them into semantic demand fields.

## Reference Ubuntu profile

A real Ubuntu worker/server profile can compose the established and next-tranche providers according to actual host availability, for example:

```text
workspace         Git/worktree provider
execution         host process, Docker and, when available, MicroVM/Arrakis
project runtime   host process and/or Docker Compose provider
artifact storage  directory/object provider
services          configured local or remote services
control           Workcell Control Service for remote operation
```

The exact provider inventory is deployment configuration. If Docker, Arrakis, a GPU or another optional provider is absent, discovery changes accordingly. Nothing in `ExecutionDemand` changes from provider-neutral requirement language.

No fixed worker hostname or IP belongs in the semantic contract. Addressing is a material binding concern.

Physical claims remain evidence-bound: hosted CI may prove deterministic profile/conformance logic, but a claim that a real Docker Engine, Arrakis/KVM host or Ubuntu machine satisfied a demand requires output from that actual environment.

## Distributed composition

A distributed Workcell remains one coherent operational resolution domain even when providers live in several places. The proof fixture uses opaque placement labels only. A production implementation may use ordinary SSH, remote provider APIs, VMs, cloud resources or other transport/fabric mechanisms without requiring a Kubernetes-shaped ontology.

The defining test is unchanged: the semantic client expresses the world it requires; the Workcell resolves where and how that world can be supplied.
