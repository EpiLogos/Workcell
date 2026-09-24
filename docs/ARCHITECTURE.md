# Workcell architecture

Workcell answers one question: **given a provider-neutral material execution demand, what executable world can this Workcell materialise here?**

```text
semantic client
  owns purpose + canonical identity
        |
        v
ExecutionDemand
        |
        v
Workcell core
  identity · discovery · offers · capacity · planning
        |
        v
provider ports
  workspace · execution · project-runtime · service · artifact/storage · fabric as proven
        |
        v
BindingGraph + MaterialisedExecutionWorld
        |
        v
physical resources / native data plane
```

## Independence

The client boundary is deliberately narrow. Workcell preserves opaque semantic refs for provenance but does not require a Factory ontology or Factory package to function. Cross-repository fixture conformance is an adapter/interop concern.

Workcell does not own Agent, Harness, AgentSession, communication-surface, Project or Run semantics. A persistent agent gateway, daemon, API host, messaging bridge or runtime service can be materialised and managed by a Workcell without becoming a Workcell semantic primitive.

## Control plane and Workcell Control Service

Every Workcell has a control plane. Not every Workcell needs a continuously running Workcell Control Service.

The canonical operations remain:

`discover · plan · prepare · observe · expose · collect · release · reconcile`

A collapsed-local Workcell may execute those operations directly through the native CLI/application/core without a daemon. A remote/server Workcell may expose the same contract through an authenticated long-running **Workcell Control Service**.

```text
collapsed local
workcell CLI -> application/core -> local providers

remote/server
workcell client/SDK -> authenticated transport -> Workcell Control Service -> application/core -> providers
```

The Control Service is a transport host for the Workcell control plane. It is not a second planner, an agent gateway, a Harness, a session host or a universal application-data proxy. Service-process identity and transport choice do not become material-world or caller semantic identity.

The protocol and the path that carries it are distinct. A Control Service may later be reached across Tailscale, ordinary private TCP, SSH forwarding, a cloud private network or another fabric without changing the Workcell operation semantics.

See [`CONTROL-SERVICE-AND-AGENT-HOSTING.md`](CONTROL-SERVICE-AND-AGENT-HOSTING.md).

## Connectivity / Fabric plane

The canonical Workcell design already owns networking as **logical relationships resolved into material connectivity**. The Rust foundation currently expresses the portable side through `ExecutionDemand.connectivity` and provider-offer matching. The next fabric tranche makes the resolved relationship itself inspectable in planning, bindings and observation.

Keep three things separate:

```text
host acquisition/bootstrap
    how a machine is created or initially managed

Workcell control connectivity
    how a client reaches the Workcell Control Service

material-world fabric
    how Workcell-placed executions/services reach each other
```

One technology may participate in several layers, but it does not collapse them.

Tailscale is the first rich reference fabric because it provides a serious private-network shape with stable device/service addressability, policy, service/host separation and multiple physical path modes. It remains a provider/reference implementation, not Workcell identity or semantic demand vocabulary.

Current exe.dev is a useful contrasting remote-host/bootstrap specimen because its management API is SSH and the resulting VM has target-managed SSH/HTTPS reachability. It is not the fabric ontology and does not make SSH the Workcell Control protocol.

See [`CONNECTIVITY-FABRIC.md`](CONNECTIVITY-FABRIC.md) and #26/#27.

## Persistent services and communication bindings

Workcell may materialise long-lived executable services with durable state, supervised lifecycle, health/readiness, credentials, logical endpoints, streaming/event-capable connectivity, ingress/egress and exposure policy.

Those are material affordances. The meaning of a CLI, TUI, GUI, messaging channel, API, webhook or other agent-facing Surface belongs to the owning application/Harness and to higher-level operational resolution such as AIKit. Workcell supplies and observes the processes, bindings, storage and network relations beneath such Surfaces.

Application protocols remain opaque unless a Workcell provider port genuinely owns a material property of that protocol. A WebSocket/HTTP/stdio/socket binding may be exposed without Workcell interpreting prompts, conversations, tool calls or Agent identity.

Private reachability and public exposure remain distinct material properties. A provider may offer both, but a public path must not silently satisfy private-only intent and a private binding must not be reported as public.

For where long-lived service stacks physically belong when the host is a person's daily machine, see the hosted services VM profile in [`DEPLOYMENT-PROFILES.md`](DEPLOYMENT-PROFILES.md).

## Full destination

The implementation must cover all canonical Workcell territories without collapsing them:

- F.01 external contract
- F.02 ExecutionDemand
- F.03 OperationalOffer / planning
- F.04 provider ports
- F.05 WorkspaceProvider
- F.06 Docker providers
- F.07 optional Arrakis provider
- F.08 ProjectRuntimeProvider / ServiceProvider
- F.09 BindingGraph / MaterialisedExecutionWorld
- F.10 Candidate materialisation relation
- F.11 reconciliation / lifecycle / recovery
- F.12 deployment profiles

The Fabric/networking work in #26 completes an already-canonical part of those responsibilities. It is **not** a new F.13.

The next product tranche makes those territories directly inhabitable through the native CLI, a zero-setup local profile, an optional Control Service, persistent service/agent-host conformance, public SDK/conformance tooling, the reference Ubuntu remote topology, real connectivity-fabric conformance, source-pinned remote bootstrap and gateway-management integrations.

The reference Ubuntu worker is a specimen, not the ontology. Tailscale is a reference fabric, not the ontology. exe.dev is a reference remote-bootstrap shape, not the ontology. Hermes/OpenClaw gateway integrations are conformance targets, not a gateway ontology. Later distribution is a placement/provider extension, not a reason to make a cluster framework, VPN brand or harness protocol part of semantic demand.

### Existing runs and exact task scopes

`workcell --json run list --full` returns complete `workcell.run/v1` records in
one owner read; ordinary `run list` keeps its short listing. Stored scope files
are not runs. `run show --run SLUG` also returns `run_revision`, the digest of the
exact native record. The ledger serializes writes and supports a scope's
compare-and-swap against the actual record inspected, so concurrent release
cannot be overwritten by scope preparation.

`run scope --run SLUG --expected-demand-digest DIGEST --write-boundary FILE`
prepares the supplied `workcell.write-boundary/v1` requirements verbatim. The
policy ref, revision, authority, expiry, writable/protected paths and coverage
belong to their existing owners. Workcell does not synthesize any of them. The
selected run's exact worktree must appear in that boundary; a parent path is
not substituted and the owner never silently widens the request. Unsupported
protection returns the explicit unavailable boundary; an execution consumer
must refuse it. This operation reuses existing material and never creates a
worktree.

An Agency-bearing run is admitted by the actual Actuation
`agency actualise` operation using bounded exact source bytes. The returned
receipt must preserve the request, binding, bounds, authority and source
lineage. `run start --agency-ref REF --agency-source PATH --agency-rev REV`
retains this native admission; a caller's asserted binding-revision string is
not proof. Scope preparation re-admits an attached Agency and verifies the
requested task Action and authority. A previously material-only run can attach
an admitted source at scope preparation with the same Agency flags plus
`--expected-agency-digest blake3:DIGEST`; a different existing Agency basis is
refused. No source policy is rewritten.

An arbitrary `--place-grant` file is not accepted as an owned place. A scope
without a native place receipt keeps that field absent. Material preparation
and a live agent/terminal remain different claims.

### Native machine, credential and run disclosures

`workcell system --json` includes machine declarations, recorded connections,
control-service declarations, credential-reference inventory, projection
metadata and material runs. These are read-only observations of native records.
They do not connect to remote machines, resolve secrets, or treat a saved run
status as current provider health. Each unavailable live axis names the native
operation needed for a current reading.

`workcell --json secret list` returns `workcell.secret-reference-inventory/v1`:
known refs, schemes and names from machine declarations, connection receipts and
projection records. It does not enumerate the complete origin store. Native
providers do not supply credential creation dates here; these remain null.
No secret material is read. The response explicitly reports this partial
coverage rather than mistaking an empty reference list for an empty vault.

Prepared run scopes use `<slug>-<run-revision>.scope.json` by default so a new
scope revision preserves earlier receipts. Workcell canonicalizes the already
existing provider worktree before comparing exact owner boundary paths; it does
not substitute an ancestor or create a replacement directory.
