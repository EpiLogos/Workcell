# Workcell connectivity fabric

**Programme:** #26 / W23  
**Status:** architecture calibration against the canonical Fabric plane and current Tailscale/exe.dev reference systems

## Determination

Connectivity is already a Workcell responsibility. The canonical design names a **Fabric plane** and requires Projects/Runs to express logical connectivity while Workcell resolves the physical network relation.

The current Rust foundation carries the provider-neutral half of that law:

```text
ExecutionDemand
  connectivity: required | preferred | optional LogicalConnectionRequirement
        ↓
planner
  MatchRule::Connection
        ↓
provider offers
```

That is a correct portability floor but not yet the complete distributed-material answer. A distributed plan also needs to know whether the selected placements can actually communicate, how the relationship is materially bound, whether policy permits it, and what was observed after preparation.

The next implementation therefore deepens **networking as relationships** rather than adding a VPN-shaped semantic primitive.

## Three concerns that must remain separate

Remote operation often collapses three different things into one word such as "network" or "gateway":

```text
1. HOST ACQUISITION / BOOTSTRAP
   how a machine comes to exist or is initially managed

2. WORKCELL CONTROL CONNECTIVITY
   how a client reaches the optional Workcell Control Service

3. MATERIAL-WORLD FABRIC
   how executions/services placed by Workcell can reach each other
```

A single technology may participate in more than one concern, but the relations remain distinct.

Examples:

```text
exe.dev SSH API
    host acquisition / bootstrap specimen

Tailscale
    private connectivity / identity-aware fabric
    may carry Workcell Control Service traffic
    may also carry material service traffic

Workcell Control Service protocol
    semantic remote-control contract for Workcell operations
    independent of whether the path is Tailscale, ordinary TCP, SSH tunnel,
    local forwarding, cloud private network, or another fabric
```

This separation is important because replacing the bootstrap provider, network fabric, or physical route must not silently replace Workcell identity or caller semantic identity.

## Logical relationship before physical route

The client-facing demand should express the relationship it needs, not the path that should realise it.

A structured relation may need to preserve concepts equivalent to:

```text
NetworkRelationship {
    source_role
    destination_role_or_service
    necessity
    protocol_property?          # only when materially relevant
    reachability_scope?
    security_requirement?
}
```

The exact Rust/serialized shape is implementation work under #26. Existing string `LogicalConnectionRequirement` values can remain useful compatibility shorthand for the common case in which the materialised execution/world needs access to one named logical service.

The semantic boundary must not require:

```text
Tailscale IP
MagicDNS name
tailnet
tag
DERP region
SSH hostname
Docker bridge
host IP
cloud subnet
```

Those are candidate answers to the relationship, not the relationship itself.

## Material fabric binding

The prepared world needs an inspectable answer to each relevant logical relationship.

Conceptually:

```text
NetworkRelationship
       ↓ plan / resolve
MaterialFabricBinding
  logical relationship ref
  provider ref
  source placement
  destination placement/service binding
  effective reachability/exposure
  authorization/policy result
  concrete locator/path data
  health/presence
  provenance
```

The concrete name may differ. The important distinction is:

```text
logical relationship identity
        !=
provider path / address / route identity
```

A route may move from direct to relay, a service may move between hosts, or an endpoint may acquire a new address without creating a new logical relationship.

The existing `BindingGraph` is the natural place for this material answer. #26 should determine whether relationship lifecycle/observation requires a dedicated Fabric/Network provider port or can honestly be owned by an existing provider seam. The current `ProviderPortKind` is already non-exhaustive, so adding a material port is mechanically possible without declaring a new semantic F-node.

## Planning law

Provider-local token satisfaction is not enough for distributed planning.

A valid plan must establish:

```text
selected source placement
        +
selected destination/service placement
        +
available material path/fabric
        +
required policy/security relation
        ↓
realisable NetworkRelationship
```

If a required cross-placement relation cannot be realised, planning must fail before claiming a prepared world.

Preferred/optional relationships use the existing degradation/omission law.

Policy denial, destination absence, fabric-provider absence and path degradation are different observations and should not collapse into one generic "network unavailable" state.

## Why Tailscale is the first rich reference

Tailscale is useful because its current architecture independently exercises several distinctions the Workcell needs.

### Stable network identity versus physical network location

A tailnet gives devices stable private addressability even when their ordinary network location changes. Workcell should preserve the same higher-level rule without adopting Tailscale identity as Workcell identity.

### Stable service identity versus service host

Tailscale Services expose a named service independently from the particular host currently advertising it and can route to available hosts.

That is a strong external proof of the relation:

```text
logical service
       !=
host / process / endpoint currently realising it
```

A Workcell service binding should preserve the same separation regardless of provider.

### Policy versus reachability

Tailscale Grants/access policy separately determines whether a source may access a destination. Workcell should retain an inspectable material policy result where relevant without importing the tailnet policy ontology into ExecutionDemand.

### Logical destination versus current route

A Tailscale connection may be direct or relayed. That is useful provider/path provenance and may affect observed performance, but it does not create a different Workcell, service, Project, Run or Agent.

### Private versus public exposure

Tailscale Serve/service access and Tailscale Funnel demonstrate a materially consequential distinction between private-tailnet and public-internet exposure.

Workcell exposure planning must therefore remain explicit: a public path cannot silently satisfy a private-only requirement, and a private binding must not be reported as publicly reachable.

### SSH is a management surface, not the Workcell protocol

Ordinary SSH over a tailnet or Tailscale SSH may be useful for host administration and bootstrap. The Workcell Control Service still owns the versioned remote Workcell control contract. SSH must not become an implicit replacement API merely because the network provider can authorize it.

## Reference physical topology

The first intended live fabric acceptance mirrors the existing reference-server testing philosophy:

```text
USER WORKSTATION
  collapsed-local Workcell/client
        |
        | Tailscale private fabric
        |
REFERENCE UBUNTU HOME SERVER
  Workcell Control Service
  Workcell providers
  material services
```

Two separate relations must be proven across this topology:

```text
A. client -> Workcell Control Service
B. material execution/service -> another material service
```

Even if Tailscale supplies both, they are different bindings.

The live receipt should record enough evidence to distinguish:

- tailnet/provider availability;
- logical relationship refs;
- effective service/fabric bindings;
- access permitted/denied;
- endpoint/host substitution;
- route class where observable;
- service/control restart and reconciliation;
- provider loss/reappearance;
- private versus public exposure.

Actual workstation/home-server claims require actual output from those machines.

## Deterministic conformance before physical acceptance

Hosted tests should prove the generic laws without Tailscale installed:

- local/localhost or bridge realisation;
- fake private-overlay realisation;
- SSH-tunnel-style alternate realisation where useful;
- required path unavailable;
- preferred/optional path degradation;
- policy denial distinct from provider absence;
- source/destination placement change;
- endpoint/route rebinding without logical identity drift;
- public/private exposure mismatch;
- Control Service path distinct from material-world path.

The Tailscale adapter is then one provider-specific proof of those generic laws.

## exe.dev as a comparison specimen

Current exe.dev solves a different problem. Its programmatic API is SSH and can create persistent remote VMs with structured output; VM SSH/HTTPS reachability is managed by exe.dev.

That makes it useful for a second remote-Workcell proof:

```text
ssh exe.dev new ...
        ↓
remote VM
        ↓ bootstrap/install
Workcell Control Service
        ↓
run ordinary remote Workcell acceptance
```

This proves that `reference Ubuntu home server` is not the remote ontology and that `Tailscale fabric` is not the only way a remote machine can initially be acquired/reached.

It does **not** by itself prove Workcell needs a universal host-provisioning provider. #27 should let implementation evidence decide whether host acquisition becomes a reusable public port or remains deployment/bootstrap tooling.

## Ownership summary

```text
semantic client
    asks for logical connectivity / exposure

Workcell planner
    proves the relationship can be materially realised

Workcell Fabric / provider bindings
    realise and observe physical reachability

Tailscale / local bridge / SSH tunnel / cloud network / future provider
    concrete implementation choice
```

Tailscale is the reference fabric, not the ontology. exe.dev is a reference remote-bootstrap shape, not the ontology. The Ubuntu home server is a reference physical Workcell, not the ontology.
