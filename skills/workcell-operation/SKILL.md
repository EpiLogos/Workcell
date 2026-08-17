---
name: workcell-operation
description: Inspect and request bounded Workcell material operations through provider-neutral control contracts without confusing material capability with semantic authority.
---

# Workcell operation

Use this Skill when an authorised actor needs to inspect or request material execution, runtime, service, storage, network, model-serving or secret-materialisation operations from Workcell.

## Contract metadata

- Semantic ref: `workcell:operator`
- Native owner: `EpiLogos/Workcell`
- Public client seam: `epilogos-workcell-sdk` / `ControlClient`
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

## Outputs

Return the operation outcome, Workcell/provider identity, binding refs, material endpoints where appropriate, explicit degradation, observed-state evidence, safe secret receipts, and unresolved authority or lifecycle conditions. Never return secret values as ordinary evidence.

## Verification

Run `./scripts/verify.sh`. For material operation acceptance, include provider-port/core planning/lifecycle tests. For secret work, include the repository secret-conformance workflow and adversarial fixtures. For model serving, use the native model-serving conformance tests rather than inventing a special model-server ontology.
