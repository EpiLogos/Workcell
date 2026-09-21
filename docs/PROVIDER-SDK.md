# The Workcell provider SDK

Workcell is provider-neutral: the demand/planning/material contracts in
`workcell-core` describe *what* is needed, and providers — local or external,
in-process or remote — describe *how* it is delivered. `epilogos-workcell-sdk`
is the single author-facing facade: it re-exports the canonical ports and
contract types so an external provider or client depends on one crate and
never on Workcell implementation crates.

This document is the entry point for authoring a provider against it. The
model-serving conformance round (`MODEL-SERVING-CONFORMANCE.md`) is the
precedent: that provider was implemented through this facade with no SDK
expansion.

## What the SDK exports

```text
workcell_sdk::contract   the demand/planning/material grammar
                         (ExecutionDemand, ExecutionMaterialRequest,
                         RetentionExpectation, ReleaseDisposition, ...)
workcell_sdk::provider   the provider ports and their law
                         (ProviderPort, ExecutionProvider, WorkspaceProvider,
                         ServiceProvider, StorageProvider, ... plus
                         validate_allocation / validate_provider_port)
workcell_sdk::client     the control-plane client and transports
workcell_sdk::testkit    port conformance checks and fault fixtures
```

## Authoring a provider

1. **Pick the port.** Each `ProviderPortKind` (workspace, execution,
   project-runtime, service, storage, artifact-storage) has its own trait
   layered over `ProviderPort`: identity (`provider_ref`), family
   (`port_kind`) and honest `offers()`.
2. **Implement the lifecycle** your port declares. For execution that is
   `prepare_execution` → `execute_operation` → `observe_execution` →
   `release_execution`. Allocation identity is enforced: a provider refuses
   an allocation whose `provider_ref` or `port` is not its own — identity
   non-collapse is part of the port law (`validate_allocation`).
3. **Honour retention, refuse the rest.** `release_execution` receives the
   caller's `RetentionExpectation`. A provider that cannot suspend or
   snapshot refuses that expectation outright; it never silently keeps or
   drops the world.
4. **Prove it with the testkit.** `testkit::verify_provider_port` checks the
   port-neutral invariants (identity coherence, offer ports, duplicate refs).
   The specimen below shows the full shape, including the negatives.

## The specimen

`crates/workcell-sdk/tests/external_execution_specimen.rs` is an
external-style provider written against the SDK alone — a directory-process
executor that materialises a working directory per demand, runs shell
commands, observes live state, and releases under retention semantics. Its
four tests are the template for an external author's own proof:

- full lifecycle: prepare → execute (real command, real file, collected
  stdout/exit) → observe → release, with only the provider's own allocation
  removed;
- retention honoured (`Preserve` keeps the world) and unsupported retention
  (`SnapshotIfSupported`) refused by name;
- a forged allocation (foreign `provider_ref`) is refused, not executed;
- the demand arrives through the contract grammar
  (`ExecutionDemand::new` + subjects), and the provider's provenance carries
  the caller's demand identity.

Run it with:

```sh
cargo test -p epilogos-workcell-sdk --test external_execution_specimen
```

The specimen deliberately adds no dependency to the SDK crate: an external
author's proof should not cost the SDK anything.

## Admission boundary

Proving a provider through the SDK is conformance; *selection* happens at
composition. `CollapsedLocalConfig::with_external_execution_provider` is the
admission seam: the composing host supplies a factory that constructs the
provider (as `Box<dyn ExecutionProvider>` — boxes implement the ports), and
the registered provider joins discovery and planning exactly like the
built-in ports. Duplicate identities are refused by the same law, and a
failing construction refuses the whole composition rather than starting half
a Workcell. The proof lives in
`crates/workcell-runtime/tests/external_provider_registration.rs`: a
registered external provider is discoverable, and a demand for its affordance
plans a binding naming its identity.

Whether an ordinary deployment declares your provider is still a decision of
whoever composes that Workcell (the CLI and Control Service compose from
their own configuration); the seam makes that a one-line declaration, not a
runtime change.
