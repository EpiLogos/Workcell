# Deployment profiles and reference specimens

**Ticket:** Workcell #12 / W11 / F.12; next tranche #20/#24/#26/#27  
**Status:** provider-neutral deployment composition over the public Workcell contract

## Principle

A deployment profile describes one physical arrangement capable of presenting the Workcell contract. It does not create a new semantic execution model.

The invariant surface remains:

```text
semantic ExecutionDemand
        ↓
Workcell discover / plan / prepare / observe / expose / collect / release / reconcile
        ↓
provider inventory + placement + fabric + bindings
```

Profiles may differ in Workcell identity, aggregate health, aggregate capacity, provider offers, fabric/path availability and opaque placement references. The semantic demand does not acquire hostnames, IP addresses, VPN brands, tailnets, VM provider names, container names, cluster objects or deployment-specific provider brands.

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
workcell client/SDK -> material connectivity -> Workcell Control Service -> core -> server providers
```

The material connectivity may be supplied by Tailscale, a conventional private route, SSH forwarding, cloud networking or another implementation. The Workcell Control protocol does not change with that path.

This distinction does not create `LocalWorkcell` and `ServerWorkcell` types. The Control Service is normal for a remotely operated Workcell but optional for ordinary local use.

## Proof specimens

The conformance programme uses deliberately different arrangements.

| Specimen | Shape | Example placement/connectivity |
|---|---|---|
| `collapsed-local` | zero-setup local operational domain using ordinary host/filesystem facilities first | `execution → same-host`, local/loopback fabric |
| `reference-ubuntu-worker` | intended Ubuntu home-server/worker specimen with independently configured provider inventory and normally a Control Service | `execution/state → worker-host`, private fabric supplied by #26 in the first rich live proof |
| `distributed-fake-provider` | provider roles intentionally placed in more than one opaque domain | `execution → compute-domain-a`, `state → state-domain-b`, deterministic fake fabric |
| `remote-exedev-specimen` | optional source-pinned cloud VM/bootstrap portability proof | host acquired through exe.dev SSH API, Workcell Control Service tested independently of that bootstrap API |

These strings are fixture/configuration values, not variants in the Rust type system. There is no `UbuntuWorkcell`, `TailscaleWorkcell`, `ExeDevWorkcell`, `LocalWorkcell`, `DistributedWorkcell`, semantic cluster type, `AgentGatewayWorkcell`, or Kubernetes prerequisite.

The Ubuntu specimen is important because it is the first rich physical remote deployment target. Tailscale is useful because it is the first rich private-fabric reference. exe.dev is useful because it is a materially different remote-host/bootstrap reference. Each proves an abstraction rather than defining it.

## Collapsed-local is the portability floor

An ordinary supported computer should be a valid Workcell before Docker, Arrakis, Tailscale, a dedicated server or a Workcell daemon is installed.

The baseline target for #20 is therefore approximately:

```text
workspace         local directory / Git-worktree facilities
execution         host-process provider
artifact storage  local filesystem provider
services          host-process/local logical bindings
fabric            local/loopback relationships
control           native CLI / embedded application path
```

Discovery may report richer optional providers when they exist. Installing Docker, Arrakis, Tailscale, a GPU provider or a remote Workcell changes the offer set; it does not change the meaning or identity of an existing provider-neutral demand.

A missing preferred isolation/snapshot/connectivity affordance must degrade explicitly. A missing required affordance must fail. Zero setup is not permission to pretend unavailable properties exist.

## Discovery

`Discovery` reports the operational domain directly:

```text
Workcell identity
health
aggregate capacity
provider offers
```

Provider-local capacity remains on individual `OperationalOffer` values. Aggregate capacity answers what the Workcell currently reports about its overall operational domain without erasing provider detail.

Fabric/network providers likewise report material capabilities and health rather than changing logical service or Workcell identity.

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
  realisable logical connectivity
preferred/optional:
  streaming/event ingress
  stronger isolation
  snapshot/recovery
```

The same demand must be testable against collapsed-local and reference-server profiles. Differences belong in offers, bindings, degradation and provenance, not in semantic identity.

A distributed variant must also prove that a required cross-placement logical relationship has a realisable material path before the plan is accepted.

`deployment_parity_report()` or its successor exposes the physical differences for inspection without converting them into semantic demand fields.

## Reference Ubuntu profile

A real Ubuntu home-server/worker profile can compose the established and next-tranche providers according to actual host availability, for example:

```text
workspace         Git/worktree provider
execution         host process, Docker and, when available, MicroVM/Arrakis
project runtime   host process and/or Docker Compose provider
artifact storage  directory/object provider
services          configured local or remote services
fabric            Tailscale private connectivity as first rich live reference
control           Workcell Control Service for remote operation
```

The exact provider inventory is deployment configuration. If Docker, Arrakis, Tailscale, a GPU or another optional provider is absent, discovery changes accordingly. Nothing in `ExecutionDemand` changes from provider-neutral requirement language.

No fixed worker hostname, Tailscale IP, MagicDNS name or tailnet belongs in the semantic contract. Addressing is a material binding concern.

### First rich physical connectivity receipt

#24 + #26 should eventually exercise the real topology:

```text
user workstation
        |
        | private Tailscale fabric
        |
Ubuntu home-server Workcell
        |
        ├─ Workcell Control Service
        └─ at least one material-world service
```

The receipt must prove **two separate logical relationships** even when the same fabric supplies both:

1. workstation/client → Workcell Control Service;
2. material execution/service → another Workcell-managed service.

It should then exercise policy denial, server restart/rejoin/reconcile, endpoint/service rebinding, and provider/path observation without identity drift. Direct versus relay path is provider provenance/quality, not logical identity.

Private exposure must remain distinct from public exposure. A provider's public ingress feature cannot silently satisfy a private-only requirement.

Physical claims remain evidence-bound: hosted CI may prove deterministic profile/conformance logic, but a claim that a real Docker Engine, Arrakis/KVM host, Tailscale path or Ubuntu machine satisfied a demand requires output from that actual environment.

## exe.dev remote bootstrap specimen

#27 adds a deliberately different remote-machine case.

Current exe.dev exposes a programmatic SSH management API capable of creating persistent VMs with structured output. The test shape is:

```text
source-pinned exe.dev API
    ssh exe.dev new ...
          ↓
remote VM
          ↓ install/register
Workcell Control Service
          ↓
ordinary remote Workcell acceptance
```

The SSH API is used to acquire/bootstrap the host. It is **not** the Workcell Control protocol and it does not imply a Tailscale dependency.

This specimen exists to answer two questions with evidence:

- can the remote Workcell contract survive a very different host-provisioning shape?;
- does reusable Workcell-owned host lifecycle actually emerge strongly enough to justify a public host-acquisition provider port, or is this better left as deployment/bootstrap tooling?

Do not decide the second question from one provider before running the proof.

## Profile: hosted services VM (desktop or workstation host)

A person's daily machine can also be asked to host long-lived service stacks: compose fleets, databases, vaults. When it is, those stacks belong **inside a VM whose disk lives on durable storage**, not in the host OS.

The reason is migration before isolation. A stack installed into the host OS entangles its lifecycle with the host's; relocating the Docker daemon's data-root, authoring containerd ordering units and moving compose trees are host-level operations, each performed against the machine a person is actively using. A VM boundary turns that entire class of work into a VM lifecycle operation: the disk image is the unit of state, and moving, snapshotting or rebuilding it leaves the host OS untouched.

```text
desktop host (ordinary daily-use OS, unchanged)
        |
        | rootless QEMU/KVM  (no hypervisor OS, no host root)
        |
services VM (disk image on durable storage)
        ├─ Docker and the service stacks
        ├─ hostfwd bindings on the service-facing address
        └─ 9p/virtfs share to the existing backup pool
```

The profile differs from the reference Ubuntu specimen in role. That specimen is a dedicated server; this one shares a machine with a person's daily work, which is why the boundary matters more, not less.

**Docker inside the VM is the considered default.** Docker in an LXC or other shared-kernel container is the weaker-isolation alternative: lighter in memory and startup, but a container-runtime or kernel escalation reaches the host a person's daily work depends on. Bare-metal Docker on a personal host is the anti-pattern this profile exists to prevent.

**Rootless QEMU/KVM is the desktop-compatible mechanism.** The host stays an ordinary desktop OS: no hypervisor OS and no root for the VM runtime. QEMU is extracted user-side from the distro packages, KVM acceleration comes through `/dev/kvm` group access, and the VM reaches the network through usermode NAT.

**hostfwd preserves existing exposures.** Service ports are forwarded bound to the service-facing address, for example the machine's tailnet address, so existing clients keep their configuration. Exposure is preserved across the migration, not reinvented, and the exposure declarations survive the move verbatim.

**A file share keeps the backup pipeline in place.** Backup jobs running inside the VM write through a 9p/virtfs file share onto their existing pool locations on the host's durable storage, so the host-side timer and any off-machine puller keep their shape.

### Evidence law

The hosting shape of a machine is claimed from the machine's own declaration, a Control machine record or equivalent source-owned statement, not reconstructed from what happens to be running when someone looks. As everywhere in this document, a claim that a real environment satisfied a demand requires output from that actual environment.

A migration into the services VM is complete only when the whole pipeline is verified against the new location: the backup timer produces dumps through the share and the off-machine puller collects from the pool before the previous arrangement is retired. The previous arrangement retires disabled, not deleted; its data is kept as a cold, verified-then-pruned copy until the VM fleet is confirmed good.

### What this profile does not cover

Fleet-scale sandboxing, meaning many isolated workloads under clustering, live migration or central management, is the Proxmox/cluster conversation, not a desktop-host profile. Sandbox-execution VMs, short-lived environments for scoped or untrusted work, belong to the OpenSandbox integration; see [`OPENSANDBOX-SOURCE-INTEGRATION.md`](OPENSANDBOX-SOURCE-INTEGRATION.md).

The Workcell contract is unchanged by any of this: the VM is where the providers and their durable state live, not a new semantic execution model.

## Distributed composition

A distributed Workcell remains one coherent operational resolution domain even when providers live in several places. The proof fixture uses opaque placement labels only. A production implementation may use ordinary SSH, Tailscale/private overlay, remote provider APIs, VMs, cloud private networking or other transport/fabric mechanisms without requiring a Kubernetes-shaped ontology.

The defining test is unchanged: the semantic client expresses the world and relationships it requires; the Workcell resolves where and how that world can be supplied, including whether the selected placements can actually communicate.
