# OpenSandbox provider source integration

**Ticket:** Workcell #43 / W26  
**O:I convergence:** `EpiLogos/O-I#155` → `EpiLogos/O-I#97`  
**Integration mode:** protocol-first OpenSandbox lifecycle/execd/egress adapter behind Workcell material contracts

## Relation

OpenSandbox is a compound material provider beneath Workcell. It is not a second Workcell ontology and a provider-native sandbox ID never replaces a Workcell `WorldRef`, caller Project/Run/Candidate/Agent identity, or Central source authority.

The operative relation is:

```text
semantic client / Factory / AIKit
        ↓ provider-neutral ExecutionDemand
Workcell semantic control
  embedded core | Workcell Control Service
        ↓ provider adapter
OpenSandbox lifecycle API
        ↓ provider-owned materialisation
Docker | Kubernetes | pools | secure runtimes
        ↓ native endpoint resolution
OpenSandbox data plane
  execd | files | user services | browser | desktop | code-server
```

Workcell plans, binds, observes and reconciles the material world. OpenSandbox owns how a sandbox is provisioned and how its native data plane operates. Workcell does not proxy `execd`, browser, desktop or application protocols after a material endpoint has been resolved.

## Inspected upstream seam

This integration is source-pinned because OpenSandbox currently releases several independently versioned components rather than one umbrella product version.

| Upstream seam | Pinned source |
|---|---|
| Repository | `opensandbox-group/OpenSandbox` |
| Source revision | `173a576d3afcd1fb9ab116b4c1353b2f4b0848d1` |
| Lifecycle OpenAPI blob | `8723921f2599e2e349428af3b864f30a5987e9a8` |
| execd OpenAPI blob | `ccfbce2d4330ec8a70ea359206fab17afa9e7a98` |
| Lifecycle API version in spec | `0.1.0` |
| execd default port | `44772` |
| egress sidecar port | `18080` |

The adapter is intentionally written against the upstream protocol specifications and injectable HTTP transport. It does not depend on the `osb` CLI as Workcell's provider control seam and does not require an upstream Rust SDK.

Provider provenance records the inspected source/spec revisions on allocations, checkpoints, native data-plane readings and provider-specific material receipts. Runtime deployment facts remain separately observable provider material state.

## One physical sandbox, several Workcell bindings

An OpenSandbox sandbox is a physical parent for several distinct material relations. A single prepared world may therefore contain, for example:

- an `Execution` binding whose `material_ref` is the OpenSandbox sandbox ID;
- a `Storage` binding for durable Project state;
- a service/native endpoint relation for an application, browser, desktop or code-server port;
- an external egress policy binding;
- a credential broker materialisation receipt.

`compose_opensandbox_material_world(...)` delegates canonical `WorldRef` and `BindingGraph` construction to Workcell core. It records the shared sandbox material ref in provenance on sibling bindings so the physical composition is inspectable without promoting the sandbox ID into semantic identity.

Provider removal or a Docker↔Kubernetes placement change therefore changes availability/provenance. It does not rewrite the Project, Agent, demand or Workcell world identity.

## Lifecycle mapping

`OpenSandboxExecutionProvider` maps the provider-owned lifecycle API to existing Workcell execution and lifecycle contracts:

- `prepare_execution` → `POST /sandboxes`;
- `observe_execution` → `GET /sandboxes/{sandboxId}`;
- discovery/availability → bounded sandbox-list probe;
- `Release` → sandbox deletion;
- `Preserve` → no provider lifecycle mutation;
- `SuspendIfSupported` → pause;
- `SnapshotIfSupported` → snapshot/checkpoint;
- lease observation → sandbox `expiresAt`;
- lease renewal → `renew-expiration`;
- checkpoint → OpenSandbox snapshot with a reusable provider-local checkpoint ref.

A Workcell `MaterialCheckpoint` retains source material ref, checkpoint ref, state, reusability and source-pinned provenance. Its checkpoint ref can be supplied directly to `OpenSandboxConfig::from_snapshot(...)` when rematerialising through the same provider family.

Lease/expiry is deliberately distinct from `PersistenceScope` and `RetentionExpectation`: persistence says which material lifetime matters to the caller; retention says what release should attempt; a lease records when the provider may independently terminate the current allocation.

## Resources and provider backend shape

CPU/memory/GPU-style resource requirements are rendered into the OpenSandbox lifecycle request without adding OpenSandbox, Docker or Kubernetes vocabulary to `ExecutionDemand`.

OpenSandbox's lifecycle server already abstracts its Docker and Kubernetes backends. Workcell therefore does not add a runtime-brand field merely to select those implementations. Deterministic conformance sends the same `ExecutionMaterialRequest` through local/Docker-shaped and remote/Kubernetes-shaped lifecycle endpoints and requires the material request body to remain identical while provider endpoints/material IDs differ.

Pool IDs, node names, Pod names, container IDs and runtime classes remain provider provenance/capacity facts. Pool exhaustion is material unavailability/capacity pressure, not semantic identity.

## Attached storage

W26 promotes attached storage into a real Workcell provider port because OpenSandbox demonstrates that execution runtime and mounted storage are independent material dimensions.

`StorageProvider` is distinct from:

- `WorkspaceProvider`, which materialises source/revision/access provenance; and
- `ArtifactStorageProvider`, which collects output channels after or during execution.

The reference `OpenSandboxPvcStorageProvider` binds already-existing provider-native named storage. OpenSandbox maps the `pvc` volume backend to a Docker named volume or Kubernetes PVC according to its runtime backend. The Workcell storage binding records that external lifecycle explicitly: releasing the binding does not claim to delete the underlying volume/PVC.

`OpenSandboxVolumeMount::pvc_from_storage_allocation(...)` converts the selected Workcell Storage allocation into the provider-native OpenSandbox mount request while retaining the storage provider/material identity in Workcell provenance.

## Central Project World

`OpenSandboxProjectWorldMaterialiser` materialises a Central Project World into the sandbox through native execd file operations.

The source remains Central. The OpenSandbox filesystem is a material presentation of that authored Project context, not a new knowledge authority.

The materialiser:

- stages ordinary Project files;
- stages the Project-local `ProjectCentral` tree;
- records the Central source root/path in provenance;
- writes through native execd file operations;
- reads staged files back through the native execd file endpoint and verifies the bytes;
- rejects a root `Control/` directory rather than copying the user's whole Central control/personal context into the Project World.

This preserves the existing Central distinction: Project-owned temporal/working context belongs in ProjectCentral; root Control remains outside an ordinary Project materialisation unless a separate authorised relation explicitly supplies something from it.

## Native data plane

OpenSandbox's native data plane remains native.

Workcell exposes two bounded adapter surfaces:

1. execution operation `command`, which resolves the sandbox's execd endpoint and uses execd's command/SSE protocol; and
2. `OpenSandboxDataPlane::read_file(...)`, which resolves execd and uses its native `/files/download` endpoint.

`endpoint_reading(...)` resolves any provider-native sandbox service port. This is sufficient for application, browser, desktop and code-server surfaces without introducing browser/desktop/code-server ontologies into Workcell. The caller receives the endpoint and required authentication **header names**; provider-returned header values remain inside the provider/data-plane request boundary.

A provider-native service can therefore move from one physical endpoint to another while its logical exposure relation remains stable.

## External connectivity and egress policy

W26 separates two material relations that some providers happen to bundle:

- **path/reachability** — how source material can actually reach a destination; and
- **policy enforcement** — whether a provider permits a logical flow over an otherwise available path.

Workcell Fabric can now target an external endpoint rather than requiring every destination to be another `WorkcellRef`.

`OpenSandboxEgressPolicyProvider` implements the policy-enforcement seam only. It renders external endpoint policy into OpenSandbox's sandbox network policy and never advertises itself as a `FabricPathProvider`. A Tailscale path, local Docker/Kubernetes path, cloud network or another provider may independently supply actual reachability.

This allows the BindingGraph/evidence to say both **how the path exists** and **which component enforces the policy** instead of collapsing those facts into one provider abstraction.

## Credential Vault / use without read

OpenSandbox Credential Vault is a provider-native credential broker/materialisation sink, not the source secret provider.

Workcell retains the complete authorisation relation in `OpenSandboxCredentialMaterialisation`: source `SecretProvider`, broker policy/handle, `SecretMaterialisationRequest`, authorised `BrokerRoute`, selected sandbox allocation and provider-native rendering spec.

The order is deliberate:

1. Workcell authorises the broker boundary;
2. only then is the trusted OpenSandbox egress endpoint resolved;
3. raw secret material crosses only that trusted sink boundary;
4. the returned Workcell/OpenSandbox receipt contains no raw secret.

The sandbox workload receives no readable credential value. The egress sidecar injects the real credential only into requests matching the authorised scheme/host/method/path relation. Deterministic tests require a denied Workcell route to make **zero** provider-side writes.

OpenSandbox recreates its egress sidecar in lifecycle cases where the provider-native Vault is lost. The material receipt therefore records `reinjection_required_after_sidecar_recreation = true`; restoration/resume orchestration must re-authorise and re-materialise the credential before a dependent workload proceeds.

This physically exercises the Workcell law:

```text
Agent may use credential ≠ Agent may read credential
```

## Provider loss, recovery and reconciliation

A provider's current availability is material state. It does not own the logical world it serves.

Deterministic conformance registers an already-composed OpenSandbox-backed world in `PreparedWorldControlPlane`, then changes only the lifecycle provider's reachability:

```text
Present
  → provider unavailable
Stale / desired Present → action: recover
  → same provider/material reachable again
Present
```

Across that transition the `WorldRef`, `DemandRef`, caller Project subject and provider material ref remain unchanged. Workcell re-observes the persisted handle; it does not silently allocate a replacement under the old identity.

## Control Service boundary

Workcell Control Service and OpenSandbox lifecycle server remain different systems with different responsibilities.

```text
remote semantic client
        ↓
Workcell Control Service
  discover / plan / prepare / observe / expose / collect / release / reconcile
        ↓
OpenSandbox adapter
        ↓
OpenSandbox lifecycle server
        ↓
provider-native sandbox/data plane
```

Collapsed-local Workcell may call the same core/provider contracts directly and may run an OpenSandbox server locally. A remote/server Workcell may reach an OpenSandbox deployment elsewhere. This deployment difference does not change the Workcell semantic contract.

Likewise, a Workcell can host an OpenSandbox lifecycle server as an ordinary managed service if a deployment chooses to do so; this is bootstrap/composition, not a circular Workcell primitive.

## Verification

The ordinary repository gate is:

```bash
./scripts/verify.sh
```

The deterministic W26 suite covers, without requiring a live OpenSandbox server on hosted CI:

- exact source/spec pin provenance;
- provider-neutral demand with no OpenSandbox/pool/sandbox-ID vocabulary;
- protocol-first sandbox creation/resource rendering;
- lease observation and renewal;
- structured checkpoint creation and checkpoint-ref restore reuse;
- native execd command execution;
- native execd file reading;
- arbitrary native browser/desktop/code-server-style endpoint resolution;
- attached storage lifetime independent of execution binding lifetime;
- several Workcell bindings sharing one physical sandbox provenance without sandbox/world identity collapse;
- Central Project + ProjectCentral staging and byte verification with root Control rejection;
- external egress policy distinct from material path provision;
- Credential Vault use-without-read, denied-route no-write, and reinjection requirement;
- provider disappearance/reappearance and reconciliation with world/material identity stable;
- identical generic material request across local/Docker-shaped and remote/Kubernetes-shaped lifecycle endpoints.

## Physical acceptance at O:I #97

Hosted CI establishes source, type, deterministic protocol and regression evidence. It does **not** establish that a real local OpenSandbox/Docker deployment works on the user's machine.

The #97 physical cut must exercise the actual integrated software and record concrete material provenance. At minimum:

1. start or connect the selected OpenSandbox lifecycle deployment;
2. discover its Workcell offer and materialise a real Project World;
3. stage a real Central Project + ProjectCentral and verify the material copy;
4. execute a command and read a file through native execd;
5. resolve a target-native service endpoint without routing application traffic through Workcell Control Service;
6. bind durable/shared storage and prove its lifetime is independent from the sandbox binding;
7. inspect/renew a real lease;
8. create a checkpoint and rematerialise from it;
9. exercise an allowed and denied external egress relation;
10. materialise a real brokered credential through Credential Vault and verify the workload can use it without receiving the raw value;
11. interrupt provider reachability, observe stale/recovery semantics, and restore reachability without changing semantic world identity;
12. record actual OpenSandbox server/runtime/component versions alongside the source/spec pin because upstream component release versions are presently heterogeneous.

Only that physical evidence should close the local-operation portion of W26/O:I #97.
