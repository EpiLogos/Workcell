# EpiLogos Workcell

Workcell is the relatively standalone **material execution product** beneath EpiLogos semantic clients.

It accepts provider-neutral `ExecutionDemand`, resolves current material offers, composes provider bindings into a `MaterialisedExecutionWorld`, and owns the lifecycle of that material state. It does **not** own Project, Run, Candidate, Context, Agent, Agency, Claim truth or Recognition.

## Canonical control plane

```text
discover
plan
prepare
observe
expose
collect
release
reconcile
```

The data plane remains native once Workcell has resolved bindings.

## Rust implementation

The product is implemented as a Rust workspace. The first crate is `epilogos-workcell-core`, which establishes the provider-neutral public domain seam and opaque client-reference boundary. Concrete provider ports and adapters are developed behind that seam in the Workcell Wayfinder order.

## Product programme

See issue #1. The implementation preserves the complete `F.01–F.12` Workcell design: external contract, ExecutionDemand, OperationalOffer/planning, provider ports, workspace, Docker, optional Arrakis, project-runtime/services, BindingGraph/material world, Candidate materialisation relation, reconciliation and deployment profiles.

Factory #113 is an interoperability/conformance seam. It is not a runtime dependency of Workcell core.

## Verification

```bash
./scripts/verify.sh
```

The same operation is used locally, by agents and by GitHub Actions.
