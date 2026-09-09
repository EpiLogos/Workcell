# Material Development Worlds

**Development Field S5 / Workcell #67**

A Development World is not a new Workcell ontology. It is the existing `ExecutionDemand -> MaterialisationPlan -> MaterialisedExecutionWorld` relation used for software-development work while the semantic owners remain outside Workcell.

```text
Factory plan / caller refs
        |
        v
ExecutionDemand                 Workcell-owned material requirement
        |
        v
discover -> plan
        |
        v
prepare
        |
        v
MaterialisedExecutionWorld      stable binding graph + exact material provenance
        |
        +--> AIKit/native Git    repository/worktree/branch ownership stays here
        +--> Harness/Actuation   Agent/Agency/Run/session semantics stay here
        |
        v
inspect -> observe -> collect -> release / reconcile
```

## Ownership law

Workcell owns **where and under what material conditions** development can occur. It does not own why the work exists, which Git branch or worktree represents it, which Agent performs it, which Factory Run/Journey contains it, or which O:I suite revision is active.

`ExecutionDemand.subjects` may carry those caller identities as opaque `ExternalRef`s for correlation. Workcell must preserve them; it must not reinterpret them or derive material identity from them.

A material binding is therefore four different things kept distinct:

- `binding_ref`: identity of this binding relation inside the material world;
- `provider_ref`: identity of the provider satisfying it;
- `material_ref`: provider-owned identity of the concrete material allocation;
- `logical_ref`: the requested material role which lets the caller correlate the binding back to its demand.

Provider replacement, Workcell relocation, release, loss and recovery may change material standing without rewriting the caller's Project, Run, Agent, Candidate or suite refs.

## One provider-neutral demand

The existing `ExecutionDemand` is the Development World demand. It already expresses the required material dimensions without naming provider brands:

| Need | Existing contract |
| --- | --- |
| filesystem/workspace access | `WorkspaceRequirement` |
| writable attached state | tiered `StorageRequirement` |
| commands/process execution | tiered `AffordanceRequirement` + `ExecutionProvider` |
| tool/runtime class | affordance / `ProjectRuntimeRequirement` |
| network/reachability | tiered `LogicalConnectionRequirement` and Workcell Fabric |
| resource/capacity | `ResourceRequirement` |
| isolation/trust | `IsolationTrustRequirement` |
| persistence/lifetime | `PersistenceScope` |
| artifact/evidence collection | tiered `OutputRequirement` |
| cleanup semantics | `RetentionExpectation` |
| external semantic correlation | opaque `subjects: BTreeMap<String, ExternalRef>` |

Development Field code must not add Git branch/worktree, Factory Run/Journey, Harness brand, Agent or O:I suite fields to this demand.

## Binding and inspection

`prepare()` returns the canonical `MaterialisedExecutionWorld`. Its `BindingGraph` records every concrete `binding_ref -> provider_ref -> offer_ref -> material_ref` relation, properties, provenance, health and lifecycle presence.

`inspect(world_ref)` re-reads that canonical material world where the control-plane implementation has durable world state. The default trait implementation is deliberately `Unsupported`, preserving compatibility for external control planes that have not implemented durable inspection. `CollapsedLocalWorkcell` implements inspection and therefore exposes it identically in-process and through the Workcell Control Service.

The wire representation remains `workcell.material-world/v1`; inspection returns the same representation as preparation rather than inventing a Development-Field-specific receipt.

`observe()` remains live provider observation. Each prepared-world observation includes `provider_ref`, `material_ref` and lifecycle standing in its material detail, so cached binding state and live observation remain distinguishable.

`collect()` returns material locators and provider provenance from the ordinary artifact-storage contract. The canonical inspected binding graph remains the authority for the exact provider/material allocation that owns a requested output channel.

`release()` applies the world's provider-neutral retention expectation. Re-inspection exposes the resulting binding presence. `reconcile()` compares desired material state with live provider state; it may report `recover`, `lost`, `rematerialise`, `resume` or `restore`, but it does not silently fabricate a replacement allocation under an old binding.

## Placement reality

Development World is one protocol across placements; placement names are provider/deployment facts rather than new demand types.

| Placement | Current Workcell standing | What is proved now |
| --- | --- | --- |
| current machine | `CollapsedLocalWorkcell`, `workcell:local` | deterministic full prepare -> inspect -> observe -> collect -> release -> reconcile |
| isolated local container | Docker providers | deterministic provider/conformance coverage; live Docker remains separately environment-gated |
| sandbox / VM-like isolated region | OpenSandbox execution/checkpoint/material composition | source-pinned deterministic conformance, lifecycle-loss/recovery and local-vs-cluster endpoint parity |
| remote Workcell | placement seam + Workcell Control Service/TCP transport | same `ExecutionDemand` planning/control protocol and explicit transport/placement provenance |
| reference remote Ubuntu host | deployment profile / Control Service composition | deterministic profile/conformance; physical host acceptance remains separate |
| cloud/CI ephemeral | no universal cloud provider is claimed | use a genuinely configured remote/sandbox provider when present; otherwise the external CI/cloud environment remains external rather than receiving a fake Workcell identity |
| Arrakis | optional provider | deterministic adapter coverage only until physical Arrakis acceptance exists |

OpenSandbox already proves that one provider request is projected unchanged to materially distinct local-Docker-style and remote-cluster-style lifecycle endpoints, while the multi-Workcell placement tests prove re-placement can change Workcell/provider provenance without mutating external semantic subjects.

## `workcell:local`

`workcell:local` is the default current-machine material context, not a machine account or version selector.

Keep these facts separate:

```text
Central machine role / desired policy     authored intent
O:I active suite receipt                  installed software composition
Workcell workcell:local                   current material capability and actuality
Workcell instance/process readings        observed executable/process evidence
```

Current instance scanning and resource-use surfaces may provide executable/process evidence to O:I verification. That evidence does not authorize Workcell to decide which suite should be installed.

## Public operations

The native lifecycle is intentionally one surface:

```text
discover
plan(demand)
prepare(demand)
inspect(world)
observe(world)
expose(world)
collect(world)
release(world)
reconcile(desired material state)
```

The Client SDK exports the same demand, binding, world and lifecycle contracts and the Control Client uses the same Control Service protocol. No parallel Development World API or orchestration language is introduced.

## Deterministic proof and physical gates

Deterministic CI can prove contract shape, provider identity/provenance invariants, collapsed-local lifecycle, Control Service parity, OpenSandbox projection/lifecycle fixtures, placement/re-placement and failure reporting. Those checks are development evidence, not physical acceptance.

Remaining environment-dependent claims stay open until their environments are actually exercised: live Docker isolation, live OpenSandbox deployment as configured, reference Ubuntu remote host/control/fabric, optional Arrakis, and any future cloud provider. External GitHub/ChatGPT execution remains externally observed unless a real Workcell provider/control endpoint is bound to it.
