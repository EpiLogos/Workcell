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

The reference Ubuntu worker is a specimen, not the ontology. Later distribution is a placement/provider extension, not a reason to make a cluster framework part of semantic demand.
