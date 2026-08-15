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
  workspace · execution · project-runtime · service · artifact/storage
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

See [`CONTROL-SERVICE-AND-AGENT-HOSTING.md`](CONTROL-SERVICE-AND-AGENT-HOSTING.md).

## Persistent services and communication bindings

Workcell may materialise long-lived executable services with durable state, supervised lifecycle, health/readiness, credentials, logical endpoints, streaming/event-capable connectivity, ingress/egress and exposure policy.

Those are material affordances. The meaning of a CLI, TUI, GUI, messaging channel, API, webhook or other agent-facing Surface belongs to the owning application/Harness and to higher-level operational resolution such as AIKit. Workcell supplies and observes the processes, bindings, storage and network relations beneath such Surfaces.

Application protocols remain opaque unless a Workcell provider port genuinely owns a material property of that protocol. A WebSocket/HTTP/stdio/socket binding may be exposed without Workcell interpreting prompts, conversations, tool calls or Agent identity.

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

The next product tranche makes those territories directly inhabitable through the native CLI, a zero-setup local profile, an optional Control Service, persistent service/agent-host conformance, public SDK/conformance tooling, the reference Ubuntu remote topology and source-pinned gateway-management integrations.

The reference Ubuntu worker is a specimen, not the ontology. Hermes/OpenClaw gateway integrations are conformance targets, not a gateway ontology. Later distribution is a placement/provider extension, not a reason to make a cluster framework or harness protocol part of semantic demand.
