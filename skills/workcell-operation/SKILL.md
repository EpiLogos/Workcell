---
name: workcell-operation
description: Inspect, request and extend bounded Workcell material operations through provider-neutral control contracts without confusing material capability with semantic authority.
---

# Workcell operation

Use this Skill when an authorised actor needs to inspect or request material execution, runtime, service, storage, network, model-serving or secret-materialisation operations from Workcell, or when a provider author needs to extend one of the existing material ports.

## Contract metadata

- Semantic ref: `workcell:operator`
- Native owner: `EpiLogos/Workcell`
- Public client/provider seam: `epilogos-workcell-sdk`
- Control protocol: `workcell.control/v1`
- Control operations: `status`, `discover`, `plan`, `prepare`, `observe`, `expose`, `collect`, `release`, `reconcile`
- Secret contract: `workcell.secret-materialisation/v1`
- Verification: `./scripts/verify.sh` plus `bash scripts/verify-native-skills.sh`
- Risk class: material/credential-sensitive; inspection and planning do not grant mutation

## Product boundary

Workcell materialises part of an already-resolved operative world. It does not own semantic `Project`, `Run`, `Candidate`, `Context`, `Agent`, `Agency`, Claim truth or Recognition. Express requirements and logical relationships; do not hard-code Docker networks, host IPs, provider brands or secret values into semantic intent.

Keep these distinctions explicit:

```text
ExecutionDemand != provider choice
provider offer != selected binding
binding available != Action authorised
material endpoint != semantic identity
NetworkRelationship != route/provider/path
secret ref != secret value
SecretMaterialReceipt != credential material
Skill available != Capability granted
```

## Inputs

Obtain the current Workcell identity/offer, the caller's provider-neutral `ExecutionDemand`, required/preferred/optional affordances, resource and connectivity requirements, persistence/retention semantics, any existing binding/material-world refs, and authority-bearing request context from the calling product.

## Procedure

1. **Discover before assuming.** Use `status`/`discover` to inspect current provider offers, capacity, services and health. A provider or service not reported is unavailable until evidence says otherwise.
2. **Plan provider-neutrally.** Send the semantic demand to `plan`. Required affordances must be satisfiable; preferred/optional degradation must be surfaced rather than hidden. Do not rewrite a caller's model identity into an engine/provider identity: model serving remains ordinary resource/material demand.
3. **Prepare only with bounded authority.** If the caller is authorised, use `prepare` to materialise the chosen workspace, execution/runtime, services, storage, networking and exposure relationships. Record the resulting `MaterialisedExecutionWorld` and binding graph.
4. **Observe actual material truth.** Use `observe` for desired/observed state, health and lifecycle evidence. Report provider-unavailable, degraded and stale states as such.
5. **Expose and collect deliberately.** Use `expose` for application/candidate surfaces and `collect` for artifacts/log/evidence references. The control plane returns bindings; workloads normally use their native data-plane protocols directly.
6. **Handle secrets as materialisation, never configuration text.** Build a `SecretMaterialisationRequest` from a credential ref, provider ref, binding, consumer/workload, class, purpose, destination and scope. Do not place a secret value in Claims, logs, receipts, manifests, Skill bodies or projected state.
7. **Respect secret classes and broker boundaries.** `SecretValue` is privileged material whose debug form is redacted. `SecretMaterialReceipt` is safe provenance and deliberately cannot carry the value. Brokered access must match credential/binding, purpose/scope, provider and an authorised `BrokerRoute`; expired/revoked material fails closed.
8. **Release by semantic retention policy.** Use `release` to clean, suspend, snapshot or preserve material resources according to retention semantics; do not equate an environment's lifetime with Candidate/Run identity.
9. **Reconcile persistent desired state.** Use `reconcile` only for declared Workcell-owned desired state. Return observed deltas and evidence to the semantic caller; do not turn infrastructure health into product Recognition.

## Extend Workcell

Use this mode when a target system can supply a material capability that one of Workcell's existing public ports or the public fabric seam can honestly represent.

1. **Start from target-system reality.** Pin or record the exact target version/revision and read its current native lifecycle, API, health, routing and failure behavior. Do not begin from a brand-shaped Workcell abstraction.
2. **Choose the smallest existing public seam.** Implement `WorkspaceProvider`, `ExecutionProvider`, `ProjectRuntimeProvider`, `ServiceProvider`, `ArtifactStorageProvider`, or the SDK `FabricPathProvider` only when that seam owns the target's material behavior. Do not create a new provider family for symmetry.
3. **Keep semantic identity opaque.** Caller refs and logical service/relationship identities pass through unchanged. Provider IDs, hostnames, addresses, tunnel endpoints, mesh node IDs, regions, process IDs and target-native route names belong in offers, bindings and provenance.
4. **Publish truthful offers.** Use current availability, health and capacity. Required absence must fail; preferred absence must degrade; optional absence may omit. Never emulate an unavailable material property merely to satisfy a plan.
5. **Exercise the public conformance kit.** `verify_provider_port` checks provider/port/offer identity. `FaultingExecutionProvider` supplies deterministic unavailable/degraded and partial-lifecycle failure pressure. `diff_provider_inventory` makes removal/replacement explicit without rewriting caller identity. Fabric authors must also exercise relationship feasibility, policy denial, provider/route replacement, private/public scope and provenance.
6. **Test lifecycle and recovery through public types.** Cover prepare/resolve, observe, operation where applicable, release/retention, restart or re-entry where applicable, provider loss/reappearance, and target-owned state. A provider must not delete or stop state it merely discovered unless the target contract explicitly delegates that authority.
7. **Keep application protocols opaque.** A service binding can report an endpoint/protocol material fact, but Workcell does not become the protocol implementation, conversation/session owner or application proxy unless a separately justified material port owns that behavior.
8. **Prove external-style use.** At least one conformance test should import only `epilogos-workcell-sdk` plus the target system's own public interface. If implementation requires private planner/runtime modules, the public seam is incomplete.
9. **Version incompatibility is explicit.** Remote clients use the versioned Control Protocol and must surface protocol incompatibility separately from transport, authentication and remote semantic failure. Rust provider authors use the SDK crate's semver/public API rather than private module paths.
10. **Leave physical claims physical.** Deterministic fixtures prove contract shape. Docker, Arrakis, Tailscale, cloud hosts and real gateway processes are only accepted when the relevant live environment has actually been exercised and the receipt records what was observed.

## Outputs

Return the operation outcome, Workcell/provider identity, binding refs, material endpoints where appropriate, explicit degradation, observed-state evidence, safe secret receipts, and unresolved authority or lifecycle conditions. For extension work, also return the target source/version, public Workcell seam used, conformance summary, provider/fabric provenance, and any remaining live-acceptance gate. Never return secret values as ordinary evidence.

## Verification

Run `./scripts/verify.sh`. For material operation acceptance, include provider-port/core planning/lifecycle tests. For SDK extension work, run the public `workcell-sdk` conformance tests and any target-specific external-style tests. For fabric work, prove relationship identity survives route/provider replacement and that policy/scope failures remain explicit. For secret work, include the repository secret-conformance workflow and adversarial fixtures. For model serving, use the native model-serving conformance tests rather than inventing a special model-server ontology.
