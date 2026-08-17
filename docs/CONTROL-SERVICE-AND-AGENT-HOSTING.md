# Workcell Control Service and agent-hosting boundary

## Determination

Workcell owns the material conditions under which executable worlds can exist, persist, remain reachable and recover. It does not own the semantic identity of the Agent, Harness, session, Project or communication surface that uses those conditions.

The current agent ecosystem makes this boundary especially important. Systems may expose a persistent "gateway", daemon, API server, messaging bridge, RPC host or other long-lived service. Those names describe target-owned runtime/control arrangements. They do not define one universal Workcell protocol or a new Workcell ontology.

The Workcell law is:

> **Every Workcell has a control plane, but not every Workcell needs a continuously running Workcell Control Service.**

The second law is:

> **A gateway may host or mediate agency; a Workcell hosts and manages the material conditions under which that gateway can persist and remain reachable.**

## Two Workcell control shapes

Ordinary collapsed-local operation may remain fully embedded:

```text
human / agent / O:I alias
          ↓
     workcell CLI
          ↓
 application / Workcell core
          ↓
      local providers
```

No daemon is required merely to make a normal machine a Workcell.

A remote or server Workcell may expose the same control contract through a persistent service:

```text
workcell client / SDK
          ↓ authenticated transport/fabric binding
Workcell Control Service
          ↓
 application / Workcell core
          ↓
       providers
```

The service is a transport host for the Workcell control plane. It must not become a second planner, a second material-world store or an application-data proxy.

The canonical semantic operations remain:

`discover · plan · prepare · observe · expose · collect · release · reconcile`

Transport, authentication, fabric provider and service-process identity remain below that contract.

## Control protocol is not connectivity fabric

Remote control has two independent questions:

```text
What Workcell operation/protocol is being invoked?
        !=
How does this client physically reach the Control Service?
```

The first belongs to #21 and must remain versioned and transport-neutral.

The second belongs to material connectivity/fabric resolution. Tailscale is the first rich reference under #26, but an ordinary private TCP route, SSH tunnel/forward, cloud private network or future fabric may carry the same Control Service protocol.

A Tailscale node ID/IP/MagicDNS name, SSH destination, URL, port or tunnel is therefore a binding/provenance detail. None is a `WorkcellRef` or remote-control semantic identity.

The same distinction applies inside a materialised world: a Control-Service path and a hosted-service path are separate `NetworkRelationship`s even when the same private fabric happens to realise both.

See [`CONNECTIVITY-FABRIC.md`](CONNECTIVITY-FABRIC.md).

## Persistent executable services

A materialised world may legitimately require a long-lived executable plus services around it. The Workcell requirement vocabulary must be able to express material properties such as:

- long-lived or restartable execution;
- writable durable state;
- supervised lifecycle and reconciliation;
- health/readiness observation;
- authenticated control endpoint;
- streaming-capable or event-capable connectivity;
- inbound and outbound network reachability;
- exposure scope and policy;
- logical service bindings;
- credentials/secrets bound through an appropriate provider seam.

These are ordinary material affordances. They do not imply a canonical `Gateway`, `AgentHost` or `AgentGateway` semantic object.

An agent-hosting conformance fixture may compose these requirements to represent a realistic persistent agent workload. The fixture is a demand pattern, not a Workcell type.

## Communication surfaces versus material bindings

A user may encounter one persistent agent through many surfaces:

```text
CLI
TUI
GUI
messaging application
HTTP/API
webhook / event trigger
editor or application integration
```

The meaning of those surfaces and their relation to an Agent/Harness/session belongs above Workcell, currently in AIKit's runtime-composition and Surface model.

Workcell may materialise the physical services that support them. For example:

```text
logical binding: agent-control
kind: interactive-stream
transport: websocket
scope: private-network
health: ready

logical binding: agent-events
kind: event-ingress
transport: https
scope: authenticated-external

logical binding: operator-shell
kind: terminal
transport: local-stdio
scope: local
```

Those descriptors communicate material reachability. Workcell must not infer that `agent-control` is a particular Agent, interpret prompts/tool calls/conversations, or make a messaging provider part of semantic Agent identity.

Application protocols remain opaque unless a provider port genuinely owns a material property of that protocol.

A real private fabric also sharpens exposure semantics. Private tailnet/service reachability and public internet exposure are different material properties. A provider such as Tailscale may support both private Serve/Services and explicitly public Funnel, but Workcell must never widen the effective exposure silently.

## Gateway-management interoperability

Hermes, OpenClaw and later systems are useful conformance targets because they make persistent agent-hosting requirements concrete. They must be integrated from their actual current management surfaces rather than from a remembered or invented common gateway API.

A Workcell integration may:

- discover an already-installed compatible service;
- start/stop/supervise a service when the target supports that relation;
- bind target-owned state directories and credentials;
- observe health/readiness;
- expose or withdraw logical endpoints;
- reconcile process/service loss;
- report target/provider provenance;
- host the service on a different Workcell when the same provider-neutral demand can be satisfied there.

It must not:

- copy target-owned Agent/Harness/session semantics into Workcell core;
- translate two unrelated gateway protocols into a fake universal `GatewayProtocol`;
- treat process, endpoint, container, daemon or gateway IDs as caller semantic identity;
- own the user's complete target configuration merely because it can supervise the process;
- turn the Workcell Control Service itself into an agent gateway.

## Remote-host bootstrap is separate again

A remote host may already exist, or it may be acquired through a separate deployment/bootstrap facility. Current exe.dev is a useful comparison because its programmatic API is SSH and creates persistent VMs with target-owned SSH/HTTPS reachability.

That yields a third separation:

```text
exe.dev SSH API / other host acquisition
        ↓ bootstrap machine
Workcell Control Service
        ↓ carried over some network/fabric
Workcell operations
```

SSH used to create/manage the host does not become the Workcell Control protocol. The existence of one convenient VM API does not by itself justify a universal Workcell `HostProvider`; #27 is the evidence-gathering portability proof.

## Cross-product responsibility

The intended boundary is:

```text
Factory
  authors/develops why an agency arrangement should exist
  and what evidence proves the developmental act
        ↓
AIKit
  resolves Agent / Agency / Harness / session / capabilities
  and the Surfaces through which that agency is encountered
        ↓
Workcell
  resolves processes / services / bindings / storage / network
  and keeps the material arrangement alive and observable
```

This is not a mandatory runtime pipeline. It is an ownership distinction for cases where all three concerns are present.

Changing a communication adapter must not by itself change Agent identity. Moving a material host from local process to remote server must not by itself change Project/Run/Agent identity. Restarting the Workcell Control Service must not by itself change a material world whose recoverable bindings still identify the same world. Changing direct/relay network path or a concrete endpoint must not by itself change logical service identity.

## Next Wayfinder

The implementation programme is tracked by:

- #19 — native CLI and agent-operable Workcell surface;
- #20 — zero-setup collapsed-local Workcell;
- #21 — optional Workcell Control Service and remote control protocol;
- #22 — service bindings and persistent agent-hosting conformance;
- #23 — client SDK, provider SDK and conformance kit;
- #24 — reference Ubuntu remote Workcell and local/server parity;
- #25 — gateway-management interoperability with Hermes and OpenClaw;
- #26 — Workcell Fabric and Tailscale reference conformance;
- #27 — remote Workcell bootstrap portability with exe.dev.

Physical Docker and Arrakis acceptance remain separately owned by #10 and #13. Their absence does not block deterministic implementation that does not claim live provider evidence. Physical Tailscale/home-server claims under #26/#24 likewise require actual workstation/server output rather than hosted fixtures.
