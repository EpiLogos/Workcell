# Workcell SDK

`epilogos-workcell-sdk` is a deliberately thin supported façade over the public contracts already implemented by `epilogos-workcell-core` and `epilogos-workcell-control`.

It exists so client and provider authors can target a stable Workcell seam without importing runtime/provider implementation crates. It is **not** a dynamic plugin framework and does not create a second Workcell ontology.

## Client surface

The SDK re-exports the existing generic `ControlClient` and transport/error contract. Its semantic operations remain the Workcell control protocol:

```text
status · discover · plan · prepare · observe · expose · collect · release · reconcile
```

## Provider surface

The SDK re-exports the existing provider-neutral ports for workspace, execution, project runtime, service, artifact storage and material exposure.

`testkit::verify_provider_port()` checks the cross-provider invariants already owned by core: stable provider identity, matching port family, valid unique offer refs and provider-consistent offers.

Provider-specific live lifecycle behavior remains the provider implementation's own conformance responsibility.

## Model serving

Model serving does not require a `ModelServer` or `LocalModelProvider` Workcell primitive. A caller can supply opaque model/variant refs and ordinary resource/material requirements through `ExecutionDemand`; engine and placement remain provider/materialisation facts.

The external-style SDK test proves this shape for Ollama, llama.cpp and vLLM demands while preserving one opaque higher model identity. Actuation/AIKit retain semantic ownership of the model relation.

## Verification

The repository's ordinary verification command covers this crate:

```sh
./scripts/verify.sh
```
