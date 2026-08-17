---
name: workcell-provider-authoring
description: Author and verify Workcell providers through the stable SDK ports without leaking provider-specific mechanics into semantic demand.
---

# Workcell provider authoring

Use this Skill to implement or extend a Workcell workspace, execution, project-runtime, service, artifact-storage, exposure or other supported provider through `epilogos-workcell-sdk`.

## Contract metadata

- Semantic ref: `workcell:provider-developer`
- Native owner: `EpiLogos/Workcell`
- Supported façade: `crates/workcell-sdk`
- Provider conformance specimen: `crates/workcell-sdk/tests/sdk_conformance.rs`
- Native verification: `./scripts/verify.sh`
- Risk class: material/provider implementation

## Authoring law

The SDK is a thin supported façade, not a dynamic plugin framework and not a second Workcell ontology. A provider satisfies a provider-neutral port. Higher products continue to express requirements, preferences, logical connectivity and retention semantics without naming your implementation.

## Procedure

1. Choose the existing provider port that owns the affordance: workspace, execution, project runtime, service, artifact storage or material exposure. Add a new family only when no accepted port can represent the material responsibility.
2. Give the provider a stable provider identity and advertise only truthful offers for its port family. Offer refs must be valid and unique.
3. Implement the port without teaching callers provider-native network names, host paths or lifecycle commands as semantic inputs.
4. Preserve the control/data-plane split: allocation/resolution/observation happens through Workcell; once bound, native workload protocols normally carry data directly.
5. For credentials, consume credential refs/materialisation requests through the secret boundary. Never make provider configuration a durable plaintext secret store.
6. Run `epilogos_workcell_sdk::testkit::verify_provider_port()` against the provider. This proves stable provider identity, matching port family, unique offer refs and provider-consistent offers.
7. Add provider-specific lifecycle/adversarial tests for preparation, observation, recovery, release and any degradation unique to the provider. Use the existing Docker/Arrakis/workspace implementations as examples, not ontology.
8. Run `./scripts/verify.sh`; where a real external substrate is required, run the provider's live smoke/conformance workflow and record the exact source/environment evidence.
9. Submit the source revision for Workcell-owner review. Projection, repeated successful use or a Factory benchmark may motivate promotion but cannot perform it.

## Representative specimen

`crates/workcell-sdk/tests/sdk_conformance.rs` is the public-SDK specimen. A new provider is not ready because it compiles privately; it is ready when it implements the exported port, passes `verify_provider_port()`, has lifecycle evidence, and does not force provider-specific assumptions back into `ExecutionDemand`.

## Self-improvement route

```text
observed material deficiency
  -> Factory Claim / Run
  -> proposed Workcell provider/source revision
  -> SDK + lifecycle/adversarial evidence
  -> Workcell-owner review / Recognition
  -> explicit promotion
```
